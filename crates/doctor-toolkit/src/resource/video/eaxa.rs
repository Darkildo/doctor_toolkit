//! EA-XA "revision 3" ADPCM, the audio of the game's movies (`SCDl` chunks).
//!
//! ```text
//! SCDl body (big-endian):
//!   u32  samples per channel in this chunk (a multiple of 28)
//!   u32  per channel: offset of its data, counted from the end of this table
//!   per channel: frames of 28 samples
//!     u8   coefficient index << 4 | shift s  → 14 bytes, two 4-bit samples each,
//!          high nibble first; sample = clip16(((n << (20 - s)) + c1·cur + c2·prev) >> 8)
//!     0xEE → u16 cur, u16 prev, 28 × i16 raw samples (big-endian)
//! ```
//!
//! The predictor state (`cur`, `prev`) carries over from chunk to chunk.

use crate::{Error, Result};

/// Samples in one frame.
pub const FRAME: usize = 28;

const COEF1: [i32; 4] = [0, 240, 460, 392];
const COEF2: [i32; 4] = [0, 0, -208, -220];

/// Predictor state of one channel.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct State {
    pub cur: i32,
    pub prev: i32,
}

fn be32(d: &[u8], at: usize) -> Result<u32> {
    let s = d
        .get(at..at + 4)
        .ok_or_else(|| Error::malformed("SCDl", at, "truncated"))?;
    Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

/// Number of samples per channel a chunk declares.
pub fn chunk_samples(body: &[u8]) -> Result<u32> {
    be32(body, 0)
}

/// Decodes one `SCDl` body into `out[channel]`, updating `state`.
pub fn decode_chunk(body: &[u8], state: &mut [State], out: &mut [Vec<i16>]) -> Result<()> {
    let channels = state.len();
    let samples = be32(body, 0)? as usize;
    let frames = samples / FRAME;
    let table = 4 + 4 * channels;
    for ch in 0..channels {
        let mut at = table + be32(body, 4 + 4 * ch)? as usize;
        let st = &mut state[ch];
        let dst = &mut out[ch];
        for _ in 0..frames {
            let byte = *body
                .get(at)
                .ok_or_else(|| Error::malformed("SCDl", at, "truncated frame"))?;
            at += 1;
            if byte == 0xEE {
                let raw = body
                    .get(at..at + 4 + 2 * FRAME)
                    .ok_or_else(|| Error::malformed("SCDl", at, "truncated raw frame"))?;
                let s16 = |i: usize| i16::from_be_bytes([raw[2 * i], raw[2 * i + 1]]);
                st.cur = s16(0) as i32;
                st.prev = s16(1) as i32;
                for i in 0..FRAME {
                    dst.push(s16(2 + i));
                }
                at += 4 + 2 * FRAME;
                continue;
            }
            let data = body
                .get(at..at + FRAME / 2)
                .ok_or_else(|| Error::malformed("SCDl", at, "truncated frame"))?;
            at += FRAME / 2;
            let (c1, c2) = (
                COEF1.get((byte >> 4) as usize).copied().unwrap_or(0),
                COEF2.get((byte >> 4) as usize).copied().unwrap_or(0),
            );
            let shift = 20 - (byte & 0x0F) as i32;
            for i in 0..FRAME {
                let b = data[i / 2];
                let nib = if i & 1 == 0 { b >> 4 } else { b & 0x0F };
                let n = ((nib as i32) << 28) >> 28;
                let v = (n << shift) + st.cur * c1 + st.prev * c2;
                let v = (v >> 8).clamp(i16::MIN as i32, i16::MAX as i32);
                st.prev = st.cur;
                st.cur = v;
                dst.push(v as i16);
            }
        }
    }
    Ok(())
}

/// Encodes one frame of up to 28 samples (missing ones are silence).
fn encode_frame(samples: &[i16], st: &mut State, out: &mut Vec<u8>) {
    let mut x = [0i32; FRAME];
    for (d, &s) in x.iter_mut().zip(samples) {
        *d = s as i32;
    }
    let mut best = (u64::MAX, 0usize, 0i32);
    for coef in 0..4 {
        for s in 0..=12 {
            let err = simulate(&x, *st, coef, s, None);
            if err < best.0 {
                best = (err, coef, s);
            }
        }
    }
    let (_, coef, s) = best;
    let mut nibbles = [0u8; FRAME];
    simulate(&x, *st, coef, s, Some((&mut nibbles, st)));
    out.push((coef as u8) << 4 | s as u8);
    for pair in nibbles.chunks(2) {
        out.push(pair[0] << 4 | pair[1]);
    }
}

/// Squared error of coding `x` with a coefficient pair and shift; with `emit`,
/// stores the nibbles and the final state.
fn simulate(
    x: &[i32; FRAME],
    st: State,
    coef: usize,
    s: i32,
    emit: Option<(&mut [u8; FRAME], &mut State)>,
) -> u64 {
    let (c1, c2) = (COEF1[coef], COEF2[coef]);
    let shift = 20 - s;
    let step = 1i64 << shift;
    let (mut cur, mut prev) = (st.cur, st.prev);
    let mut err = 0u64;
    let mut nibbles = [0u8; FRAME];
    for i in 0..FRAME {
        let pred = cur * c1 + prev * c2;
        let target = ((x[i] as i64) << 8) - pred as i64;
        let n = (target + step / 2).div_euclid(step).clamp(-8, 7) as i32;
        let v = (((n << shift) + pred) >> 8).clamp(i16::MIN as i32, i16::MAX as i32);
        let d = (v - x[i]) as i64;
        err += (d * d) as u64;
        nibbles[i] = (n & 0xF) as u8;
        prev = cur;
        cur = v;
    }
    if let Some((out, state)) = emit {
        *out = nibbles;
        *state = State { cur, prev };
    }
    err
}

/// Builds one `SCDl` body from per-channel samples (all channels the same
/// length; padded with silence to a multiple of 28, so only the last chunk of a
/// track may have a length that is not a multiple of 28).
pub fn encode_chunk(channels: &[&[i16]], state: &mut [State]) -> Vec<u8> {
    let len = channels.first().map_or(0, |c| c.len());
    let frames = len.div_ceil(FRAME);
    let mut data: Vec<Vec<u8>> = Vec::with_capacity(channels.len());
    for (samples, st) in channels.iter().zip(state.iter_mut()) {
        let mut d = Vec::with_capacity(frames * 15 + 1);
        for f in 0..frames {
            let end = ((f + 1) * FRAME).min(samples.len());
            encode_frame(&samples[f * FRAME..end], st, &mut d);
        }
        if d.len() % 2 == 1 {
            d.push(0);
        }
        data.push(d);
    }
    let mut body = Vec::new();
    body.extend_from_slice(&((frames * FRAME) as u32).to_be_bytes());
    let mut offset = 0u32;
    for d in &data {
        body.extend_from_slice(&offset.to_be_bytes());
        offset += d.len() as u32;
    }
    for d in &data {
        body.extend_from_slice(d);
    }
    // Chunks (8-byte header + body) stay 4-byte aligned, as in the game's files.
    while body.len() % 4 != 0 {
        body.push(0);
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_sine() {
        let n = 28 * 40 + 5;
        let left: Vec<i16> = (0..n)
            .map(|i| ((i as f32 / 17.0).sin() * 12000.0) as i16)
            .collect();
        let right: Vec<i16> = (0..n)
            .map(|i| ((i as f32 / 5.0).cos() * 3000.0 + (i as f32 / 51.0).sin() * 9000.0) as i16)
            .collect();
        let mut enc_state = [State::default(); 2];
        let mut dec_state = [State::default(); 2];
        let mut out = vec![Vec::new(), Vec::new()];
        for range in [0..588, 588..n] {
            let body = encode_chunk(
                &[&left[range.clone()], &right[range.clone()]],
                &mut enc_state,
            );
            assert_eq!(body.len() % 4, 0);
            decode_chunk(&body, &mut dec_state, &mut out).unwrap();
        }
        assert_eq!(enc_state, dec_state);
        for (src, dec) in [(&left, &out[0]), (&right, &out[1])] {
            let se: f64 = src
                .iter()
                .zip(dec.iter())
                .map(|(&a, &b)| (a as f64 - b as f64).powi(2))
                .sum();
            let sig: f64 = src.iter().map(|&a| (a as f64).powi(2)).sum();
            let snr = 10.0 * (sig / se).log10();
            assert!(snr > 25.0, "SNR {snr}");
        }
    }

    #[test]
    fn raw_frames_and_truncation() {
        let mut body = vec![0, 0, 0, 28, 0, 0, 0, 0, 0xEE];
        body.extend_from_slice(&5i16.to_be_bytes());
        body.extend_from_slice(&4i16.to_be_bytes());
        for i in 0..28i16 {
            body.extend_from_slice(&(i * 100).to_be_bytes());
        }
        let mut st = [State::default()];
        let mut out = vec![Vec::new()];
        decode_chunk(&body, &mut st, &mut out).unwrap();
        assert_eq!(out[0][27], 2700);
        assert_eq!(st[0], State { cur: 5, prev: 4 });
        assert!(decode_chunk(&body[..20], &mut st, &mut out).is_err());
    }
}

//! Sound import: an EA-XAS v1 encoder, SNR/SNS and SNU writers, and sound
//! replacement inside `sbk` banks.
//!
//! The encoder is the exact inverse of [`audio::decode_xas_frame`]: for each
//! group of 32 samples it tries all four predictors and all sixteen shifts,
//! quantises in a closed loop (prediction from the decoded samples, as the
//! decoder sees them) and keeps the combination with the smallest squared
//! error. Input of any rate and channel count is first converted to the
//! format of the sound being replaced ([`fit_pcm`]).

use crate::resource::audio::{self, CODEC_XAS1, Sample, SnrHeader, decode_xas_frame, parse_sbk};
use crate::resource::video::input::Pcm;
use crate::{Error, Result, io};

const XAS_FRAME: usize = 0x4C;
const XAS_SAMPLES: usize = 128;
const COEFS: [(i32, i32); 4] = [(0, 0), (240, 0), (460, -208), (392, -220)];

/// Most samples one SNR header can describe (29 bits).
pub const MAX_SAMPLE_COUNT: usize = 0x1FFF_FFFF;
/// Highest sample rate of the SNR header (18 bits).
pub const MAX_SAMPLE_RATE: u32 = 0x3_FFFF;

impl SnrHeader {
    /// The 8- or 12-byte big-endian header.
    pub fn to_bytes(&self) -> Vec<u8> {
        let a = (self.version as u32 & 0xF) << 28
            | (self.codec as u32 & 0xF) << 24
            | ((self.channels as u32).saturating_sub(1) & 0x3F) << 18
            | self.sample_rate & 0x3_FFFF;
        let b = (self.kind as u32 & 3) << 30
            | (self.loop_start.is_some() as u32) << 29
            | self.sample_count & 0x1FFF_FFFF;
        let mut out = Vec::with_capacity(12);
        out.extend_from_slice(&a.to_be_bytes());
        out.extend_from_slice(&b.to_be_bytes());
        if let Some(start) = self.loop_start {
            out.extend_from_slice(&start.to_be_bytes());
        }
        out
    }
}

/// A coding of one group: error, coefficient index, shift, nibbles, decoded samples.
type Candidate = (i64, usize, i32, [u8; 30], [i16; 32]);

/// Encodes one group of 32 samples into its 4-byte header and 30 nibbles.
/// Returns the header and the nibbles; `out` receives the decoded samples.
fn encode_group(input: &[i16; 32], out: &mut [i16; 32]) -> ([u8; 4], [u8; 30]) {
    // The first two samples are stored with their low nibble replaced.
    let keep = |v: i16| ((v as i32 + 8).min(i16::MAX as i32) & !0xF) as i16;
    let s0 = keep(input[0]);
    let s1 = keep(input[1]);
    let mut best: Option<Candidate> = None;
    for (ci, &(c1, c2)) in COEFS.iter().enumerate() {
        for sn in 0..16i32 {
            let shift = 20 - sn;
            let mut s = [0i16; 32];
            s[0] = s0;
            s[1] = s1;
            let mut nibbles = [0u8; 30];
            let mut err = 0i64;
            for i in 2..32 {
                let pred = s[i - 1] as i32 * c1 + s[i - 2] as i32 * c2;
                let want = input[i] as i64 * 256 - pred as i64;
                let step = 1i64 << shift;
                let n = (want as f64 / step as f64).round().clamp(-8.0, 7.0) as i32;
                let v = (((n << 28) >> 28 << shift) + pred + 0x80) >> 8;
                let v = v.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
                s[i] = v;
                nibbles[i - 2] = (n & 0xF) as u8;
                let d = v as i64 - input[i] as i64;
                err += d * d;
                if best.as_ref().is_some_and(|b| err >= b.0) {
                    break;
                }
            }
            if best.as_ref().is_none_or(|b| err < b.0) {
                best = Some((err, ci, sn, nibbles, s));
            }
        }
    }
    let (_, ci, sn, nibbles, s) = best.expect("at least one candidate");
    *out = s;
    let h0 = (s0 as u16 & !0xF) | ci as u16;
    let h1 = (s1 as u16 & !0xF) | sn as u16;
    let mut header = [0u8; 4];
    header[..2].copy_from_slice(&h0.to_le_bytes());
    header[2..].copy_from_slice(&h1.to_le_bytes());
    (header, nibbles)
}

/// Encodes 128 samples of one channel into an EA-XAS v1 frame.
pub fn encode_xas_frame(samples: &[i16; XAS_SAMPLES]) -> [u8; XAS_FRAME] {
    let mut frame = [0u8; XAS_FRAME];
    let mut decoded = [0i16; 32];
    for group in 0..4 {
        let input: &[i16; 32] = samples[group * 32..group * 32 + 32].try_into().unwrap();
        let (header, nibbles) = encode_group(input, &mut decoded);
        frame[4 * group..4 * group + 4].copy_from_slice(&header);
        for row in 0..15 {
            frame[16 + 4 * row + group] = nibbles[2 * row] << 4 | nibbles[2 * row + 1];
        }
    }
    frame
}

/// Encodes interleaved PCM into EA-XAS frames: for every 128 samples, one frame
/// per channel in channel order. The last frame is padded with silence.
pub fn encode_xas(pcm: &[i16], channels: usize) -> Vec<u8> {
    let channels = channels.max(1);
    let total = pcm.len() / channels;
    let frames = total.div_ceil(XAS_SAMPLES);
    let mut out = Vec::with_capacity(frames * channels * XAS_FRAME);
    let mut buf = [0i16; XAS_SAMPLES];
    for f in 0..frames {
        for c in 0..channels {
            for (i, b) in buf.iter_mut().enumerate() {
                let at = f * XAS_SAMPLES + i;
                *b = if at < total {
                    pcm[at * channels + c]
                } else {
                    0
                };
            }
            out.extend_from_slice(&encode_xas_frame(&buf));
        }
    }
    out
}

/// Frames per SNS block, as in the game's files: a shorter first block (13
/// stereo or 26 mono frames, 1984 bytes), then blocks of about 2 KB.
fn block_frames(channels: usize, first: bool) -> usize {
    match (channels, first) {
        (1, true) => 26,
        (1, false) => 27,
        (2, true) => 13,
        (2, false) => 14,
        (c, true) => (1984 / (XAS_FRAME * c)).max(1),
        (c, false) => (2136 / (XAS_FRAME * c)).max(1),
    }
}

/// How a new sample is described.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SampleSpec {
    pub channels: u8,
    pub sample_rate: u32,
    /// 0 RAM (banks), 1 stream (`.snu`).
    pub kind: u8,
    pub loop_start: Option<u32>,
}

impl SampleSpec {
    /// The format of an existing sample; the loop is kept when it fits.
    pub fn like(header: &SnrHeader) -> Self {
        Self {
            channels: header.channels,
            sample_rate: header.sample_rate,
            kind: header.kind,
            loop_start: header.loop_start,
        }
    }
}

fn check(pcm: &Pcm, spec: &SampleSpec) -> Result<usize> {
    if pcm.channels as u8 != spec.channels || pcm.sample_rate != spec.sample_rate {
        return Err(Error::InvalidArgument(format!(
            "PCM is {} Hz, {} channel(s); the sample needs {} Hz, {} channel(s)",
            pcm.sample_rate, pcm.channels, spec.sample_rate, spec.channels
        )));
    }
    if spec.channels == 0 || spec.channels > 64 || spec.sample_rate > MAX_SAMPLE_RATE {
        return Err(Error::InvalidArgument(
            "bad channel count or sample rate".into(),
        ));
    }
    let n = pcm.frames();
    if n == 0 {
        return Err(Error::InvalidArgument("the sound is empty".into()));
    }
    if n > MAX_SAMPLE_COUNT {
        return Err(Error::InvalidArgument("the sound is too long".into()));
    }
    Ok(n)
}

/// SNS blocks for `pcm` (already in the format of `spec`).
pub fn build_sns(pcm: &Pcm, spec: &SampleSpec) -> Result<Vec<u8>> {
    let n = check(pcm, spec)?;
    let ch = spec.channels as usize;
    let data = encode_xas(&pcm.samples, ch);
    let frame_bytes = XAS_FRAME * ch;
    let total_frames = n.div_ceil(XAS_SAMPLES);
    let mut out = Vec::with_capacity(data.len() + total_frames * 8 / 13 + 16);
    let mut frame = 0;
    let mut done = 0;
    while frame < total_frames {
        let count = block_frames(ch, frame == 0).min(total_frames - frame);
        let samples = (count * XAS_SAMPLES).min(n - done);
        let last = frame + count == total_frames;
        let size = 8 + count * frame_bytes;
        let flag = if last { 0x80u32 } else { 0 };
        out.extend_from_slice(&(flag << 24 | size as u32).to_be_bytes());
        out.extend_from_slice(&(samples as u32).to_be_bytes());
        out.extend_from_slice(&data[frame * frame_bytes..(frame + count) * frame_bytes]);
        frame += count;
        done += samples;
    }
    Ok(out)
}

/// A complete sample: SNR header followed by its SNS blocks (the layout of
/// `sbk` banks).
pub fn build_inline(pcm: &Pcm, spec: &SampleSpec) -> Result<Vec<u8>> {
    let n = check(pcm, spec)?;
    let header = header_for(spec, n);
    let mut out = header.to_bytes();
    out.extend_from_slice(&build_sns(pcm, spec)?);
    Ok(out)
}

fn header_for(spec: &SampleSpec, n: usize) -> SnrHeader {
    SnrHeader {
        version: 0,
        codec: CODEC_XAS1,
        channels: spec.channels,
        sample_rate: spec.sample_rate,
        kind: spec.kind,
        sample_count: n as u32,
        loop_start: spec
            .loop_start
            .map(|s| if (s as usize) < n { s } else { 0 }),
    }
}

/// An `.snu` file for the streamed audio of the `.viv` archives. From
/// `template` (the `.snu` being replaced) the flags byte 1, the field at 0x0C
/// and the metadata between the header and the SNS blocks (speech files keep
/// their subtitles there, an `LCH2` chunk) are kept; the duration field at 4
/// is set to the new length in milliseconds. The file is padded to a multiple
/// of 16 bytes inside the last block, as the game's files are.
pub fn build_snu(pcm: &Pcm, spec: &SampleSpec, template: Option<&[u8]>) -> Result<Vec<u8>> {
    let n = check(pcm, spec)?;
    let header = header_for(spec, n);
    let mut out = vec![0u8; 0x10];
    out[0] = 3;
    out[3] = spec.channels;
    if let Some(t) = template.filter(|t| t.len() >= 0x10 && t[0] == 3) {
        out[1] = t[1];
        out[0x0C..0x10].copy_from_slice(&t[0x0C..0x10]);
    }
    let ms = (n as u64 * 1000 / spec.sample_rate as u64) as u32;
    out[4..8].copy_from_slice(&ms.to_le_bytes());
    out.extend_from_slice(&header.to_bytes());
    out.resize(0x20, 0);
    if let Some(t) = template.filter(|t| t.len() >= 0x20 && t[0] == 3) {
        let start = io::read_u32(t, 8, io::Endian::Little)? as usize;
        if (0x20..=t.len()).contains(&start) {
            out.extend_from_slice(&t[0x20..start]);
        }
    }
    let blocks_at = out.len();
    out[8..12].copy_from_slice(&(blocks_at as u32).to_le_bytes());
    out.extend_from_slice(&build_sns(pcm, spec)?);
    let pad = out.len().next_multiple_of(16) - out.len();
    if pad > 0 {
        // Grow the last block: the decoder reads only the frames it needs.
        let mut at = blocks_at;
        loop {
            let head = io::read_u32(&out, at, io::Endian::Big)?;
            let size = (head & 0xFF_FFFF) as usize;
            if head >> 24 == 0x80 {
                let grown = (head & 0xFF00_0000) | (size + pad) as u32;
                out[at..at + 4].copy_from_slice(&grown.to_be_bytes());
                break;
            }
            at += size;
        }
        out.resize(out.len() + pad, 0);
    }
    Ok(out)
}

/// Converts PCM to `channels` and `sample_rate`: mono is duplicated, extra
/// channels are mixed down (odd channels left, even right), the rate is
/// converted with a windowed-sinc filter.
pub fn fit_pcm(pcm: &Pcm, channels: u8, sample_rate: u32) -> Pcm {
    let from = pcm.channels.max(1) as usize;
    let to = channels.max(1) as usize;
    let n = pcm.frames();
    let mut planes: Vec<Vec<i16>> = vec![Vec::with_capacity(n); to];
    for i in 0..n {
        let s = &pcm.samples[i * from..(i + 1) * from];
        for (c, plane) in planes.iter_mut().enumerate() {
            let v = if from == to {
                s[c] as i32
            } else if to == 1 {
                s.iter().map(|&v| v as i32).sum::<i32>() / from as i32
            } else if from == 1 {
                s[0] as i32
            } else if to == 2 {
                let picked: Vec<i32> = s.iter().skip(c).step_by(2).map(|&v| v as i32).collect();
                picked.iter().sum::<i32>() / picked.len().max(1) as i32
            } else {
                s[c.min(from - 1)] as i32
            };
            plane.push(v as i16);
        }
    }
    let planes: Vec<Vec<i16>> = planes
        .into_iter()
        .map(|p| crate::resource::video::resample(&p, pcm.sample_rate, sample_rate))
        .collect();
    let len = planes.iter().map(Vec::len).min().unwrap_or(0);
    let mut samples = Vec::with_capacity(len * to);
    for i in 0..len {
        for p in &planes {
            samples.push(p[i]);
        }
    }
    Pcm {
        channels: to as u16,
        sample_rate,
        samples,
    }
}

/// Quality of a round trip: signal-to-noise ratio and peak SNR in dB.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quality {
    pub snr_db: f64,
    pub psnr_db: f64,
    pub max_error: i32,
}

/// Compares `decoded` with `reference` (same length and layout).
pub fn quality(reference: &[i16], decoded: &[i16]) -> Quality {
    let mut signal = 0f64;
    let mut noise = 0f64;
    let mut max_error = 0;
    for (&a, &b) in reference.iter().zip(decoded) {
        signal += (a as f64).powi(2);
        let d = a as i32 - b as i32;
        noise += (d as f64).powi(2);
        max_error = max_error.max(d.abs());
    }
    let n = reference.len().min(decoded.len()).max(1) as f64;
    let db = |x: f64| {
        if noise == 0.0 {
            f64::INFINITY
        } else {
            10.0 * x.log10()
        }
    };
    Quality {
        snr_db: db(signal / noise.max(1e-12)),
        psnr_db: db(32767f64.powi(2) * n / noise.max(1e-12)),
        max_error,
    }
}

/// Decodes the SNS blocks of an encoded sample (`build_inline` output).
pub fn decode_inline(data: &[u8]) -> Result<Vec<i16>> {
    Sample::parse_inline(data, 0)?.decode()
}

/// Rebuilds an `sbk` bank with the sound at list position `position` (as
/// returned by [`parse_sbk`]) replaced by `sample` (SNR header and SNS
/// blocks). The `sdat` table keeps its order; samples are laid out again with
/// the original alignment and the header's `sdat` size fields are updated.
pub fn replace_sbk_sound(bank: &[u8], position: usize, sample: &[u8]) -> Result<Vec<u8>> {
    let le = io::Endian::Little;
    let sounds = parse_sbk(bank)?;
    if position >= sounds.len() {
        return Err(Error::InvalidArgument(format!(
            "the bank has {} sound(s), there is no sound {position}",
            sounds.len()
        )));
    }
    Sample::parse_inline(sample, 0)?;
    let sdat = io::read_u32(bank, 0x10, le)? as usize;
    let count = sounds.len();
    let table_end = sdat + 8 + 16 * count;
    // Original sample starts relative to sdat, in table order.
    let offsets: Vec<usize> = (0..count)
        .map(|i| io::read_u32(bank, sdat + 8 + 16 * i + 8, le).map(|o| o as usize))
        .collect::<Result<_>>()?;
    let align = offsets
        .iter()
        .map(|&o| 1usize << o.trailing_zeros().min(12))
        .min()
        .unwrap_or(4)
        .max(4);
    // Samples in the order of their data, so the layout stays the same.
    let mut order: Vec<usize> = (0..count).collect();
    order.sort_by_key(|&i| offsets[i]);
    let mut out = bank[..table_end].to_vec();
    let mut new_offsets = vec![0usize; count];
    let first = order
        .first()
        .map(|&i| sdat + offsets[i])
        .unwrap_or(table_end);
    // Keep whatever lies between the table and the first sample.
    if first > table_end {
        out.extend_from_slice(io::slice(bank, table_end, first - table_end)?);
    }
    for &i in &order {
        let at = sdat + (out.len() - sdat).next_multiple_of(align);
        out.resize(at, 0);
        new_offsets[i] = at - sdat;
        if i == position {
            out.extend_from_slice(sample);
        } else {
            out.extend_from_slice(&bank[sounds[i].sample.range.clone()]);
        }
    }
    // Trailing bytes after the last sample (padding) are kept as padding.
    let last = order
        .last()
        .map(|&i| sounds[i].sample.range.end)
        .unwrap_or(table_end);
    let tail = bank.len().saturating_sub(last);
    out.resize(out.len() + tail, 0);
    for (i, &o) in new_offsets.iter().enumerate() {
        let at = sdat + 8 + 16 * i + 8;
        out[at..at + 4].copy_from_slice(&(o as u32).to_le_bytes());
    }
    let new_size = (out.len() - sdat) as u32;
    let old_size = io::read_u32(bank, 0x14, le)?;
    out[0x14..0x18].copy_from_slice(&new_size.to_le_bytes());
    if io::read_u32(bank, 0x0C, le)? == old_size {
        out[0x0C..0x10].copy_from_slice(&new_size.to_le_bytes());
    }
    Ok(out)
}

/// Encodes `pcm` (any format) for the slot of an existing sample: converted to
/// the sample's channels and rate, the loop flag kept.
pub fn encode_like(pcm: &Pcm, header: &SnrHeader) -> Result<(Pcm, Vec<u8>)> {
    let spec = SampleSpec::like(header);
    let fitted = fit_pcm(pcm, spec.channels, spec.sample_rate);
    let data = build_inline(&fitted, &spec)?;
    Ok((fitted, data))
}

/// Decodes a frame produced by the encoder; used by tests and quality checks.
pub fn roundtrip_frame(samples: &[i16; XAS_SAMPLES]) -> [i16; XAS_SAMPLES] {
    let mut out = [0i16; XAS_SAMPLES];
    decode_xas_frame(&encode_xas_frame(samples), &mut out);
    out
}

/// Encodes PCM for a streamed `.snu` slot, keeping the format of `original`.
pub fn encode_snu_like(pcm: &Pcm, original: &[u8]) -> Result<Vec<u8>> {
    let sample = audio::Sample::parse_snu(original)?;
    let spec = SampleSpec::like(&sample.header);
    let fitted = fit_pcm(pcm, spec.channels, spec.sample_rate);
    build_snu(&fitted, &spec, Some(original))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(n: usize, rate: u32, freq: f64, amp: f64) -> Vec<i16> {
        (0..n)
            .map(|i| {
                let t = i as f64 / rate as f64;
                (amp * (2.0 * std::f64::consts::PI * freq * t).sin()) as i16
            })
            .collect()
    }

    #[test]
    fn header_round_trips() {
        let h = SnrHeader {
            version: 0,
            codec: CODEC_XAS1,
            channels: 2,
            sample_rate: 32000,
            kind: 1,
            sample_count: 7_821_632,
            loop_start: Some(0),
        };
        let bytes = h.to_bytes();
        assert_eq!(bytes[..8], [0x04, 0x04, 0x7D, 0x00, 0x60, 0x77, 0x59, 0x40]);
        assert_eq!(SnrHeader::parse(&bytes, 0).unwrap(), h);
    }

    #[test]
    fn frame_encoding_is_close_and_stable() {
        let pcm = tone(128, 22050, 440.0, 12000.0);
        let input: [i16; 128] = pcm.try_into().unwrap();
        let once = roundtrip_frame(&input);
        let q = quality(&input, &once);
        assert!(q.snr_db > 25.0, "{q:?}");
        // Encoding decoded data again changes little.
        let twice = roundtrip_frame(&once);
        assert!(quality(&once, &twice).snr_db > 30.0);
        // Silence stays silence.
        assert_eq!(roundtrip_frame(&[0; 128]), [0; 128]);
    }

    #[test]
    fn inline_sample_decodes_to_input_length() {
        let samples: Vec<i16> = tone(5000, 24000, 300.0, 9000.0)
            .into_iter()
            .flat_map(|v| [v, v / 2])
            .collect();
        let pcm = Pcm {
            channels: 2,
            sample_rate: 24000,
            samples,
        };
        let spec = SampleSpec {
            channels: 2,
            sample_rate: 24000,
            kind: 0,
            loop_start: None,
        };
        let data = build_inline(&pcm, &spec).unwrap();
        let sample = Sample::parse_inline(&data, 0).unwrap();
        assert_eq!(sample.header.sample_count, 5000);
        assert_eq!(sample.range, 0..data.len());
        let decoded = sample.decode().unwrap();
        assert_eq!(decoded.len(), 10000);
        assert!(quality(&pcm.samples, &decoded).snr_db > 25.0);
        // Blocks: 13 frames, then 14.
        assert_eq!(sample.blocks[0].samples, 13 * 128);
        assert_eq!(sample.blocks[0].data.len(), 1976);
        assert!(sample.blocks.last().unwrap().last);
    }

    #[test]
    fn snu_keeps_template_fields() {
        let pcm = Pcm {
            channels: 1,
            sample_rate: 32000,
            samples: tone(32000, 32000, 1000.0, 8000.0),
        };
        let mut template = vec![0u8; 0x20];
        template[0] = 3;
        template[1] = 8;
        template[0x0C] = 0x80;
        let spec = SampleSpec {
            channels: 1,
            sample_rate: 32000,
            kind: 1,
            loop_start: None,
        };
        let snu = build_snu(&pcm, &spec, Some(&template)).unwrap();
        assert_eq!(snu.len() % 16, 0);
        assert_eq!(snu[1], 8);
        assert_eq!(snu[3], 1);
        assert_eq!(snu[0x0C], 0x80);
        assert_eq!(u32::from_le_bytes(snu[4..8].try_into().unwrap()), 1000);
        let sample = Sample::parse_snu(&snu).unwrap();
        assert_eq!(sample.decode().unwrap().len(), 32000);
    }

    #[test]
    fn fit_pcm_converts_channels_and_rate() {
        let pcm = Pcm {
            channels: 2,
            sample_rate: 44100,
            samples: [1000, 3000].repeat(44100),
        };
        let mono = fit_pcm(&pcm, 1, 22050);
        assert_eq!(mono.channels, 1);
        assert!((mono.frames() as i64 - 22050).abs() <= 1);
        assert!(
            mono.samples[100..200]
                .iter()
                .all(|&v| (v - 2000).abs() <= 2)
        );
        let stereo = fit_pcm(&mono, 2, 22050);
        assert_eq!(stereo.samples[200], stereo.samples[201]);
    }

    fn bank(samples: &[Vec<u8>]) -> Vec<u8> {
        let mut d = vec![0u8; 0x20];
        d[..4].copy_from_slice(b"sbnk");
        d[0x10..0x14].copy_from_slice(&0x20u32.to_le_bytes());
        d.extend_from_slice(b"sdat");
        d.extend_from_slice(&(samples.len() as u32).to_le_bytes());
        let mut at = 8 + 16 * samples.len();
        let mut body = Vec::new();
        for (i, s) in samples.iter().enumerate() {
            at = at.next_multiple_of(16);
            body.resize(at - 8 - 16 * samples.len(), 0);
            for v in [i as u32, 0x0313_BABE, at as u32, 0xFEED_FEED] {
                d.extend_from_slice(&v.to_le_bytes());
            }
            body.extend_from_slice(s);
            at += s.len();
        }
        d.extend_from_slice(&body);
        let size = (d.len() - 0x20) as u32;
        d[0x0C..0x10].copy_from_slice(&size.to_le_bytes());
        d[0x14..0x18].copy_from_slice(&size.to_le_bytes());
        d
    }

    #[test]
    fn sbk_sound_is_replaced() {
        let spec = SampleSpec {
            channels: 1,
            sample_rate: 22050,
            kind: 0,
            loop_start: None,
        };
        let make = |n: usize, f: f64| {
            let pcm = Pcm {
                channels: 1,
                sample_rate: 22050,
                samples: tone(n, 22050, f, 7000.0),
            };
            build_inline(&pcm, &spec).unwrap()
        };
        let original = bank(&[make(300, 200.0), make(500, 400.0), make(700, 800.0)]);
        let before = parse_sbk(&original).unwrap();
        let new = make(4000, 1000.0);
        let rebuilt = replace_sbk_sound(&original, 1, &new).unwrap();
        let after = parse_sbk(&rebuilt).unwrap();
        assert_eq!(after.len(), 3);
        assert_eq!(after[1].sample.header.sample_count, 4000);
        for i in [0, 2] {
            assert_eq!(
                after[i].sample.decode().unwrap(),
                before[i].sample.decode().unwrap()
            );
            assert_eq!(after[i].index, before[i].index);
        }
        let size = u32::from_le_bytes(rebuilt[0x14..0x18].try_into().unwrap()) as usize;
        assert_eq!(size, rebuilt.len() - 0x20);
        assert_eq!(rebuilt[0x0C..0x10], rebuilt[0x14..0x18]);
        // Putting the original back gives the original bank.
        let orig_sample = &original[before[1].sample.range.clone()];
        assert_eq!(
            replace_sbk_sound(&rebuilt, 1, orig_sample).unwrap(),
            original
        );
        assert!(replace_sbk_sound(&original, 3, &new).is_err());
        assert!(replace_sbk_sound(&original, 0, b"junk").is_err());
    }
}

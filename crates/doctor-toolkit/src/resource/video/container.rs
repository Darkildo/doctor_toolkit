//! The EA movie container of `.vp6` files: a flat list of chunks.
//!
//! ```text
//! chunk: [4] tag, u32 LE size including these 8 bytes, body
//! MVhd  video header (24 bytes): [4] codec "vp60", u16 width, u16 height,
//!       u32 frame count, u32 largest frame, u32 rate numerator, u32 rate denominator
//!       (frames per second = numerator / denominator, 982026 / 32767 ≈ 29.97)
//! MV0K  VP6 key frame        MV0F  VP6 inter frame
//! SCHl  audio header: "GSTR" + u32, then EA "PT" elements (below)
//! SCCl  u32 BE number of SCDl chunks
//! SCDl  audio data (EA-XA, see eaxa.rs)
//! SCEl  end of audio (empty)
//! ```
//!
//! PT elements: `u8 key, u8 length, value (big-endian)`; `0xFD` opens the audio
//! sub-header, `0xFF` ends it. In the sub-header `0x80` revision (3), `0x82`
//! channels, `0x84` sample rate (48000 when absent), `0x85` sample count,
//! `0xA0` codec (10 = EA-XA when absent, 23 = EALayer3).

use serde::Serialize;

use crate::{Error, Result};

pub const MVHD: [u8; 4] = *b"MVhd";
pub const MV0K: [u8; 4] = *b"MV0K";
pub const MV0F: [u8; 4] = *b"MV0F";
pub const SCHL: [u8; 4] = *b"SCHl";
pub const SCCL: [u8; 4] = *b"SCCl";
pub const SCDL: [u8; 4] = *b"SCDl";
pub const SCEL: [u8; 4] = *b"SCEl";

/// Codec values of the movie player (`0xA0` element).
pub const AUDIO_EAXA: u32 = 10;
pub const AUDIO_EAL3: u32 = 23;

/// One chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chunk<'a> {
    pub tag: [u8; 4],
    /// Offset of the chunk header in the file.
    pub offset: usize,
    pub body: &'a [u8],
}

impl Chunk<'_> {
    pub fn tag_str(&self) -> String {
        String::from_utf8_lossy(&self.tag).into_owned()
    }
}

/// Splits a movie into chunks. The chunks must cover the file exactly.
pub fn chunks(data: &[u8]) -> Result<Vec<Chunk<'_>>> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < data.len() {
        let head = data
            .get(at..at + 8)
            .ok_or_else(|| Error::malformed("EA movie", at, "truncated chunk header"))?;
        let size = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
        if size < 8 || at + size > data.len() {
            return Err(Error::malformed(
                "EA movie",
                at,
                format!("chunk size {size}"),
            ));
        }
        out.push(Chunk {
            tag: [head[0], head[1], head[2], head[3]],
            offset: at,
            body: &data[at + 8..at + size],
        });
        at += size;
    }
    Ok(out)
}

/// Appends a chunk.
pub fn put_chunk(out: &mut Vec<u8>, tag: [u8; 4], body: &[u8]) {
    out.extend_from_slice(&tag);
    out.extend_from_slice(&((body.len() + 8) as u32).to_le_bytes());
    out.extend_from_slice(body);
}

/// Writes chunks back; for chunks of a parsed file the result is identical.
pub fn write_chunks(chunks: &[Chunk<'_>]) -> Vec<u8> {
    let mut out = Vec::with_capacity(chunks.iter().map(|c| c.body.len() + 8).sum());
    for c in chunks {
        put_chunk(&mut out, c.tag, c.body);
    }
    out
}

/// The `MVhd` video header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct VideoHeader {
    #[serde(serialize_with = "fourcc")]
    pub codec: [u8; 4],
    pub width: u16,
    pub height: u16,
    pub frame_count: u32,
    pub largest_frame: u32,
    pub rate_num: u32,
    pub rate_den: u32,
}

fn fourcc<S: serde::Serializer>(v: &[u8; 4], s: S) -> std::result::Result<S::Ok, S::Error> {
    s.serialize_str(&String::from_utf8_lossy(v))
}

/// The rate of the game's movies: 982026 / 32767 ≈ 29.97 frames per second.
pub const NTSC_RATE: (u32, u32) = (982026, 32767);

impl VideoHeader {
    pub fn parse(body: &[u8]) -> Result<Self> {
        if body.len() < 24 {
            return Err(Error::malformed("MVhd", 0, "header shorter than 24 bytes"));
        }
        let u16le = |i: usize| u16::from_le_bytes([body[i], body[i + 1]]);
        let u32le = |i: usize| u32::from_le_bytes([body[i], body[i + 1], body[i + 2], body[i + 3]]);
        Ok(Self {
            codec: [body[0], body[1], body[2], body[3]],
            width: u16le(4),
            height: u16le(6),
            frame_count: u32le(8),
            largest_frame: u32le(12),
            rate_num: u32le(16),
            rate_den: u32le(20),
        })
    }

    pub fn to_bytes(&self) -> [u8; 24] {
        let mut b = [0u8; 24];
        b[..4].copy_from_slice(&self.codec);
        b[4..6].copy_from_slice(&self.width.to_le_bytes());
        b[6..8].copy_from_slice(&self.height.to_le_bytes());
        b[8..12].copy_from_slice(&self.frame_count.to_le_bytes());
        b[12..16].copy_from_slice(&self.largest_frame.to_le_bytes());
        b[16..20].copy_from_slice(&self.rate_num.to_le_bytes());
        b[20..24].copy_from_slice(&self.rate_den.to_le_bytes());
        b
    }

    pub fn fps(&self) -> f64 {
        self.rate_num as f64 / self.rate_den.max(1) as f64
    }
}

/// The `SCHl` audio header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AudioHeader {
    pub channels: u32,
    pub sample_rate: u32,
    pub sample_count: u32,
    pub revision: Option<u32>,
    /// `0xA0` element; 10 (EA-XA) when absent.
    pub codec: u32,
}

impl AudioHeader {
    pub fn parse(body: &[u8]) -> Result<Self> {
        let bad = |at: usize, why: &str| Error::malformed("SCHl", at, why.to_owned());
        let mut at = if body.get(..4) == Some(b"GSTR") {
            8
        } else if body.first() == Some(&b'P') {
            4
        } else {
            8
        };
        let mut h = Self {
            channels: 1,
            sample_rate: 48000,
            sample_count: 0,
            revision: None,
            codec: AUDIO_EAXA,
        };
        let mut in_sub = false;
        loop {
            let key = *body.get(at).ok_or_else(|| bad(at, "no end marker"))?;
            at += 1;
            match key {
                0xFF => break,
                0xFD => {
                    in_sub = true;
                    continue;
                }
                _ => {}
            }
            let len = *body.get(at).ok_or_else(|| bad(at, "truncated element"))? as usize;
            let raw = body
                .get(at + 1..at + 1 + len)
                .ok_or_else(|| bad(at, "truncated element"))?;
            at += 1 + len;
            let value = raw.iter().fold(0u32, |v, &b| v << 8 | b as u32);
            if in_sub {
                match key {
                    0x80 => h.revision = Some(value),
                    0x82 => h.channels = value,
                    0x84 => h.sample_rate = value,
                    0x85 => h.sample_count = value,
                    0xA0 => h.codec = value,
                    0x8A => in_sub = false,
                    _ => {}
                }
            }
        }
        if h.channels == 0 || h.channels > 8 {
            return Err(bad(0, "channel count"));
        }
        Ok(h)
    }

    /// An `SCHl` body for EA-XA revision 3, laid out like the game's movies.
    pub fn build(channels: u32, sample_rate: u32, sample_count: u32) -> Vec<u8> {
        let mut b = b"GSTR".to_vec();
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&[0x00, 0x04, 0x1D, 0xF8, 0x51, 0xE0]);
        b.extend_from_slice(&[0x05, 0x01, 0x64, 0x06, 0x01, 0x65]);
        b.push(0xFD);
        b.extend_from_slice(&[0x80, 0x01, 0x03]);
        let mut element = |key: u8, value: u32| {
            let bytes = value.to_be_bytes();
            let skip = bytes.iter().take_while(|&&x| x == 0).count().min(3);
            b.push(key);
            b.push((4 - skip) as u8);
            b.extend_from_slice(&bytes[skip..]);
        };
        element(0x85, sample_count);
        element(0x82, channels);
        if sample_rate != 48000 {
            element(0x84, sample_rate);
        }
        b.push(0xFF);
        while !(b.len() + 8).is_multiple_of(4) {
            b.push(0);
        }
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_header_from_game_bytes() {
        let body = hex("475354520100000000041df851e0050164060165fd80010385031ad23d820102ff000000");
        let h = AudioHeader::parse(&body).unwrap();
        assert_eq!(h.channels, 2);
        assert_eq!(h.sample_rate, 48000);
        assert_eq!(h.sample_count, 0x1ad23d);
        assert_eq!(h.revision, Some(3));
        assert_eq!(h.codec, AUDIO_EAXA);
        let built = AudioHeader::build(2, 48000, 0x1ad23d);
        assert_eq!(built, body);
        let other = AudioHeader::parse(&AudioHeader::build(1, 22050, 5)).unwrap();
        assert_eq!(
            (other.channels, other.sample_rate, other.sample_count),
            (1, 22050, 5)
        );
    }

    #[test]
    fn video_header_and_chunks() {
        let body = hex("767036300005d002840300001af501000afc0e00ff7f0000");
        let h = VideoHeader::parse(&body).unwrap();
        assert_eq!((h.width, h.height, h.frame_count), (1280, 720, 900));
        assert_eq!((h.rate_num, h.rate_den), NTSC_RATE);
        assert_eq!(h.to_bytes().to_vec(), body);
        let mut file = Vec::new();
        put_chunk(&mut file, MVHD, &body);
        put_chunk(&mut file, SCEL, &[]);
        let c = chunks(&file).unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!(write_chunks(&c), file);
        assert!(chunks(&file[..file.len() - 1]).is_err());
    }

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }
}

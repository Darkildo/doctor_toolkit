//! EA AudioCore sample formats: SNR headers, SNS data blocks, SNU files and
//! `sbk` sound banks, with an EA-XAS v1 decoder.
//!
//! # SNR header (8 or 12 bytes, big-endian)
//!
//! ```text
//! u32  version:4 codec:4 channels-1:6 sample rate:18
//! u32  type:2 (0 RAM, 1 stream, 2 gigastream) loop:1 sample count:29
//! u32  loop start, only when the loop flag is set
//! ```
//!
//! Every sample in The Godfather II uses codec 4, EA-XAS v1.
//!
//! # SNS blocks (big-endian)
//!
//! ```text
//! u8   0x00, or 0x80 for the last block
//! u24  block size including this header
//! u32  samples in the block
//!      EA-XAS frames: 0x4C bytes per channel for 128 samples, channels interleaved
//! ```
//!
//! # SNU file (`.exa.snu` in the `GODFATH*.viv` archives)
//!
//! ```text
//! 0x00  u8   3 (two files have 1 here and 0 at 0x03)
//! 0x01  u8   0, 4, 8 or 12 (unknown)
//! 0x03  u8   channel count
//! 0x04  u32  unknown
//! 0x08  u32  offset of the SNS blocks (little-endian), 0x20
//! 0x10  SNR header
//! ```
//!
//! # `sbk` sound bank
//!
//! ```text
//! 0x00  "sbnk"
//! 0x04  u32  version, 0xCE
//! 0x10  u32  offset of the "sdat" section
//! 0x14  u32  size of the "sdat" section
//! sdat: "sdat", u32 count, count × {u32 index, u32 0x0313BABE, u32 offset from sdat, u32 0xFEEDFEED}
//!       each offset points at an SNR header followed by its SNS blocks (RAM samples)
//! ```
//!
//! The rest of the `sbnk` header (sound definitions, hashes) is not decoded.

use serde::Serialize;

use crate::io::{self, Endian};
use crate::{Error, Result};

/// Type id of `sbk` resources.
pub const TYPE_SBK: u32 = crate::hash::sdbm(b"sbk");

/// EA-XAS v1.
pub const CODEC_XAS1: u8 = 4;

const XAS_FRAME: usize = 0x4C;
const XAS_SAMPLES: usize = 128;
/// Refuse to decode more than this many samples per channel (about 3 hours at 48 kHz).
const MAX_SAMPLES: usize = 1 << 29;

/// A parsed SNR header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SnrHeader {
    pub version: u8,
    pub codec: u8,
    pub channels: u8,
    pub sample_rate: u32,
    /// 0 RAM, 1 stream, 2 gigastream.
    pub kind: u8,
    pub sample_count: u32,
    pub loop_start: Option<u32>,
}

impl SnrHeader {
    pub fn parse(data: &[u8], offset: usize) -> Result<Self> {
        let be = Endian::Big;
        let a = io::read_u32(data, offset, be)?;
        let b = io::read_u32(data, offset + 4, be)?;
        let looped = b >> 29 & 1 != 0;
        let header = Self {
            version: (a >> 28) as u8,
            codec: (a >> 24 & 0xF) as u8,
            channels: (a >> 18 & 0x3F) as u8 + 1,
            sample_rate: a & 0x3_FFFF,
            kind: (b >> 30) as u8,
            sample_count: b & 0x1FFF_FFFF,
            loop_start: if looped {
                Some(io::read_u32(data, offset + 8, be)?)
            } else {
                None
            },
        };
        if header.version > 1 || header.sample_rate == 0 {
            return Err(Error::malformed(
                "SNR",
                offset,
                format!("not an SNR header: {a:08x} {b:08x}"),
            ));
        }
        Ok(header)
    }

    /// Size of the header in bytes.
    pub fn size(&self) -> usize {
        if self.loop_start.is_some() { 12 } else { 8 }
    }
}

/// One SNS block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnsBlock<'a> {
    pub last: bool,
    pub samples: u32,
    pub data: &'a [u8],
}

/// Walks SNS blocks from `offset` until the last block or `sample_count` samples.
pub fn sns_blocks(data: &[u8], offset: usize, sample_count: u32) -> Result<Vec<SnsBlock<'_>>> {
    let mut blocks = Vec::new();
    let mut at = offset;
    let mut total = 0u64;
    while total < sample_count as u64 {
        let head = io::read_u32(data, at, Endian::Big)?;
        let size = (head & 0xFF_FFFF) as usize;
        let last = head >> 24 == 0x80;
        if !(head >> 24 == 0 || last) || size < 8 {
            return Err(Error::malformed(
                "SNS",
                at,
                format!("block header {head:08x}"),
            ));
        }
        let samples = io::read_u32(data, at + 4, Endian::Big)?;
        blocks.push(SnsBlock {
            last,
            samples,
            data: io::slice(data, at + 8, size - 8)?,
        });
        total += samples as u64;
        at += size;
        if last {
            break;
        }
    }
    Ok(blocks)
}

/// A sample: header and data blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sample<'a> {
    pub header: SnrHeader,
    pub blocks: Vec<SnsBlock<'a>>,
    /// Byte range of header and blocks in the source.
    pub range: std::ops::Range<usize>,
}

impl<'a> Sample<'a> {
    /// An SNR header at `offset` followed directly by its SNS blocks.
    pub fn parse_inline(data: &'a [u8], offset: usize) -> Result<Self> {
        let header = SnrHeader::parse(data, offset)?;
        let start = offset + header.size();
        let blocks = sns_blocks(data, start, header.sample_count)?;
        let end = start + blocks.iter().map(|b| b.data.len() + 8).sum::<usize>();
        Ok(Self {
            header,
            blocks,
            range: offset..end,
        })
    }

    /// An `.snu` file.
    pub fn parse_snu(data: &'a [u8]) -> Result<Self> {
        let header = SnrHeader::parse(data, 0x10)?;
        let start = io::read_u32(data, 8, Endian::Little)? as usize;
        let blocks = sns_blocks(data, start, header.sample_count)?;
        Ok(Self {
            header,
            blocks,
            range: 0..data.len(),
        })
    }

    /// Duration in seconds.
    pub fn duration(&self) -> f64 {
        self.header.sample_count as f64 / self.header.sample_rate as f64
    }

    /// Decodes to interleaved 16-bit PCM.
    pub fn decode(&self) -> Result<Vec<i16>> {
        if self.header.codec != CODEC_XAS1 {
            return Err(Error::InvalidArgument(format!(
                "codec {} is not supported (only EA-XAS v1)",
                self.header.codec
            )));
        }
        let channels = self.header.channels as usize;
        let total = (self.header.sample_count as usize).min(MAX_SAMPLES);
        let mut out = Vec::with_capacity(total.saturating_mul(channels).min(1 << 26));
        let mut frame = [0i16; XAS_SAMPLES];
        let mut done = 0usize;
        for block in &self.blocks {
            let wanted = (block.samples as usize).min(total - done);
            let frames = wanted.div_ceil(XAS_SAMPLES);
            if block.data.len() < frames * XAS_FRAME * channels {
                return Err(Error::malformed("SNS", 0, "block shorter than its samples"));
            }
            let mut decoded = vec![vec![0i16; frames * XAS_SAMPLES]; channels];
            for f in 0..frames {
                for (c, channel) in decoded.iter_mut().enumerate() {
                    let at = (f * channels + c) * XAS_FRAME;
                    decode_xas_frame(&block.data[at..at + XAS_FRAME], &mut frame);
                    channel[f * XAS_SAMPLES..][..XAS_SAMPLES].copy_from_slice(&frame);
                }
            }
            for i in 0..wanted {
                for channel in &decoded {
                    out.push(channel[i]);
                }
            }
            done += wanted;
            if done >= total {
                break;
            }
        }
        Ok(out)
    }

    /// Decodes to a RIFF WAVE file.
    pub fn to_wav(&self) -> Result<Vec<u8>> {
        Ok(wav(
            &self.decode()?,
            self.header.channels as u16,
            self.header.sample_rate,
        ))
    }
}

/// EA-XA prediction coefficients (×256).
const COEFS: [(i32, i32); 4] = [(0, 0), (240, 0), (460, -208), (392, -220)];

/// Decodes one EA-XAS v1 frame of one channel: four groups of 32 samples.
///
/// Layout: 4 group headers (two little-endian i16 each: the first sample with
/// the coefficient index in its low nibble, the second with the shift), then 15
/// rows of 4 bytes; byte `g` of each row holds two samples of group `g`, high nibble first.
pub fn decode_xas_frame(frame: &[u8], out: &mut [i16; XAS_SAMPLES]) {
    for group in 0..4 {
        let h0 = i16::from_le_bytes([frame[4 * group], frame[4 * group + 1]]);
        let h1 = i16::from_le_bytes([frame[4 * group + 2], frame[4 * group + 3]]);
        let (c1, c2) = COEFS[(h0 & 3) as usize];
        let shift = 20 - (h1 & 0x0F) as i32;
        let s = &mut out[group * 32..group * 32 + 32];
        s[0] = h0 & !0x0F;
        s[1] = h1 & !0x0F;
        for row in 0..15 {
            let byte = frame[16 + 4 * row + group];
            for (k, nibble) in [byte >> 4, byte & 0x0F].into_iter().enumerate() {
                let i = 2 + 2 * row + k;
                let level = (((nibble as i32) << 28) >> 28) << shift;
                let pred = s[i - 1] as i32 * c1 + s[i - 2] as i32 * c2;
                s[i] = ((level + pred + 0x80) >> 8).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            }
        }
    }
}

/// A 16-bit PCM RIFF WAVE file.
pub fn wav(samples: &[i16], channels: u16, sample_rate: u32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * channels as u32 * 2).to_le_bytes());
    out.extend_from_slice(&(channels * 2).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Builds a standalone `.snu` file around an inline SNR header and SNS blocks
/// (players such as vgmstream read it).
pub fn to_snu(source: &[u8], sample: &Sample<'_>) -> Vec<u8> {
    let header_len = sample.header.size();
    let body = &source[sample.range.clone()];
    let mut out = vec![0u8; 0x10];
    out[0] = 3;
    out[3] = sample.header.channels;
    let start = (0x10 + header_len).next_multiple_of(4);
    out[8..12].copy_from_slice(&(start as u32).to_le_bytes());
    out.extend_from_slice(&body[..header_len]);
    out.resize(start, 0);
    out.extend_from_slice(&body[header_len..]);
    out
}

/// One sound of an `sbk` bank.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BankSound<'a> {
    pub index: u32,
    pub sample: Sample<'a>,
}

/// Parses the sounds of an `sbk` resource.
pub fn parse_sbk(data: &[u8]) -> Result<Vec<BankSound<'_>>> {
    let le = Endian::Little;
    if io::slice(data, 0, 4)? != b"sbnk" {
        return Err(Error::BadMagic {
            what: "sbk",
            found: io::read_u32(data, 0, le)?,
        });
    }
    let sdat = io::read_u32(data, 0x10, le)? as usize;
    if io::slice(data, sdat, 4)? != b"sdat" {
        return Err(Error::malformed("sbk", sdat, "missing sdat section"));
    }
    let count = io::read_u32(data, sdat + 4, le)? as usize;
    io::slice(data, sdat + 8, count.saturating_mul(16))?;
    (0..count)
        .map(|i| {
            let entry = sdat + 8 + 16 * i;
            let offset = io::read_u32(data, entry + 8, le)? as usize;
            Ok(BankSound {
                index: io::read_u32(data, entry, le)?,
                sample: Sample::parse_inline(data, sdat + offset)?,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mono XAS frame whose groups hold a constant level with coefficient 1 (0.9375).
    fn frame() -> Vec<u8> {
        let mut f = vec![0u8; XAS_FRAME];
        for g in 0..4 {
            // first sample 0x0100 with coefficient index 0, second 0x0100 with shift 12
            f[4 * g..4 * g + 2].copy_from_slice(&0x0100i16.to_le_bytes());
            f[4 * g + 2..4 * g + 4].copy_from_slice(&(0x0100i16 | 12).to_le_bytes());
        }
        f
    }

    #[test]
    fn xas_frame_decodes_headers_and_nibbles() {
        let mut f = frame();
        f[16] = 0x10; // group 0, row 0: +1 then 0
        let mut out = [0i16; 128];
        decode_xas_frame(&f, &mut out);
        assert_eq!(out[0], 0x100);
        assert_eq!(out[1], 0x100);
        // coefficient 0: prediction 0; level 1 << (20 - 12) = 256 → (256 + 128) >> 8 = 1
        assert_eq!(out[2], 1);
        assert_eq!(out[3], 0);
        assert_eq!(out[32], 0x100);
    }

    fn snr(channels: u8, rate: u32, samples: u32) -> [u8; 8] {
        let a = (CODEC_XAS1 as u32) << 24 | ((channels as u32 - 1) << 18) | rate;
        let mut out = [0u8; 8];
        out[..4].copy_from_slice(&a.to_be_bytes());
        out[4..].copy_from_slice(&samples.to_be_bytes());
        out
    }

    fn inline_sample(samples: u32) -> Vec<u8> {
        let mut d = snr(1, 24000, samples).to_vec();
        let frames = (samples as usize).div_ceil(128);
        let size = 8 + frames * XAS_FRAME;
        d.extend_from_slice(&((0x80 << 24) | size as u32).to_be_bytes());
        d.extend_from_slice(&samples.to_be_bytes());
        for _ in 0..frames {
            d.extend_from_slice(&frame());
        }
        d
    }

    #[test]
    fn inline_sample_decodes_to_wav() {
        let data = inline_sample(200);
        let sample = Sample::parse_inline(&data, 0).unwrap();
        assert_eq!(sample.header.sample_rate, 24000);
        assert_eq!(sample.header.channels, 1);
        assert_eq!(sample.range, 0..data.len());
        let pcm = sample.decode().unwrap();
        assert_eq!(pcm.len(), 200);
        let wav = sample.to_wav().unwrap();
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(wav.len(), 44 + 400);
        // Wrapping into SNU and parsing back gives the same samples.
        let snu = to_snu(&data, &sample);
        assert_eq!(Sample::parse_snu(&snu).unwrap().decode().unwrap(), pcm);
    }

    #[test]
    fn sbk_lists_its_sounds() {
        let mut d = vec![0u8; 0x20];
        d[..4].copy_from_slice(b"sbnk");
        d[0x10..0x14].copy_from_slice(&0x20u32.to_le_bytes());
        d.extend_from_slice(b"sdat");
        d.extend_from_slice(&1u32.to_le_bytes());
        for v in [7u32, 0x0313_BABE, 0x18, 0xFEED_FEED] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.extend_from_slice(&inline_sample(128));
        let sounds = parse_sbk(&d).unwrap();
        assert_eq!(sounds.len(), 1);
        assert_eq!(sounds[0].index, 7);
        assert_eq!(sounds[0].sample.header.sample_count, 128);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(SnrHeader::parse(&[0xF4, 0, 0x5D, 0xC0, 0, 0, 0, 1], 0).is_err());
        let mut data = inline_sample(200);
        data[8] = 0x55; // bad block flag
        assert!(Sample::parse_inline(&data, 0).is_err());
        let data = inline_sample(200);
        assert!(Sample::parse_inline(&data[..data.len() - 1], 0).is_err());
        assert!(parse_sbk(b"sbnk").is_err());
        assert!(Sample::parse_snu(&[]).is_err());
    }
}

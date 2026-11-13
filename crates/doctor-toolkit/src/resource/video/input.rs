//! Inputs and outputs for movie import and export: YUV4MPEG2 video, WAV and
//! AIFF audio, VP6 frames from AVI and FLV files.

use std::io::Read;

use super::vp6::{FrameHeader, Picture};
use crate::{Error, Result};

/// Reads 4:2:0 frames from a YUV4MPEG2 stream.
pub struct Y4mReader<R> {
    reader: R,
    pub width: usize,
    pub height: usize,
    /// Frame rate as a fraction (`F` parameter), if given.
    pub rate: Option<(u32, u32)>,
}

impl<R: Read> Y4mReader<R> {
    pub fn new(mut reader: R) -> Result<Self> {
        let line =
            read_line(&mut reader)?.ok_or_else(|| Error::malformed("Y4M", 0, "empty stream"))?;
        let mut parts = line.split(' ');
        if parts.next() != Some("YUV4MPEG2") {
            return Err(Error::malformed("Y4M", 0, "not a YUV4MPEG2 stream"));
        }
        let (mut width, mut height, mut rate) = (0, 0, None);
        for p in parts {
            let (k, v) = p.split_at(1.min(p.len()));
            match k {
                "W" => width = v.parse().unwrap_or(0),
                "H" => height = v.parse().unwrap_or(0),
                "F" => {
                    rate = v
                        .split_once(':')
                        .and_then(|(n, d)| Some((n.parse().ok()?, d.parse().ok()?)))
                        .filter(|&(n, d): &(u32, u32)| n > 0 && d > 0)
                }
                "C" if !v.starts_with("420") => {
                    return Err(Error::InvalidArgument(format!(
                        "Y4M colour space C{v}: only 4:2:0 is supported \
                         (convert with `ffmpeg -i in -pix_fmt yuv420p out.y4m`)"
                    )));
                }
                _ => {}
            }
        }
        if width == 0 || height == 0 || width % 2 != 0 || height % 2 != 0 {
            return Err(Error::InvalidArgument(format!(
                "Y4M size {width}x{height}: width and height must be even"
            )));
        }
        Ok(Self {
            reader,
            width,
            height,
            rate,
        })
    }

    /// The next frame, or `None` at the end of the stream.
    pub fn next_frame(&mut self) -> Result<Option<Picture>> {
        let Some(line) = read_line(&mut self.reader)? else {
            return Ok(None);
        };
        if !line.starts_with("FRAME") {
            return Err(Error::malformed("Y4M", 0, "expected FRAME"));
        }
        let mut p = Picture::new(self.width, self.height);
        for plane in &mut p.planes {
            self.reader
                .read_exact(plane)
                .map_err(|e| Error::malformed("Y4M", 0, format!("truncated frame: {e}")))?;
        }
        Ok(Some(p))
    }
}

fn read_line(r: &mut impl Read) -> Result<Option<String>> {
    let mut out = Vec::new();
    let mut b = [0u8; 1];
    loop {
        match r.read(&mut b) {
            Ok(0) if out.is_empty() => return Ok(None),
            Ok(0) => break,
            Ok(_) if b[0] == b'\n' => break,
            Ok(_) => out.push(b[0]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(Error::malformed("Y4M", 0, e.to_string())),
        }
        if out.len() > 4096 {
            return Err(Error::malformed("Y4M", 0, "header line too long"));
        }
    }
    Ok(Some(String::from_utf8_lossy(&out).into_owned()))
}

/// Writes a YUV4MPEG2 header.
pub fn y4m_header(width: usize, height: usize, rate: (u32, u32)) -> String {
    format!(
        "YUV4MPEG2 W{width} H{height} F{}:{} Ip A1:1 C420jpeg\n",
        rate.0, rate.1
    )
}

/// 16-bit PCM audio, channels interleaved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcm {
    pub channels: u16,
    pub sample_rate: u32,
    pub samples: Vec<i16>,
}

impl Pcm {
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1) as usize
    }

    /// One channel.
    pub fn channel(&self, c: usize) -> Vec<i16> {
        let n = self.channels as usize;
        self.samples.iter().skip(c).step_by(n).copied().collect()
    }
}

/// Reads a RIFF WAVE file with 8-, 16-, 24- or 32-bit integer or 32-bit float samples.
pub fn read_wav(data: &[u8]) -> Result<Pcm> {
    let bad = |why: &str| Error::malformed("WAV", 0, why.to_owned());
    if data.len() < 12 || &data[..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return Err(bad("not a RIFF WAVE file"));
    }
    let mut at = 12;
    let mut fmt = None;
    while at + 8 <= data.len() {
        let id = &data[at..at + 4];
        let size = u32::from_le_bytes(data[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = data
            .get(at + 8..(at + 8 + size).min(data.len()))
            .unwrap_or(&[]);
        if id == b"fmt " && body.len() >= 16 {
            let mut tag = u16::from_le_bytes([body[0], body[1]]);
            let channels = u16::from_le_bytes([body[2], body[3]]);
            let rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
            let bits = u16::from_le_bytes([body[14], body[15]]);
            if tag == 0xFFFE && body.len() >= 26 {
                tag = u16::from_le_bytes([body[24], body[25]]);
            }
            fmt = Some((tag, channels, rate, bits));
        } else if id == b"data" {
            let (tag, channels, rate, bits) = fmt.ok_or_else(|| bad("data before fmt"))?;
            if channels == 0 || rate == 0 {
                return Err(bad("bad format"));
            }
            let samples: Vec<i16> = match (tag, bits) {
                (1, 8) => body.iter().map(|&b| ((b as i16) - 128) << 8).collect(),
                (1, 16) => body
                    .chunks_exact(2)
                    .map(|c| i16::from_le_bytes([c[0], c[1]]))
                    .collect(),
                (1, 24) => body
                    .chunks_exact(3)
                    .map(|c| i16::from_le_bytes([c[1], c[2]]))
                    .collect(),
                (1, 32) => body
                    .chunks_exact(4)
                    .map(|c| i16::from_le_bytes([c[2], c[3]]))
                    .collect(),
                (3, 32) => body
                    .chunks_exact(4)
                    .map(|c| {
                        let f = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                        (f * 32767.0).round().clamp(-32768.0, 32767.0) as i16
                    })
                    .collect(),
                _ => {
                    return Err(Error::InvalidArgument(format!(
                        "WAV format {tag} with {bits} bits is not supported"
                    )));
                }
            };
            let whole = samples.len() / channels as usize * channels as usize;
            return Ok(Pcm {
                channels,
                sample_rate: rate,
                samples: samples[..whole].to_vec(),
            });
        }
        at += 8 + size + (size & 1);
    }
    Err(bad("no data chunk"))
}

/// Reads an uncompressed AIFF file (16-bit big-endian PCM), such as
/// `movies/common/logo.vp6`.
pub fn read_aiff(data: &[u8]) -> Result<Pcm> {
    let bad = |why: &str| Error::malformed("AIFF", 0, why.to_owned());
    if data.len() < 12 || &data[..4] != b"FORM" || &data[8..12] != b"AIFF" {
        return Err(bad("not an AIFF file"));
    }
    let mut at = 12;
    let mut comm = None;
    while at + 8 <= data.len() {
        let id = &data[at..at + 4];
        let size = u32::from_be_bytes(data[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = data
            .get(at + 8..(at + 8 + size).min(data.len()))
            .unwrap_or(&[]);
        if id == b"COMM" && body.len() >= 18 {
            let channels = u16::from_be_bytes([body[0], body[1]]);
            let bits = u16::from_be_bytes([body[6], body[7]]);
            comm = Some((channels, bits, extended_to_u32(&body[8..18])));
        } else if id == b"SSND" && body.len() >= 8 {
            let (channels, bits, rate) = comm.ok_or_else(|| bad("SSND before COMM"))?;
            if bits != 16 || channels == 0 {
                return Err(Error::InvalidArgument(format!(
                    "AIFF with {bits}-bit samples is not supported"
                )));
            }
            let offset = u32::from_be_bytes(body[0..4].try_into().unwrap()) as usize;
            let pcm = body.get(8 + offset..).unwrap_or(&[]);
            let mut samples: Vec<i16> = pcm
                .chunks_exact(2)
                .map(|c| i16::from_be_bytes([c[0], c[1]]))
                .collect();
            samples.truncate(samples.len() / channels as usize * channels as usize);
            return Ok(Pcm {
                channels,
                sample_rate: rate,
                samples,
            });
        }
        at += 8 + size + (size & 1);
    }
    Err(bad("no SSND chunk"))
}

/// An 80-bit IEEE extended float (AIFF sample rate) as an integer.
fn extended_to_u32(b: &[u8]) -> u32 {
    let exp = (u16::from_be_bytes([b[0], b[1]]) & 0x7FFF) as i32 - 16383;
    let mant = u64::from_be_bytes(b[2..10].try_into().unwrap());
    if !(0..32).contains(&exp) {
        return 0;
    }
    (mant >> (63 - exp)) as u32
}

/// VP6 frames taken from another container.
#[derive(Debug, Clone)]
pub struct Vp6Frames {
    pub width: u16,
    pub height: u16,
    pub rate: Option<(u32, u32)>,
    pub frames: Vec<(bool, Vec<u8>)>,
    /// The frames are stored top-down (FLV `VP6F`): the game shows them upside down.
    pub top_down: bool,
}

fn frame_is_key(f: &[u8]) -> bool {
    f.first().is_some_and(|b| b & 0x80 == 0)
}

/// VP6 frames of an AVI file (`vids` stream with fourcc `VP60`, `VP61` or `VP62`).
pub fn read_avi_vp6(data: &[u8]) -> Result<Vp6Frames> {
    let bad = |at: usize, why: &str| Error::malformed("AVI", at, why.to_owned());
    if data.len() < 12 || &data[..4] != b"RIFF" || &data[8..12] != b"AVI " {
        return Err(bad(0, "not an AVI file"));
    }
    let le32 = |at: usize| -> u32 {
        data.get(at..at + 4)
            .map_or(0, |s| u32::from_le_bytes(s.try_into().unwrap()))
    };
    let mut out = Vp6Frames {
        width: 0,
        height: 0,
        rate: None,
        frames: Vec::new(),
        top_down: false,
    };
    let mut video_stream = None;
    let mut stream_no = 0u32;
    // Walk RIFF lists recursively with an explicit stack of (start, end).
    let mut stack = vec![(12usize, data.len())];
    while let Some((mut at, end)) = stack.pop() {
        while at + 8 <= end {
            let id = &data[at..at + 4];
            let size = le32(at + 4) as usize;
            let body_end = (at + 8 + size).min(end);
            if id == b"LIST" {
                // Siblings after the list, then the list itself (popped first).
                stack.push((body_end + (size & 1), end));
                stack.push((at + 12, body_end));
                break;
            }
            if id == b"strh" && size >= 36 {
                let kind = &data[at + 8..at + 12];
                let handler = data[at + 12..at + 16].to_ascii_uppercase();
                if kind == b"vids" {
                    if !handler.starts_with(b"VP6") || handler == *b"VP6F" {
                        return Err(Error::InvalidArgument(format!(
                            "AVI video codec {} is not VP6",
                            String::from_utf8_lossy(&handler)
                        )));
                    }
                    video_stream = Some(stream_no);
                    let scale = le32(at + 28);
                    let rate = le32(at + 32);
                    if scale > 0 && rate > 0 {
                        out.rate = Some((rate, scale));
                    }
                }
                stream_no += 1;
            } else if id == b"strf" && video_stream == Some(stream_no - 1) && size >= 12 {
                out.width = le32(at + 12) as u16;
                out.height = (le32(at + 16) as i32).unsigned_abs() as u16;
            } else if id.len() == 4 && (&id[2..] == b"dc" || &id[2..] == b"db") {
                let n = std::str::from_utf8(&id[..2])
                    .ok()
                    .and_then(|s| s.parse::<u32>().ok());
                if n.is_some() && n == video_stream && size > 0 {
                    let f = data[at + 8..body_end].to_vec();
                    out.frames.push((frame_is_key(&f), f));
                }
            }
            at = body_end + (size & 1);
        }
    }
    if video_stream.is_none() || out.frames.is_empty() {
        return Err(bad(0, "no VP6 video frames"));
    }
    Ok(out)
}

/// VP6 frames of an FLV file (codec 4, `VP6F`). Flash VP6 is coded top-down,
/// the game's movies bottom-up: flip the source picture before encoding it.
pub fn read_flv_vp6(data: &[u8]) -> Result<Vp6Frames> {
    let bad = |at: usize, why: &str| Error::malformed("FLV", at, why.to_owned());
    if data.len() < 9 || &data[..3] != b"FLV" {
        return Err(bad(0, "not an FLV file"));
    }
    let mut at = u32::from_be_bytes(data[5..9].try_into().unwrap()) as usize + 4;
    let mut out = Vp6Frames {
        width: 0,
        height: 0,
        rate: None,
        frames: Vec::new(),
        top_down: true,
    };
    let mut crop = (0u16, 0u16);
    while at + 11 <= data.len() {
        let kind = data[at] & 0x1F;
        let size = u32::from_be_bytes([0, data[at + 1], data[at + 2], data[at + 3]]) as usize;
        let body = data
            .get(at + 11..at + 11 + size)
            .ok_or_else(|| bad(at, "truncated tag"))?;
        if kind == 9 && body.len() > 2 {
            let codec = body[0] & 0x0F;
            if codec != 4 {
                return Err(Error::InvalidArgument(format!(
                    "FLV video codec {codec} is not VP6 (4)"
                )));
            }
            crop = ((body[1] >> 4) as u16, (body[1] & 0x0F) as u16);
            let f = body[2..].to_vec();
            if frame_is_key(&f)
                && let Ok(h) = FrameHeader::parse(&f)
            {
                out.width = h.display_cols as u16 * 16;
                out.height = h.display_rows as u16 * 16;
            }
            out.frames.push((frame_is_key(&f), f));
        }
        at += 11 + size + 4;
    }
    if out.frames.is_empty() {
        return Err(bad(0, "no VP6 video frames"));
    }
    out.width = out.width.saturating_sub(crop.0);
    out.height = out.height.saturating_sub(crop.1);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn y4m_round_trip() {
        let mut data = y4m_header(4, 2, (30, 1)).into_bytes();
        for i in 0..2u8 {
            data.extend_from_slice(b"FRAME\n");
            data.extend(std::iter::repeat_n(i, 8 + 2 + 2));
        }
        let mut r = Y4mReader::new(&data[..]).unwrap();
        assert_eq!((r.width, r.height, r.rate), (4, 2, Some((30, 1))));
        assert_eq!(r.next_frame().unwrap().unwrap().planes[0], vec![0; 8]);
        assert_eq!(r.next_frame().unwrap().unwrap().planes[2], vec![1; 2]);
        assert!(r.next_frame().unwrap().is_none());
        assert!(Y4mReader::new(&b"YUV4MPEG2 W4 H2 C444\n"[..]).is_err());
    }

    #[test]
    fn wav_and_aiff() {
        let pcm = vec![1i16, -2, 300, -400];
        let wav = crate::resource::audio::wav(&pcm, 2, 48000);
        let p = read_wav(&wav).unwrap();
        assert_eq!((p.channels, p.sample_rate, p.frames()), (2, 48000, 2));
        assert_eq!(p.samples, pcm);
        assert_eq!(p.channel(1), vec![-2, -400]);

        let mut aiff = b"FORM\0\0\0\0AIFFCOMM\0\0\0\x12".to_vec();
        aiff.extend_from_slice(&[0, 1, 0, 0, 0, 2, 0, 16]);
        aiff.extend_from_slice(&[0x40, 0x0E, 0xBB, 0x80, 0, 0, 0, 0, 0, 0]);
        aiff.extend_from_slice(b"SSND\0\0\0\x0c\0\0\0\0\0\0\0\0");
        aiff.extend_from_slice(&[0x01, 0x02, 0xFF, 0xFE]);
        let a = read_aiff(&aiff).unwrap();
        assert_eq!((a.channels, a.sample_rate), (1, 48000));
        assert_eq!(a.samples, vec![0x0102, -2]);
    }
}

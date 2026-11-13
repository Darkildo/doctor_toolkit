//! The VP6 decoder against ffmpeg on synthetic streams: frame headers of
//! every kind (sub-versions 6–8, filter headers, one or two partitions) with
//! random payloads, so the range coder reads random macroblock types,
//! vectors, filter settings (bicubic, deblocking), golden frames, model
//! updates and Huffman or range-coded coefficients. Both decoders must give
//! the same pictures byte for byte, damaged frames included.
//!
//! The game's movies use only part of the format (no filter header); the
//! corpus test `vp6_decoder_matches_ffmpeg` covers them. Skipped when
//! `ffmpeg` is not installed (`FFMPEG` may name it).

use std::process::{Command, Stdio};

use doctor_toolkit::resource::video::container::{self, VideoHeader};
use doctor_toolkit::resource::video::vp6::{Decoder, FrameStats};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
}

/// One synthetic stream: an EA movie of `count` frames.
fn stream(rng: &mut Rng, cols: u8, rows: u8, count: usize) -> Vec<u8> {
    let sub_version = [6u8, 7, 8][rng.below(3) as usize];
    let filter_header = [0u8, 2, 4, 6][rng.below(4) as usize];
    let mbs = cols as usize * rows as usize;
    let mut out = Vec::new();
    let header = VideoHeader {
        codec: *b"vp60",
        width: cols as u16 * 16,
        height: rows as u16 * 16,
        frame_count: count as u32,
        largest_frame: 0,
        rate_num: 30,
        rate_den: 1,
    };
    container::put_chunk(&mut out, container::MVHD, &header.to_bytes());
    for i in 0..count {
        let key = i == 0 || rng.below(25) == 0;
        let q = rng.below(64) as u8;
        let separated = rng.below(2) as u8;
        let mut frame = vec![if key { 0 } else { 0x80 } | q << 1 | separated];
        if key {
            frame.push(sub_version << 3 | filter_header);
        }
        let has_offset = separated == 1 || filter_header == 0;
        let at = frame.len();
        if has_offset {
            frame.extend_from_slice(&[0, 0]);
        }
        if key {
            frame.extend_from_slice(&[rows, cols, rows, cols]);
        }
        // Now and then a frame too short for its macroblocks: damaged, concealed.
        let short = i > 0 && rng.below(10) == 0;
        let part1 = if short {
            6 + mbs
        } else {
            40 + 120 * mbs + rng.below(50) as usize
        };
        frame.extend(rng.bytes(part1));
        if has_offset {
            // Sometimes one partition for everything (offset 2).
            let offset = if separated == 0 && rng.below(2) == 0 {
                2
            } else {
                frame.len() as u16
            };
            frame[at..at + 2].copy_from_slice(&offset.to_be_bytes());
            frame.extend(rng.bytes(if short { 3 + mbs } else { 200 * mbs }));
        }
        let tag = if key {
            container::MV0K
        } else {
            container::MV0F
        };
        container::put_chunk(&mut out, tag, &frame);
    }
    out
}

fn ffmpeg() -> Option<String> {
    let ffmpeg = std::env::var("FFMPEG").unwrap_or_else(|_| "ffmpeg".into());
    Command::new(&ffmpeg)
        .arg("-version")
        .output()
        .ok()
        .map(|_| ffmpeg)
}

/// ffmpeg's YUV 4:2:0 frames of a movie, concatenated. The movie goes
/// through a temporary file: the EA demuxer misreads large chunks from a
/// pipe. C code paths only (`-cpuflags 0`): ffmpeg's SIMD transform differs
/// from its C one when random coefficients overflow 16 bits.
fn ffmpeg_decode(ffmpeg: &str, movie: &[u8], name: &str) -> Vec<u8> {
    let path = std::env::temp_dir().join(format!(
        "doctor_toolkit_vp6_{}_{name}.vp6",
        std::process::id()
    ));
    std::fs::write(&path, movie).unwrap();
    let out = Command::new(ffmpeg)
        .args([
            "-v",
            "quiet",
            "-cpuflags",
            "0",
            "-threads",
            "1",
            "-f",
            "ea",
            "-i",
        ])
        .arg(&path)
        .args([
            "-map",
            "0:v:0",
            "-fps_mode",
            "passthrough",
            "-f",
            "rawvideo",
        ])
        .args(["-pix_fmt", "yuv420p", "-"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .expect("run ffmpeg");
    let _ = std::fs::remove_file(&path);
    out.stdout
}

#[test]
fn random_streams_decode_like_ffmpeg() {
    let Some(ffmpeg) = ffmpeg() else {
        eprintln!("ffmpeg not found: skipped");
        return;
    };
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut totals = FrameStats::default();
    let (mut frames, mut damaged, mut dropped, mut bicubic, mut deblock) = (0, 0, 0, 0, 0);
    let mut mismatches = Vec::new();
    for round in 0..40 {
        let (cols, rows) = if round % 10 == 9 {
            (20, 12)
        } else {
            (1 + rng.below(6) as u8, 1 + rng.below(5) as u8)
        };
        let movie = stream(&mut rng, cols, rows, 24);
        let theirs_all = ffmpeg_decode(&ffmpeg, &movie, &round.to_string());
        let size = cols as usize * 16 * rows as usize * 16 * 3 / 2;
        let parsed = container::chunks(&movie).unwrap();
        let mut dec = Decoder::new();
        let mut at = 0;
        for (i, chunk) in parsed.iter().skip(1).enumerate() {
            let result = dec.decode(chunk.body).map(|_| ());
            if result.is_err() && !dec.last_damaged() {
                // Dropped by both decoders (no output).
                dropped += 1;
                continue;
            }
            damaged += result.is_err() as usize;
            let s = *dec.stats();
            totals.huffman |= s.huffman;
            totals.golden |= s.golden && !s.key;
            bicubic += (s.filter_mode != 0 && !s.key) as usize;
            deblock += s.deblock as usize;
            for (a, b) in totals.mb_types.iter_mut().zip(s.mb_types) {
                *a += b;
            }
            let ours = dec.picture().unwrap().flipped();
            let theirs = theirs_all
                .get(at..at + size)
                .unwrap_or_else(|| panic!("round {round}: ffmpeg has no frame {i}"));
            if ours.planes.concat()[..] != theirs[..] {
                // Later frames depend on this one: next stream.
                mismatches.push(format!(
                    "round {round} ({cols}x{rows} MB), frame {i} ({}): differs; {s:?}",
                    if s.key { "key" } else { "inter" },
                ));
                at = theirs_all.len();
                break;
            }
            at += size;
            frames += 1;
        }
        assert_eq!(
            at,
            theirs_all.len(),
            "round {round}: ffmpeg has more frames"
        );
    }
    eprintln!(
        "{frames} frames equal to ffmpeg ({damaged} concealed, {dropped} dropped); \
         bicubic {bicubic}, deblocking {deblock}, macroblock types {:?}",
        totals.mb_types
    );
    assert!(mismatches.is_empty(), "{mismatches:#?}");
    assert!(damaged > 0);
    assert!(totals.huffman && totals.golden);
    assert!(bicubic > 0 && deblock > 0);
    assert!(totals.mb_types.iter().all(|&n| n > 0));
}

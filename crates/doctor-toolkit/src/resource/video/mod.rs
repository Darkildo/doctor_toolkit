//! Movies (`movies/**/*.vp6`): the EA movie container with On2 VP6 video and
//! EA-XA audio, plus the one AIFF file that carries the same extension.
//!
//! * [`container`] — chunks, `MVhd` and `SCHl` headers;
//! * [`eaxa`] — EA-XA (revision 3) audio decoder and encoder;
//! * [`vp6`] — VP6 frame headers, the decoder and an encoder;
//! * [`frames`] — decoded frames in any order (players, exports);
//! * [`input`] — Y4M, WAV, AIFF, AVI and FLV readers;
//! * [`build_movie`] — a complete movie from new video and audio.
//!
//! The game plays movies with its `RWMovie` player: an EA-container demuxer
//! (`0x00d0a9b0`) and an AVI demuxer (`0x00d0cfb0`), but only one video decoder,
//! `VideoDecoder_Vp6`; audio decoders are EA-XA and EALayer3. See
//! `docs/formats/video.md`.

pub mod container;
pub mod eaxa;
pub mod frames;
pub mod import;
pub mod input;
// The VP6 code mirrors the reference decoder's table indexing.
#[allow(clippy::needless_range_loop)]
pub mod vp6;

use serde::Serialize;

use container::{AudioHeader, Chunk, VideoHeader};
pub use frames::MovieFrames;
use input::Pcm;
use vp6::{EncoderConfig, Picture};

use crate::{Error, Result};

/// A parsed EA movie.
#[derive(Debug, Clone)]
pub struct Movie<'a> {
    pub chunks: Vec<Chunk<'a>>,
    pub video: Option<VideoHeader>,
    pub audio: Option<AudioHeader>,
}

/// True for an AIFF file (`movies/common/logo.vp6` is one).
pub fn is_aiff(data: &[u8]) -> bool {
    data.len() >= 12 && &data[..4] == b"FORM" && &data[8..12] == b"AIFF"
}

impl<'a> Movie<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let chunks = container::chunks(data)?;
        let mut video = None;
        let mut audio = None;
        for c in &chunks {
            match c.tag {
                container::MVHD if video.is_none() => video = Some(VideoHeader::parse(c.body)?),
                container::SCHL if audio.is_none() => audio = Some(AudioHeader::parse(c.body)?),
                _ => {}
            }
        }
        if video.is_none() && audio.is_none() {
            return Err(Error::malformed("EA movie", 0, "no MVhd or SCHl header"));
        }
        Ok(Self {
            chunks,
            video,
            audio,
        })
    }

    /// Video frames in order: (key frame, VP6 data).
    pub fn frames(&self) -> impl Iterator<Item = (bool, &'a [u8])> + '_ {
        self.chunks.iter().filter_map(|c| match c.tag {
            container::MV0K => Some((true, c.body)),
            container::MV0F => Some((false, c.body)),
            _ => None,
        })
    }

    /// Decodes the audio track to interleaved PCM.
    pub fn decode_audio(&self) -> Result<Option<Pcm>> {
        let Some(h) = &self.audio else {
            return Ok(None);
        };
        if h.codec != container::AUDIO_EAXA {
            return Err(Error::InvalidArgument(format!(
                "movie audio codec {} is not EA-XA",
                h.codec
            )));
        }
        let ch = h.channels as usize;
        let mut state = vec![eaxa::State::default(); ch];
        let mut out = vec![Vec::new(); ch];
        for c in self.chunks.iter().filter(|c| c.tag == container::SCDL) {
            eaxa::decode_chunk(c.body, &mut state, &mut out)?;
        }
        let n = out.iter().map(Vec::len).min().unwrap_or(0);
        let n = if h.sample_count > 0 {
            n.min(h.sample_count as usize)
        } else {
            n
        };
        let mut samples = Vec::with_capacity(n * ch);
        for i in 0..n {
            for c in &out {
                samples.push(c[i]);
            }
        }
        Ok(Some(Pcm {
            channels: ch as u16,
            sample_rate: h.sample_rate,
            samples,
        }))
    }

    pub fn info(&self) -> MovieInfo {
        let video = self.video.map(|h| {
            let mut frames = 0;
            let mut keys = 0;
            let mut largest = 0;
            let mut quantizers = std::collections::BTreeSet::new();
            for (key, f) in self.frames() {
                frames += 1;
                keys += key as u32;
                largest = largest.max(f.len());
                if let Some(b) = f.first() {
                    quantizers.insert(b >> 1 & 0x3F);
                }
            }
            VideoInfo {
                header: h,
                fps: h.fps(),
                duration: frames as f64 / h.fps().max(1e-9),
                frames,
                key_frames: keys,
                largest_frame: largest as u32,
                quantizers: quantizers.into_iter().collect(),
            }
        });
        let audio = self.audio.clone().map(|h| AudioInfo {
            codec: match h.codec {
                container::AUDIO_EAXA => "EA-XA".into(),
                container::AUDIO_EAL3 => "EALayer3".into(),
                c => format!("unknown ({c})"),
            },
            duration: h.sample_count as f64 / h.sample_rate.max(1) as f64,
            chunks: self
                .chunks
                .iter()
                .filter(|c| c.tag == container::SCDL)
                .count() as u32,
            header: h,
        });
        MovieInfo {
            format: "ea".into(),
            chunks: self.chunks.len(),
            video,
            audio,
        }
    }
}

/// Summary of a movie file.
#[derive(Debug, Clone, Serialize)]
pub struct MovieInfo {
    /// `ea` or `aiff`.
    pub format: String,
    pub chunks: usize,
    pub video: Option<VideoInfo>,
    pub audio: Option<AudioInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VideoInfo {
    pub header: VideoHeader,
    pub fps: f64,
    pub duration: f64,
    /// Frames found in the file (the header count can differ by a few).
    pub frames: u32,
    pub key_frames: u32,
    pub largest_frame: u32,
    pub quantizers: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AudioInfo {
    pub header: AudioHeader,
    pub codec: String,
    pub duration: f64,
    pub chunks: u32,
}

/// Describes any `.vp6` file of the game.
pub fn info(data: &[u8]) -> Result<MovieInfo> {
    if is_aiff(data) {
        let pcm = input::read_aiff(data)?;
        let samples = pcm.frames() as u32;
        return Ok(MovieInfo {
            format: "aiff".into(),
            chunks: 0,
            video: None,
            audio: Some(AudioInfo {
                header: AudioHeader {
                    channels: pcm.channels as u32,
                    sample_rate: pcm.sample_rate,
                    sample_count: samples,
                    revision: None,
                    codec: 0,
                },
                codec: "PCM".into(),
                duration: samples as f64 / pcm.sample_rate.max(1) as f64,
                chunks: 0,
            }),
        });
    }
    Ok(Movie::parse(data)?.info())
}

/// Audio of any `.vp6` file (EA-XA track or AIFF), as PCM.
pub fn decode_audio(data: &[u8]) -> Result<Option<Pcm>> {
    if is_aiff(data) {
        return input::read_aiff(data).map(Some);
    }
    Movie::parse(data)?.decode_audio()
}

/// The picture of frame `index` (clamped to the last frame) as shown:
/// decoded from the key frame at or before it, upright and cropped to the
/// header's size. Returns the index of the frame shown.
pub fn preview_frame(data: &[u8], index: usize) -> Result<(usize, Picture)> {
    let mut frames = MovieFrames::new(data)?;
    let index = index.min(frames.len() - 1);
    let pic = frames.display(index)?;
    Ok((index, pic))
}

/// Changes the frame rate in the `MVhd` header; everything else stays byte for byte.
pub fn retime(data: &[u8], rate: (u32, u32)) -> Result<Vec<u8>> {
    if rate.0 == 0 || rate.1 == 0 {
        return Err(Error::InvalidArgument("frame rate must be positive".into()));
    }
    let movie = Movie::parse(data)?;
    let mut out = data.to_vec();
    let c = movie
        .chunks
        .iter()
        .find(|c| c.tag == container::MVHD)
        .ok_or_else(|| Error::InvalidArgument("the movie has no video header".into()))?;
    let mut h = VideoHeader::parse(c.body)?;
    h.rate_num = rate.0;
    h.rate_den = rate.1;
    out[c.offset + 8..c.offset + 32].copy_from_slice(&h.to_bytes());
    Ok(out)
}

/// Video for [`build_movie`].
pub enum VideoInput<'a> {
    /// Raw 4:2:0 frames (YUV4MPEG2), encoded with the built-in VP6 encoder.
    Y4m(&'a mut dyn std::io::Read),
    /// The VP6 frames of another movie in the game's format, copied as they are.
    Movie(&'a [u8]),
    /// VP6 frames remuxed from AVI or FLV ([`input::read_avi_vp6`], [`input::read_flv_vp6`]).
    Frames(input::Vp6Frames),
}

/// Audio for [`build_movie`].
pub enum AudioInput {
    /// No audio track.
    None,
    /// Keep the audio of a [`VideoInput::Movie`] source unchanged.
    Keep,
    /// New audio, converted to 48 kHz stereo and encoded as EA-XA.
    Pcm(Pcm),
}

/// Settings of [`build_movie`].
#[derive(Default)]
pub struct BuildOptions<'p> {
    /// Frame rate; default: the source's, else 29.97 (982026/32767).
    pub rate: Option<(u32, u32)>,
    pub encoder: EncoderConfig,
    /// Called with the number of frames done so far.
    pub progress: Option<&'p mut dyn FnMut(usize)>,
}

/// Sample rate of the game's movie audio.
pub const MOVIE_AUDIO_RATE: u32 = 48000;

/// Builds a complete movie in the game's format (`SCHl`, `MVhd`, `SCCl`, then
/// each video frame followed by its share of audio, `SCEl`).
pub fn build_movie(
    video: VideoInput<'_>,
    audio: AudioInput,
    opts: &mut BuildOptions<'_>,
) -> Result<Vec<u8>> {
    let mut frames: Vec<(bool, Vec<u8>)> = Vec::new();
    let (width, height, source_rate) = match video {
        VideoInput::Y4m(reader) => {
            let mut r = input::Y4mReader::new(reader)?;
            let (w, h) = (r.width, r.height);
            if w > 4080 || h > 4080 {
                return Err(Error::InvalidArgument(format!(
                    "picture {w}x{h} is too large"
                )));
            }
            let (cw, ch) = (w.div_ceil(16) * 16, h.div_ceil(16) * 16);
            let mut enc = vp6::Encoder::new(cw, ch, opts.encoder);
            while let Some(pic) = r.next_frame()? {
                // Pad to whole macroblocks below and right, then store bottom-up.
                let coded = pic.padded(cw, ch).flipped();
                let f = enc.encode(&coded);
                frames.push((f.key, f.data));
                if let Some(p) = opts.progress.as_mut() {
                    p(frames.len());
                }
            }
            (w as u16, h as u16, r.rate)
        }
        VideoInput::Movie(data) => {
            let movie = Movie::parse(data)?;
            let h = movie
                .video
                .ok_or_else(|| Error::InvalidArgument("the source movie has no video".into()))?;
            if let AudioInput::Keep = audio {
                let rate = opts.rate.unwrap_or((h.rate_num, h.rate_den));
                return retime(data, rate);
            }
            frames = movie.frames().map(|(k, f)| (k, f.to_vec())).collect();
            (h.width, h.height, Some((h.rate_num, h.rate_den)))
        }
        VideoInput::Frames(f) => {
            frames = f.frames;
            (f.width, f.height, f.rate)
        }
    };
    if frames.is_empty() {
        return Err(Error::InvalidArgument("no video frames".into()));
    }
    if !frames[0].0 {
        return Err(Error::InvalidArgument(
            "the first frame is not a key frame".into(),
        ));
    }
    let rate = opts.rate.or(source_rate).unwrap_or(container::NTSC_RATE);
    let pcm = match audio {
        AudioInput::Pcm(p) => {
            // Audio shorter than the picture is padded with silence, so the
            // track covers every frame as in the game's movies.
            let mut p = to_movie_audio(&p);
            let video_samples = (frames.len() as u128 * MOVIE_AUDIO_RATE as u128 * rate.1 as u128)
                .div_ceil(rate.0 as u128) as usize;
            if p.frames() < video_samples {
                p.samples.resize(video_samples * p.channels as usize, 0);
            }
            Some(p)
        }
        AudioInput::None => None,
        AudioInput::Keep => {
            return Err(Error::InvalidArgument(
                "keeping audio needs a movie as the video source".into(),
            ));
        }
    };
    Ok(mux(width, height, rate, &frames, pcm.as_ref()))
}

/// Writes frames and audio into the container.
pub fn mux(
    width: u16,
    height: u16,
    rate: (u32, u32),
    frames: &[(bool, Vec<u8>)],
    audio: Option<&Pcm>,
) -> Vec<u8> {
    let header = VideoHeader {
        codec: *b"vp60",
        width,
        height,
        frame_count: frames.len() as u32,
        largest_frame: frames.iter().map(|f| f.1.len()).max().unwrap_or(0) as u32,
        rate_num: rate.0,
        rate_den: rate.1,
    };
    let total: usize = frames.iter().map(|f| f.1.len() + 8).sum();
    let mut out = Vec::with_capacity(total + 64 + audio.map_or(0, |a| a.samples.len()));
    let Some(pcm) = audio.filter(|p| p.frames() > 0) else {
        container::put_chunk(&mut out, container::MVHD, &header.to_bytes());
        for (key, f) in frames {
            put_frame(&mut out, *key, f);
        }
        return out;
    };
    let ch = pcm.channels as usize;
    let channels: Vec<Vec<i16>> = (0..ch).map(|c| pcm.channel(c)).collect();
    let n = pcm.frames();
    let mut state = vec![eaxa::State::default(); ch];
    let mut chunks: Vec<Vec<u8>> = Vec::new();
    let mut sent = 0usize;
    let mut take = |end: usize, sent: &mut usize, chunks: &mut Vec<Vec<u8>>| {
        let end = end.min(n);
        if end > *sent {
            let parts: Vec<&[i16]> = channels.iter().map(|c| &c[*sent..end]).collect();
            chunks.push(eaxa::encode_chunk(&parts, &mut state));
            *sent = end;
        }
    };
    // Audio up to the end of each frame, rounded up to whole EA-XA frames.
    let mut per_frame = Vec::with_capacity(frames.len());
    for i in 1..=frames.len() {
        let t = (i as u128 * pcm.sample_rate as u128 * rate.1 as u128).div_ceil(rate.0 as u128);
        let end = (t as usize).div_ceil(eaxa::FRAME) * eaxa::FRAME;
        let before = chunks.len();
        take(end, &mut sent, &mut chunks);
        per_frame.push(chunks.len() > before);
    }
    let step = ((pcm.sample_rate as u64 * rate.1 as u64 / rate.0.max(1) as u64) as usize)
        .div_ceil(eaxa::FRAME)
        .max(1)
        * eaxa::FRAME;
    let mut tail = Vec::new();
    while sent < n {
        let before = chunks.len();
        take(sent + step, &mut sent, &mut chunks);
        if chunks.len() > before {
            tail.push(chunks.len() - 1);
        }
    }

    container::put_chunk(
        &mut out,
        container::SCHL,
        &AudioHeader::build(ch as u32, pcm.sample_rate, n as u32),
    );
    container::put_chunk(&mut out, container::MVHD, &header.to_bytes());
    container::put_chunk(
        &mut out,
        container::SCCL,
        &(chunks.len() as u32).to_be_bytes(),
    );
    let mut next_audio = 0;
    for ((key, f), has_audio) in frames.iter().zip(per_frame) {
        put_frame(&mut out, *key, f);
        if has_audio {
            container::put_chunk(&mut out, container::SCDL, &chunks[next_audio]);
            next_audio += 1;
        }
    }
    for c in &chunks[next_audio..] {
        container::put_chunk(&mut out, container::SCDL, c);
    }
    container::put_chunk(&mut out, container::SCEL, &[]);
    out
}

/// A video chunk, padded to 4 bytes like the game's movies.
fn put_frame(out: &mut Vec<u8>, key: bool, frame: &[u8]) {
    let tag = if key {
        container::MV0K
    } else {
        container::MV0F
    };
    let pad = (4 - frame.len() % 4) % 4;
    out.extend_from_slice(&tag);
    out.extend_from_slice(&((frame.len() + pad + 8) as u32).to_le_bytes());
    out.extend_from_slice(frame);
    out.extend(std::iter::repeat_n(0u8, pad));
}

/// Converts audio to what the game's movies use: 48 kHz, stereo.
pub fn to_movie_audio(pcm: &Pcm) -> Pcm {
    let n = pcm.frames();
    let ch = pcm.channels.max(1) as usize;
    let mut left = Vec::with_capacity(n);
    let mut right = Vec::with_capacity(n);
    for i in 0..n {
        let s = &pcm.samples[i * ch..(i + 1) * ch];
        let (l, r) = match ch {
            1 => (s[0] as i32, s[0] as i32),
            2 => (s[0] as i32, s[1] as i32),
            _ => {
                // Odd channels to the left, even to the right.
                let l: i32 = s.iter().step_by(2).map(|&v| v as i32).sum();
                let r: i32 = s.iter().skip(1).step_by(2).map(|&v| v as i32).sum();
                (l / ch.div_ceil(2) as i32, r / (ch / 2) as i32)
            }
        };
        left.push(l as i16);
        right.push(r as i16);
    }
    let left = resample(&left, pcm.sample_rate, MOVIE_AUDIO_RATE);
    let right = resample(&right, pcm.sample_rate, MOVIE_AUDIO_RATE);
    let mut samples = Vec::with_capacity(left.len() * 2);
    for (l, r) in left.into_iter().zip(right) {
        samples.push(l);
        samples.push(r);
    }
    Pcm {
        channels: 2,
        sample_rate: MOVIE_AUDIO_RATE,
        samples,
    }
}

/// Windowed-sinc resampling of one channel.
pub fn resample(input: &[i16], from: u32, to: u32) -> Vec<i16> {
    if from == to || input.is_empty() || from == 0 {
        return input.to_vec();
    }
    const TAPS: i64 = 16;
    let ratio = to as f64 / from as f64;
    let cutoff = ratio.min(1.0) * 0.95;
    let out_len = (input.len() as f64 * ratio).round() as usize;
    let mut out = Vec::with_capacity(out_len);
    let half = (TAPS as f64 / cutoff).ceil() as i64;
    for i in 0..out_len {
        let t = i as f64 / ratio;
        let center = t.floor() as i64;
        let mut acc = 0.0f64;
        let mut wsum = 0.0f64;
        for k in center - half + 1..=center + half {
            let x = t - k as f64;
            let w_window = {
                let r = x / half as f64;
                if r.abs() >= 1.0 {
                    0.0
                } else {
                    0.5 + 0.5 * (std::f64::consts::PI * r).cos()
                }
            };
            let arg = std::f64::consts::PI * x * cutoff;
            let sinc = if arg.abs() < 1e-9 {
                1.0
            } else {
                arg.sin() / arg
            };
            let w = sinc * w_window;
            let s = input[k.clamp(0, input.len() as i64 - 1) as usize] as f64;
            acc += s * w;
            wsum += w;
        }
        let v = if wsum.abs() > 1e-9 { acc / wsum } else { 0.0 };
        out.push(v.round().clamp(-32768.0, 32767.0) as i16);
    }
    out
}

/// A movie file of the game.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MovieFile {
    /// Path relative to the game directory with `/` (`movies/common/mainmenu.vp6`).
    pub rel_path: String,
    #[serde(skip)]
    pub path: std::path::PathBuf,
    pub len: u64,
}

/// All `movies/**/*.vp6` files, sorted by path.
pub fn game_movies(game_dir: &std::path::Path) -> Result<Vec<MovieFile>> {
    let mut out = Vec::new();
    let Some(movies) = std::fs::read_dir(game_dir)
        .map_err(|e| Error::io(game_dir, e))?
        .filter_map(|e| e.ok())
        .find(|e| e.file_name().eq_ignore_ascii_case("movies"))
    else {
        return Ok(out);
    };
    let mut dirs = vec![(
        movies.path(),
        movies.file_name().to_string_lossy().into_owned(),
    )];
    while let Some((dir, rel)) = dirs.pop() {
        for e in std::fs::read_dir(&dir).map_err(|e| Error::io(&dir, e))? {
            let e = e.map_err(|e| Error::io(&dir, e))?;
            let name = e.file_name().to_string_lossy().into_owned();
            let path = e.path();
            let rel_path = format!("{rel}/{name}");
            if path.is_dir() {
                dirs.push((path, rel_path));
            } else if name.to_ascii_lowercase().ends_with(".vp6") {
                let len = std::fs::metadata(&path)
                    .map_err(|e| Error::io(&path, e))?
                    .len();
                out.push(MovieFile {
                    rel_path,
                    path,
                    len,
                });
            }
        }
    }
    out.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(out)
}

/// Finds a movie by relative path (`movies/common/mainmenu.vp6`), path inside
/// `movies/` (`common/mainmenu.vp6`) or unique file name (`mainmenu`), in any case.
pub fn find_movie(game_dir: &std::path::Path, name: &str) -> Result<MovieFile> {
    let movies = game_movies(game_dir)?;
    let want = name.replace('\\', "/").to_ascii_lowercase();
    let want = want.trim_start_matches("./");
    let matches: Vec<&MovieFile> = movies
        .iter()
        .filter(|m| {
            let rel = m.rel_path.to_ascii_lowercase();
            let file = rel.rsplit('/').next().unwrap_or(&rel).to_owned();
            let inner = rel.strip_prefix("movies/").unwrap_or(&rel);
            rel == want
                || inner == want
                || inner.strip_suffix(".vp6") == Some(want)
                || file == want
                || file.strip_suffix(".vp6") == Some(want)
        })
        .collect();
    match matches.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(Error::InvalidArgument(format!("no movie named {name}"))),
        many => Err(Error::InvalidArgument(format!(
            "{name} is ambiguous: {}",
            many.iter()
                .map(|m| m.rel_path.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

impl Picture {
    /// PNG of the picture.
    pub fn to_png(&self) -> Result<Vec<u8>> {
        let mut rgba = Vec::new();
        self.rgba_into(self.width, self.height, false, &mut rgba);
        crate::resource::pixels::encode_png(self.width & !1, self.height & !1, &rgba)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn y4m(w: usize, h: usize, n: usize) -> Vec<u8> {
        let mut d = input::y4m_header(w, h, (30, 1)).into_bytes();
        for t in 0..n {
            d.extend_from_slice(b"FRAME\n");
            for y in 0..h {
                for x in 0..w {
                    d.push(((x * 3 + y + t * 5) % 220 + 16) as u8);
                }
            }
            d.extend(std::iter::repeat_n(128u8, w * h / 2));
        }
        d
    }

    #[test]
    fn build_decode_and_retime() {
        let src = y4m(40, 24, 5);
        let mut reader = &src[..];
        let pcm = Pcm {
            channels: 1,
            sample_rate: 22050,
            samples: (0..22050 / 4)
                .map(|i| ((i as f32 / 10.0).sin() * 8000.0) as i16)
                .collect(),
        };
        let mut done = 0;
        let mut progress = |n: usize| done = n;
        let mut opts = BuildOptions {
            progress: Some(&mut progress),
            ..BuildOptions::default()
        };
        let movie = build_movie(
            VideoInput::Y4m(&mut reader),
            AudioInput::Pcm(pcm),
            &mut opts,
        )
        .unwrap();
        assert_eq!(done, 5);
        let m = Movie::parse(&movie).unwrap();
        let info = m.info();
        let v = info.video.unwrap();
        assert_eq!((v.header.width, v.header.height, v.frames), (40, 24, 5));
        assert_eq!((v.header.rate_num, v.header.rate_den), (30, 1));
        let a = m.decode_audio().unwrap().unwrap();
        assert_eq!((a.channels, a.sample_rate), (2, 48000));
        // 5 frames at 30 fps are 8000 samples; the 0.25 s of audio is longer.
        assert!(a.frames().abs_diff(12000) <= 1, "{}", a.frames());
        let scdl = m.chunks.iter().filter(|c| c.tag == container::SCDL).count();
        assert_eq!(info.audio.unwrap().chunks as usize, scdl);
        let sccl = m.chunks.iter().find(|c| c.tag == container::SCCL).unwrap();
        assert_eq!(
            u32::from_be_bytes(sccl.body.try_into().unwrap()) as usize,
            scdl
        );

        let (shown, pic) = preview_frame(&movie, 4).unwrap();
        assert_eq!(shown, 4);
        assert_eq!((pic.width, pic.height), (40, 24));
        assert!(pic.planes[0][0].abs_diff(36) <= 8, "{}", pic.planes[0][0]);

        let retimed = retime(&movie, (25, 1)).unwrap();
        assert_eq!(retimed.len(), movie.len());
        assert_eq!(Movie::parse(&retimed).unwrap().video.unwrap().rate_num, 25);
        let kept = build_movie(
            VideoInput::Movie(&movie),
            AudioInput::Keep,
            &mut BuildOptions {
                rate: Some((25, 1)),
                ..BuildOptions::default()
            },
        )
        .unwrap();
        assert_eq!(kept, retimed);
        let silent = build_movie(
            VideoInput::Movie(&movie),
            AudioInput::None,
            &mut BuildOptions::default(),
        )
        .unwrap();
        assert!(Movie::parse(&silent).unwrap().audio.is_none());
    }

    #[test]
    fn resample_keeps_a_tone() {
        let src: Vec<i16> = (0..4410)
            .map(|i| {
                ((i as f64 * 2.0 * std::f64::consts::PI * 441.0 / 44100.0).sin() * 10000.0) as i16
            })
            .collect();
        let out = resample(&src, 44100, 48000);
        assert_eq!(out.len(), 4800);
        let peak = out[200..4600]
            .iter()
            .map(|v| v.unsigned_abs())
            .max()
            .unwrap();
        assert!((9500..=10500).contains(&peak), "{peak}");
    }
}

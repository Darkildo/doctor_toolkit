//! A cut-scene written as a video (stage 62): pictures rendered frame by
//! frame (by the editor's renderer, which this crate does not have) go to
//! a PNG sequence, a movie in the game's format (VP6 by the built-in
//! encoder of [`crate::resource::video`], with the cut-scene's sound) and an
//! MP4 through ffmpeg when it is installed. Frames are rendered one at a
//! time as the encoders take them: a long cut-scene is never held in
//! memory.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::resource::video::input::{Pcm, y4m_header};
use crate::resource::video::{AudioInput, BuildOptions, VideoInput, build_movie, import};
use crate::{Error, Result};

/// What to write and how.
#[derive(Debug, Clone, Default)]
pub struct VideoOptions {
    pub width: usize,
    pub height: usize,
    /// Frames per second as a fraction.
    pub rate: (u32, u32),
    /// A folder for `frame_00000.png` …
    pub png_dir: Option<PathBuf>,
    /// A movie in the game's format (`.vp6`).
    pub vp6: Option<PathBuf>,
    /// An MP4 (H.264 and AAC) through ffmpeg.
    pub mp4: Option<PathBuf>,
    /// The sound, 16-bit PCM (any rate; the movie takes 48 kHz stereo).
    pub audio: Option<Pcm>,
}

/// What was written.
#[derive(Debug, Clone, Default)]
pub struct VideoReport {
    pub frames: usize,
    /// Time spent in the renderer (the frame callback).
    pub render: Duration,
    /// Everything: rendering, conversion, encoding, files.
    pub total: Duration,
    pub vp6_bytes: Option<u64>,
    pub mp4_bytes: Option<u64>,
    pub pngs: usize,
    /// Why there is no MP4 (no ffmpeg) and other notes.
    pub notes: Vec<String>,
}

/// One frame of RGBA pixels (`width × height × 4`, top row first) as the
/// three planes of a 4:2:0 YUV4MPEG2 frame (BT.601, limited range: what
/// the game's movies use).
pub fn rgba_to_yuv420(rgba: &[u8], width: usize, height: usize, out: &mut Vec<u8>) {
    let (cw, ch) = (width.div_ceil(2), height.div_ceil(2));
    out.clear();
    out.resize(width * height + 2 * cw * ch, 0);
    let (luma, chroma) = out.split_at_mut(width * height);
    let (u_plane, v_plane) = chroma.split_at_mut(cw * ch);
    let px = |x: usize, y: usize| {
        let i = (y * width + x) * 4;
        (rgba[i] as f32, rgba[i + 1] as f32, rgba[i + 2] as f32)
    };
    for y in 0..height {
        for x in 0..width {
            let (r, g, b) = px(x, y);
            luma[y * width + x] = (16.0 + 0.257 * r + 0.504 * g + 0.098 * b)
                .round()
                .clamp(0.0, 255.0) as u8;
        }
    }
    for cy in 0..ch {
        for cx in 0..cw {
            let (mut r, mut g, mut b, mut n) = (0.0, 0.0, 0.0, 0.0);
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (x, y) = (2 * cx + dx, 2 * cy + dy);
                if x < width && y < height {
                    let p = px(x, y);
                    r += p.0;
                    g += p.1;
                    b += p.2;
                    n += 1.0;
                }
            }
            let (r, g, b) = (r / n, g / n, b / n);
            u_plane[cy * cw + cx] = (128.0 - 0.148 * r - 0.291 * g + 0.439 * b)
                .round()
                .clamp(0.0, 255.0) as u8;
            v_plane[cy * cw + cx] = (128.0 + 0.439 * r - 0.368 * g - 0.071 * b)
                .round()
                .clamp(0.0, 255.0) as u8;
        }
    }
}

/// Where each rendered frame goes besides the movie.
struct Sinks {
    png_dir: Option<PathBuf>,
    ffmpeg: Option<Child>,
    pngs: usize,
}

impl Sinks {
    fn take(&mut self, index: usize, rgba: &[u8], width: usize, height: usize) -> Result<()> {
        if let Some(dir) = &self.png_dir {
            let path = dir.join(format!("frame_{index:05}.png"));
            let png = crate::resource::pixels::encode_png(width, height, rgba)?;
            std::fs::write(&path, png).map_err(|e| Error::io(&path, e))?;
            self.pngs += 1;
        }
        if let Some(child) = &mut self.ffmpeg
            && let Some(stdin) = child.stdin.as_mut()
        {
            stdin.write_all(rgba).map_err(|e| {
                Error::InvalidArgument(format!("ffmpeg stopped taking frames: {e}"))
            })?;
        }
        Ok(())
    }
}

/// The frames as a YUV4MPEG2 stream, rendered when the encoder reads them.
struct Frames<'a> {
    render: &'a mut dyn FnMut(usize) -> Result<Vec<u8>>,
    sinks: &'a mut Sinks,
    count: usize,
    next: usize,
    width: usize,
    height: usize,
    buffer: Vec<u8>,
    at: usize,
    yuv: Vec<u8>,
    render_time: Duration,
}

impl Frames<'_> {
    fn frame(&mut self) -> Result<Vec<u8>> {
        let started = Instant::now();
        let rgba = (self.render)(self.next)?;
        self.render_time += started.elapsed();
        if rgba.len() != self.width * self.height * 4 {
            return Err(Error::InvalidArgument(format!(
                "frame {}: {} bytes for {}×{}",
                self.next,
                rgba.len(),
                self.width,
                self.height
            )));
        }
        self.sinks.take(self.next, &rgba, self.width, self.height)?;
        self.next += 1;
        Ok(rgba)
    }
}

impl Read for Frames<'_> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.at >= self.buffer.len() {
            if self.next >= self.count {
                return Ok(0);
            }
            let rgba = self.frame().map_err(std::io::Error::other)?;
            rgba_to_yuv420(&rgba, self.width, self.height, &mut self.yuv);
            self.buffer.clear();
            self.buffer.extend_from_slice(b"FRAME\n");
            self.buffer.extend_from_slice(&self.yuv);
            self.at = 0;
        }
        let n = out.len().min(self.buffer.len() - self.at);
        out[..n].copy_from_slice(&self.buffer[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

/// Starts ffmpeg reading raw RGBA frames from its input (and the sound
/// from `wav`) into an MP4.
fn start_ffmpeg(
    ffmpeg: &Path,
    options: &VideoOptions,
    out: &Path,
    wav: Option<&Path>,
) -> Result<Child> {
    let mut command = Command::new(ffmpeg);
    command
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(["-f", "rawvideo", "-pix_fmt", "rgba"])
        .args(["-s", &format!("{}x{}", options.width, options.height)])
        .args(["-r", &format!("{}/{}", options.rate.0, options.rate.1)])
        .args(["-i", "-"]);
    if let Some(wav) = wav {
        command.arg("-i").arg(wav);
    }
    command.args([
        "-c:v", "libx264", "-pix_fmt", "yuv420p", "-crf", "18", "-preset", "medium",
    ]);
    if wav.is_some() {
        command.args(["-c:a", "aac", "-b:a", "192k", "-shortest"]);
    }
    command
        .arg(out)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::io(ffmpeg, e))
}

/// Renders `frames` pictures with `render` (frame number → RGBA) and
/// writes them as the options ask.
pub fn write_video(
    frames: usize,
    render: &mut dyn FnMut(usize) -> Result<Vec<u8>>,
    options: &VideoOptions,
) -> Result<VideoReport> {
    let started = Instant::now();
    let mut report = VideoReport::default();
    if options.width == 0 || options.height == 0 || frames == 0 {
        return Err(Error::InvalidArgument("nothing to render".into()));
    }
    if options.vp6.is_some()
        && (!options.width.is_multiple_of(2) || !options.height.is_multiple_of(2))
    {
        return Err(Error::InvalidArgument(
            "the movie needs an even width and height".into(),
        ));
    }
    if let Some(dir) = &options.png_dir {
        std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    }
    // ffmpeg for the MP4, with the sound as a WAV file next to it.
    let mut wav_file = None;
    let ffmpeg = match &options.mp4 {
        Some(out) => match import::ffmpeg() {
            Some(ffmpeg) => {
                if let Some(pcm) = &options.audio {
                    let path = out.with_extension("wav.tmp");
                    let data =
                        crate::resource::audio::wav(&pcm.samples, pcm.channels, pcm.sample_rate);
                    std::fs::write(&path, data).map_err(|e| Error::io(&path, e))?;
                    wav_file = Some(path);
                }
                Some(start_ffmpeg(&ffmpeg, options, out, wav_file.as_deref())?)
            }
            None => {
                report
                    .notes
                    .push("no MP4: ffmpeg is not installed (or set FFMPEG=/path/to/ffmpeg)".into());
                None
            }
        },
        None => None,
    };
    let mut sinks = Sinks {
        png_dir: options.png_dir.clone(),
        ffmpeg,
        pngs: 0,
    };
    let mut source = Frames {
        render,
        sinks: &mut sinks,
        count: frames,
        next: 0,
        width: options.width,
        height: options.height,
        buffer: y4m_header(options.width, options.height, options.rate).into_bytes(),
        at: 0,
        yuv: Vec::new(),
        render_time: Duration::ZERO,
    };
    let result = match &options.vp6 {
        Some(path) => {
            let audio = match &options.audio {
                Some(pcm) => AudioInput::Pcm(pcm.clone()),
                None => AudioInput::None,
            };
            let mut build = BuildOptions {
                rate: Some(options.rate),
                ..BuildOptions::default()
            };
            build_movie(VideoInput::Y4m(&mut source), audio, &mut build).and_then(|movie| {
                std::fs::write(path, &movie).map_err(|e| Error::io(path, e))?;
                report.vp6_bytes = Some(movie.len() as u64);
                Ok(())
            })
        }
        None => (0..frames).try_for_each(|_| source.frame().map(|_| ())),
    };
    report.frames = source.next;
    report.render = source.render_time;
    // Finish ffmpeg whatever happened.
    if let Some(mut child) = sinks.ffmpeg.take() {
        drop(child.stdin.take());
        let output = child.wait_with_output();
        if let Some(path) = &wav_file {
            let _ = std::fs::remove_file(path);
        }
        match output {
            Ok(o) if o.status.success() => {
                if let Some(out) = &options.mp4 {
                    report.mp4_bytes = std::fs::metadata(out).ok().map(|m| m.len());
                }
            }
            Ok(o) => report.notes.push(format!(
                "ffmpeg failed: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            )),
            Err(e) => report.notes.push(format!("ffmpeg: {e}")),
        }
    }
    result?;
    report.pngs = sinks.pngs;
    report.total = started.elapsed();
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_go_to_limited_range_yuv() {
        let mut out = Vec::new();
        // White, black, red, green on a 2×2 picture.
        let rgba = [
            255, 255, 255, 255, 0, 0, 0, 255, //
            255, 0, 0, 255, 0, 255, 0, 255,
        ];
        rgba_to_yuv420(&rgba, 2, 2, &mut out);
        assert_eq!(out.len(), 6);
        assert_eq!(&out[..4], &[235, 16, 82, 145]);
    }

    #[test]
    fn frames_become_a_movie_and_pngs() {
        let dir = tempfile::tempdir().unwrap();
        let options = VideoOptions {
            width: 32,
            height: 16,
            rate: (30, 1),
            png_dir: Some(dir.path().join("png")),
            vp6: Some(dir.path().join("out.vp6")),
            mp4: None,
            audio: Some(Pcm {
                channels: 2,
                sample_rate: 48000,
                samples: vec![0; 48000 / 10 * 2],
            }),
        };
        let mut render = |i: usize| -> Result<Vec<u8>> {
            Ok((0..32 * 16)
                .flat_map(|p| [(p * 8 + i * 40) as u8, (p / 2) as u8, 90, 255])
                .collect())
        };
        let report = write_video(5, &mut render, &options).unwrap();
        assert_eq!(report.frames, 5);
        assert_eq!(report.pngs, 5);
        let movie = std::fs::read(dir.path().join("out.vp6")).unwrap();
        let info = crate::resource::video::info(&movie).unwrap();
        let video = info.video.unwrap();
        assert_eq!(
            (video.header.width, video.header.height, video.frames),
            (32, 16, 5)
        );
        assert!(info.audio.is_some());
        let (index, picture) = crate::resource::video::preview_frame(&movie, 4).unwrap();
        assert_eq!(index, 4);
        assert_eq!((picture.width & !1, picture.height & !1), (32, 16));
        assert!(dir.path().join("png/frame_00004.png").exists());
    }
}

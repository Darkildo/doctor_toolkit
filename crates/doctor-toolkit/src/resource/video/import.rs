//! Movie import from files: picks the reader by content and, for formats the
//! toolkit does not read itself (MP4, MKV, MP3…), converts through ffmpeg when
//! it is installed. ffmpeg is optional: Y4M, WAV, game movies, AVI and FLV with
//! VP6 need nothing else.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::{AudioInput, BuildOptions, Movie, VideoInput, build_movie, input};
use crate::{Error, Result};

/// Where the new movie's audio comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioSource {
    /// Keep the audio of the movie being replaced (or of a `.vp6` video source).
    Keep,
    /// No audio track.
    None,
    /// A WAV file, or any audio file ffmpeg reads.
    File(PathBuf),
}

/// The ffmpeg executable (`$FFMPEG` or `ffmpeg` on the `PATH`), if it runs.
pub fn ffmpeg() -> Option<PathBuf> {
    let name = std::env::var_os("FFMPEG").unwrap_or_else(|| "ffmpeg".into());
    Command::new(&name)
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
        .then(|| PathBuf::from(name))
}

fn need_ffmpeg(path: &Path) -> Result<PathBuf> {
    ffmpeg().ok_or_else(|| {
        Error::InvalidArgument(format!(
            "{}: not Y4M, WAV, a game movie, AVI or FLV with VP6; converting it needs ffmpeg \
             (install it or set FFMPEG=/path/to/ffmpeg)",
            path.display()
        ))
    })
}

fn magic(path: &Path, len: usize) -> Result<Vec<u8>> {
    let mut head = vec![0u8; len];
    let mut f = std::fs::File::open(path).map_err(|e| Error::io(path, e))?;
    let n = f.read(&mut head).map_err(|e| Error::io(path, e))?;
    head.truncate(n);
    Ok(head)
}

/// Reads audio from a WAV file, or converts any other file with ffmpeg.
pub fn read_audio_file(path: &Path) -> Result<input::Pcm> {
    let data = if magic(path, 4)? == b"RIFF" {
        std::fs::read(path).map_err(|e| Error::io(path, e))?
    } else {
        let out = Command::new(need_ffmpeg(path)?)
            .args(["-hide_banner", "-loglevel", "error", "-i"])
            .arg(path)
            .args(["-f", "wav", "-acodec", "pcm_s16le", "-"])
            .stdin(Stdio::null())
            .output()
            .map_err(|e| Error::io(path, e))?;
        if !out.status.success() {
            return Err(Error::InvalidArgument(format!(
                "ffmpeg could not convert {}",
                path.display()
            )));
        }
        out.stdout
    };
    input::read_wav(&data)
}

/// Result of [`build_from_files`].
pub struct Imported {
    pub movie: Vec<u8>,
    pub warnings: Vec<String>,
}

/// Builds a replacement for the movie `original` (its bytes) from files.
/// `video` defaults to the original itself (to change only the audio or rate).
pub fn build_from_files(
    video: Option<&Path>,
    audio: &AudioSource,
    original: &[u8],
    opts: &mut BuildOptions<'_>,
) -> Result<Imported> {
    let mut warnings = Vec::new();
    let audio_in = match audio {
        AudioSource::None => AudioInput::None,
        AudioSource::File(p) => AudioInput::Pcm(read_audio_file(p)?),
        AudioSource::Keep => AudioInput::Keep,
    };
    // With a new picture, "keep" re-encodes the original movie's audio.
    let original_audio = || -> AudioInput {
        match Movie::parse(original).ok().map(|m| m.decode_audio()) {
            Some(Ok(Some(pcm))) => AudioInput::Pcm(pcm),
            _ => AudioInput::None,
        }
    };
    let Some(path) = video else {
        return Ok(Imported {
            movie: build_movie(VideoInput::Movie(original), audio_in, opts)?,
            warnings,
        });
    };
    let head = magic(path, 9)?;
    let is_movie = head.starts_with(b"MVhd") || head.starts_with(b"SCHl");
    let audio_for_new = |a: AudioInput| match a {
        AudioInput::Keep => original_audio(),
        other => other,
    };
    let movie = if head.starts_with(b"YUV4MPEG2") {
        let file = std::fs::File::open(path).map_err(|e| Error::io(path, e))?;
        let mut reader = std::io::BufReader::new(file);
        build_movie(VideoInput::Y4m(&mut reader), audio_for_new(audio_in), opts)?
    } else if is_movie {
        let data = std::fs::read(path).map_err(|e| Error::io(path, e))?;
        build_movie(VideoInput::Movie(&data), audio_in, opts)?
    } else if head.starts_with(b"RIFF") {
        let data = std::fs::read(path).map_err(|e| Error::io(path, e))?;
        let frames = input::read_avi_vp6(&data)?;
        build_movie(VideoInput::Frames(frames), audio_for_new(audio_in), opts)?
    } else if head.starts_with(b"FLV") {
        let data = std::fs::read(path).map_err(|e| Error::io(path, e))?;
        let frames = input::read_flv_vp6(&data)?;
        warnings.push(
            "FLV VP6 is stored top-down: the game shows it upside down unless the source \
             was flipped before encoding"
                .to_owned(),
        );
        build_movie(VideoInput::Frames(frames), audio_for_new(audio_in), opts)?
    } else {
        let mut child = Command::new(need_ffmpeg(path)?)
            .args(["-hide_banner", "-loglevel", "error", "-i"])
            .arg(path)
            .args(["-f", "yuv4mpegpipe", "-pix_fmt", "yuv420p", "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| Error::io(path, e))?;
        let stdout = child.stdout.take().expect("piped stdout");
        let mut reader = std::io::BufReader::new(stdout);
        let result = build_movie(VideoInput::Y4m(&mut reader), audio_for_new(audio_in), opts);
        drop(reader);
        let status = child.wait().map_err(|e| Error::io(path, e))?;
        let movie = result?;
        if !status.success() {
            return Err(Error::InvalidArgument(format!(
                "ffmpeg could not convert {}",
                path.display()
            )));
        }
        movie
    };
    Ok(Imported { movie, warnings })
}

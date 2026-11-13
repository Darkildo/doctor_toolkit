//! Movie commands: `video-info`, `video-export`, `video-import`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use clap::Args;

use doctor_toolkit::resource::audio::wav;
use doctor_toolkit::resource::video::import::{self, AudioSource};
use doctor_toolkit::resource::video::{
    self, BuildOptions, Movie, MovieFrames, input, vp6::EncoderConfig,
};

#[derive(Args)]
pub struct InfoArgs {
    /// A `.vp6` file, or the game directory to list all movies
    pub path: PathBuf,
    /// Print JSON
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct ExportArgs {
    /// A `.vp6` file, or a movie name with `--game`
    pub movie: String,
    /// Game directory (then `movie` is a name such as `mainmenu` or `en/load.vp6`)
    #[arg(long)]
    pub game: Option<PathBuf>,
    #[arg(short, long)]
    pub output: PathBuf,
    /// Write the audio track as WAV
    #[arg(long)]
    pub audio: bool,
    /// Write these frames as PNG (built-in VP6 decoder)
    #[arg(long, value_delimiter = ',')]
    pub frame: Vec<usize>,
    /// Write every key frame as PNG
    #[arg(long)]
    pub key_frames: bool,
    /// Write frames as YUV4MPEG2 (lossless 4:2:0) instead of PNG
    #[arg(long)]
    pub y4m: bool,
    /// Decode every frame with the built-in decoder into one YUV4MPEG2 file
    /// (`-` for standard output); prints the decoding speed
    #[arg(long, value_name = "FILE")]
    pub decode: Option<PathBuf>,
    /// Convert the whole movie with ffmpeg (if installed) to this file, e.g. `movie.mp4`
    #[arg(long)]
    pub ffmpeg: Option<PathBuf>,
}

#[derive(Args)]
pub struct ImportArgs {
    /// Game directory
    pub game_dir: PathBuf,
    /// Movie to replace: `mainmenu`, `common/mainmenu.vp6`, `movies/en/load.vp6`
    pub movie: String,
    /// New video: `.y4m` (encoded to VP6), another game movie `.vp6` (frames
    /// copied), `.avi` with VP6, `.flv` with VP6; anything else is converted to
    /// Y4M with ffmpeg when it is installed
    #[arg(long)]
    pub video: Option<PathBuf>,
    /// New audio: `.wav` (any rate, converted to 48 kHz stereo EA-XA); other
    /// formats through ffmpeg
    #[arg(long, conflicts_with = "no_audio")]
    pub audio: Option<PathBuf>,
    /// Drop the audio track
    #[arg(long)]
    pub no_audio: bool,
    /// Frame rate: `30`, `29.97` or `30000/1001` (default: the source's)
    #[arg(long)]
    pub fps: Option<String>,
    /// VP6 quantizer 0..63, higher is better quality and larger (game: 60)
    #[arg(long, default_value_t = 52)]
    pub quantizer: u8,
    /// Key frame at least every N frames
    #[arg(long, default_value_t = 30)]
    pub keyframe_interval: u32,
    /// Write the new movie to this file instead of the game
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    #[command(flatten)]
    pub mode: crate::workspace_cmd::EditMode,
}

pub fn info(args: &InfoArgs) -> Result<()> {
    if args.path.is_dir() {
        let movies = video::game_movies(&args.path)?;
        let mut rows = Vec::new();
        for m in &movies {
            let data = fs::read(&m.path).with_context(|| m.rel_path.clone())?;
            let info = video::info(&data).with_context(|| m.rel_path.clone())?;
            if args.json {
                rows.push(serde_json::json!({ "path": m.rel_path, "size": m.len, "info": info }));
                continue;
            }
            let v = info.video.as_ref().map_or("-".to_owned(), |v| {
                format!(
                    "{}x{} {} frames {:.2} fps {:.1} s",
                    v.header.width, v.header.height, v.frames, v.fps, v.duration
                )
            });
            let a = info.audio.as_ref().map_or("-".to_owned(), |a| {
                format!(
                    "{} {} ch {} Hz {:.1} s",
                    a.codec, a.header.channels, a.header.sample_rate, a.duration
                )
            });
            println!(
                "{:40} {:>10}  {:5} {v:42} {a}",
                m.rel_path, m.len, info.format
            );
        }
        if args.json {
            println!("{}", serde_json::to_string_pretty(&rows)?);
        } else {
            println!("{} movies", movies.len());
        }
        return Ok(());
    }
    let data = fs::read(&args.path).with_context(|| args.path.display().to_string())?;
    let info = video::info(&data)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&info)?);
        return Ok(());
    }
    println!("format: {}", info.format);
    if let Some(v) = &info.video {
        let h = &v.header;
        println!(
            "video: {} {}x{}, {} frames ({} in header), {} key frames, {:.4} fps ({}/{}), {:.2} s",
            String::from_utf8_lossy(&h.codec),
            h.width,
            h.height,
            v.frames,
            h.frame_count,
            v.key_frames,
            v.fps,
            h.rate_num,
            h.rate_den,
            v.duration
        );
        println!(
            "       largest frame {} bytes (header {}), quantizers {:?}",
            v.largest_frame, h.largest_frame, v.quantizers
        );
    }
    if let Some(a) = &info.audio {
        println!(
            "audio: {}, {} channels, {} Hz, {} samples, {:.2} s, {} chunks",
            a.codec,
            a.header.channels,
            a.header.sample_rate,
            a.header.sample_count,
            a.duration,
            a.chunks
        );
    }
    Ok(())
}

fn movie_path(movie: &str, game: Option<&Path>) -> Result<PathBuf> {
    match game {
        Some(dir) => Ok(video::find_movie(dir, movie)?.path),
        None => Ok(PathBuf::from(movie)),
    }
}

pub fn export(args: &ExportArgs) -> Result<()> {
    let path = movie_path(&args.movie, args.game.as_deref())?;
    let data = fs::read(&path).with_context(|| path.display().to_string())?;
    let stem = path
        .file_stem()
        .map_or("movie".into(), |s| s.to_string_lossy().into_owned());
    fs::create_dir_all(&args.output)?;
    let mut did = false;
    if args.audio {
        match video::decode_audio(&data)? {
            Some(pcm) => {
                let out = args.output.join(format!("{stem}.wav"));
                fs::write(&out, wav(&pcm.samples, pcm.channels, pcm.sample_rate))?;
                println!(
                    "{} ({:.2} s)",
                    out.display(),
                    pcm.frames() as f64 / pcm.sample_rate as f64
                );
            }
            None => println!("no audio track"),
        }
        did = true;
    }
    let mut frames: Vec<usize> = args.frame.clone();
    if args.key_frames {
        let movie = Movie::parse(&data)?;
        frames.extend(
            movie
                .frames()
                .enumerate()
                .filter(|(_, (k, _))| *k)
                .map(|(i, _)| i),
        );
    }
    let mut decoded = (!frames.is_empty())
        .then(|| MovieFrames::new(&data[..]))
        .transpose()?;
    for index in frames {
        let decoded = decoded.as_mut().expect("frames");
        let shown = index.min(decoded.len() - 1);
        let pic = decoded.display(shown)?;
        let out = if args.y4m {
            let out = args.output.join(format!("{stem}_{shown:05}.y4m"));
            let mut bytes = input::y4m_header(pic.width, pic.height, (30, 1)).into_bytes();
            bytes.extend_from_slice(b"FRAME\n");
            for plane in &pic.planes {
                bytes.extend_from_slice(plane);
            }
            fs::write(&out, bytes)?;
            out
        } else {
            let out = args.output.join(format!("{stem}_{shown:05}.png"));
            fs::write(&out, pic.to_png()?)?;
            out
        };
        if shown == index {
            println!("{}", out.display());
        } else {
            println!("{} (the movie has {} frames)", out.display(), shown + 1);
        }
        did = true;
    }
    if let Some(target) = &args.decode {
        decode_all(&data, target, &args.output)?;
        did = true;
    }
    if let Some(target) = &args.ffmpeg {
        let target = if target.is_absolute() {
            target.clone()
        } else {
            args.output.join(target)
        };
        let status = Command::new(ffmpeg()?)
            .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
            .arg(&path)
            .arg(&target)
            .stdin(Stdio::null())
            .status()
            .context("running ffmpeg")?;
        if !status.success() {
            bail!("ffmpeg failed ({status})");
        }
        println!("{}", target.display());
        did = true;
    }
    if !did {
        bail!("nothing to export: use --audio, --frame, --key-frames, --decode or --ffmpeg");
    }
    Ok(())
}

/// Every frame through the built-in decoder into one Y4M stream.
fn decode_all(data: &[u8], target: &Path, dir: &Path) -> Result<()> {
    use std::io::Write;
    let mut frames = MovieFrames::new(data)?;
    let header = *frames.header();
    let rate = (header.rate_num.max(1), header.rate_den.max(1));
    let mut out: Box<dyn Write> = if target == Path::new("-") {
        Box::new(std::io::BufWriter::new(std::io::stdout().lock()))
    } else {
        let path = if target.is_absolute() {
            target.to_owned()
        } else {
            dir.join(target)
        };
        Box::new(std::io::BufWriter::new(
            fs::File::create(&path).with_context(|| path.display().to_string())?,
        ))
    };
    let start = std::time::Instant::now();
    let mut decode_time = std::time::Duration::ZERO;
    let mut rgba_time = std::time::Duration::ZERO;
    let mut rgba = Vec::new();
    let mut header_written = false;
    for i in 0..frames.len() {
        let t = std::time::Instant::now();
        frames.seek(i)?;
        decode_time += t.elapsed();
        // What a player does with the picture (for the speed report).
        let t = std::time::Instant::now();
        frames.rgba(i, &mut rgba)?;
        rgba_time += t.elapsed();
        let pic = frames.display(i)?;
        if !header_written {
            out.write_all(input::y4m_header(pic.width, pic.height, rate).as_bytes())?;
            header_written = true;
        }
        out.write_all(b"FRAME\n")?;
        for plane in &pic.planes {
            out.write_all(plane)?;
        }
    }
    out.flush()?;
    let n = frames.len() as f64;
    let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0 / n.max(1.0);
    eprintln!(
        "{} frames in {:.2} s: decoding {:.0} frames/s ({:.2} ms per frame), RGBA {:.2} ms per frame",
        frames.len(),
        start.elapsed().as_secs_f64(),
        n / decode_time.as_secs_f64().max(1e-9),
        ms(decode_time),
        ms(rgba_time),
    );
    Ok(())
}

fn ffmpeg() -> Result<PathBuf> {
    import::ffmpeg().ok_or_else(|| {
        anyhow::anyhow!("ffmpeg was not found (install it or set FFMPEG=/path/to/ffmpeg)")
    })
}

fn parse_fps(s: &str) -> Result<(u32, u32)> {
    if let Some((n, d)) = s.split_once('/') {
        let (n, d): (u32, u32) = (n.trim().parse()?, d.trim().parse()?);
        if n == 0 || d == 0 {
            bail!("bad frame rate {s}");
        }
        return Ok((n, d));
    }
    let f: f64 = s
        .trim()
        .parse()
        .with_context(|| format!("bad frame rate {s}"))?;
    if !(0.5..=240.0).contains(&f) {
        bail!("bad frame rate {s}");
    }
    if (f - 29.97).abs() < 0.005 {
        return Ok(video::container::NTSC_RATE);
    }
    if f.fract() == 0.0 {
        return Ok((f as u32, 1));
    }
    Ok(((f * 1000.0).round() as u32, 1000))
}

pub fn import(args: &ImportArgs) -> Result<()> {
    let target = video::find_movie(&args.game_dir, &args.movie)?;
    let key = doctor_toolkit::workspace::FileKey::loose(&target.rel_path);
    let game = doctor_toolkit::game::GameInstance::open(&args.game_dir)?;
    let mut ws = (args.output.is_none())
        .then(|| crate::workspace_cmd::open_for_edit(&game, &args.mode))
        .transpose()?;
    // The workspace version: an earlier import, or the original.
    let original = match &ws {
        Some(ws) => ws.current_file(&game, &key)?,
        None => fs::read(&target.path)?,
    };
    let audio = if args.no_audio {
        AudioSource::None
    } else if let Some(a) = &args.audio {
        AudioSource::File(a.clone())
    } else {
        AudioSource::Keep
    };
    let rate = args.fps.as_deref().map(parse_fps).transpose()?;
    let mut done = 0usize;
    let mut progress = |n: usize| {
        done = n;
        if n.is_multiple_of(100) {
            eprint!("\r{n} frames");
        }
    };
    let mut opts = BuildOptions {
        rate,
        encoder: EncoderConfig {
            quantizer: args.quantizer.min(63),
            keyframe_interval: args.keyframe_interval.max(1),
            ..EncoderConfig::default()
        },
        progress: Some(&mut progress),
    };
    let imported = import::build_from_files(args.video.as_deref(), &audio, &original, &mut opts)?;
    if done > 0 {
        eprintln!("\r{done} frames encoded");
    }
    for w in &imported.warnings {
        eprintln!("warning: {w}");
    }
    let movie = imported.movie;
    let info = video::info(&movie)?;
    if let Some(v) = &info.video {
        println!(
            "{}x{}, {} frames ({} key), {:.3} fps, largest frame {} bytes, {} bytes",
            v.header.width,
            v.header.height,
            v.frames,
            v.key_frames,
            v.fps,
            v.largest_frame,
            movie.len()
        );
    }
    if let Some(a) = &info.audio {
        println!(
            "audio: {} ch, {} Hz, {:.2} s",
            a.header.channels, a.header.sample_rate, a.duration
        );
    }
    match &args.output {
        Some(out) => {
            fs::write(out, &movie)?;
            println!("wrote {}", out.display());
        }
        None => {
            let ws = ws.as_mut().expect("opened without --output");
            ws.set_file(
                &game,
                &key,
                movie,
                &format!("Import movie {}", target.rel_path),
            )?;
            println!("{key}: recorded");
            crate::workspace_cmd::finish_edit(
                &game,
                ws,
                &args.mode,
                vec![doctor_toolkit::workspace::FILES.to_owned()],
            )?;
        }
    }
    Ok(())
}

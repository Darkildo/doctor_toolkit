//! A cut-scene with its world written as a video (stage 62): the picture
//! of the shot camera frame by frame, as the Cut-scene Preview draws it
//! (the zones of the cut-scene, its light and sky, the actors), into PNG
//! frames, a movie in the game's format (VP6 with the cut-scene's sound)
//! and an MP4 through ffmpeg (`doctor_toolkit::cutscene::video`).
//!
//! The editor runs it in the background (the Timeline panel's Export
//! video…); without a window it is
//! `doctor-toolkit-gui --render-cutscene <GAME> <FOLDER STREAM> …`, which
//! `doctor-toolkit cutscene render` starts.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use doctor_toolkit::cutscene::video::{VideoOptions, VideoReport, write_video};
use doctor_toolkit::cutscene::{Cutscene, Source, sound};
use doctor_toolkit::game::GameInstance;
use doctor_toolkit::resource::trinity::{SeqMaster, TYPE_TRINITY_SEQ_MASTER};
use doctor_toolkit::resource::video::input::Pcm;
use doctor_toolkit::workspace::Workspace;

use super::preview::Preview;
use super::world::{Edits, World};
use crate::render::{Gpu, GpuContext};

/// A video of a cut-scene to render.
pub struct Job {
    pub root: PathBuf,
    pub cutscene: Arc<Cutscene>,
    /// Workspace edits the world shows.
    pub edits: Edits,
    pub audio: Option<Pcm>,
    /// Seconds from the start of the cut-scene.
    pub from: f32,
    pub to: f32,
    pub width: u32,
    pub height: u32,
    pub fps: f32,
    pub world: bool,
    pub characters: bool,
    pub hour: Option<f32>,
    pub png_dir: Option<PathBuf>,
    pub vp6: Option<PathBuf>,
    pub mp4: Option<PathBuf>,
}

/// What a render did.
pub struct Done {
    pub video: VideoReport,
    /// Loading the world (zones and meshes) and the characters.
    pub load: Duration,
}

impl Done {
    /// One line for the status bar and the command line.
    pub fn summary(&self) -> String {
        let v = &self.video;
        let fps = v.frames as f32 / v.render.as_secs_f32().max(1e-3);
        let mut parts = vec![format!(
            "{} frames: world {:.1} s, render {:.1} s ({fps:.1} frames/s), all {:.1} s",
            v.frames,
            self.load.as_secs_f32(),
            v.render.as_secs_f32(),
            v.total.as_secs_f32()
        )];
        if let Some(b) = v.vp6_bytes {
            parts.push(format!("VP6 {:.1} MiB", b as f64 / 1048576.0));
        }
        if let Some(b) = v.mp4_bytes {
            parts.push(format!("MP4 {:.1} MiB", b as f64 / 1048576.0));
        }
        if v.pngs > 0 {
            parts.push(format!("{} PNG", v.pngs));
        }
        parts.extend(v.notes.iter().cloned());
        parts.join("; ")
    }
}

impl Job {
    /// Frames in the video.
    pub fn frames(&self) -> usize {
        ((self.to - self.from).max(0.0) * self.fps).round().max(1.0) as usize
    }

    /// Renders on `gpu` (a renderer of its own: it may share the device of
    /// the editor); `done` counts the frames rendered.
    pub fn run(&self, gpu: Gpu, done: &AtomicUsize) -> Result<Done, String> {
        let started = Instant::now();
        let ctx = egui::Context::default();
        let mut preview = Preview::new(self.root.clone(), Some(gpu));
        let mut world = World::new(self.edits.clone());
        world.enabled = self.world;
        preview.world = Arc::new(std::sync::Mutex::new(world));
        preview.hour = self.hour;
        preview.shot_camera = true;
        if self.characters {
            preview.load();
            if let Some(e) = preview.load_error() {
                return Err(e);
            }
        }
        if !preview.load_world(&ctx, &self.cutscene, Duration::from_secs(900)) {
            let error = preview
                .world
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .error
                .clone();
            return Err(error.unwrap_or_else(|| "the world did not load".into()));
        }
        let load = started.elapsed();
        let pixels = [self.width, self.height];
        let mut render = |i: usize| -> doctor_toolkit::Result<Vec<u8>> {
            let t = self.from + i as f32 / self.fps;
            let rgba = preview
                .render_pixels(&ctx, &self.cutscene, t, pixels)
                .ok_or_else(|| {
                    doctor_toolkit::Error::InvalidArgument("the GPU did not render".into())
                })?;
            done.store(i + 1, Ordering::Relaxed);
            Ok(rgba)
        };
        let options = VideoOptions {
            width: self.width as usize,
            height: self.height as usize,
            rate: rate_of(self.fps),
            png_dir: self.png_dir.clone(),
            vp6: self.vp6.clone(),
            mp4: self.mp4.clone(),
            audio: self.audio.clone(),
        };
        let video = write_video(self.frames(), &mut render, &options).map_err(|e| e.to_string())?;
        Ok(Done { video, load })
    }
}

/// A frame rate as a fraction: 29.97 is NTSC's 30000/1001.
pub fn rate_of(fps: f32) -> (u32, u32) {
    if (fps - 29.97).abs() < 0.01 {
        (30000, 1001)
    } else if fps.fract().abs() < 1e-3 {
        (fps.round() as u32, 1)
    } else {
        ((fps * 1000.0).round() as u32, 1000)
    }
}

/// The shot named `name` (source name or resource index) of a stream.
fn locate_shot(game: &GameInstance, stream: usize, name: &str) -> Result<usize, String> {
    if let Ok(index) = name.parse::<usize>() {
        return Ok(index);
    }
    let file = game.open_stream(stream).map_err(|e| e.to_string())?;
    for (index, r) in file.layout().resources.iter().enumerate() {
        if r.header.type_id == TYPE_TRINITY_SEQ_MASTER
            && let Ok(data) = file.resource_data(index)
            && let Ok(seq) = SeqMaster::parse(&data)
            && seq.source_name().eq_ignore_ascii_case(name)
        {
            return Ok(index);
        }
    }
    Err(format!("no shot `{name}`"))
}

/// `--render-cutscene <GAME> <FOLDER STREAM> [options]` without a window;
/// returns the exit code.
pub fn main(args: &[String]) -> i32 {
    match command_line(args) {
        Ok(line) => {
            println!("{line}");
            0
        }
        Err(e) => {
            eprintln!("render-cutscene: {e}");
            1
        }
    }
}

const USAGE: &str = "usage: --render-cutscene <GAME> <FOLDER STREAM> [--shot NAME] \
[--workspace-dir DIR] [-o OUT.vp6] [--mp4 OUT.mp4] [--png-dir DIR] [--width 1280] \
[--height 720] [--fps 30] [--from S] [--to S] [--no-world] [--characters] [--hour H] \
[--no-sound]";

fn command_line(args: &[String]) -> Result<String, String> {
    let mut positional = Vec::new();
    let mut options = std::collections::HashMap::new();
    let mut flags = std::collections::HashSet::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--no-world" | "--characters" | "--no-sound" => {
                flags.insert(a.clone());
            }
            _ if a.starts_with('-') => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| format!("{a} needs a value; {USAGE}"))?;
                options.insert(a.trim_start_matches('-').to_owned(), value.clone());
                i += 1;
            }
            _ => positional.push(a.clone()),
        }
        i += 1;
    }
    let [root, stream] = positional.as_slice() else {
        return Err(USAGE.into());
    };
    let number = |key: &str, default: f32| -> Result<f32, String> {
        options.get(key).map_or(Ok(default), |v| {
            v.parse().map_err(|_| format!("--{key} {v}"))
        })
    };
    let path = |key: &str| options.get(key).map(PathBuf::from);
    let vp6 = path("o").or_else(|| path("output")).or_else(|| path("vp6"));
    let mp4 = path("mp4");
    let png_dir = path("png-dir");
    if vp6.is_none() && mp4.is_none() && png_dir.is_none() {
        return Err(format!("nothing to write; {USAGE}"));
    }
    let root = PathBuf::from(root);
    let game = GameInstance::open(&root).map_err(|e| e.to_string())?;
    let ws_dir = path("workspace-dir").unwrap_or_else(|| Workspace::default_dir(game.root()));
    let ws = if Workspace::exists(&ws_dir) {
        Some(Workspace::open(&ws_dir).map_err(|e| e.to_string())?)
    } else {
        None
    };
    let stream_index = game
        .find_stream(stream)
        .ok_or_else(|| format!("stream `{stream}` not found"))?;
    let shot = options
        .get("shot")
        .map(|s| locate_shot(&game, stream_index, s))
        .transpose()?;
    let source = Source::new(&game, ws.as_ref());
    let cutscene = Cutscene::load(&source, stream_index, shot).map_err(|e| e.to_string())?;
    // A shot alone: its span, unless --from and --to say otherwise.
    let (mut from, mut to) = (0.0, cutscene.duration());
    if let Some(s) = shot.and_then(|r| cutscene.shots.iter().find(|s| s.resource == r)) {
        (from, to) = (s.start, s.end());
    }
    from = number("from", from)?.max(0.0);
    to = number("to", to)?.min(cutscene.duration());
    if to <= from {
        return Err(format!("nothing between {from} and {to} s"));
    }
    let audio = if flags.contains("--no-sound") || vp6.is_none() && mp4.is_none() {
        None
    } else {
        sound::mix(&source, &cutscene, from, to).map_err(|e| e.to_string())?
    };
    let context = GpuContext::headless().ok_or("no GPU adapter (wgpu)")?;
    println!("adapter: {}", context.adapter_summary());
    let job = Job {
        root: game.root().to_path_buf(),
        edits: Edits::of(ws.as_ref()),
        cutscene: Arc::new(cutscene),
        audio,
        from,
        to,
        width: number("width", 1280.0)? as u32 & !1,
        height: number("height", 720.0)? as u32 & !1,
        fps: number("fps", 30.0)?,
        world: !flags.contains("--no-world"),
        characters: flags.contains("--characters"),
        hour: options.get("hour").and_then(|h| h.parse().ok()),
        png_dir,
        vp6,
        mp4,
    };
    println!(
        "{} ({}): {:.2}–{:.2} s, {} frames {}×{} at {} frames/s",
        job.cutscene.name,
        job.cutscene.stream_name,
        from,
        to,
        job.frames(),
        job.width,
        job.height,
        job.fps
    );
    let done = AtomicUsize::new(0);
    let result = job.run(Gpu::new(context), &done)?;
    Ok(result.summary())
}

/// Where a video goes from the path the user chose: `.mp4` through
/// ffmpeg, a folder of PNG for no extension, otherwise a game movie.
pub fn outputs(path: &Path) -> (Option<PathBuf>, Option<PathBuf>, Option<PathBuf>) {
    match path.extension().and_then(|e| e.to_str()) {
        Some(e) if e.eq_ignore_ascii_case("mp4") => (None, Some(path.to_path_buf()), None),
        None => (None, None, Some(path.to_path_buf())),
        _ => (Some(path.to_path_buf()), None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use doctor_toolkit::testing as mini;

    /// A few frames of the miniature game's cut-scene: PNG frames and a
    /// movie that reads back (skipped without a GPU adapter).
    #[test]
    fn a_few_frames_become_pictures_and_a_movie() {
        let Some(context) = GpuContext::headless() else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let dir = std::env::temp_dir().join(format!("dt-render-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        mini::write_game(&dir).unwrap();
        mini::write_cutscene(&dir).unwrap();
        let game = GameInstance::open(&dir).unwrap();
        let stream = game.find_stream(mini::CUTSCENE).unwrap();
        let cutscene = Cutscene::load(&Source::new(&game, None), stream, None).unwrap();
        let job = Job {
            root: dir.clone(),
            cutscene: Arc::new(cutscene),
            edits: Edits::default(),
            audio: None,
            from: 0.5,
            to: 0.6,
            width: 64,
            height: 36,
            fps: 30.0,
            world: false,
            characters: false,
            hour: None,
            png_dir: Some(dir.join("png")),
            vp6: Some(dir.join("out.vp6")),
            mp4: None,
        };
        assert_eq!(job.frames(), 3);
        let done = AtomicUsize::new(0);
        let result = job.run(Gpu::new(context), &done).unwrap();
        assert_eq!(result.video.frames, 3);
        assert_eq!(done.load(Ordering::Relaxed), 3);
        assert!(dir.join("png/frame_00002.png").exists());
        let movie = std::fs::read(dir.join("out.vp6")).unwrap();
        let info = doctor_toolkit::resource::video::info(&movie).unwrap();
        assert_eq!(info.video.unwrap().frames, 3);
        assert!(result.summary().contains("3 frames"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rates_and_outputs() {
        assert_eq!(rate_of(30.0), (30, 1));
        assert_eq!(rate_of(29.97), (30000, 1001));
        assert_eq!(rate_of(12.5), (12500, 1000));
        let (vp6, mp4, png) = outputs(Path::new("a/b.mp4"));
        assert!(vp6.is_none() && mp4.is_some() && png.is_none());
        let (vp6, mp4, png) = outputs(Path::new("a/frames"));
        assert!(vp6.is_none() && mp4.is_none() && png.is_some());
        assert!(outputs(Path::new("x.vp6")).0.is_some());
    }
}

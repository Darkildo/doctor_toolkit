//! In-engine cut-scenes (`trinity_seq_master` shots, `docs/formats/cutscenes.md`):
//! list, show the time line, export and import JSON, key the camera and the
//! actors, move and edit commands, swap clips, retime subtitles, export to
//! glTF. The edits are the operations of `doctor_toolkit::cutscene::ops`,
//! the same the MCP server records.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};

use doctor_toolkit::cutscene::cast::Cast;
use doctor_toolkit::cutscene::gltf::{ExportOptions, export};
use doctor_toolkit::cutscene::ops::{self, ActorEdit, CameraEdit, CommandEdit, SubtitleEdit};
use doctor_toolkit::cutscene::{Cutscene, Source};
use doctor_toolkit::game::{GameAssets, GameInstance};
use doctor_toolkit::resource::trinity::SeqMaster;
use doctor_toolkit::workspace::{FILES, Workspace};

use crate::workspace_cmd::{self, EditMode};

macro_rules! out {
    ($($arg:tt)*) => {
        if writeln!(std::io::stdout(), $($arg)*).is_err() {
            std::process::exit(0);
        }
    };
}

/// A cut-scene: the folder stream and optionally one of its shots.
#[derive(Args)]
pub struct Target {
    game_dir: PathBuf,
    /// Folder stream with the shots, e.g.
    /// `gf2/cuba/global/objectives/cp_010_030_mission_1/cp_009/cp_009_hotel_interior_folderstream`
    stream: String,
    /// Shot: resource index or source name (`igc01_s03`); default: the whole sequence
    #[arg(long)]
    shot: Option<String>,
    /// The workspace to read edited shots from: `--workspace-dir` of the
    /// command (or of its edit mode).
    #[arg(skip)]
    workspace_dir: Option<PathBuf>,
}

/// The workspace a read-only command reads edits from.
#[derive(Args)]
pub struct ReadMode {
    /// Workspace directory [default: <GAME_DIR>/_doctor_toolkit/workspace]
    #[arg(long, value_name = "DIR")]
    workspace_dir: Option<PathBuf>,
}

#[derive(Subcommand)]
pub enum CutsceneCommand {
    /// List the shots of the game by stream, with the sequences that play them
    List { game_dir: PathBuf },
    /// Print a cut-scene: shots, commands on the time line, actors, sound and subtitles
    Show {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        read: ReadMode,
        /// Print it as JSON
        #[arg(long)]
        json: bool,
        /// With `--shot`: where the camera and the actors are at this time
        /// (seconds from the start of the shot)
        #[arg(long)]
        time: Option<f32>,
    },
    /// Write one shot as editable JSON (the form `cutscene import` reads)
    Export {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        read: ReadMode,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Replace a shot by edited JSON (recorded in the workspace)
    Import {
        #[command(flatten)]
        target: Target,
        input: PathBuf,
        #[command(flatten)]
        mode: EditMode,
    },
    /// Move a command of a shot to another time (seconds from the shot start)
    Move {
        #[command(flatten)]
        target: Target,
        /// Command index (see `cutscene show`)
        #[arg(long)]
        command: usize,
        #[arg(long)]
        time: f32,
        #[command(flatten)]
        mode: EditMode,
    },
    /// Key the camera of a shot at a time, or move its whole path and change its lens
    Camera {
        #[command(flatten)]
        target: Target,
        /// Seconds from the start of the shot: the key to set (changed, or
        /// added on the path); without it `--offset`, `--turn` and the lens
        /// change every key of the shot
        #[arg(long)]
        time: Option<f32>,
        /// Position `x,y,z` of the key
        #[arg(long, value_parser = triple, allow_hyphen_values = true)]
        position: Option<[f32; 3]>,
        /// Angles about `x,y,z` in degrees of the key
        #[arg(long, value_parser = triple, allow_hyphen_values = true)]
        rotation: Option<[f32; 3]>,
        /// The lens: focal length in millimetres (70, 50, 35 …)
        #[arg(long, conflicts_with = "fov")]
        focal: Option<f32>,
        /// The lens as the vertical field of view of the 16:9 picture, degrees
        #[arg(long)]
        fov: Option<f32>,
        /// `x,y,z` added to the position (of the key, or of every key)
        #[arg(long, value_parser = triple, allow_hyphen_values = true)]
        offset: Option<[f32; 3]>,
        /// Degrees `x,y,z` added to the angles (of the key, or of every key)
        #[arg(long, value_parser = triple, allow_hyphen_values = true)]
        turn: Option<[f32; 3]>,
        /// Remove the camera keys at `--time`
        #[arg(long, requires = "time")]
        delete_key: bool,
        #[command(flatten)]
        mode: EditMode,
    },
    /// Key an actor of a shot at a time, or move it for the whole shot
    Actor {
        #[command(flatten)]
        target: Target,
        /// Actor name (`igc01_dominic`) or GUID
        #[arg(long)]
        actor: String,
        /// Seconds from the start of the shot: the key to set; without it
        /// `--offset` moves the actor for the whole shot
        #[arg(long)]
        time: Option<f32>,
        /// Position `x,y,z` of the key
        #[arg(long, value_parser = triple, allow_hyphen_values = true)]
        position: Option<[f32; 3]>,
        /// Angles about `x,y,z` in degrees of the key
        #[arg(long, value_parser = triple, allow_hyphen_values = true)]
        rotation: Option<[f32; 3]>,
        /// `x,y,z` added to the position (of the key, or for the whole shot)
        #[arg(long, value_parser = triple, allow_hyphen_values = true)]
        offset: Option<[f32; 3]>,
        /// Remove the actor's keys at `--time`
        #[arg(long, requires = "time")]
        delete_key: bool,
        #[command(flatten)]
        mode: EditMode,
    },
    /// Change fields of a command of a shot (and move it)
    Command {
        #[command(flatten)]
        target: Target,
        /// Command index (see `cutscene show`)
        #[arg(long)]
        command: usize,
        /// A field of the command's JSON form and its new value
        /// (`text=start_igc_17`, `seconds=2.5`, `color=[0,0,0,1]`), repeatable
        #[arg(long = "set", value_name = "FIELD=VALUE", required = true)]
        fields: Vec<String>,
        /// Move it to this time too (seconds from the shot start)
        #[arg(long)]
        time: Option<f32>,
        #[command(flatten)]
        mode: EditMode,
    },
    /// Let an actor of a shot play another clip of the shot's banks
    Clip {
        #[command(flatten)]
        target: Target,
        /// Actor name (`igc01_dominic`) or the index of its animation command
        #[arg(long)]
        actor: String,
        /// Clip name, e.g. `igc01_fredo_igc01_s03`
        #[arg(long)]
        clip: String,
        #[command(flatten)]
        mode: EditMode,
    },
    /// Change the start or length of a subtitle of the cut-scene's sound track
    Subtitle {
        #[command(flatten)]
        target: Target,
        /// Line index (see `cutscene show`)
        #[arg(long)]
        line: usize,
        /// New start in milliseconds from the start of the sound
        #[arg(long)]
        start: Option<u32>,
        /// New length in milliseconds
        #[arg(long)]
        duration: Option<u32>,
        /// Sound track (archive entry); default: the first one with subtitles
        #[arg(long)]
        track: Option<String>,
        #[command(flatten)]
        mode: EditMode,
    },
    /// Export the camera and the actors with their clips as one glTF animation
    Gltf {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        read: ReadMode,
        /// Only these shots (index in the sequence, repeatable); default: `--shot` or all
        #[arg(long = "index")]
        indices: Vec<usize>,
        /// Skeletons only, no character meshes
        #[arg(long)]
        skeletons: bool,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Render the cut-scene with its world through the shot cameras into a
    /// video: a movie in the game's format (VP6, with the sound), an MP4
    /// (ffmpeg) or PNG frames. Runs `doctor-toolkit-gui` (next to this
    /// program, or $DOCTOR_TOOLKIT_GUI) without a window; needs a GPU.
    Render {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        read: ReadMode,
        /// The movie in the game's format (.vp6)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// An MP4 through ffmpeg
        #[arg(long)]
        mp4: Option<PathBuf>,
        /// A folder for PNG frames
        #[arg(long)]
        png_dir: Option<PathBuf>,
        #[arg(long, default_value_t = 1280)]
        width: u32,
        #[arg(long, default_value_t = 720)]
        height: u32,
        /// Frames per second (29.97 is NTSC)
        #[arg(long, default_value_t = 30.0)]
        fps: f32,
        /// Seconds from the start of the cut-scene [default: the start of `--shot` or 0]
        #[arg(long)]
        from: Option<f32>,
        /// [default: the end of `--shot` or of the cut-scene]
        #[arg(long)]
        to: Option<f32>,
        /// The actors and cameras alone, without the level
        #[arg(long)]
        no_world: bool,
        /// The actors as characters (reads every material once; slower)
        #[arg(long)]
        characters: bool,
        /// Time of day, hours [default: the cut-scene's]
        #[arg(long)]
        hour: Option<f32>,
        /// No sound
        #[arg(long)]
        no_sound: bool,
    },
}

pub fn run(command: CutsceneCommand) -> Result<()> {
    match command {
        CutsceneCommand::List { game_dir } => list(&game_dir),
        CutsceneCommand::Show {
            target,
            read,
            json,
            time,
        } => show(&target.reading(read.workspace_dir), json, time),
        CutsceneCommand::Export {
            target,
            read,
            output,
        } => export_json(&target.reading(read.workspace_dir), &output),
        CutsceneCommand::Import {
            target,
            input,
            mode,
        } => import_json(&target.editing(&mode), &input, &mode),
        CutsceneCommand::Move {
            target,
            command,
            time,
            mode,
        } => edit_shot(&target.editing(&mode), &mode, |game, ws, stream, shot| {
            let p = CommandEdit {
                command,
                time: Some(time),
                fields: None,
            };
            Ok(ops::command(game, ws, stream, shot, &p)?)
        }),
        CutsceneCommand::Camera {
            target,
            time,
            position,
            rotation,
            focal,
            fov,
            offset,
            turn,
            delete_key,
            mode,
        } => edit_shot(&target.editing(&mode), &mode, |game, ws, stream, shot| {
            let p = CameraEdit {
                time,
                position,
                rotation,
                focal_length: focal,
                vertical_fov: fov,
                offset,
                turn,
                delete_key,
            };
            Ok(ops::camera(game, ws, stream, shot, &p)?)
        }),
        CutsceneCommand::Actor {
            target,
            actor,
            time,
            position,
            rotation,
            offset,
            delete_key,
            mode,
        } => edit_shot(&target.editing(&mode), &mode, |game, ws, stream, shot| {
            let p = ActorEdit {
                actor,
                time,
                position,
                rotation,
                offset,
                delete_key,
                ..Default::default()
            };
            Ok(ops::actor(game, ws, stream, shot, &p)?)
        }),
        CutsceneCommand::Command {
            target,
            command,
            fields,
            time,
            mode,
        } => {
            let fields = parse_fields(&fields)?;
            edit_shot(&target.editing(&mode), &mode, |game, ws, stream, shot| {
                let p = CommandEdit {
                    command,
                    time,
                    fields: Some(fields),
                };
                Ok(ops::command(game, ws, stream, shot, &p)?)
            })
        }
        CutsceneCommand::Clip {
            target,
            actor,
            clip,
            mode,
        } => edit_shot(&target.editing(&mode), &mode, |game, ws, stream, shot| {
            let p = ActorEdit {
                actor,
                clip: Some(clip),
                ..Default::default()
            };
            Ok(ops::actor(game, ws, stream, shot, &p)?)
        }),
        CutsceneCommand::Subtitle {
            target,
            line,
            start,
            duration,
            track,
            mode,
        } => retime_subtitle(
            &target.editing(&mode),
            line,
            start,
            duration,
            track.as_deref(),
            &mode,
        ),
        CutsceneCommand::Gltf {
            target,
            read,
            indices,
            skeletons,
            output,
        } => gltf(
            &target.reading(read.workspace_dir),
            &indices,
            skeletons,
            &output,
        ),
        CutsceneCommand::Render {
            target,
            read,
            output,
            mp4,
            png_dir,
            width,
            height,
            fps,
            from,
            to,
            no_world,
            characters,
            hour,
            no_sound,
        } => {
            let mut args: Vec<std::ffi::OsString> = vec![
                "--render-cutscene".into(),
                target.game_dir.clone().into(),
                target.stream.clone().into(),
            ];
            let mut add = |name: &str, value: std::ffi::OsString| {
                args.push(name.into());
                args.push(value);
            };
            if let Some(shot) = &target.shot {
                add("--shot", shot.into());
            }
            if let Some(dir) = &read.workspace_dir {
                add("--workspace-dir", dir.into());
            }
            if let Some(p) = output {
                add("-o", p.into());
            }
            if let Some(p) = mp4 {
                add("--mp4", p.into());
            }
            if let Some(p) = png_dir {
                add("--png-dir", p.into());
            }
            add("--width", width.to_string().into());
            add("--height", height.to_string().into());
            add("--fps", fps.to_string().into());
            if let Some(v) = from {
                add("--from", v.to_string().into());
            }
            if let Some(v) = to {
                add("--to", v.to_string().into());
            }
            if let Some(v) = hour {
                add("--hour", v.to_string().into());
            }
            for (on, flag) in [
                (no_world, "--no-world"),
                (characters, "--characters"),
                (no_sound, "--no-sound"),
            ] {
                if on {
                    args.push(flag.into());
                }
            }
            render(&args)
        }
    }
}

/// The editor program, which renders without a window.
fn gui_program() -> PathBuf {
    if let Some(p) = std::env::var_os("DOCTOR_TOOLKIT_GUI") {
        return PathBuf::from(p);
    }
    let name = format!("doctor-toolkit-gui{}", std::env::consts::EXE_SUFFIX);
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join(&name)))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from(name))
}

fn render(args: &[std::ffi::OsString]) -> Result<()> {
    let program = gui_program();
    let status = std::process::Command::new(&program)
        .args(args)
        .status()
        .with_context(|| format!("cannot run {}", program.display()))?;
    if !status.success() {
        bail!("rendering failed ({status})");
    }
    Ok(())
}

impl Target {
    fn reading(mut self, dir: Option<PathBuf>) -> Self {
        self.workspace_dir = dir;
        self
    }

    fn editing(mut self, mode: &EditMode) -> Self {
        self.workspace_dir = mode.workspace_dir.clone();
        self
    }
}

fn workspace(game: &GameInstance, dir: Option<&Path>) -> Result<Option<Workspace>> {
    let dir = dir.map_or_else(|| Workspace::default_dir(game.root()), Path::to_path_buf);
    Ok(if Workspace::exists(&dir) {
        Some(Workspace::open(&dir)?)
    } else {
        None
    })
}

/// Stream index and shot resource index of a target.
fn locate(game: &GameInstance, target: &Target) -> Result<(usize, Option<usize>)> {
    Ok(ops::locate(game, &target.stream, target.shot.as_deref())?)
}

/// `x,y,z`.
fn triple(text: &str) -> std::result::Result<[f32; 3], String> {
    let parts: Vec<f32> = text
        .split(',')
        .map(|p| p.trim().parse::<f32>())
        .collect::<std::result::Result<_, _>>()
        .map_err(|e| format!("`{text}`: {e}"))?;
    parts
        .try_into()
        .map_err(|_| format!("`{text}`: three numbers `x,y,z` expected"))
}

/// `field=value` pairs as a JSON object: a value that is not JSON is text.
fn parse_fields(pairs: &[String]) -> Result<serde_json::Value> {
    let mut fields = serde_json::Map::new();
    for pair in pairs {
        let (name, value) = pair
            .split_once('=')
            .with_context(|| format!("`{pair}`: FIELD=VALUE expected"))?;
        let value = serde_json::from_str(value)
            .unwrap_or_else(|_| serde_json::Value::String(value.to_owned()));
        fields.insert(name.trim().to_owned(), value);
    }
    Ok(serde_json::Value::Object(fields))
}

fn list(game_dir: &Path) -> Result<()> {
    let game = GameInstance::open(game_dir)?;
    let streams = ops::overview(&Source::new(&game, None))?;
    let mut total = 0;
    for stream in &streams {
        out!(
            "{}  ({} shots, {:.1} s)",
            stream.stream,
            stream.shots.len(),
            stream.seconds
        );
        for seq in &stream.sequences {
            out!(
                "  sequence {}: starts on `{}`, ends with `{}`, {} shots",
                seq.guid,
                seq.start_message.as_deref().unwrap_or("-"),
                seq.end_message.as_deref().unwrap_or("-"),
                seq.shots
            );
        }
        for s in &stream.shots {
            out!("    {:5}  {:<28} {:6.2} s", s.resource, s.name, s.seconds);
        }
        total += stream.shots.len();
    }
    out!("{total} shot(s)");
    Ok(())
}

fn load(target: &Target) -> Result<(GameInstance, Option<Workspace>, Cutscene)> {
    let game = GameInstance::open(&target.game_dir)?;
    let ws = workspace(&game, target.workspace_dir.as_deref())?;
    let (stream, shot) = locate(&game, target)?;
    let cutscene = Cutscene::load(&Source::new(&game, ws.as_ref()), stream, shot)?;
    Ok((game, ws, cutscene))
}

fn show(target: &Target, json: bool, time: Option<f32>) -> Result<()> {
    let (game, _, mut cutscene) = load(target)?;
    // With `--shot`: that shot alone, not the sequence that plays it.
    let only = match &target.shot {
        Some(_) => locate(&game, target)?.1,
        None => None,
    };
    if time.is_some() && only.is_none() {
        bail!("--time needs --shot: it is seconds from the start of a shot");
    }
    if json {
        if let Some(resource) = only {
            cutscene.shots.retain(|s| s.resource == resource);
        }
        out!("{}", serde_json::to_string_pretty(&cutscene)?);
        return Ok(());
    }
    out!(
        "{} ({}): {} shots, {:.2} s",
        cutscene.name,
        cutscene.stream_name,
        cutscene.shots.len(),
        cutscene.duration()
    );
    if let Some(seq) = &cutscene.sequence {
        out!(
            "sequence {}: starts on `{}`, ends with `{}`",
            seq.guid,
            seq.start_message.as_deref().unwrap_or("-"),
            seq.end_message.as_deref().unwrap_or("-")
        );
    }
    for (i, shot) in cutscene.shots.iter().enumerate() {
        if only.is_some_and(|r| r != shot.resource) {
            continue;
        }
        let view = ops::shot_view(&cutscene, i);
        out!(
            "\nshot {i}: {} (resource {}), {:.2}–{:.2} s, stream {}",
            view.name,
            view.resource,
            view.start,
            view.end,
            view.stream.as_deref().unwrap_or("(none)")
        );
        for a in &view.actors {
            out!(
                "  actor {} {}: {}{}{}",
                a.guid,
                a.name,
                a.class.as_deref().unwrap_or("?"),
                a.assembly
                    .as_deref()
                    .map(|d| format!(", {d}"))
                    .unwrap_or_default(),
                a.set
                    .as_deref()
                    .map(|s| format!(" / {s}"))
                    .unwrap_or_default()
            );
        }
        for c in &view.commands {
            let missing = if c.clip_missing {
                "  (clip not found)"
            } else {
                ""
            };
            out!(
                "  {:4}  {:7.3} s  +{:6.3}  {}{missing}",
                c.index,
                c.time,
                c.length,
                c.label
            );
        }
        if only.is_some() {
            let keys: Vec<String> = view.camera_keys.iter().map(|k| format!("{k:.3}")).collect();
            out!("  camera keys at: {}", keys.join(" "));
        }
        if let Some(t) = time {
            let m = ops::moment(shot, t);
            out!("  at {t:.3} s:");
            match &m.camera {
                Some(c) => out!(
                    "    camera at ({:.3}; {:.3}; {:.3}), angles ({:.2}; {:.2}; {:.2}), lens {:.1} mm ({:.1} deg){}",
                    c.position[0],
                    c.position[1],
                    c.position[2],
                    c.rotation[0],
                    c.rotation[1],
                    c.rotation[2],
                    c.focal_length,
                    c.vertical_fov,
                    if c.on_key { ", on a key" } else { "" }
                ),
                None => out!("    no camera yet"),
            }
            for a in &m.actors {
                out!(
                    "    {} at ({:.3}; {:.3}; {:.3}){}",
                    a.name,
                    a.position[0],
                    a.position[1],
                    a.position[2],
                    a.clip
                        .as_deref()
                        .map(|c| format!(", plays {c}"))
                        .unwrap_or_default()
                );
            }
        }
    }
    if only.is_some() {
        if !cutscene.audio.is_empty() {
            out!("\nsound and subtitles belong to the whole cut-scene: run without --shot");
        }
        return Ok(());
    }
    for track in &cutscene.audio {
        let p = &track.placement;
        let start = match (p.start, &p.message) {
            (Some(t), None) => format!("starts at {t:.3} s"),
            (Some(t), Some(m)) => format!("starts at {t:.3} s on `{m}`"),
            (None, m) => format!(
                "started outside the cut-scene by `{}`",
                m.as_deref().unwrap_or("?")
            ),
        };
        out!(
            "\nsound {} ({:.2} s, {} channels; {start}, volume {})",
            track.entry,
            track.duration_ms as f32 / 1000.0,
            track.channels,
            p.volume
        );
        for (i, s) in track.subtitles.iter().enumerate() {
            out!(
                "  {i:3}  {:>7.3} s  +{:.3}  {} {:08x}  {}",
                s.start_ms as f32 / 1000.0,
                s.duration_ms as f32 / 1000.0,
                s.group,
                s.string,
                s.text.as_deref().unwrap_or("")
            );
        }
    }
    Ok(())
}

/// The one shot a command works on.
fn one_shot(target: &Target) -> Result<(GameInstance, usize, usize)> {
    let game = GameInstance::open(&target.game_dir)?;
    let (stream, shot) = locate(&game, target)?;
    let shot = shot.context("give the shot with --shot")?;
    Ok((game, stream, shot))
}

fn export_json(target: &Target, output: &Path) -> Result<()> {
    let (game, stream, shot) = one_shot(target)?;
    let ws = workspace(&game, target.workspace_dir.as_deref())?;
    let file = game.open_stream(stream)?;
    let data = Source::new(&game, ws.as_ref()).resource(stream, &file, shot)?;
    let seq = SeqMaster::parse(&data)?;
    std::fs::write(output, serde_json::to_string_pretty(&seq)?)
        .with_context(|| format!("cannot write {}", output.display()))?;
    out!(
        "{}: {} commands",
        output.display(),
        seq.blocks.iter().map(|b| b.commands.len()).sum::<usize>()
    );
    Ok(())
}

fn import_json(target: &Target, input: &Path, mode: &EditMode) -> Result<()> {
    let text = std::fs::read_to_string(input)
        .with_context(|| format!("cannot read {}", input.display()))?;
    edit_shot(target, mode, |game, ws, stream, shot| {
        Ok(ops::import_json(game, ws, stream, shot, &text)?)
    })
}

/// Runs an operation on the one shot of `target` and tells what it recorded.
fn edit_shot(
    target: &Target,
    mode: &EditMode,
    edit: impl FnOnce(&GameInstance, &mut Workspace, &str, &str) -> Result<ops::ShotEdit>,
) -> Result<()> {
    let game = GameInstance::open(&target.game_dir)?;
    let shot = target
        .shot
        .as_deref()
        .context("give the shot with --shot")?;
    let mut ws = workspace_cmd::open_for_edit(&game, mode)?;
    let done = edit(&game, &mut ws, &target.stream, shot)?;
    if !done.changed {
        out!("nothing changed");
        return Ok(());
    }
    out!("{}", done.label);
    for line in &done.done {
        out!("  {line}");
    }
    workspace_cmd::finish_edit(
        &game,
        &mut ws,
        mode,
        vec![game.streams()[done.stream].stream_name()],
    )
}

fn retime_subtitle(
    target: &Target,
    line: usize,
    start: Option<u32>,
    duration: Option<u32>,
    track: Option<&str>,
    mode: &EditMode,
) -> Result<()> {
    let game = GameInstance::open(&target.game_dir)?;
    let mut ws = workspace_cmd::open_for_edit(&game, mode)?;
    let p = SubtitleEdit {
        line,
        start_ms: start,
        duration_ms: duration,
    };
    let done = ops::subtitle(
        &game,
        &mut ws,
        &target.stream,
        target.shot.as_deref(),
        track,
        &p,
    )?;
    out!("{}", done.label);
    out!(
        "  {} ({:.1} MB) is the change: {}",
        done.entry,
        done.file_bytes as f64 / 1e6,
        done.note
    );
    workspace_cmd::finish_edit(&game, &mut ws, mode, vec![FILES.to_owned()])
}

fn gltf(target: &Target, indices: &[usize], skeletons: bool, output: &Path) -> Result<()> {
    let game = Arc::new(GameInstance::open(&target.game_dir)?);
    let ws = workspace(&game, target.workspace_dir.as_deref())?;
    let (stream, shot) = locate(&game, target)?;
    let source = Source::new(&game, ws.as_ref());
    let cutscene = Cutscene::load(&source, stream, shot)?;
    let mut shots = indices.to_vec();
    if shots.is_empty()
        && let Some(r) = shot
        && let Some(i) = cutscene.shots.iter().position(|s| s.resource == r)
    {
        shots.push(i);
    }
    let (index, _) = game.build_index(|_| {});
    let mut assets = GameAssets::new(game.clone(), &index);
    let rigs = assets.rigs().to_vec();
    let mut cast = Cast::new(&game, &index, rigs);
    let options = ExportOptions {
        shots,
        skeletons_only: skeletons,
    };
    let (glb, report) = export(&cutscene, &source, &mut cast, &mut assets, &options)?;
    std::fs::write(output, &glb).with_context(|| format!("cannot write {}", output.display()))?;
    for p in &report.problems {
        out!("  {p}");
    }
    out!(
        "{}: {} actors, {} characters, {} clips, {} bytes",
        output.display(),
        report.actors,
        report.characters,
        report.animations,
        glb.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::*;

    #[derive(Parser)]
    struct Cli {
        #[command(subcommand)]
        command: CutsceneCommand,
    }

    #[test]
    fn edits_go_to_the_given_workspace() {
        Cli::command().debug_assert();
        let cli = Cli::parse_from([
            "x",
            "subtitle",
            "game",
            "stream",
            "--line",
            "0",
            "--start",
            "10",
            "--workspace-dir",
            "/tmp/ws",
        ]);
        let CutsceneCommand::Subtitle { target, mode, .. } = cli.command else {
            panic!("not a subtitle command");
        };
        let target = target.editing(&mode);
        assert_eq!(target.workspace_dir.as_deref(), Some(Path::new("/tmp/ws")));
        let cli = Cli::parse_from(["x", "show", "game", "stream", "--workspace-dir", "/tmp/r"]);
        let CutsceneCommand::Show { target, read, .. } = cli.command else {
            panic!("not a show command");
        };
        let target = target.reading(read.workspace_dir);
        assert_eq!(target.workspace_dir.as_deref(), Some(Path::new("/tmp/r")));
    }
}

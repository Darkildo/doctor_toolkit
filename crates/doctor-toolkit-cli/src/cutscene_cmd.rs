//! In-engine cut-scenes (`trinity_seq_master` shots, `docs/formats/cutscenes.md`):
//! list, show the time line, export and import JSON, move commands, swap
//! clips, retime subtitles, export to glTF.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};

use doctor_toolkit::cutscene::cast::Cast;
use doctor_toolkit::cutscene::gltf::{ExportOptions, export};
use doctor_toolkit::cutscene::{Cutscene, Source, command_span, list_shots, sequences_in};
use doctor_toolkit::game::{GameAssets, GameInstance};
use doctor_toolkit::resource::tob::Tob;
use doctor_toolkit::resource::trinity::{Command, FixedText, SeqMaster};
use doctor_toolkit::workspace::{FILES, FileKey, Workspace};

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
}

pub fn run(command: CutsceneCommand) -> Result<()> {
    match command {
        CutsceneCommand::List { game_dir } => list(&game_dir),
        CutsceneCommand::Show { target, read, json } => {
            show(&target.reading(read.workspace_dir), json)
        }
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
        } => edit_shot(&target.editing(&mode), &mode, |seq| {
            let block = seq.block_mut().context("the shot has no block")?;
            let label = block
                .commands
                .get(command)
                .map(Command::label)
                .with_context(|| format!("no command {command}"))?;
            block.move_command(command, time)?;
            Ok(format!("Move `{label}` to {time:.3} s"))
        }),
        CutsceneCommand::Clip {
            target,
            actor,
            clip,
            mode,
        } => swap_clip(&target.editing(&mode), &actor, &clip, &mode),
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
    }
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
    let stream = game
        .find_stream(&target.stream)
        .with_context(|| format!("stream `{}` not found", target.stream))?;
    let Some(shot) = &target.shot else {
        return Ok((stream, None));
    };
    if let Ok(index) = shot.parse::<usize>() {
        return Ok((stream, Some(index)));
    }
    let file = game.open_stream(stream)?;
    for (index, r) in file.layout().resources.iter().enumerate() {
        if r.header.type_id == doctor_toolkit::resource::trinity::TYPE_TRINITY_SEQ_MASTER {
            let seq = SeqMaster::parse(&file.resource_data(index)?)?;
            if seq.source_name().eq_ignore_ascii_case(shot) {
                return Ok((stream, Some(index)));
            }
        }
    }
    bail!("no shot `{shot}` in {}", target.stream)
}

fn list(game_dir: &Path) -> Result<()> {
    let game = GameInstance::open(game_dir)?;
    let shots = list_shots(&game)?;
    let mut streams: Vec<usize> = shots.iter().map(|s| s.stream).collect();
    streams.dedup();
    let source = Source::new(&game, None);
    for stream in streams {
        let here: Vec<_> = shots.iter().filter(|s| s.stream == stream).collect();
        let total: f32 = here.iter().map(|s| s.duration).sum();
        out!(
            "{}  ({} shots, {total:.1} s)",
            here[0].stream_name,
            here.len()
        );
        for seq in sequences_in(&source, stream)? {
            out!(
                "  sequence {}: starts on `{}`, ends with `{}`, {} shots",
                seq.guid,
                seq.start_message.as_deref().unwrap_or("-"),
                seq.end_message.as_deref().unwrap_or("-"),
                seq.shots.len()
            );
        }
        for s in here {
            out!("    {:5}  {:<28} {:6.2} s", s.resource, s.name, s.duration);
        }
    }
    out!("{} shot(s)", shots.len());
    Ok(())
}

fn load(target: &Target) -> Result<(GameInstance, Option<Workspace>, Cutscene)> {
    let game = GameInstance::open(&target.game_dir)?;
    let ws = workspace(&game, target.workspace_dir.as_deref())?;
    let (stream, shot) = locate(&game, target)?;
    let cutscene = Cutscene::load(&Source::new(&game, ws.as_ref()), stream, shot)?;
    Ok((game, ws, cutscene))
}

fn show(target: &Target, json: bool) -> Result<()> {
    let (_, _, cutscene) = load(target)?;
    if json {
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
        out!(
            "\nshot {i}: {} (resource {}), {:.2}–{:.2} s, stream {}",
            shot.seq.source_name(),
            shot.resource,
            shot.start,
            shot.end(),
            if shot.stream.is_some() {
                shot.stream_name.as_str()
            } else {
                "(none)"
            }
        );
        for a in &shot.actors {
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
        let block = shot.block();
        for (k, (c, t)) in block.commands.iter().zip(block.times()).enumerate() {
            if matches!(c, Command::Wait { .. }) {
                continue;
            }
            let (_, length) = command_span(shot, k, t);
            let missing = match c {
                Command::ActorAnim { clip, .. } if shot.clip(&clip.text).is_none() => {
                    "  (clip not found)"
                }
                _ => "",
            };
            out!("  {k:4}  {t:7.3} s  +{length:6.3}  {}{missing}", c.label());
        }
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

fn record(
    game: &GameInstance,
    stream: usize,
    shot: usize,
    data: Vec<u8>,
    label: &str,
    mode: &EditMode,
) -> Result<()> {
    let mut ws = workspace_cmd::open_for_edit(game, mode)?;
    if !ws.set_resource(game, stream, shot, data, label)? {
        out!("nothing changed");
        return Ok(());
    }
    out!("{label}");
    workspace_cmd::finish_edit(
        game,
        &mut ws,
        mode,
        vec![game.streams()[stream].stream_name()],
    )
}

fn import_json(target: &Target, input: &Path, mode: &EditMode) -> Result<()> {
    let (game, stream, shot) = one_shot(target)?;
    let text = std::fs::read_to_string(input)
        .with_context(|| format!("cannot read {}", input.display()))?;
    let seq: SeqMaster = serde_json::from_str(&text)?;
    let label = format!("Import cut-scene shot {}", seq.source_name());
    record(&game, stream, shot, seq.to_bytes()?, &label, mode)
}

fn edit_shot(
    target: &Target,
    mode: &EditMode,
    edit: impl FnOnce(&mut SeqMaster) -> Result<String>,
) -> Result<()> {
    let (game, stream, shot) = one_shot(target)?;
    let ws = workspace_cmd::open_for_edit(&game, mode)?;
    let mut seq = SeqMaster::parse(&ws.current_data(&game, stream, shot)?)?;
    drop(ws);
    let label = edit(&mut seq)?;
    record(&game, stream, shot, seq.to_bytes()?, &label, mode)
}

fn swap_clip(target: &Target, actor: &str, clip: &str, mode: &EditMode) -> Result<()> {
    let (game, _, cutscene) = load(target)?;
    let (_, resource) = locate(&game, target)?;
    let resource = resource.context("give the shot with --shot")?;
    let shot = cutscene
        .shots
        .iter()
        .find(|s| s.resource == resource)
        .context("the shot is not in its sequence")?;
    if shot.clip(clip).is_none() {
        let names: Vec<String> = shot.clips.iter().filter_map(|c| c.name.clone()).collect();
        bail!(
            "no clip `{clip}` in {} (clips: {})",
            shot.stream_name,
            names.join(", ")
        );
    }
    let wanted = actor.to_owned();
    let new_clip = clip.to_owned();
    edit_shot(target, mode, move |seq| {
        let block = seq.block_mut().context("the shot has no block")?;
        let guid = block.commands.iter().find_map(|c| match c {
            Command::ActorCreate { actor, name, .. } if name.text.eq_ignore_ascii_case(&wanted) => {
                Some(*actor)
            }
            _ => None,
        });
        let index: Option<usize> = wanted.parse().ok();
        let mut changed = None;
        for (k, c) in block.commands.iter_mut().enumerate() {
            if let Command::ActorAnim { actor, clip, .. } = c
                && (Some(*actor) == guid || Some(k) == index)
            {
                changed = Some(std::mem::replace(clip, FixedText::new(new_clip.clone())).text);
                break;
            }
        }
        let old = changed.with_context(|| format!("no animation of actor `{wanted}`"))?;
        Ok(format!("Swap clip {old} for {new_clip}"))
    })
}

fn retime_subtitle(
    target: &Target,
    line: usize,
    start: Option<u32>,
    duration: Option<u32>,
    track: Option<&str>,
    mode: &EditMode,
) -> Result<()> {
    let (game, _, cutscene) = load(target)?;
    let audio = match track {
        Some(name) => cutscene
            .audio
            .iter()
            .find(|a| a.entry.eq_ignore_ascii_case(name))
            .with_context(|| format!("no sound track `{name}`"))?,
        None => cutscene
            .audio
            .iter()
            .find(|a| a.tob.is_some())
            .context("the cut-scene's sound has no subtitles")?,
    };
    let sub = audio
        .subtitles
        .get(line)
        .with_context(|| format!("no line {line}"))?;
    let mut tob: Tob = audio.tob.clone().context("no subtitle table")?;
    let entry = &mut tob.groups[sub.group_index].entries[sub.entry];
    if let Some(s) = start {
        entry.start_ms = s;
    }
    if let Some(d) = duration {
        entry.duration_ms = d;
    }
    let (s, d) = (entry.start_ms, entry.duration_ms);
    let group = &mut tob.groups[sub.group_index];
    group.entries.sort_by_key(|e| e.start_ms);
    let key = FileKey::archive_entry(&audio.entry);
    let mut ws = workspace_cmd::open_for_edit(&game, mode)?;
    let snu = ws.current_file(&game, &key)?;
    let data = tob.replace_in_snu(&snu)?;
    let label = format!(
        "Subtitle {line} of {} at {:.3} s for {:.3} s",
        cutscene.name,
        s as f32 / 1000.0,
        d as f32 / 1000.0
    );
    ws.set_file(&game, &key, data, &label)?;
    out!("{label}");
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

//! `workspace` subcommands and the workspace/direct choice of the editing commands.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use clap::{Args, Subcommand};

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::workspace::{
    ApplyOptions, ApplyReport, ChangeState, FileKey, ResourceKey, SemanticDiff, StreamAction,
    StreamState, Workspace,
};

/// Writes a line to stdout and exits quietly when the pipe is closed.
macro_rules! out {
    ($($arg:tt)*) => {
        if writeln!(std::io::stdout(), $($arg)*).is_err() {
            std::process::exit(0);
        }
    };
}

/// The game and its workspace.
#[derive(Args)]
pub struct Target {
    pub game_dir: PathBuf,
    /// Workspace directory [default: <GAME_DIR>/_doctor_toolkit/workspace]
    #[arg(long = "dir", value_name = "DIR")]
    pub dir: Option<PathBuf>,
}

/// How an editing command applies its change.
#[derive(Args)]
pub struct EditMode {
    /// Record the change in the workspace only (the default); `workspace apply` writes it
    #[arg(long, conflicts_with = "direct")]
    pub workspace: bool,
    /// Record the change in the workspace and write it into the game at once
    #[arg(long)]
    pub direct: bool,
    /// Workspace directory [default: <GAME_DIR>/_doctor_toolkit/workspace]
    #[arg(long, value_name = "DIR")]
    pub workspace_dir: Option<PathBuf>,
}

#[derive(Subcommand)]
pub enum WorkspaceCommand {
    /// Create the workspace of a game
    Init(Target),
    /// List the changes and whether the game has them
    Status(Target),
    /// Compare the changes with the original data
    Diff {
        #[command(flatten)]
        target: Target,
        /// Only changes whose key contains this text
        filter: Option<String>,
        /// List the changed byte ranges
        #[arg(long)]
        bytes: bool,
    },
    /// Write the changes into the game, one rewrite per stream
    Apply {
        #[command(flatten)]
        target: Target,
        /// Overwrite streams that were changed outside the workspace
        #[arg(long)]
        force: bool,
        /// Only this stream, e.g. `gf2/frontend` (repeatable)
        #[arg(long = "stream", value_name = "STREAM")]
        streams: Vec<String>,
    },
    /// Remove changes from the workspace (undoable); `apply` then updates the game
    ///
    /// With `--game`, put the original files back into the game instead and
    /// keep the changes in the workspace.
    Revert {
        #[command(flatten)]
        target: Target,
        /// Resource keys, or text that matches part of the keys
        #[arg(required_unless_present_any = ["all", "game"])]
        keys: Vec<String>,
        /// Remove every change
        #[arg(long, conflicts_with = "keys")]
        all: bool,
        /// Restore the original game files; the workspace keeps its changes
        #[arg(long, conflicts_with_all = ["keys", "all"])]
        game: bool,
        /// With `--game`: also restore files changed outside the workspace
        #[arg(long, requires = "game")]
        force: bool,
    },
    /// Undo the last operation
    Undo(Target),
    /// Redo the last undone operation
    Redo(Target),
    /// List the operations; `*` marks those in effect
    Log(Target),
}

pub fn run(command: WorkspaceCommand) -> Result<()> {
    match command {
        WorkspaceCommand::Init(target) => {
            let game = GameInstance::open(&target.game_dir)?;
            let dir = target
                .dir
                .unwrap_or_else(|| Workspace::default_dir(game.root()));
            let ws = Workspace::create(&dir, &game)?;
            out!("created workspace {}", ws.dir().display());
            Ok(())
        }
        WorkspaceCommand::Status(target) => {
            let (game, ws) = open(&target)?;
            status(&game, &ws)
        }
        WorkspaceCommand::Diff {
            target,
            filter,
            bytes,
        } => {
            let (_, ws) = open(&target)?;
            diff(&ws, filter.as_deref(), bytes)
        }
        WorkspaceCommand::Apply {
            target,
            force,
            streams,
        } => {
            let (game, mut ws) = open(&target)?;
            let options = ApplyOptions {
                force,
                streams: (!streams.is_empty()).then_some(streams),
            };
            let report = ws.apply(&game, options)?;
            print_report(&report);
            Ok(())
        }
        WorkspaceCommand::Revert {
            target,
            keys,
            all,
            game: from_game,
            force,
        } => {
            let (game, mut ws) = open(&target)?;
            if from_game {
                let report = ws.unapply(&game, force)?;
                print_report(&report);
                out!("{} change(s) stay in the workspace", ws.len());
                return Ok(());
            }
            if all {
                if !ws.revert_all()? {
                    out!("nothing to revert");
                }
                return Ok(());
            }
            let file_keys: Vec<FileKey> = ws
                .file_changes()
                .map(|c| c.key.clone())
                .filter(|k| {
                    let text = k.to_string();
                    keys.iter().any(|q| text == *q || text.contains(q.as_str()))
                })
                .collect();
            let res_keys = if file_keys.is_empty() {
                match_keys(&ws, &keys)?
            } else {
                match_keys(&ws, &keys).unwrap_or_default()
            };
            for key in &res_keys {
                out!("revert {key}");
            }
            for key in &file_keys {
                out!("revert {key}");
            }
            let label = format!("Revert {} change(s)", res_keys.len() + file_keys.len());
            let mut any = false;
            if !res_keys.is_empty() {
                any |= ws.revert(&res_keys, &label)?;
            }
            if !file_keys.is_empty() {
                any |= ws.revert_files(&file_keys, &label)?;
            }
            if !any {
                out!("nothing to revert");
            }
            Ok(())
        }
        WorkspaceCommand::Undo(target) => {
            let (_, mut ws) = open(&target)?;
            match ws.undo()? {
                Some(label) => out!("undone: {label}"),
                None => out!("nothing to undo"),
            }
            Ok(())
        }
        WorkspaceCommand::Redo(target) => {
            let (_, mut ws) = open(&target)?;
            match ws.redo()? {
                Some(label) => out!("redone: {label}"),
                None => out!("nothing to redo"),
            }
            Ok(())
        }
        WorkspaceCommand::Log(target) => {
            let (_, ws) = open(&target)?;
            if ws.operations().is_empty() {
                out!("no operations");
            }
            for (i, op) in ws.operations().iter().enumerate() {
                out!(
                    "{} {:>4}  {}  {}  ({} resource(s){})",
                    if i < ws.head() { '*' } else { ' ' },
                    op.seq,
                    format_time(op.time),
                    op.label,
                    op.edits.len(),
                    if op.file_edits.is_empty() {
                        String::new()
                    } else {
                        format!(", {} file(s)", op.file_edits.len())
                    }
                );
            }
            Ok(())
        }
    }
}

fn open(target: &Target) -> Result<(GameInstance, Workspace)> {
    let game = GameInstance::open(&target.game_dir)?;
    let dir = target
        .dir
        .clone()
        .unwrap_or_else(|| Workspace::default_dir(game.root()));
    if !Workspace::exists(&dir) {
        bail!(
            "no workspace in {}; create it with `workspace init` or by editing",
            dir.display()
        );
    }
    let ws = Workspace::open(&dir)?;
    Ok((game, ws))
}

/// Opens or creates the workspace for an editing command.
pub fn open_for_edit(game: &GameInstance, mode: &EditMode) -> Result<Workspace> {
    Ok(Workspace::for_game(game, mode.workspace_dir.as_deref())?)
}

/// After an editing command: apply the touched streams in direct mode, or
/// tell how to apply them.
pub fn finish_edit(
    game: &GameInstance,
    ws: &mut Workspace,
    mode: &EditMode,
    streams: Vec<String>,
) -> Result<()> {
    if mode.direct {
        let report = ws.apply(
            game,
            ApplyOptions {
                force: false,
                streams: Some(streams),
            },
        )?;
        print_report(&report);
    } else {
        out!(
            "recorded in workspace {}; run `doctor-toolkit workspace apply {}` to write it into the game",
            ws.dir().display(),
            quote(game.root())
        );
    }
    Ok(())
}

fn quote(path: &Path) -> String {
    let text = path.display().to_string();
    if text.contains(' ') {
        format!("\"{text}\"")
    } else {
        text
    }
}

fn status(game: &GameInstance, ws: &Workspace) -> Result<()> {
    let status = ws.status(game)?;
    out!("workspace {}", ws.dir().display());
    if !status.fingerprint_matches {
        out!(
            "warning: the workspace was made for another game build ({}, executable {} bytes)",
            ws.manifest().game.profile,
            ws.manifest().game.executable_size
        );
    }
    out!(
        "{} change(s): {} pending, {} applied, {} in conflict",
        status.changes.len(),
        status.count(ChangeState::Pending),
        status.count(ChangeState::Applied),
        status.count(ChangeState::Conflict)
    );
    for (key, state) in &status.changes {
        let change = ws.change(key).expect("status lists workspace changes");
        out!(
            "  {:<9} {key}  {} -> {} byte(s)  {}",
            match state {
                ChangeState::Pending => "pending",
                ChangeState::Applied => "applied",
                ChangeState::Conflict => "conflict",
            },
            change.original.size,
            change.payload.size,
            if change.is_added() {
                format!("(added) {}", change.label)
            } else {
                change.label.clone()
            }
        );
    }
    for stream in ws.new_streams() {
        out!(
            "  new stream {}  hash {:08x}, parent {}{}, in {}",
            stream.name,
            stream.hash,
            stream.parent,
            if stream.with_parent {
                " (loaded with it)"
            } else {
                ""
            },
            stream.tocs.join(", ")
        );
    }
    for key in &status.pending_reverts {
        out!(
            "  {:<9} {key}  (the game still has the old change)",
            "revert"
        );
    }
    if !status.files.is_empty() {
        out!("{} file change(s) outside the streams:", status.files.len());
    }
    for (key, state) in &status.files {
        let change = ws.file_change(key).expect("status lists workspace files");
        out!(
            "  {:<9} {key}  {} -> {} byte(s)  {}",
            match state {
                ChangeState::Pending => "pending",
                ChangeState::Applied => "applied",
                ChangeState::Conflict => "conflict",
            },
            change.original.size,
            change.payload.size,
            change.label
        );
    }
    for path in &status.pending_file_reverts {
        out!(
            "  {:<9} {path}  (written earlier, no longer wanted)",
            "revert"
        );
    }
    if !status.streams.is_empty() {
        out!("streams:");
    }
    for stream in &status.streams {
        let state = match stream.state {
            StreamState::Original => "original (or not created yet)",
            StreamState::Applied => "written by the workspace",
            StreamState::ChangedExternally => "changed outside the workspace",
            StreamState::Missing => "missing from the game",
        };
        let todo = if stream.up_to_date {
            "up to date"
        } else {
            "needs apply"
        };
        out!("  {:<40} {state}, {todo}", stream.stream);
    }
    let conflicts = status.conflicts();
    if !conflicts.is_empty() {
        out!(
            "{} stream(s) were changed by another program; `apply --force` overwrites them",
            conflicts.len()
        );
    } else if !status.is_up_to_date() {
        out!(
            "run `doctor-toolkit workspace apply {}` to write the changes",
            quote(game.root())
        );
    } else {
        out!("the game is up to date");
    }
    Ok(())
}

fn diff(ws: &Workspace, filter: Option<&str>, bytes: bool) -> Result<()> {
    let entries: Vec<_> = ws
        .diff()
        .into_iter()
        .filter(|e| filter.is_none_or(|f| e.key.to_string().contains(f)))
        .collect();
    let files: Vec<_> = ws
        .file_changes()
        .filter(|c| filter.is_none_or(|f| c.key.to_string().contains(f)))
        .collect();
    if entries.is_empty() && files.is_empty() {
        out!("no changes");
    }
    for change in files {
        out!(
            "{}  {} -> {} byte(s)  {}",
            change.key,
            change.original.size,
            change.payload.size,
            change.label
        );
    }
    for entry in entries {
        out!(
            "{}  {} -> {} byte(s)  {}",
            entry.key,
            entry.original_size,
            entry.new_size,
            entry.label
        );
        let Some(detail) = ws.detail(&entry.key)? else {
            continue;
        };
        out!(
            "  {} byte(s) differ in {}{} range(s)",
            detail.bytes.changed_bytes,
            detail.bytes.ranges.len(),
            if detail.bytes.truncated { "+" } else { "" }
        );
        if bytes {
            for range in &detail.bytes.ranges {
                out!("    {:#010x}..{:#010x}", range.start, range.end);
            }
        }
        match detail.semantic {
            Some(SemanticDiff::Strings(strings)) => {
                for change in &strings.changed {
                    out!(
                        "  [{}] {:08x}: {:?} -> {:?}",
                        change.language_name,
                        change.hash,
                        change.old,
                        change.new
                    );
                }
                for hash in &strings.added {
                    out!("  added {hash:08x}");
                }
                for hash in &strings.removed {
                    out!("  removed {hash:08x}");
                }
            }
            Some(SemanticDiff::Texture(texture)) => {
                out!(
                    "  texture {}: {} -> {}",
                    texture.name,
                    texture.old,
                    texture.new
                );
            }
            Some(SemanticDiff::Logic(logic)) => {
                for change in &logic.values {
                    out!("  {}: {} -> {}", change.place, change.old, change.new);
                }
                if logic.truncated {
                    out!("  (more values changed)");
                }
                for line in &logic.other {
                    out!("  {line}");
                }
            }
            Some(SemanticDiff::Apx(apx)) => {
                let size =
                    |s: Option<usize>| s.map_or_else(|| "none".to_owned(), |s| s.to_string());
                for block in &apx.blocks {
                    out!(
                        "  block {} #{}: {} -> {} byte(s)",
                        block.tag,
                        block.ordinal,
                        size(block.old_size),
                        size(block.new_size)
                    );
                }
                for texture in &apx.textures {
                    out!(
                        "  embedded texture {}: {} -> {}",
                        texture.name,
                        texture.old,
                        texture.new
                    );
                }
            }
            None => {}
        }
    }
    Ok(())
}

fn print_report(report: &ApplyReport) {
    if report.streams.is_empty() {
        out!("nothing to do: the game is up to date");
    }
    for (stream, action) in &report.streams {
        match action {
            StreamAction::Rebuilt(n) => out!("rebuilt  {stream} ({n} resource(s) replaced)"),
            StreamAction::Restored => out!("restored {stream}"),
        }
    }
    for path in &report.backed_up {
        out!("backed up {path}");
    }
    for path in &report.written {
        out!("wrote     {path}");
    }
    for warning in &report.warnings {
        out!("warning: {warning}");
    }
}

/// Resolves key arguments: an exact key, or text contained in keys of the workspace.
fn match_keys(ws: &Workspace, args: &[String]) -> Result<Vec<ResourceKey>> {
    let mut keys = Vec::new();
    for arg in args {
        if let Ok(key) = arg.parse::<ResourceKey>()
            && ws.change(&key).is_some()
        {
            keys.push(key);
            continue;
        }
        let found: Vec<_> = ws
            .changes()
            .filter(|c| c.key.to_string().contains(arg.as_str()))
            .map(|c| c.key.clone())
            .collect();
        if found.is_empty() {
            bail!("no change matches `{arg}`");
        }
        keys.extend(found);
    }
    keys.sort();
    keys.dedup();
    Ok(keys)
}

/// `YYYY-MM-DD HH:MM:SS` in UTC.
fn format_time(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn formats_utc_times() {
        assert_eq!(super::format_time(0), "1970-01-01 00:00:00");
        assert_eq!(super::format_time(1_794_441_600), "2026-11-12 00:00:00");
        assert_eq!(
            super::format_time(951_782_400 + 3661),
            "2000-02-29 01:01:01"
        );
    }
}

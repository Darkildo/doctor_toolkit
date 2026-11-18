//! `workspace_status`, `workspace_diff` and `project_status`: what is
//! edited and not yet in the game. Read-only: the workspace is opened
//! without repairing it on disk, the project's build cache is not updated.

use serde_json::{Value, json};

use doctor_toolkit::project::{self, EntryState, Project};
use doctor_toolkit::workspace::{ChangeState, SemanticDiff, StreamState, Workspace};

use super::{Output, ProjectStatusParams, WorkspaceDiffParams, WorkspaceStatusParams, clamp, hex};
use crate::state::{ToolError, ToolResult, Toolkit, fail};

/// Lines of detail shown per change at most.
const DETAIL_LINES: usize = 40;

fn state_name(state: &ChangeState) -> &'static str {
    match state {
        ChangeState::Pending => "pending",
        ChangeState::Applied => "applied",
        ChangeState::Conflict => "conflict",
    }
}

fn no_workspace(kit: &Toolkit) -> Output {
    Output::Json(json!({
        "workspace": kit.workspace_dir,
        "exists": false,
        "note": "there is no workspace: nothing is edited",
    }))
}

pub fn workspace_status(kit: &Toolkit, p: WorkspaceStatusParams) -> ToolResult<Output> {
    let Some(ws) = kit.workspace()? else {
        return Ok(no_workspace(kit));
    };
    let status = ws.status(&kit.game)?;
    let offset = p.offset.unwrap_or(0);
    let limit = clamp(p.limit, 100, 500);
    let changes: Vec<Value> = status
        .changes
        .iter()
        .skip(offset)
        .take(limit)
        .map(|(key, state)| {
            let change = ws.change(key);
            json!({
                "key": key.to_string(),
                "state": state_name(state),
                "label": change.map(|c| c.label.as_str()),
                "added": change.is_some_and(|c| c.is_added()),
                "original_bytes": change.map(|c| c.original.size),
                "new_bytes": change.map(|c| c.payload.size),
            })
        })
        .collect();
    let files: Vec<Value> = status
        .files
        .iter()
        .take(limit)
        .map(|(key, state)| {
            let change = ws.file_change(key);
            json!({
                "file": key.to_string(),
                "state": state_name(state),
                "label": change.map(|c| c.label.as_str()),
            })
        })
        .collect();
    let streams: Vec<Value> = status
        .streams
        .iter()
        .take(limit)
        .map(|s| {
            json!({
                "stream": s.stream,
                "state": match s.state {
                    StreamState::Original => "original",
                    StreamState::Applied => "written by the workspace",
                    StreamState::ChangedExternally => "changed outside the workspace",
                    StreamState::Missing => "missing from the game",
                },
                "up_to_date": s.up_to_date,
            })
        })
        .collect();
    let new_streams: Vec<Value> = ws
        .new_streams()
        .map(|s| json!({ "name": s.name, "hash": hex(s.hash), "parent": s.parent }))
        .collect();
    let history: Vec<Value> = ws.operations()[..ws.head()]
        .iter()
        .rev()
        .take(20)
        .map(|op| json!({ "label": op.label, "time": op.time, "resources": op.edits.len() }))
        .collect();
    Ok(Output::Json(json!({
        "workspace": ws.dir(),
        "exists": true,
        "made_for_this_game": status.fingerprint_matches,
        "counts": {
            "changes": status.changes.len(),
            "pending": status.count(ChangeState::Pending),
            "applied": status.count(ChangeState::Applied),
            "conflict": status.count(ChangeState::Conflict),
        },
        "up_to_date": status.is_up_to_date(),
        "offset": offset,
        "changes": changes,
        "pending_reverts": status.pending_reverts.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "files": files,
        "streams": streams,
        "new_streams": new_streams,
        "recent_operations": history,
        "undo": ws.undo_label(),
        "redo": ws.redo_label(),
    })))
}

/// What the change of `key` does, in lines.
pub(crate) fn detail_lines(
    ws: &Workspace,
    key: &doctor_toolkit::workspace::ResourceKey,
) -> ToolResult<Vec<String>> {
    let Some(detail) = ws.detail(key)? else {
        return Ok(Vec::new());
    };
    let mut lines = vec![format!(
        "{} byte(s) differ in {}{} range(s)",
        detail.bytes.changed_bytes,
        detail.bytes.ranges.len(),
        if detail.bytes.truncated { "+" } else { "" }
    )];
    match detail.semantic {
        Some(SemanticDiff::Strings(s)) => {
            for c in &s.changed {
                lines.push(format!(
                    "[{}] {}: {:?} -> {:?}",
                    c.language_name,
                    hex(c.hash),
                    c.old,
                    c.new
                ));
            }
            lines.extend(s.added.iter().map(|h| format!("added string {}", hex(*h))));
            lines.extend(
                s.removed
                    .iter()
                    .map(|h| format!("removed string {}", hex(*h))),
            );
        }
        Some(SemanticDiff::Texture(t)) => {
            lines.push(format!("texture {}: {} -> {}", t.name, t.old, t.new));
        }
        Some(SemanticDiff::Logic(l)) => {
            lines.extend(
                l.values
                    .iter()
                    .map(|c| format!("{}: {} -> {}", c.place, c.old, c.new)),
            );
            if l.truncated {
                lines.push("(more values changed)".into());
            }
            lines.extend(l.other.iter().cloned());
        }
        Some(SemanticDiff::Apx(a)) => {
            let size = |s: Option<usize>| s.map_or_else(|| "none".to_owned(), |s| s.to_string());
            for b in &a.blocks {
                lines.push(format!(
                    "block {} #{}: {} -> {} byte(s)",
                    b.tag,
                    b.ordinal,
                    size(b.old_size),
                    size(b.new_size)
                ));
            }
            for t in &a.textures {
                lines.push(format!(
                    "embedded texture {}: {} -> {}",
                    t.name, t.old, t.new
                ));
            }
        }
        None => {}
    }
    if lines.len() > DETAIL_LINES {
        let more = lines.len() - DETAIL_LINES;
        lines.truncate(DETAIL_LINES);
        lines.push(format!("… {more} more line(s)"));
    }
    Ok(lines)
}

pub fn workspace_diff(kit: &Toolkit, p: WorkspaceDiffParams) -> ToolResult<Output> {
    let Some(ws) = kit.workspace()? else {
        return Ok(no_workspace(kit));
    };
    let filter = p.filter.as_deref().map(str::to_ascii_lowercase);
    let keep = |key: String| {
        filter
            .as_deref()
            .is_none_or(|f| key.to_ascii_lowercase().contains(f))
    };
    let entries: Vec<_> = ws
        .diff()
        .into_iter()
        .filter(|e| keep(e.key.to_string()))
        .collect();
    let total = entries.len();
    let offset = p.offset.unwrap_or(0);
    let limit = clamp(p.limit, 20, 100);
    let mut items = Vec::new();
    for e in entries.iter().skip(offset).take(limit) {
        items.push(json!({
            "key": e.key.to_string(),
            "label": e.label,
            "added": e.added,
            "original_bytes": e.original_size,
            "new_bytes": e.new_size,
            "detail": detail_lines(&ws, &e.key)
                .unwrap_or_else(|err| vec![format!("cannot compare: {}", err.0)]),
        }));
    }
    let files: Vec<Value> = ws
        .file_changes()
        .filter(|c| keep(c.key.to_string()))
        .take(limit)
        .map(|c| {
            json!({
                "file": c.key.to_string(),
                "label": c.label,
                "original_bytes": c.original.size,
                "new_bytes": c.payload.size,
            })
        })
        .collect();
    Ok(Output::Json(json!({
        "workspace": ws.dir(),
        "total": total,
        "offset": offset,
        "more": offset + items.len() < total,
        "changes": items,
        "files": files,
    })))
}

pub fn project_status(kit: &Toolkit, p: ProjectStatusParams) -> ToolResult<Output> {
    let Some(dir) = &kit.project_dir else {
        return fail("the server was started without --project; there is no project to show");
    };
    let project = Project::open(dir)
        .map_err(|e| ToolError(format!("{} is not a project: {e}", dir.display())))?;
    let ws = match kit.workspace()? {
        Some(ws) => ws,
        // An empty workspace in memory: nothing is created on disk.
        None => Workspace::for_game(&kit.game, Some(&kit.workspace_dir))?,
    };
    let status = project.status_read_only(&ws)?;
    // Edits made on other data than the base gives now (stage 111).
    let stale = project::stale_warning(&project.stale(&kit.game, &ws)?);
    let all = p.all.unwrap_or(false);
    let offset = p.offset.unwrap_or(0);
    let limit = clamp(p.limit, 100, 500);
    let listed: Vec<Value> = status
        .entries
        .iter()
        .filter(|e| all || e.state != EntryState::Unchanged)
        .skip(offset)
        .take(limit)
        .map(|e| {
            json!({
                "entry": e.id,
                "state": e.state.label(),
                "kind": e.kind.label(),
                "type": e.type_tag,
                "name": e.name,
                "patch": e.patch,
                "missing": match &e.state {
                    EntryState::Missing(file) => Some(file.as_str()),
                    _ => None,
                },
            })
        })
        .collect();
    let c = &project.config.project;
    Ok(Output::Json(json!({
        "project": dir,
        "name": c.name,
        "id": c.id,
        "version": c.version,
        "counts": {
            "entries": status.entries.len(),
            "unchanged": status.count(&EntryState::Unchanged),
            "built": status.count(&EntryState::Built),
            "modified": status.count(&EntryState::Modified),
            "out_of_sync": status.count(&EntryState::OutOfSync),
            "removed": status.count(&EntryState::Removed),
            "to_build": status.to_build(),
        },
        "offset": offset,
        "entries": listed,
        // Streams the project creates, and those whose folder is gone.
        "new_streams": status.new_streams.iter().map(|s| json!({
            "stream": s.stream,
            "folder": s.dir,
            "built": s.built,
        })).collect::<Vec<_>>(),
        "removed_streams": status.removed_streams.iter().map(|(_, s)| s).collect::<Vec<_>>(),
        "unknown_files": status.unknown_files.iter().take(limit).collect::<Vec<_>>(),
        "warnings": status.warnings.iter().chain(project.game_warnings(&kit.game).iter()).chain(stale.iter()).collect::<Vec<_>>(),
    })))
}

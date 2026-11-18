//! Resources as files, projects and the state of the workspace (stage
//! 104): unpack streams into a folder and pack the edited files back, make
//! a project and pull the workspace into it, take single changes out of the
//! workspace, put the original game files back, and the cells of the world
//! grid of a level. Files are written into the output folder of the server
//! (`--output-dir`) only.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use doctor_toolkit::level::world_grid;
use doctor_toolkit::navigation::unpack::{self, PackOptions, UnpackOptions};
use doctor_toolkit::project::{self, NewProject, Project};
use doctor_toolkit::resource::names::type_tags;
use doctor_toolkit::workspace::{RebaseOutcome, RebaseRequest, Stale, Workspace};

use super::edit::finish;
use super::{Output, clamp};
use crate::state::{ToolError, ToolResult, Toolkit, fail};

// ------------------------------------------------------------ parameters

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct UnpackParams {
    /// Streams to unpack, e.g. `gf2/frontend` (at most 40 per call).
    pub streams: Vec<String>,
    /// Only resources of these types: tags such as `tg4h`, `lh2`, `mesh`
    /// (a texture needs `tg4h` or `tg4d`).
    #[serde(default)]
    pub types: Vec<String>,
    /// Folder name inside the output folder [default: `unpacked`]. An
    /// existing unpacked folder is kept and updated.
    #[serde(default)]
    pub name: Option<String>,
    /// Textures as DDS instead of PNG.
    #[serde(default)]
    pub dds: Option<bool>,
    /// Sounds of banks as WAV instead of `.snu`.
    #[serde(default)]
    pub wav: Option<bool>,
    /// No raw `.bin` next to views (models, game logic, rigs, sound banks).
    #[serde(default)]
    pub no_raw: Option<bool>,
    /// No views: raw data, textures and strings only.
    #[serde(default)]
    pub no_views: Option<bool>,
    /// Models without materials, textures and skeletons.
    #[serde(default)]
    pub geometry_only: Option<bool>,
    /// Model file format: glb (default), fbx, obj, usd, usda, usdc or usdz.
    #[serde(default)]
    pub model_format: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct PackParams {
    /// The folder `unpack` wrote: its name inside the output folder, or an
    /// absolute path.
    pub folder: String,
    /// Only list the changed files.
    #[serde(default)]
    pub dry_run: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ProjectNewParams {
    /// Folder of the new project inside the output folder (new or empty).
    pub folder: String,
    #[serde(flatten)]
    pub project: NewProject,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ProjectPullParams {
    /// Only entries whose path starts with one of these [default: every
    /// entry that is out of sync with the workspace].
    #[serde(default)]
    pub only: Vec<String>,
    /// Write sources for everything the project's own workspace holds: also
    /// new entries for the changes that have none yet (resources replaced
    /// and added, the streams the workspace creates), models included, and
    /// nothing left of what the workspace no longer adds (as the command
    /// `project sources`). Not with `only`.
    #[serde(default)]
    pub sources: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct RevertParams {
    /// Changes to take out of the workspace: whole keys (as
    /// `workspace_status` lists them) or text that keys contain.
    #[serde(default)]
    pub keys: Vec<String>,
    /// Take every change out.
    #[serde(default)]
    pub all: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct RebaseParams {
    /// Only list the changes made on other data than the game gives now;
    /// nothing is recorded.
    #[serde(default)]
    pub check: Option<bool>,
    /// Rebase only these changes: whole keys (as `workspace_status` lists
    /// them) or text that keys contain. Without it, every stale change
    /// that is in neither `keep` nor `take_base`.
    #[serde(default)]
    pub only: Vec<String>,
    /// "Keep mine": these changes stay as they are and count as made on
    /// what the game gives now.
    #[serde(default)]
    pub keep: Vec<String>,
    /// "Take the base's": these changes are removed; the resource is what
    /// the game gives.
    #[serde(default)]
    pub take_base: Vec<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct RestoreOriginalsParams {}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ZoneCellsParams {
    /// Root stream of the level, e.g. `gf2/queens`.
    pub level: String,
    /// List the cells of every grid (stream, kind, level of detail, centre,
    /// radius, box), not only the summary of each grid.
    #[serde(default)]
    pub cells: Option<bool>,
    /// With `cells`: only cells whose stream name contains this text.
    #[serde(default)]
    pub filter: Option<String>,
    /// Read the game files only, without the edits of the workspace.
    #[serde(default)]
    pub original: Option<bool>,
    /// Skip this many cells (paging).
    #[serde(default)]
    pub offset: Option<usize>,
    /// At most this many cells (default 100, at most 1000).
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct RemoveZoneCellParams {
    /// Root stream of the level, e.g. `gf2/queens`.
    pub level: String,
    /// The stream whose cell goes: its name, or its table-of-contents hash
    /// as eight hexadecimal digits.
    pub stream: String,
}

// ------------------------------------------------------------ helpers

/// A folder inside the output folder of the server.
fn output_folder(kit: &Toolkit, name: &str) -> ToolResult<PathBuf> {
    let probe = kit.output_path(&format!("{}/probe", name.trim().trim_end_matches('/')))?;
    Ok(probe
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or(probe))
}

/// The workspace for reading: the saved one, or an empty one in memory
/// (nothing is written before an edit is recorded).
fn read_workspace(kit: &Toolkit, original: bool) -> ToolResult<Workspace> {
    match kit.workspace_unless(original)? {
        Some(ws) => Ok(ws),
        None => Ok(Workspace::for_game(&kit.game, Some(&kit.workspace_dir))?),
    }
}

// ------------------------------------------------------------ unpack, pack

pub fn unpack(kit: &Toolkit, p: UnpackParams) -> ToolResult<Output> {
    if p.streams.is_empty() || p.streams.len() > 40 {
        return fail(
            "give 1 to 40 streams to unpack (`list_streams` lists them); the whole game is \
             tens of gigabytes — the command line unpacks it",
        );
    }
    let out = output_folder(kit, p.name.as_deref().unwrap_or("unpacked"))?;
    let types: Vec<u32> = p
        .types
        .iter()
        .map(|t| {
            type_tags()
                .resolve(t)
                .ok_or_else(|| ToolError(format!("unknown type `{t}`")))
        })
        .collect::<ToolResult<_>>()?;
    for stream in &p.streams {
        kit.stream(stream)?;
    }
    let model_format = match p.model_format.as_deref() {
        Some(f) => f
            .parse()
            .map_err(|e| ToolError(format!("`model_format`: {e}")))?,
        None => UnpackOptions::default().model_format,
    };
    let no_views = p.no_views.unwrap_or(false);
    let options = UnpackOptions {
        dds: p.dds.unwrap_or(false),
        wav: p.wav.unwrap_or(false),
        raw: !p.no_raw.unwrap_or(false),
        views: !no_views,
        model_assets: !p.geometry_only.unwrap_or(false),
        model_format,
    };
    let selection = unpack::select(&kit.game, &p.streams, &types)?;
    let report = unpack::unpack(
        &kit.game,
        Some(kit.index()),
        &selection,
        &out,
        options,
        &|_, _| {},
        &AtomicBool::new(false),
    )?;
    Ok(Output::Json(json!({
        "folder": out,
        "manifest": out.join(unpack::MANIFEST),
        "resources": report.resources,
        "files": report.files,
        "bytes": report.bytes,
        "not_converted": report.failed.iter().take(40).collect::<Vec<_>>(),
        "not_converted_total": report.failed.len(),
        "layout": "<folder>/<stream>/<type>/<name>.<ext>",
        "next": "edit the files, then `pack` records the changed ones in the workspace \
                 (views for reading only are marked in the manifest)",
    })))
}

pub fn pack(kit: &Toolkit, p: PackParams) -> ToolResult<Output> {
    let given = PathBuf::from(p.folder.trim());
    let dir = if given.is_absolute() {
        given
    } else {
        output_folder(kit, &p.folder)?
    };
    if !dir.join(unpack::MANIFEST).is_file() {
        return fail(format!(
            "{} has no {}: it is not a folder `unpack` wrote",
            dir.display(),
            unpack::MANIFEST
        ));
    }
    let dry_run = p.dry_run.unwrap_or(false);
    let listed = |report: &unpack::PackReport| -> Vec<Value> {
        report
            .changed
            .iter()
            .map(|f| {
                json!({
                    "file": f.path,
                    "resources": f.keys.iter().map(ToString::to_string).collect::<Vec<_>>(),
                    "recorded": f.recorded,
                })
            })
            .collect()
    };
    if dry_run {
        let report = unpack::pack(
            &kit.game,
            &dir,
            &PackOptions {
                workspace_dir: Some(kit.workspace_dir.clone()),
                dry_run: true,
            },
        )?;
        return Ok(Output::Json(json!({
            "dry_run": true,
            "would_record": listed(&report),
            "unchanged": report.unchanged,
            "warnings": report.warnings,
        })));
    }
    let mut access = kit.write_workspace(&format!("pack: {}", dir.display()))?;
    let since = access.ws.head();
    // `pack` records through a workspace of its own, under the lock held here.
    let report = unpack::pack(
        &kit.game,
        &dir,
        &PackOptions {
            workspace_dir: Some(kit.workspace_dir.clone()),
            dry_run: false,
        },
    )?;
    access.ws = Workspace::for_game(&kit.game, Some(&kit.workspace_dir))?;
    let ws = &mut access.ws;
    let recorded = report.changed.iter().filter(|f| f.recorded).count();
    let label = format!(
        "Pack {}: {recorded} file(s)",
        dir.file_name().map_or_else(
            || dir.display().to_string(),
            |n| n.to_string_lossy().into_owned()
        )
    );
    finish(
        ws,
        since,
        &label,
        json!({
            "files": listed(&report),
            "unchanged": report.unchanged,
            "warnings": report.warnings,
        }),
    )
}

// ------------------------------------------------------------ project

pub fn project_new(kit: &Toolkit, p: ProjectNewParams) -> ToolResult<Output> {
    let root = output_folder(kit, &p.folder)?;
    if std::fs::read_dir(&root).is_ok_and(|mut d| d.next().is_some()) {
        return fail(format!(
            "{} is not empty: a project is made in a new or empty folder",
            root.display()
        ));
    }
    for stream in &p.project.streams {
        kit.stream(stream)?;
    }
    let ws = read_workspace(kit, false)?;
    let custom_workspace = (kit.workspace_dir != Workspace::default_dir(kit.game.root()))
        .then(|| kit.workspace_dir.clone());
    // Neither streams nor the workspace: an empty project with a workspace
    // of its own, which names no file of the game.
    let empty = p.project.streams.is_empty() && !p.project.from_workspace;
    let (made, report) = project::create_from(
        &kit.game,
        &ws,
        (!empty).then(|| kit.index()),
        &root,
        &p.project,
        custom_workspace,
    )?;
    if empty {
        return Ok(Output::Json(json!({
            "project": made.root(),
            "id": made.config.project.id,
            "workspace": made.workspace_dir(),
            "entries": 0,
            "next": "the project is empty and has its own workspace: a server started with \
                     --project <folder> edits that workspace (this server keeps its own); \
                     project_pull with sources writes its edits as files under src/",
        })));
    }
    Ok(Output::Json(json!({
        "project": made.root(),
        "id": made.config.project.id,
        "resources": report.resources,
        "files": report.files,
        "entries": report.entries,
        "shared": report.shared,
        "new_streams": report.new_streams,
        "bytes": report.bytes,
        "notes": report.notes,
        "next": "edit the files under src/; a server started with --project <folder> \
                 has project_status and project_build",
    })))
}

pub fn project_pull(kit: &Toolkit, p: ProjectPullParams) -> ToolResult<Output> {
    let Some(dir) = &kit.project_dir else {
        return fail("the server was started without --project; there is no project to pull into");
    };
    let mut project = Project::open(dir)
        .map_err(|e| ToolError(format!("{} is not a project: {e}", dir.display())))?;
    let ws = read_workspace(kit, false)?;
    if p.sources == Some(true) {
        if !p.only.is_empty() {
            return fail("`sources` writes all of the workspace; leave `only` out");
        }
        let report = project.export_workspace(&kit.game, &ws, Some(kit.index()))?;
        return Ok(Output::Json(json!({
            "project": dir,
            "added": report.added,
            "pulled": report.pulled,
            "removed": report.removed,
            "streams": report.streams,
            "files": report.files,
            "bytes": report.bytes,
            "notes": report.notes,
        })));
    }
    let pulled = project.pull(&kit.game, &ws, &p.only)?;
    Ok(Output::Json(json!({
        "project": dir,
        "pulled": pulled,
        "note": if pulled.is_empty() {
            "no entry is out of sync with the workspace"
        } else {
            "the files of these entries now hold the workspace's version"
        },
    })))
}

// ------------------------------------------------------------ workspace

pub fn revert(kit: &Toolkit, p: RevertParams) -> ToolResult<Output> {
    let all = p.all.unwrap_or(false);
    if all == !p.keys.is_empty() {
        return fail("give `keys` (changes to take out) or `all`, not both");
    }
    if !Workspace::exists(&kit.workspace_dir) {
        return fail("there is no workspace: nothing to revert");
    }
    let mut access = kit.write_workspace("revert: take changes out of the workspace")?;
    let ws = &mut access.ws;
    let since = ws.head();
    if all {
        let before = ws.len();
        ws.revert_all()?;
        let label = super::create::last_label(ws);
        return finish(ws, since, &label, json!({ "reverted_changes": before }));
    }
    let done = ws.revert_matching(&p.keys)?;
    finish(
        ws,
        since,
        done.operation.as_deref().unwrap_or_default(),
        json!({
            "reverted": done.resources.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "reverted_files": done.files.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "note": "an applied change stays in the game files until `apply` or `unapply`",
        }),
    )
}

/// The project whose changes the workspace of the server holds: the one
/// the server was started with, or the one the folder of the workspace
/// belongs to. Its own mod, written into the game, is not what its edits
/// stand on (`Project::given`).
fn workspace_project(kit: &Toolkit) -> ToolResult<Option<Project>> {
    let dir = match &kit.project_dir {
        Some(dir) => Some(dir.clone()),
        None => project::project_of_workspace(&kit.workspace_dir),
    };
    dir.map(|dir| {
        Project::open(&dir)
            .map_err(|e| ToolError(format!("{} is not a project: {e}", dir.display())))
    })
    .transpose()
}

fn stale_json(stale: &[Stale]) -> Vec<Value> {
    stale
        .iter()
        .map(|s| {
            json!({
                "key": s.key.to_string(),
                "merged_by": s.kind.map(|kind| kind.name()),
                "how": match s.kind {
                    Some(_) => "rebased by its operations",
                    None => "not merged: `keep` it or `take_base`",
                },
            })
        })
        .collect()
}

/// `rebase`: the changes made on other data than the game gives now are
/// moved onto that data, as one operation (stage 111).
pub fn rebase(kit: &Toolkit, p: RebaseParams) -> ToolResult<Output> {
    if !Workspace::exists(&kit.workspace_dir) {
        return fail("there is no workspace: nothing to rebase");
    }
    let project = workspace_project(kit)?;
    let stale_of = |ws: &Workspace| match &project {
        Some(project) => project.stale(&kit.game, ws),
        None => ws.stale(&kit.game),
    };
    if p.check.unwrap_or(false) {
        if !(p.only.is_empty() && p.keep.is_empty() && p.take_base.is_empty()) {
            return fail("`check` only lists: give it without `only`, `keep` and `take_base`");
        }
        let Some(ws) = kit.workspace()? else {
            return fail("there is no workspace: nothing to rebase");
        };
        let stale = stale_of(&ws)?;
        return Ok(Output::Json(json!({
            "stale": stale_json(&stale),
            "count": stale.len(),
            "note": if stale.is_empty() {
                "every change is made on what the game gives now"
            } else {
                "a mod built of these changes would take back what the mods below changed \
                 since; `rebase` without `check` moves them"
            },
        })));
    }
    let mut access = kit.write_workspace("rebase: move stale changes onto the game's data")?;
    let ws = &mut access.ws;
    let since = ws.head();
    let stale = stale_of(ws)?;
    let only = match p.only.is_empty() {
        true => None,
        false => Some(ws.matching_keys(&p.only)?),
    };
    let keep = ws.matching_keys(&p.keep)?;
    let take_base = ws.matching_keys(&p.take_base)?;
    let request = RebaseRequest {
        keys: only.as_deref(),
        keep: &keep,
        take_base: &take_base,
    };
    let report = match &project {
        Some(project) => project.rebase(&kit.game, ws, &request)?,
        None => ws.rebase_with(&kit.game, &request)?,
    };
    let changes: Vec<Value> = report
        .changes
        .iter()
        .map(|c| {
            let notes = |list: &[doctor_toolkit::resource::delta::Note]| {
                list.iter().map(ToString::to_string).collect::<Vec<_>>()
            };
            let key = c.key.to_string();
            match &c.outcome {
                RebaseOutcome::Merged {
                    applied,
                    skipped,
                    conflicts,
                    removed,
                } => json!({
                    "key": key,
                    "outcome": "rebased",
                    "applied": applied,
                    "left_out": notes(skipped),
                    "over_another_value": notes(conflicts),
                    "change_removed": removed,
                }),
                RebaseOutcome::Kept => json!({ "key": key, "outcome": "kept" }),
                RebaseOutcome::Taken => json!({ "key": key, "outcome": "taken from the game" }),
                RebaseOutcome::Undecided { reason } => json!({
                    "key": key,
                    "outcome": "needs a decision",
                    "reason": reason,
                }),
                RebaseOutcome::NotStale => json!({ "key": key, "outcome": "not stale" }),
            }
        })
        .collect();
    let undecided = report.undecided().count();
    let label = super::create::last_label(ws);
    finish(
        ws,
        since,
        &label,
        json!({
            "stale": stale_json(&stale),
            "changes": changes,
            "needs_a_decision": undecided,
            "note": match undecided {
                0 => "the changes are made on what the game gives now",
                _ => "a change that needs a decision is as it was: call again with its key \
                      in `keep` (yours stays) or in `take_base` (the game's data)",
            },
        }),
    )
}

pub fn restore_originals(kit: &Toolkit, _: RestoreOriginalsParams) -> ToolResult<Output> {
    super::play::refuse_while_running(kit)?;
    // Under the lock of the workspace: nothing applies while files go back.
    let access = kit.write_workspace("restore_originals: put every original game file back")?;
    let restored = kit.game.restore_backups()?;
    Ok(Output::Json(json!({
        "restored": restored,
        "files": restored.len(),
        "changes_kept_in_workspace": access.ws.len(),
        "note": "every file the toolkit changed is as in its backup; the edits stay in the \
                 workspace, pending (`apply` writes them again)",
    })))
}

// ------------------------------------------------------------ world grid

pub fn zone_cells(kit: &Toolkit, p: ZoneCellsParams) -> ToolResult<Output> {
    let ws = read_workspace(kit, p.original.unwrap_or(false))?;
    let grids = world_grid::level_grids(&kit.game, &ws, &p.level)?;
    if grids.is_empty() {
        return fail(format!("{} has no world grid (`wgi`)", p.level));
    }
    let names = world_grid::stream_names(&kit.game, &ws)?;
    let name_of = |hash: u32| {
        names
            .get(&hash)
            .cloned()
            .unwrap_or_else(|| format!("{hash:08x}"))
    };
    let summary: Vec<Value> = grids
        .iter()
        .map(|g| {
            json!({
                "resource": g.resource,
                "id": format!("{:08x}", g.id),
                "cells": g.grid.cells.len(),
                "level_of_detail": g.grid.main_lod_mask().map(world_grid::lod_name),
                "streams_in_table": g.grid.streams.len(),
            })
        })
        .collect();
    if !p.cells.unwrap_or(false) {
        return Ok(Output::Json(json!({
            "level": p.level,
            "grids": summary,
            "next": "`cells: true` lists the cells; add_zone_cell adds one, remove_zone_cell takes one out",
        })));
    }
    let filter = p.filter.as_deref().map(str::to_lowercase);
    let mut rows = Vec::new();
    for g in &grids {
        for (index, cell) in g.grid.cells.iter().enumerate() {
            let stream = name_of(cell.stream_hash.0);
            if filter
                .as_deref()
                .is_some_and(|f| !stream.to_lowercase().contains(f))
            {
                continue;
            }
            let needs: Vec<String> = g
                .grid
                .list(&cell.needs)
                .iter()
                .map(|&s| name_of(g.grid.streams[s as usize].hash()))
                .collect();
            rows.push(json!({
                "grid": g.resource,
                "cell": index,
                "stream": stream,
                "hash": format!("{:08x}", cell.stream_hash.0),
                "kind": cell.kind,
                "flags": cell.flags,
                "level_of_detail": world_grid::lod_name(cell.lod_mask),
                "centre": cell.centre,
                "radius": cell.radius,
                "box": cell.has_box().then(|| json!([&cell.box_min[..3], &cell.box_max[..3]])),
                "name": (cell.name_hash.0 != 0).then(|| cell.name_hash.to_string()),
                "needs": needs,
            }));
        }
    }
    let offset = p.offset.unwrap_or(0);
    let limit = clamp(p.limit, 100, 1000);
    let total = rows.len();
    let page: Vec<Value> = rows.into_iter().skip(offset).take(limit).collect();
    Ok(Output::Json(json!({
        "level": p.level,
        "grids": summary,
        "total": total,
        "offset": offset,
        "cells": page,
    })))
}

pub fn remove_zone_cell(kit: &Toolkit, p: RemoveZoneCellParams) -> ToolResult<Output> {
    let mut access = kit.write_workspace(&format!(
        "remove_zone_cell: {} from the world grid of {}",
        p.stream, p.level
    ))?;
    let ws = &mut access.ws;
    let since = ws.head();
    let hash = world_grid::remove_stream_cell(&kit.game, ws, &p.level, &p.stream)?;
    let label = super::create::last_label(ws);
    finish(
        ws,
        since,
        &label,
        json!({
            "level": p.level,
            "stream": p.stream,
            "hash": format!("{hash:08x}"),
            "note": "the game no longer loads the stream by the position of the player",
        }),
    )
}

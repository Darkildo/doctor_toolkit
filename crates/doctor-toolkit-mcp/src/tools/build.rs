//! Building and checking (`--allow-edits`): the project into the workspace,
//! a mod (format 2) from the workspace, and the checks of the workspace's
//! edits that the toolkit has.

use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use doctor_toolkit::create::{self, Provided, ReferenceChecker, Target};
use doctor_toolkit::mods::{self, ModInfo};
use doctor_toolkit::project::{BuildOptions, EntryState, Project};
use doctor_toolkit::resource::graph::{Graph, TYPE_GRAPH};
use doctor_toolkit::resource::lh2::StringTable;
use doctor_toolkit::resource::metamodel::MetaModel;
use doctor_toolkit::resource::simgroup::SimGroup;
use doctor_toolkit::workspace::{ChangeState, ResourceKey, StreamState, Workspace};

use super::{Output, hex};
use crate::state::{ToolError, ToolResult, Toolkit, fail};

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ProjectBuildParams {
    /// Only entries whose path starts with one of these.
    #[serde(default)]
    pub only: Vec<String>,
    /// Build every entry, changed or not (textures and models are encoded again).
    #[serde(default)]
    pub force: Option<bool>,
    /// Only list what would be built.
    #[serde(default)]
    pub dry_run: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ModBuildParams {
    /// Name shown to players [default: the project's, else required].
    #[serde(default)]
    pub name: Option<String>,
    /// Mod id: `a-z`, `0-9`, `-`, `_` [default: from the name].
    #[serde(default)]
    pub id: Option<String>,
    /// Version [default: the project's, else 1.0].
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Only these changes: resource keys or text matching part of them
    /// [default: the project's changes with a project, else all].
    #[serde(default)]
    pub changes: Vec<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ValidateParams {
    /// Only changes whose key contains this text.
    #[serde(default)]
    pub filter: Option<String>,
}

// ------------------------------------------------------------ project

fn project(kit: &Toolkit) -> ToolResult<Project> {
    let Some(dir) = &kit.project_dir else {
        return fail("the server was started without --project; there is no project to build");
    };
    Project::open(dir).map_err(|e| ToolError(format!("{} is not a project: {e}", dir.display())))
}

pub fn project_build(kit: &Toolkit, p: ProjectBuildParams) -> ToolResult<Output> {
    let project = project(kit)?;
    let mut access = kit.write_workspace("project_build: build the project into the workspace")?;
    let ws = &mut access.ws;
    let since = ws.head();
    let dry_run = p.dry_run.unwrap_or(false);
    let report = project.build(
        &kit.game,
        ws,
        &BuildOptions {
            dry_run,
            force: p.force.unwrap_or(false),
            only: p.only,
        },
    )?;
    let built: Vec<Value> = report
        .built
        .iter()
        .map(|b| {
            json!({
                "entry": b.id,
                "changed": b.changed.iter().map(ToString::to_string).collect::<Vec<_>>(),
                "notes": b.notes,
            })
        })
        .collect();
    let keys: BTreeSet<ResourceKey> = ws.operations()[since..ws.head()]
        .iter()
        .flat_map(|op| op.edits.iter().map(|e| e.key.clone()))
        .collect();
    Ok(Output::Json(json!({
        "dry_run": dry_run,
        "built": built,
        "up_to_date": report.up_to_date,
        "errors": report.errors,
        "warnings": report.warnings,
        "operation": (ws.head() > since).then(|| ws.operations()[ws.head() - 1].label.clone()),
        "undo": ws.undo_label(),
        "changed_resources": keys.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "game_files": "unchanged: the build is recorded in the workspace as one operation",
    })))
}

// ------------------------------------------------------------ mod

/// Keys of the workspace matching `wanted` (exact key or part of one).
fn match_keys(ws: &Workspace, wanted: &[String]) -> ToolResult<Vec<ResourceKey>> {
    let mut out = Vec::new();
    for w in wanted {
        let found: Vec<ResourceKey> = ws
            .changes()
            .map(|c| c.key.clone())
            .filter(|k| {
                let t = k.to_string();
                t == *w || t.contains(w.as_str())
            })
            .collect();
        if found.is_empty() {
            return fail(format!("no change of the workspace matches `{w}`"));
        }
        out.extend(found);
    }
    out.sort();
    out.dedup();
    Ok(out)
}

pub fn mod_build(kit: &Toolkit, p: ModBuildParams) -> ToolResult<Output> {
    let Some(ws) = kit.workspace()? else {
        return fail("there is no workspace: nothing to put in a mod");
    };
    let project = match &kit.project_dir {
        Some(_) => Some(project(kit)?),
        None => None,
    };
    let base = project.as_ref().map(Project::mod_info);
    let name = p
        .name
        .clone()
        .or_else(|| base.as_ref().map(|b| b.name.clone()))
        .ok_or_else(|| ToolError("give the mod a `name`".into()))?;
    let info = ModInfo {
        id: p
            .id
            .clone()
            .or_else(|| {
                base.as_ref()
                    .filter(|_| p.name.is_none())
                    .map(|b| b.id.clone())
            })
            .unwrap_or_else(|| mods::id_from_name(&name)),
        version: p
            .version
            .clone()
            .or_else(|| base.as_ref().map(|b| b.version.clone()))
            .unwrap_or_else(|| "1.0".into()),
        author: p
            .author
            .clone()
            .or_else(|| base.as_ref().map(|b| b.author.clone()))
            .unwrap_or_default(),
        description: p
            .description
            .clone()
            .or_else(|| base.as_ref().map(|b| b.description.clone()))
            .unwrap_or_default(),
        url: base.as_ref().and_then(|b| b.url.clone()),
        name,
    };
    let keys = if !p.changes.is_empty() {
        Some(match_keys(&ws, &p.changes)?)
    } else {
        project.as_ref().map(|pr| pr.changed_keys(&ws))
    };
    if keys.as_ref().is_some_and(Vec::is_empty) {
        return fail("the workspace has no changes of the project; build it first (project_build)");
    }
    // The mod goes next to the workspace, never to a path from the client.
    let dir = ws.dir().join("mods");
    std::fs::create_dir_all(&dir)
        .map_err(|e| ToolError(format!("cannot make {}: {e}", dir.display())))?;
    let file = dir.join(format!("{}-{}.zip", info.id, sanitize(&info.version)));
    let report = mods::build_from_workspace(&ws, &info, keys.as_deref(), None, &file)?;
    Ok(Output::Json(json!({
        "file": file,
        "id": info.id,
        "name": info.name,
        "version": info.version,
        "format": 2,
        "resources": report.resources,
        "added_resources": report.added,
        "new_streams": report.new_streams,
        "payload_bytes": report.payload_bytes,
        "file_bytes": report.file_size,
        "note": "players install it with the mod manager (doctor-toolkit-mods or `doctor-toolkit mods install`)",
    })))
}

fn sanitize(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

// ------------------------------------------------------------ validate

fn target_text(t: &Target) -> String {
    match t {
        Target::Guid(g) => format!("resource {}", g.map(hex).join(".")),
        Target::Material(h) => format!("material {}", hex(*h)),
        Target::Texture(h) => format!("texture {}", hex(*h)),
    }
}

/// What the data of a changed resource says about itself: it is read by
/// its parser, and the parsers that know problems tell them.
fn check_data(
    kit: &Toolkit,
    ws: &Workspace,
    key: &ResourceKey,
    data: &[u8],
    original: Option<&[u8]>,
) -> Vec<String> {
    use doctor_toolkit::create::{TYPE_METAMODEL, TYPE_SIMGROUP};
    use doctor_toolkit::text::TYPE_LH2;
    let tag = doctor_toolkit::resource::names::type_tags().display(key.type_id);
    let mut problems = Vec::new();
    let unreadable = |e: &dyn std::fmt::Display| format!("{tag} cannot be read: {e}");
    match key.type_id {
        TYPE_SIMGROUP => match SimGroup::parse(data) {
            Ok(after) => {
                let before = original.and_then(|o| SimGroup::parse(o).ok());
                match doctor_toolkit::level::compat::check_edit(
                    &kit.game,
                    kit.resources(),
                    Some(ws),
                    before.as_ref(),
                    &after,
                ) {
                    Ok(list) => problems.extend(list),
                    Err(e) => problems.push(format!("models cannot be checked: {e}")),
                }
            }
            Err(e) => problems.push(unreadable(&e)),
        },
        TYPE_LH2 => match StringTable::parse(data) {
            Ok(t) => {
                if let Some(o) = original.and_then(|o| StringTable::parse(o).ok())
                    && o.strings.len() != t.strings.len()
                {
                    problems.push(format!(
                        "{} language column(s) instead of {}",
                        t.strings.len(),
                        o.strings.len()
                    ));
                }
            }
            Err(e) => problems.push(unreadable(&e)),
        },
        TYPE_GRAPH => match Graph::parse(data) {
            Ok(g) => problems.extend(g.problems()),
            Err(e) => problems.push(unreadable(&e)),
        },
        TYPE_METAMODEL => {
            if let Err(e) = MetaModel::parse(data) {
                problems.push(unreadable(&e));
            }
        }
        t if t == doctor_toolkit::hash::sdbm(b"lua") => {
            if let Err(e) = doctor_toolkit::resource::lua::Chunk::parse(data) {
                problems.push(unreadable(&e));
            }
        }
        t if t == doctor_toolkit::hash::sdbm(b"dlg") => {
            if let Err(e) = doctor_toolkit::resource::dlg::Dialogue::parse(data) {
                problems.push(unreadable(&e));
            }
        }
        _ => {}
    }
    problems
}

pub fn validate(kit: &Toolkit, p: ValidateParams) -> ToolResult<Output> {
    let Some(ws) = kit.workspace()? else {
        return Ok(Output::Json(json!({
            "ok": true,
            "note": "there is no workspace: nothing to check",
        })));
    };
    let status = ws.status(&kit.game)?;
    let mut errors: Vec<Value> = Vec::new();
    let mut warnings: Vec<Value> = Vec::new();
    if !status.fingerprint_matches {
        warnings.push(json!({
            "what": "the workspace was made for another build of the game",
        }));
    }
    for (key, state) in &status.changes {
        if *state == ChangeState::Conflict {
            errors.push(json!({
                "key": key.to_string(),
                "what": "conflict: the game file changed since the workspace was applied",
            }));
        }
    }
    for s in &status.streams {
        if s.state == StreamState::ChangedExternally {
            warnings.push(json!({
                "stream": s.stream,
                "what": "changed outside the workspace: `apply` would need force and lose that change",
            }));
        }
        if s.state == StreamState::Missing {
            errors.push(json!({ "stream": s.stream, "what": "missing from the game" }));
        }
    }
    let filter = p.filter.as_deref().map(str::to_ascii_lowercase);
    // What the workspace adds to each stream also resolves references.
    let mut added: std::collections::HashMap<String, Provided> = Default::default();
    for c in ws.changes().filter(|c| c.is_added()) {
        let data = ws.data(&c.key).ok().flatten();
        let header = c
            .added
            .as_ref()
            .expect("added")
            .header(&c.key, c.payload.size);
        added
            .entry(c.key.stream.clone())
            .or_default()
            .add(&header, data.as_deref());
    }
    let mut checker = ReferenceChecker::new(&kit.game)?;
    let parents = kit.game.stream_parents().unwrap_or_default();
    let mut checked = 0usize;
    for change in ws.changes() {
        let key = &change.key;
        if filter
            .as_deref()
            .is_some_and(|f| !key.to_string().to_ascii_lowercase().contains(f))
        {
            continue;
        }
        checked += 1;
        let data = match ws.data(key) {
            Ok(Some(d)) => d,
            Ok(None) => continue,
            Err(e) => {
                errors.push(
                    json!({ "key": key.to_string(), "what": format!("payload unreadable: {e}") }),
                );
                continue;
            }
        };
        let original = ws.original_data(key).ok().flatten();
        for problem in check_data(kit, &ws, key, &data, original.as_deref()) {
            errors.push(json!({ "key": key.to_string(), "what": problem }));
        }
        // References resolve in the stream or a stream loaded before it.
        let Some(stream) = kit.game.find_stream(&key.stream) else {
            continue; // a new stream: its references are its author's business
        };
        let Ok(targets) = create::references(key.type_id, &data) else {
            continue;
        };
        // Only what the edit adds: the game's own SimGroups name resources
        // of streams loaded apart from theirs, and that works in the game.
        let had: Vec<Target> = original
            .as_deref()
            .filter(|o| !o.is_empty())
            .and_then(|o| create::references(key.type_id, o).ok())
            .unwrap_or_default();
        let mut scope = vec![key.stream.clone()];
        scope.extend(doctor_toolkit::game::ancestors(&parents, &key.stream));
        for t in targets.into_iter().filter(|t| !had.contains(t)) {
            let by_workspace = scope
                .iter()
                .any(|s| added.get(s).is_some_and(|p| p.provides(&t)));
            if by_workspace || checker.provides(stream, &t).unwrap_or(true) {
                continue;
            }
            errors.push(json!({
                "key": key.to_string(),
                "what": format!("unresolved reference: {} is in no stream loaded with it", target_text(&t)),
            }));
        }
    }
    let mut project_notes = Vec::new();
    if let Some(dir) = &kit.project_dir
        && let Ok(project) = Project::open(dir)
    {
        let pstatus = project.status_read_only(&ws)?;
        let to_build = pstatus.to_build();
        if to_build > 0 {
            warnings.push(json!({
                "project": dir,
                "what": format!("{to_build} entr(y/ies) changed since the last build: project_build records them"),
            }));
        }
        for e in &pstatus.entries {
            if let EntryState::Missing(file) = &e.state {
                errors
                    .push(json!({ "project_entry": e.id, "what": format!("missing file {file}") }));
            }
        }
        project_notes.extend(pstatus.warnings.iter().cloned());
        project_notes.extend(project.game_warnings(&kit.game));
    }
    for w in project_notes {
        warnings.push(json!({ "project": true, "what": w }));
    }
    let running = doctor_toolkit::play::game_processes();
    Ok(Output::Json(json!({
        "ok": errors.is_empty(),
        "checked_changes": checked,
        "errors": errors,
        "warnings": warnings,
        "pending": status.count(ChangeState::Pending),
        "applied": status.count(ChangeState::Applied),
        "game_running": !running.is_empty(),
        "checks": [
            "every changed resource is read by its parser (SimGroup, string table, navigation graph, metamodel, Lua, dialogue)",
            "SimGroup objects can show their models (the game crashes otherwise)",
            "navigation graphs: what the file cannot hold or the game cannot use",
            "references an edit adds resolve in the stream or the streams loaded before it",
            "conflicts with the game files, streams changed outside the workspace",
            "the project: entries to build, missing files",
        ],
    })))
}

//! Building and checking (`--allow-edits`): the project into the workspace,
//! a mod (format 3, `docs/mods.md`) from the workspace, and the checks of
//! the workspace's edits that the toolkit has.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use doctor_toolkit::mods::{self, Incompatible, ModInfo, Relations};
use doctor_toolkit::project::{BuildOptions, Project};
use doctor_toolkit::workspace::{ResourceKey, Workspace};

use super::Output;
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
    /// Home page of the mod, `http://` or `https://` [default: the project's].
    #[serde(default)]
    pub url: Option<String>,
    /// Absolute path of a PNG or JPEG picture the mod manager shows on the
    /// mod's card (at most 4 MiB).
    #[serde(default)]
    pub preview: Option<String>,
    /// Only these changes: resource keys or text matching part of them
    /// [default: the project's changes with a project, else all].
    #[serde(default)]
    pub changes: Vec<String>,
    /// Mods this one needs: id → condition on the version (`>=1.1 <2`,
    /// `^1.2`, `*`). Mods written into the game whose data the changes were
    /// made on are added by themselves.
    #[serde(default)]
    pub depends: BTreeMap<String, String>,
    /// Mods that go well with this one: id → condition (a hint).
    #[serde(default)]
    pub recommends: BTreeMap<String, String>,
    /// Mods this one does not work with: id → condition.
    #[serde(default)]
    pub incompatible: BTreeMap<String, String>,
    /// Ids of mods to load this one after / before when installed (soft rules).
    #[serde(default)]
    pub load_after: Vec<String>,
    #[serde(default)]
    pub load_before: Vec<String>,
    /// Editions of the game the mod is tested on, e.g. `1.0.789.0`; on
    /// another edition it does not fit unless compatibility is ignored
    /// [default: none, any edition: the resources are checked one by one].
    #[serde(default)]
    pub editions: Vec<String>,
    /// Condition on the version of the mod manager; empty for none
    /// [default: this program's version or newer].
    #[serde(default)]
    pub requires_manager: Option<String>,
    /// Executable code of the mod (format version 5): 32-bit DLLs for the
    /// `scripts` folder of the game, each an absolute path or
    /// `name.asi=<absolute path>`. Building it into the mod is not consent
    /// to run it: the mod manager installs code only after the player
    /// agrees, in its window or with `doctor-toolkit mods native`; no tool
    /// of this server can give that answer.
    #[serde(default)]
    pub native: Vec<String>,
    /// With `native`: the mod makes no sense without its code — it is not
    /// written into the game at all while the code is not installed.
    #[serde(default)]
    pub native_required: Option<bool>,
    /// With `native`: plain ASI plug-ins that do not use doctor_platform.asi.
    #[serde(default)]
    pub native_plain: Option<bool>,
    /// `false` keeps every resource whole. By default a changed SimGroup,
    /// string table, parts database or world grid goes into the mod as a
    /// delta — the operations that make the change — which the mod manager
    /// merges with what other mods do to the same resource; such a mod is
    /// of format version 6, which older mod managers refuse.
    pub deltas: Option<bool>,
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

/// A mod about to be made from the workspace: what the fields of
/// [`ModBuildParams`] and the project say.
pub(crate) struct PlannedMod {
    pub info: ModInfo,
    pub preview: Option<mods::Preview>,
    /// The changes that go into it; `None` for all.
    pub keys: Option<Vec<ResourceKey>>,
    /// Dependencies added because the changes were made on those mods.
    pub suggested: Vec<String>,
    pub warnings: Vec<String>,
}

/// The executable code named by the parameters, read from the disk.
fn native_build(p: &ModBuildParams) -> ToolResult<(Option<mods::NativeBuild>, Vec<String>)> {
    if p.native.is_empty() {
        if p.native_required.is_some() || p.native_plain.is_some() {
            return fail("`native_required` and `native_plain` need `native`: the modules");
        }
        return Ok((None, Vec::new()));
    }
    // Only absolute paths of files, as every input of the server.
    let mut modules = Vec::new();
    for spec in &p.native {
        let (name, path) = mods::native::module_spec(spec);
        let path = super::create::input_file(&path.to_string_lossy())?;
        modules.push(format!("{name}={}", path.display()));
    }
    let spec = mods::native::NativeSpec {
        modules,
        required: p.native_required.unwrap_or(false),
        plain: p.native_plain.unwrap_or(false),
        platform_abi: None,
    };
    Ok(spec.read()?)
}

pub(crate) fn plan_mod(
    kit: &Toolkit,
    ws: &Workspace,
    p: &ModBuildParams,
) -> ToolResult<PlannedMod> {
    let project = match &kit.project_dir {
        Some(_) => Some(project(kit)?),
        None => None,
    };
    let base = project.as_ref().map(Project::mod_info);
    let (native, warnings) = native_build(p)?;
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
        url: p
            .url
            .clone()
            .or_else(|| base.as_ref().and_then(|b| b.url.clone())),
        name,
        relations: Relations {
            native,
            depends: p.depends.clone(),
            recommends: p.recommends.clone(),
            incompatible: p
                .incompatible
                .iter()
                .map(|(id, versions)| {
                    (
                        id.clone(),
                        Incompatible {
                            versions: versions.clone(),
                            reason: String::new(),
                        },
                    )
                })
                .collect(),
            load_after: p.load_after.clone(),
            load_before: p.load_before.clone(),
            editions: p.editions.clone(),
            requires_manager: p.requires_manager.clone(),
            made_on: BTreeMap::new(),
            made_on_data: BTreeMap::new(),
        },
        // The project's own choice, unless the call says.
        whole_resources: match p.deltas {
            Some(deltas) => !deltas,
            None => base.as_ref().is_some_and(|b| b.whole_resources),
        },
    };
    let mut info = info;
    let preview = match &p.preview {
        Some(path) => {
            let path = super::create::input_file(path)?;
            let data = std::fs::read(&path)
                .map_err(|e| ToolError(format!("cannot read {}: {e}", path.display())))?;
            Some(mods::Preview::from_image(data)?)
        }
        None => None,
    };
    let keys = if !p.changes.is_empty() {
        Some(match_keys(ws, &p.changes)?)
    } else {
        project.as_ref().map(|pr| pr.changed_keys(ws))
    };
    if keys.as_ref().is_some_and(Vec::is_empty) {
        return fail("the workspace has no changes of the project; build it first (project_build)");
    }
    let suggested = mods::add_suggested_dependencies(&mut info, &kit.game, ws, keys.as_deref())?;
    Ok(PlannedMod {
        info,
        preview,
        keys,
        suggested,
        warnings,
    })
}

/// What a mod with executable code tells its author, for the answers of
/// the tools that make one.
pub(crate) fn code_note(modules: usize) -> Value {
    if modules == 0 {
        return Value::Null;
    }
    json!({
        "modules": modules,
        "note": "the mod carries executable code: the mod manager installs it only after the \
                 player agrees, in the window of Open Mod Manager or with `doctor-toolkit mods \
                 native GAME ID --allow`; this server cannot give that answer (`mods` with \
                 action `native` shows the state)",
    })
}

pub fn mod_build(kit: &Toolkit, p: ModBuildParams) -> ToolResult<Output> {
    let Some(ws) = kit.workspace()? else {
        return fail("there is no workspace: nothing to put in a mod");
    };
    let PlannedMod {
        info,
        preview,
        keys,
        suggested,
        warnings,
    } = plan_mod(kit, &ws, &p)?;
    // The mod goes next to the workspace, never to a path from the client.
    let dir = ws.dir().join("mods");
    std::fs::create_dir_all(&dir)
        .map_err(|e| ToolError(format!("cannot make {}: {e}", dir.display())))?;
    let file = dir.join(format!("{}-{}.zip", info.id, sanitize(&info.version)));
    let report = mods::build_from_workspace(&ws, &info, keys.as_deref(), preview.as_ref(), &file)?;
    Ok(Output::Json(json!({
        "file": file,
        "id": info.id,
        "name": info.name,
        "version": info.version,
        "preview": preview.as_ref().map(|p| p.name.clone()),
        "format": report.format_version,
        "depends": info.relations.depends,
        "depends_added": suggested,
        "resources": report.resources,
        "added_resources": report.added,
        "new_streams": report.new_streams,
        "deltas": json!({
            "resources": report.deltas,
            "of": report.delta_kinds,
            "note": "resources stored as operations instead of whole (of those that can be): \
                     the mod manager merges them with other mods",
        }),
        "payload_bytes": report.payload_bytes,
        "file_bytes": report.file_size,
        "code": code_note(report.native_modules),
        "warnings": warnings,
        "notes": report.notes,
        "note": "players install it with the mod manager (Open Mod Manager, `open-mod-manager`, or `doctor-toolkit mods install`)",
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

pub fn validate(kit: &Toolkit, p: ValidateParams) -> ToolResult<Output> {
    let Some(ws) = kit.workspace()? else {
        return Ok(Output::Json(json!({
            "ok": true,
            "note": "there is no workspace: nothing to check",
        })));
    };
    let params = doctor_toolkit::validate::Params {
        filter: p.filter,
        project: kit.project_dir.clone(),
    };
    let report = doctor_toolkit::validate::validate(&kit.game, kit.resources(), &ws, &params)?;
    Ok(Output::Json(serde_json::to_value(report)?))
}

//! The mod manager and the check of the game files (stage 57): `mods` and
//! `integrity` read; `mods_manage` (`--allow-game`) installs, removes,
//! orders and switches mods and writes them into the game or takes them out.
//!
//! Executable code of mods (stage 81): the tools show its state and what a
//! mod would put into the game, and they install mods that carry code, but
//! no tool answers the question about it. The answer is the player's, given
//! in the window of the mod manager or with `doctor-toolkit mods native`;
//! a mod whose code differs from what the player agreed to is not installed
//! through the server at all ([`refuse_changed_code`]).

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use doctor_toolkit::mods::integrity::{self, Condition, IntegrityOptions};
use doctor_toolkit::mods::manager::{InstallReport, ModApplyOptions, ModManager};
use doctor_toolkit::mods::native::{self, Consent};
use doctor_toolkit::mods::vpat::{self, VpatPatch};
use doctor_toolkit::mods::{ModArchive, ModManifest};
use doctor_toolkit::workspace::{ApplyReport, StreamAction, StreamState};

use super::Output;
use super::create::input_file;
use crate::state::{ToolError, ToolResult, Toolkit, fail};

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ModsParams {
    /// `list` (default: installed mods in load order, with what each needs,
    /// recommends and does not work with, the state of its executable code
    /// and the saved sets of mods), `status` (what the game has of them),
    /// `conflicts` (resources changed by several mods), `check` (whether
    /// the enabled mods fit this game), `info` (what the mod file `file`
    /// holds, checked, before it is installed), `native` (executable code
    /// of the installed mods: state, files in the game; with `id` what
    /// that mod would put into the game and the player's answer), `sets`
    /// (the saved sets with their mods).
    #[serde(default)]
    pub action: Option<String>,
    /// With `info`: the mod file (an absolute path: `.zip`, or an old
    /// `.vpat`); read only by a server with `--allow-edits` or `--allow-game`.
    #[serde(default)]
    pub file: Option<String>,
    /// With `info`: list every resource (at most 500).
    #[serde(default)]
    pub resources: Option<bool>,
    /// With `native`: one installed mod.
    #[serde(default)]
    pub id: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ModsManageParams {
    /// `install` (mod files `files`), `uninstall`, `enable`, `disable`
    /// (`ids`), `move` (`ids` with one id, `position` from 1), `sort` (the
    /// load order by the mods' rules: dependencies first, then
    /// load_after/load_before), `apply` (write the enabled mods into the
    /// game, originals backed up) or `unapply` (put the originals back);
    /// `install_workspace` (the workspace's changes as a mod, last in the
    /// load order: `mod` names it), `export` (the installed mod `ids[0]` as
    /// a mod file in the output folder), `import_old` (take over a game
    /// patched by the old toolkit: its backups become the originals, its
    /// patches mods), `save_set`, `use_set`, `delete_set` (the set `set`:
    /// the load order and which mods are enabled).
    pub action: String,
    /// With `install_workspace`: the mod, as mod_build takes it (`name`,
    /// `id`, `version`, `author`, `description`, relations, `native`).
    #[serde(default, rename = "mod")]
    pub info: Option<super::build::ModBuildParams>,
    /// With `save_set`, `use_set`, `delete_set`: the name of the set.
    #[serde(default)]
    pub set: Option<String>,
    /// Mod files to install (absolute paths: `.zip` mods of mod_build, old `.vpat` patches).
    #[serde(default)]
    pub files: Vec<String>,
    /// Mod ids for uninstall, enable, disable, move.
    #[serde(default)]
    pub ids: Vec<String>,
    /// For `move`: the new place in the load order, 1 first (later mods win).
    #[serde(default)]
    pub position: Option<usize>,
    /// With `install`: install disabled.
    #[serde(default)]
    pub disabled: Option<bool>,
    /// With `apply`/`unapply`: overwrite streams another program changed
    /// (their changes are lost). With `disable`/`uninstall`: also when
    /// enabled mods need the mod.
    #[serde(default)]
    pub force: Option<bool>,
    /// With `enable`: also enable the installed mods it needs.
    #[serde(default)]
    pub with_deps: Option<bool>,
    /// With `sort`: only tell the sorted order.
    #[serde(default)]
    pub dry_run: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct IntegrityParams {
    /// Also hash the files without a backup and compare them with the
    /// reference list (reads the whole game: minutes).
    #[serde(default)]
    pub full: Option<bool>,
}

fn manager(kit: &Toolkit) -> ToolResult<ModManager> {
    ModManager::open(&kit.game).map_err(|e| ToolError(format!("cannot open the mod manager: {e}")))
}

/// Where the player answers the question about executable code.
const WHERE_TO_AGREE: &str = "the player answers in the window of Open Mod Manager, or with \
     `doctor-toolkit mods native GAME ID --allow` (`--data-only` for the mod without its code); \
     this server cannot answer for the player";

/// The state of the code of every installed mod that has code, in words.
fn code_states(mods: &ModManager) -> std::collections::BTreeMap<String, String> {
    let states = native::states(mods, mods.native_host()).unwrap_or_default();
    mods.mods()
        .iter()
        .filter(|m| m.manifest.native.is_some())
        .map(|m| {
            let state = states
                .iter()
                .find(|(id, _)| *id == m.id)
                .map_or_else(|| "the mod is disabled".to_owned(), |(_, s)| s.to_string());
            (m.id.clone(), state)
        })
        .collect()
}

fn consent_word(consent: Consent) -> &'static str {
    match consent {
        Consent::Unasked => "waits for the player's consent",
        Consent::Code => "the player agreed to the code",
        Consent::DataOnly => "the player wants the mod without its code",
    }
}

fn list(mods: &ModManager) -> Value {
    let code = code_states(mods);
    json!(
        mods.mods()
            .iter()
            .enumerate()
            .map(|(i, m)| json!({
                "position": i + 1,
                "id": m.id,
                "name": m.manifest.name,
                "version": m.manifest.version,
                "enabled": m.enabled,
                "source": m.source,
                "problem": m.problem,
                "code": code.get(&m.id),
                "relations": mods.relations(&m.id).iter().map(|r| json!({
                    "kind": format!("{:?}", r.kind),
                    "mod": r.id,
                    "versions": r.versions,
                    "reason": r.reason,
                    "state": format!("{:?}", r.state),
                })).collect::<Vec<_>>(),
            }))
            .collect::<Vec<_>>()
    )
}

fn state(s: StreamState) -> &'static str {
    match s {
        StreamState::Original => "original",
        StreamState::Applied => "written by the mod manager",
        StreamState::ChangedExternally => "changed by another program",
        StreamState::Missing => "missing from the game",
    }
}

/// What a mod file holds (`mods info` of the command line): the manifest of
/// a `.zip` with every payload checked, or the head of an old `.vpat`.
fn file_info(path: &std::path::Path, resources: bool) -> ToolResult<Value> {
    let head = {
        use std::io::Read;
        let mut head = [0u8; 4];
        let mut f = std::fs::File::open(path)
            .map_err(|e| ToolError(format!("cannot read {}: {e}", path.display())))?;
        let n = f.read(&mut head).unwrap_or(0);
        head[..n].to_vec()
    };
    if vpat::is_vpat(&head) {
        let patch = VpatPatch::open(path)?;
        return Ok(json!({
            "kind": "old toolkit patch (.vpat)",
            "format": patch.format_version,
            "name": patch.name,
            "author": patch.author,
            "version": patch.version,
            "streams": patch.files.iter().map(|f| json!({
                "stream": f.stream,
                "resources": f.resources.len(),
            })).collect::<Vec<_>>(),
            "note": "installed as a mod by mods_manage install",
        }));
    }
    let mut archive = ModArchive::open(path)?;
    archive.verify()?;
    let operations: std::collections::BTreeMap<_, _> =
        archive.delta_operations()?.into_iter().collect();
    let m: &ModManifest = archive.manifest();
    let mut out = json!({
        "kind": "mod (.zip)",
        "checked": "every payload matches its hash",
        "format": m.format_version,
        "id": m.id,
        "name": m.name,
        "version": m.version,
        "author": m.author,
        "description": m.description,
        "url": m.url,
        "game": m.game.profile,
        "requires_manager": m.requires_manager,
        "depends": m.depends,
        "recommends": m.recommends,
        "incompatible": m.incompatible.iter().map(|(id, i)| json!({
            "mod": id, "versions": i.versions, "reason": i.reason,
        })).collect::<Vec<_>>(),
        "load_after": m.load_after,
        "load_before": m.load_before,
        "messages": m.messages,
        "resources": m.resources.len(),
        "added_resources": m.added().count(),
        "delta_resources": operations.len(),
        "delta_operations": operations.values().sum::<usize>(),
        "streams": m.streams(),
        "new_streams": m.streams.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        "payload_bytes": m.payload_bytes(),
        "files": m.files.iter().map(|f| json!({
            "file": f.key.to_string(),
            "bytes": f.payload.size,
            "label": f.label,
        })).collect::<Vec<_>>(),
        "warnings": archive.warnings(),
    });
    if let Some(code) = &m.native {
        out["code"] = json!({
            "edition": code.edition,
            "platform_abi": code.platform_abi,
            "required": code.required,
            "modules": code.modules.iter().map(|module| json!({
                "file": module.path(),
                "bytes": module.payload.size,
                "sha256": module.payload.sha256,
            })).collect::<Vec<_>>(),
            "note": format!("EXECUTABLE CODE: installed only after the player agrees; {WHERE_TO_AGREE}"),
        });
    }
    if resources {
        out["resource_list"] = json!(
            m.resources
                .iter()
                .take(500)
                .map(|r| json!({
                    "key": r.key.to_string(),
                    "added": r.added.is_some(),
                    "delta": r.delta.map(|d| json!({
                        "kind": d.kind.name(),
                        "operations": operations.get(&r.key),
                    })),
                    "original_bytes": r.original.size,
                    "bytes": r.payload.size,
                    "label": r.label,
                }))
                .collect::<Vec<_>>()
        );
    }
    Ok(out)
}

/// The executable code of the installed mods: only reads.
fn native_state(mods: &ModManager, id: Option<&str>) -> ToolResult<Value> {
    let host = mods.native_host();
    let Some(id) = id else {
        let states = code_states(mods);
        let with_code: Vec<Value> = mods
            .mods()
            .iter()
            .filter_map(|m| {
                let code = m.manifest.native.as_ref()?;
                Some(json!({
                    "id": m.id,
                    "name": m.manifest.name,
                    "version": m.manifest.version,
                    "state": states.get(&m.id),
                    "answer": native::consent(mods, &m.id).ok().map(consent_word),
                    "required": code.required,
                    "modules": code.modules.iter().map(|module| json!({
                        "file": module.path(),
                        "bytes": module.payload.size,
                        "sha256": module.payload.sha256,
                    })).collect::<Vec<_>>(),
                }))
            })
            .collect();
        return Ok(json!({
            "mods": with_code,
            "in_the_game": native::written(mods)?.into_iter().map(|(path, owner)| json!({
                "file": path, "owner": owner,
            })).collect::<Vec<_>>(),
            "game": host.edition.clone().err(),
            "asi_loader": host.loader,
            "where_to_agree": WHERE_TO_AGREE,
        }));
    };
    let Some(question) = native::question(mods, host, id)? else {
        return fail(format!("mod `{id}` has no executable code"));
    };
    let file =
        |f: &native::QuestionFile| json!({ "file": f.path, "bytes": f.size, "sha256": f.sha256 });
    Ok(json!({
        "id": question.id,
        "name": question.name,
        "version": question.version,
        "author": question.author,
        "required": question.required,
        "answer": consent_word(native::consent(mods, id)?),
        "state": code_states(mods).get(id),
        "would_install": question.modules.iter().map(file).collect::<Vec<_>>(),
        "toolkit_files": question.toolkit.iter().map(file).collect::<Vec<_>>(),
        "obstacle": question.obstacle.as_ref().map(native::NativeBlock::describe),
        "where_to_agree": WHERE_TO_AGREE,
    }))
}

fn sets(mods: &ModManager) -> ToolResult<Value> {
    Ok(json!({
        "current": mods.current_set(),
        "sets": mods.sets()?.iter().map(|s| json!({
            "name": s.name,
            "mods": s.mods.iter().map(|e| json!({
                "id": e.id,
                "enabled": e.enabled,
                "installed": mods.get(&e.id).is_some(),
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    }))
}

pub fn mods(kit: &Toolkit, p: ModsParams) -> ToolResult<Output> {
    let action = p.action.as_deref().unwrap_or("list");
    if action == "info" {
        // A file: the mod manager of the game is not needed. A server that
        // only reads takes no path of a file from the client; the servers
        // that take mod files and files to import read this one too.
        let Some(file) = &p.file else {
            return fail("give `file`: the mod file to look into");
        };
        if !kit.allow_edits && !kit.allow_game {
            return fail(
                "a read-only server opens no file named by the client: `info` of a mod file \
                 needs --allow-edits or --allow-game",
            );
        }
        let path = input_file(file)?;
        return Ok(Output::Json(file_info(
            &path,
            p.resources.unwrap_or(false),
        )?));
    }
    let mods = manager(kit)?;
    Ok(Output::Json(match action {
        "native" => native_state(&mods, p.id.as_deref())?,
        "sets" => sets(&mods)?,
        "list" => json!({
            "mods": list(&mods),
            "sets": mods.sets()?.iter().map(|s| s.name.clone()).collect::<Vec<_>>(),
            "current_set": mods.current_set(),
        }),
        "status" => {
            let status = mods.status(&kit.game)?;
            json!({
                "installed": mods.mods().len(),
                "enabled": mods.mods().iter().filter(|m| m.enabled).count(),
                "resources": status.changes.len(),
                "up_to_date": status.streams.iter().all(|s| s.up_to_date),
                "streams": status.streams.iter().map(|s| json!({
                    "stream": s.stream,
                    "state": state(s.state),
                    "up_to_date": s.up_to_date,
                })).collect::<Vec<_>>(),
            })
        }
        "conflicts" => json!({
            "conflicts": mods.conflicts(&kit.game, true)?.iter().map(|c| json!({
                "resource": c.key.to_string(),
                "mods": c.mods,
                "winner": c.winner(),
                "same_data": c.same_data,
                "patch": c.patch,
                // Empty: the winner replaces the resource. Else the mods
                // merge and only these values are set by both.
                "elements": c.elements,
            })).collect::<Vec<_>>(),
            "clashes": mods.clashes(true).iter().map(|c| json!({
                "fatal": c.is_fatal(),
                "what": c.describe(),
            })).collect::<Vec<_>>(),
        }),
        "check" => {
            let issues = mods.check(&kit.game)?;
            let fatal: Vec<String> = mods
                .clashes(true)
                .into_iter()
                .filter(|c| c.is_fatal())
                .map(|c| c.describe())
                .collect();
            json!({
                "ok": issues.is_empty() && fatal.is_empty(),
                "issues": issues.iter().map(|i| i.describe()).collect::<Vec<_>>(),
                "cannot_be_applied_together": fatal,
            })
        }
        other => {
            return fail(format!(
                "`action` is list, status, conflicts, check, info, native or sets, not `{other}`"
            ));
        }
    }))
}

fn report(r: &ApplyReport) -> Value {
    json!({
        "streams": r.streams.iter().map(|(s, a)| json!({
            "stream": s,
            "action": match a {
                StreamAction::Rebuilt(n) => format!("rewritten with {n} replaced resource(s)"),
                StreamAction::Restored => "put back to the original".to_owned(),
            },
        })).collect::<Vec<_>>(),
        "written": r.written,
        "backed_up": r.backed_up,
        "warnings": r.warnings,
    })
}

fn install_json(r: &InstallReport) -> Value {
    json!({
        "id": r.id,
        "name": r.name,
        "replaced_version": r.replaced_version,
        "resources": r.resources,
        "added": r.added,
        "new_streams": r.new_streams,
        "warnings": r.warnings,
        "issues": r.issues.iter().map(|i| i.describe()).collect::<Vec<_>>(),
    })
}

/// Refuses to install a mod `id` whose modules (`sha256`) are not the ones
/// the player agreed to: the answer holds for later versions of a mod, so
/// an update through the server would run code nobody was shown.
fn refuse_changed_code<'a>(
    mods: &ModManager,
    id: &str,
    sha256: impl IntoIterator<Item = &'a str>,
) -> ToolResult<()> {
    if native::replaces_agreed_code(mods, id, sha256)? {
        return fail(format!(
            "mod `{id}` is installed and the player agreed to its executable code; this file \
             carries other code. The answer of the player holds for later versions of a mod, \
             so an update with new code is for the player to install: in Open Mod Manager, or \
             `doctor-toolkit mods install GAME FILE`"
        ));
    }
    Ok(())
}

/// The state of the code of an installed mod, for the answer of an install.
fn code_after_install(mods: &ModManager, id: &str) -> Value {
    if mods.get(id).is_none_or(|m| m.manifest.native.is_none()) {
        return Value::Null;
    }
    json!({
        "answer": native::consent(mods, id).ok().map(consent_word),
        "state": code_states(mods).get(id),
        "where_to_agree": WHERE_TO_AGREE,
    })
}

pub fn mods_manage(kit: &Toolkit, p: ModsManageParams) -> ToolResult<Output> {
    let mut mods = manager(kit)?;
    let running = doctor_toolkit::play::game_processes();
    let done = match p.action.as_str() {
        "install_workspace" => {
            let Some(ws) = kit.workspace()? else {
                return fail("there is no workspace: nothing to install");
            };
            let Some(info) = &p.info else {
                return fail("give `mod`: at least {\"name\": \"…\"}");
            };
            if !info.changes.is_empty() || info.preview.is_some() {
                return fail(
                    "install_workspace takes every change and no preview: build a file with mod_build for those",
                );
            }
            let planned = super::build::plan_mod(kit, &ws, info)?;
            if let Some(code) = &planned.info.relations.native {
                let hashes: Vec<String> = code
                    .modules
                    .iter()
                    .map(|(_, data)| doctor_toolkit::workspace::Blob::of(data).sha256)
                    .collect();
                refuse_changed_code(&mods, &planned.info.id, hashes.iter().map(String::as_str))?;
            }
            let r = mods.install_workspace(&kit.game, &ws, &planned.info, None)?;
            json!({
                "installed": install_json(&r),
                "depends_added": planned.suggested,
                "warnings": planned.warnings,
                "code": code_after_install(&mods, &r.id),
            })
        }
        "export" => {
            let [id] = p.ids.as_slice() else {
                return fail("give one id in `ids` to export");
            };
            let m = mods
                .get(id)
                .ok_or_else(|| ToolError(format!("no mod `{id}` is installed")))?;
            let out = kit.output_path(&format!(
                "mods/{}-{}.zip",
                m.id,
                file_part(&m.manifest.version)
            ))?;
            let bytes = mods.export(id, &out)?;
            json!({ "exported": id, "file": out, "bytes": bytes })
        }
        "import_old" => {
            let import = mods.import_old_toolkit(&kit.game)?;
            json!({
                "adopted_backups": import.adopted,
                "installed": import.installed.iter().map(|(name, result)| match result {
                    Ok(r) => install_json(r),
                    Err(e) => json!({ "file": name, "error": e }),
                }).collect::<Vec<_>>(),
                "note": "the game still has the old toolkit's files: `apply` with `force: true` \
                         rewrites them from the originals with the installed mods",
            })
        }
        "save_set" | "use_set" | "delete_set" => {
            let Some(name) = p.set.as_deref().map(str::trim).filter(|n| !n.is_empty()) else {
                return fail("give `set`: the name of the set of mods");
            };
            match p.action.as_str() {
                "save_set" => {
                    mods.save_set(name)?;
                    json!({ "saved_set": name })
                }
                "use_set" => {
                    let missing = mods.use_set(name)?;
                    json!({ "using_set": name, "not_installed": missing })
                }
                _ => {
                    if !mods.delete_set(name)? {
                        return fail(format!("no set of mods `{name}`"));
                    }
                    json!({ "deleted_set": name })
                }
            }
        }
        "install" => {
            if p.files.is_empty() {
                return fail("give `files` to install");
            }
            let mut out = Vec::new();
            for file in &p.files {
                let path = input_file(file)?;
                // Code other than the player agreed to is not for the server to install.
                if let Ok(archive) = ModArchive::open(&path)
                    && let Some(code) = &archive.manifest().native
                {
                    refuse_changed_code(
                        &mods,
                        &archive.manifest().id,
                        code.modules.iter().map(|m| m.payload.sha256.as_str()),
                    )?;
                }
                let r = mods.install_file(&kit.game, &path, !p.disabled.unwrap_or(false))?;
                let mut installed = install_json(&r);
                installed["code"] = code_after_install(&mods, &r.id);
                out.push(installed);
            }
            json!({ "installed": out })
        }
        "uninstall" | "enable" | "disable" => {
            if p.ids.is_empty() {
                return fail("give `ids` of installed mods (`mods` lists them)");
            }
            if p.action != "enable" && !p.force.unwrap_or(false) {
                for id in &p.ids {
                    let needed: Vec<String> = mods
                        .dependents(id)
                        .into_iter()
                        .filter(|d| !p.ids.contains(d))
                        .collect();
                    if !needed.is_empty() {
                        return fail(format!(
                            "{id} is needed by the enabled mod(s) {}; {} them too, or give \
                             `force: true`",
                            needed.join(", "),
                            p.action
                        ));
                    }
                }
            }
            let mut also = Vec::new();
            let mut notes = Vec::new();
            for id in &p.ids {
                match p.action.as_str() {
                    "uninstall" => mods.uninstall(id)?,
                    "enable" if p.with_deps.unwrap_or(false) => {
                        also.extend(mods.enable_with_dependencies(id)?);
                    }
                    "enable" => {
                        mods.set_enabled(id, true)?;
                        let off = mods.disabled_dependencies(id);
                        if !off.is_empty() {
                            notes.push(format!(
                                "{id} needs {}, switched off: `with_deps: true` enables them",
                                off.join(", ")
                            ));
                        }
                    }
                    _ => mods.set_enabled(id, false)?,
                }
            }
            json!({ p.action.clone(): p.ids, "also_enabled": also, "notes": notes })
        }
        "sort" => {
            let r = mods.sort(p.dry_run.unwrap_or(false))?;
            json!({
                "changed": r.changed,
                "order": r.order,
                "dropped_rules": r.dropped,
                "dry_run": p.dry_run.unwrap_or(false),
            })
        }
        "move" => {
            let [id] = p.ids.as_slice() else {
                return fail("give one id in `ids` to move");
            };
            let position = p
                .position
                .filter(|&n| n > 0)
                .ok_or_else(|| ToolError("give `position`, 1 for the first place".into()))?;
            let warnings = mods.move_mod(id, position - 1)?;
            json!({ "moved": id, "position": position, "warnings": warnings })
        }
        "apply" | "unapply" => {
            if !running.is_empty() {
                return fail("the game is running: stop it first (stop)");
            }
            let force = p.force.unwrap_or(false);
            let r = if p.action == "apply" {
                mods.apply(
                    &kit.game,
                    ModApplyOptions {
                        force,
                        ignore_compatibility: false,
                    },
                )?
            } else {
                mods.unapply(&kit.game, force)?
            };
            json!({ "report": report(&r) })
        }
        other => {
            return fail(format!(
                "`action` is install, install_workspace, uninstall, enable, disable, move, sort, \
                 apply, unapply, export, import_old, save_set, use_set or delete_set, not `{other}`"
            ));
        }
    };
    let up_to_date = mods
        .status(&kit.game)
        .map(|s| s.streams.iter().all(|s| s.up_to_date))
        .unwrap_or(false);
    Ok(Output::Json(json!({
        "done": done,
        "mods": list(&mods),
        "current_set": mods.current_set(),
        "game_up_to_date": up_to_date,
        "note": if up_to_date {
            "the game has the enabled mods"
        } else {
            "the game does not have this yet: `apply` writes it, or play with `mods: true` \
             gives it to the game without writing"
        },
    })))
}

/// A version as a part of a file name.
fn file_part(text: &str) -> String {
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

pub fn integrity(kit: &Toolkit, p: IntegrityParams) -> ToolResult<Output> {
    let report = integrity::check(
        &kit.game,
        IntegrityOptions {
            full: p.full.unwrap_or(false),
        },
        |_, _| {},
    )?;
    let changed: Vec<Value> = report
        .changed()
        .take(500)
        .map(|f| json!({ "file": f.rel_path, "condition": f.condition.describe() }))
        .collect();
    Ok(Output::Json(json!({
        "files": report.files.len(),
        "original": report.count(Condition::Original),
        "changed": report.changed().count(),
        "unverified": report.count(Condition::Unverified),
        "reference_list": report.reference,
        "changed_files": changed,
        "old_toolkit": report.old_toolkit.as_ref().map(|o| json!({
            "installed_patches": o.installed_patches,
            "backups": o.backups.len(),
        })),
        "temp_files": report.temp_files,
    })))
}

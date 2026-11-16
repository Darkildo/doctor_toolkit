//! The mod manager and the check of the game files (stage 57): `mods` and
//! `integrity` read; `mods_manage` (`--allow-game`) installs, removes,
//! orders and switches mods and writes them into the game or takes them out.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use doctor_toolkit::mods::integrity::{self, Condition, IntegrityOptions};
use doctor_toolkit::mods::manager::{ModApplyOptions, ModManager};
use doctor_toolkit::workspace::{ApplyReport, StreamAction, StreamState};

use super::Output;
use super::create::input_file;
use crate::state::{ToolError, ToolResult, Toolkit, fail};

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ModsParams {
    /// `list` (default: installed mods in load order, with what each needs,
    /// recommends and does not work with), `status` (what the game has of
    /// them), `conflicts` (resources changed by several mods), `check`
    /// (whether the enabled mods fit this game).
    #[serde(default)]
    pub action: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ModsManageParams {
    /// `install` (mod files `files`), `uninstall`, `enable`, `disable`
    /// (`ids`), `move` (`ids` with one id, `position` from 1), `sort` (the
    /// load order by the mods' rules: dependencies first, then
    /// load_after/load_before), `apply` (write the enabled mods into the
    /// game, originals backed up) or `unapply` (put the originals back).
    pub action: String,
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

fn list(mods: &ModManager) -> Value {
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

pub fn mods(kit: &Toolkit, p: ModsParams) -> ToolResult<Output> {
    let mods = manager(kit)?;
    Ok(Output::Json(match p.action.as_deref().unwrap_or("list") {
        "list" => json!({ "mods": list(&mods) }),
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
            "conflicts": mods.conflicts(true).iter().map(|c| json!({
                "resource": c.key.to_string(),
                "mods": c.mods,
                "winner": c.winner(),
                "same_data": c.same_data,
                "patch": c.patch,
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
                "`action` is list, status, conflicts or check, not `{other}`"
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

pub fn mods_manage(kit: &Toolkit, p: ModsManageParams) -> ToolResult<Output> {
    let mut mods = manager(kit)?;
    let running = doctor_toolkit::play::game_processes();
    let done = match p.action.as_str() {
        "install" => {
            if p.files.is_empty() {
                return fail("give `files` to install");
            }
            let mut out = Vec::new();
            for file in &p.files {
                let path = input_file(file)?;
                let r = mods.install_file(&kit.game, &path, !p.disabled.unwrap_or(false))?;
                out.push(json!({
                    "id": r.id,
                    "name": r.name,
                    "replaced_version": r.replaced_version,
                    "resources": r.resources,
                    "added": r.added,
                    "new_streams": r.new_streams,
                    "warnings": r.warnings,
                    "issues": r.issues.iter().map(|i| i.describe()).collect::<Vec<_>>(),
                }));
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
                "`action` is install, uninstall, enable, disable, move, sort, apply or unapply, not `{other}`"
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
        "game_up_to_date": up_to_date,
        "note": if up_to_date {
            "the game has the enabled mods"
        } else {
            "the game does not have this yet: `apply` writes it, or play with `mods: true` \
             gives it to the game without writing"
        },
    })))
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

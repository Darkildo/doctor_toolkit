//! Operations on a resource from its menus and views (stage 103): a file
//! into the resource by its type, new collision for a metamodel, the
//! parameters of a material, a string in every table and language, the
//! `$name` references of an interface movie. Each one is a function of the
//! library that the MCP tools and the command line call too, recorded as
//! one undoable operation.
//!
//! `DOCTOR_TOOLKIT_GUI_RESOURCE=<item>[,…]` does them without input on the
//! resource selected with `DOCTOR_TOOLKIT_GUI_SELECT` ([`BrowserApp::resource_preset`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use doctor_toolkit::import::collision;
use doctor_toolkit::import::file::{self, FileOptions, FileTarget};
use doctor_toolkit::level::TYPE_METAMODEL;
use doctor_toolkit::resource::hkx::TYPE_HKX;
use doctor_toolkit::resource::hkx::collision::CollisionKind;
use doctor_toolkit::resource::material::{self, ParamEdit};
use doctor_toolkit::text::{self, NewString, StringEdit};
use doctor_toolkit_mods::i18n::{tr, trf, trn};

use super::BrowserApp;
use super::shell::Level;
use crate::nav::Location;

/// `DOCTOR_TOOLKIT_GUI_RESOURCE` was used.
#[derive(Default)]
pub(super) struct ResourcePreset {
    used: bool,
}

impl BrowserApp {
    /// Reports what an operation did; true when it went through.
    fn report(
        &mut self,
        what: &str,
        result: Result<String, String>,
        stream: Option<usize>,
    ) -> bool {
        let ok = result.is_ok();
        match result {
            Ok(text) => self.status = text,
            Err(error) => self.notify(
                Level::Error,
                trf("edits.cannot_record", &[("what", &what), ("error", &error)]),
            ),
        }
        self.after_edit(stream);
        ok
    }

    /// Replaces resource `loc` with what `path` holds, read by the type of
    /// the resource (`raw`: as stored), as `import_file` of the MCP server.
    pub(crate) fn import_resource_file(&mut self, loc: Location, path: &Path, raw: bool) -> bool {
        let Some(game) = self.game.clone() else {
            return false;
        };
        if let Some(dir) = path.parent() {
            self.settings.export_dirs.import = Some(dir.to_owned());
        }
        let result = self.workspace_mut().and_then(|ws| {
            let target =
                FileTarget::game(&game, loc.stream, loc.resource).map_err(|e| e.to_string())?;
            file::import_file(&game, ws, &target, path, &FileOptions { raw })
                .map_err(|e| e.to_string())
        });
        let result = result.map(|done| {
            if done.changed {
                let mut text = trf("edits.recorded", &[("label", &done.label)]);
                if !done.notes.is_empty() {
                    text = format!("{text} ({})", done.notes.join("; "));
                }
                text
            } else {
                tr("edits.no_change_data").to_owned()
            }
        });
        self.report(tr("resource.import_what"), result, Some(loc.stream))
    }

    /// The metamodel new collision is built for: `loc` itself, or the
    /// metamodel of its stream that uses the collision `loc`.
    fn metamodel_for(&self, loc: Location) -> Result<Location, String> {
        match self.type_of(loc) {
            Some(TYPE_METAMODEL) => Ok(loc),
            Some(TYPE_HKX) => {
                let game = self.game.as_ref().ok_or(tr("edits.no_game"))?;
                let file = game.open_stream(loc.stream).map_err(|e| e.to_string())?;
                let resources = &file.layout().resources;
                let ids = resources
                    .get(loc.resource)
                    .ok_or(tr("resource.no_metamodel"))?
                    .header
                    .ids;
                for (resource, r) in resources.iter().enumerate() {
                    if r.header.type_id != TYPE_METAMODEL {
                        continue;
                    }
                    let at = Location {
                        stream: loc.stream,
                        resource,
                    };
                    let uses = self
                        .current_data(at)
                        .ok()
                        .and_then(|d| collision::collision_references(&d).ok())
                        .is_some_and(|refs| refs.contains(&ids));
                    if uses {
                        return Ok(at);
                    }
                }
                Err(tr("resource.no_metamodel").to_owned())
            }
            _ => Err(tr("resource.not_metamodel").to_owned()),
        }
    }

    /// Builds new Havok collision of `kind` for the metamodel `loc` (or the
    /// metamodel that uses the collision `loc`): from the model file
    /// `source`, or from the metamodel's own first mesh.
    pub(crate) fn build_collision(
        &mut self,
        loc: Location,
        kind: CollisionKind,
        source: Option<&Path>,
    ) -> bool {
        let (Some(game), Some(index)) = (self.game.clone(), self.index.clone()) else {
            return false;
        };
        let target = self.metamodel_for(loc);
        let result = target.and_then(|at| {
            let ws = self.workspace_mut()?;
            let mesh = match source {
                Some(path) => collision::mesh_from_gltf(
                    &doctor_toolkit::import::gltf::read_file(path).map_err(|e| e.to_string())?,
                ),
                None => collision::mesh_from_metamodel(&game, ws, at.stream, at.resource)
                    .map_err(|e| e.to_string())?,
            };
            let plan = collision::plan(&game, ws, &index, at.stream, at.resource, &mesh, kind)
                .map_err(|e| e.to_string())?;
            plan.commit(ws, &game).map_err(|e| e.to_string())?;
            Ok(trf(
                "resource.collision_built",
                &[
                    ("label", &plan.label),
                    ("added", &plan.report.added),
                    ("references", &plan.report.references),
                ],
            ))
        });
        self.report(tr("resource.collision_what"), result, Some(loc.stream))
    }

    /// Sets parameters of the material resource `loc`.
    pub(crate) fn set_material_params(&mut self, loc: Location, edits: &[ParamEdit]) -> bool {
        let Some(game) = self.game.clone() else {
            return false;
        };
        let result = self.workspace_mut().and_then(|ws| {
            material::set_params(&game, ws, loc.stream, loc.resource, edits)
                .map_err(|e| e.to_string())
        });
        let result = result.map(|done| {
            if done.changed {
                trf("edits.recorded", &[("label", &done.label)])
            } else {
                tr("edits.no_change_data").to_owned()
            }
        });
        self.report(tr("resource.material_what"), result, Some(loc.stream))
    }

    /// Gives entries of the string tables new texts in every table of the
    /// game that has them (every language), as one operation.
    pub(crate) fn edit_strings_everywhere(&mut self, edits: &[StringEdit]) -> bool {
        let (Some(game), Some(index)) = (self.game.clone(), self.index.clone()) else {
            return false;
        };
        let result = self.workspace_mut().and_then(|ws| {
            let tables = text::all_tables(&index);
            let plan = text::plan_edits(&game, ws, &tables, edits).map_err(|e| e.to_string())?;
            if plan.changes.is_empty() {
                return Ok(tr("edits.no_change_strings").to_owned());
            }
            plan.commit(ws, &game).map_err(|e| e.to_string())?;
            Ok(trf(
                "resource.strings_edited",
                &[
                    ("label", &plan.label),
                    ("texts", &plan.changes.len()),
                    ("tables", &plan.tables()),
                ],
            ))
        });
        self.report(tr("resource.strings_what"), result, None)
    }

    /// Adds a string to the tables of every language, each language with
    /// its translation when `translations` (language code → text) has one.
    /// A code no table has is refused.
    pub(crate) fn add_string_translated(
        &mut self,
        name: &str,
        text_default: &str,
        translations: &BTreeMap<String, String>,
    ) -> bool {
        let (Some(game), Some(index)) = (self.game.clone(), self.index.clone()) else {
            return false;
        };
        let new = NewString {
            name: name.trim().to_owned(),
            text: text_default.to_owned(),
            translations: translations.clone(),
        };
        let result = self.workspace_mut().and_then(|ws| {
            let tables = text::language_tables(&game, &index).map_err(|e| e.to_string())?;
            let plan =
                text::plan_add(&game, ws, &index, &tables, &new).map_err(|e| e.to_string())?;
            let known: Vec<&str> = plan
                .added
                .tables
                .iter()
                .flat_map(|(_, texts)| texts.iter().map(|(code, _)| code.as_str()))
                .collect();
            if let Some(unknown) = new
                .translations
                .keys()
                .find(|c| !known.contains(&c.as_str()))
            {
                let mut codes: Vec<&str> = known.clone();
                codes.sort_unstable();
                codes.dedup();
                return Err(trf(
                    "resource.unknown_language",
                    &[("code", unknown), ("known", &codes.join(", "))],
                ));
            }
            plan.commit(ws, &game).map_err(|e| e.to_string())?;
            Ok(trf(
                "edits.string_added",
                &[
                    ("label", &plan.label),
                    ("name", &plan.added.name),
                    ("hash", &format_args!("{:08x}", plan.added.hash)),
                ],
            ))
        });
        self.report(tr("resource.add_string_what"), result, None)
    }

    /// Points the `$old` references of the interface movie `loc` at `$new`.
    pub(crate) fn retarget_apx(&mut self, loc: Location, old: &str, new: &str) -> bool {
        let Some(game) = self.game.clone() else {
            return false;
        };
        let (old, new) = (old.trim_start_matches('$'), new.trim_start_matches('$'));
        let result = self.workspace_mut().and_then(|ws| {
            let changed = text::retarget_apx(&game, ws, loc.stream, loc.resource, old, new)
                .map_err(|e| e.to_string())?;
            if changed == 0 {
                return Err(trf("apx.no_reference", &[("name", &old)]));
            }
            Ok(trn(
                "apx.retargeted",
                changed as u64,
                &[("old", &old), ("new", &new)],
            ))
        });
        self.report(tr("resource.apx_what"), result, Some(loc.stream))
    }

    /// Opens the material with name hash `hash` in its editor.
    pub(crate) fn open_material(&mut self, hash: u32) -> bool {
        let (Some(game), Some(index)) = (self.game.clone(), self.index.clone()) else {
            return false;
        };
        let found = self
            .assets
            .get_or_insert_with(|| doctor_toolkit::game::GameAssets::new(game, &index))
            .material_location(hash);
        match found {
            Some((stream, resource)) => {
                self.open_resource_tab(Location { stream, resource });
                true
            }
            None => {
                self.notify(
                    Level::Error,
                    trf("material.not_in_game", &[("hash", &format!("{hash:08x}"))]),
                );
                false
            }
        }
    }

    /// Requests of the views made through the context (the material panel's
    /// "Edit material…").
    pub(crate) fn poll_resource_requests(&mut self, ctx: &egui::Context) {
        if let Some(hash) = crate::material_view::take_request(ctx) {
            self.open_material(hash);
        }
    }

    /// `DOCTOR_TOOLKIT_GUI_RESOURCE=<item>[,…]`: operations on the resource
    /// selected with `DOCTOR_TOOLKIT_GUI_SELECT`, in the order written; the
    /// first one that fails stops the run (see `preset_failed`). Items
    /// (a comma in a value is `\,`, or the value is a JSON string):
    ///
    /// - `import=<file>`, `raw=<file>`: the file into the resource, by type
    ///   or as stored;
    /// - `collision=mesh|hull|box` (after an optional `source=<model file>`):
    ///   new collision of the metamodel;
    /// - `param=<name>=<n>/<n>/…` or `param=<name>=tex:<hash>`: a parameter
    ///   of the material; several are one operation;
    /// - `string=<entry name or 0xhash>:<text>` (after an optional
    ///   `language=<code>`): the text in every table;
    /// - `add_string=<name>:<text>` with `tr=<code>:<text>` items after it:
    ///   a new string in the tables of every language;
    /// - `apx=<old>:<new>`: the `$old` references of the movie to `$new`;
    /// - `material=<name hash>`: opens the editor of that material.
    pub(super) fn resource_preset(&mut self) {
        if self.resource_preset.used || self.game.is_none() || self.index.is_none() {
            return;
        }
        let Ok(spec) = std::env::var("DOCTOR_TOOLKIT_GUI_RESOURCE") else {
            self.resource_preset.used = true;
            return;
        };
        // Waits for the selection of the screenshot mode.
        let Some(loc) = self.details.as_ref().map(|d| d.loc) else {
            return;
        };
        self.resource_preset.used = true;
        self.run_resource_preset(loc, &spec);
    }

    /// Runs the items of `spec` on `loc` ([`Self::resource_preset`]); true
    /// when all went through.
    pub(crate) fn run_resource_preset(&mut self, loc: Location, spec: &str) -> bool {
        let items: Vec<(String, String)> = super::wizard::split_preset(spec)
            .iter()
            .filter(|i| !i.is_empty())
            .map(|i| super::wizard::preset_item(i))
            .collect();
        let mut source: Option<PathBuf> = None;
        let mut language: Option<String> = None;
        let mut at = 0;
        while at < items.len() {
            let (key, value) = (items[at].0.as_str(), items[at].1.as_str());
            at += 1;
            let pair = |value: &str| -> Result<(String, String), String> {
                value
                    .split_once(':')
                    .map(|(a, b)| (a.trim().to_owned(), b.to_owned()))
                    .ok_or_else(|| trf("resource.preset_pair", &[("item", &key)]))
            };
            let ok = match key {
                "import" => self.import_resource_file(loc, Path::new(value), false),
                "raw" => self.import_resource_file(loc, Path::new(value), true),
                "source" => {
                    source = Some(PathBuf::from(value));
                    true
                }
                "collision" => match value.parse::<CollisionKind>() {
                    Ok(kind) => self.build_collision(loc, kind, source.as_deref()),
                    Err(error) => self.preset_item_failed(key, &error.to_string()),
                },
                "param" => {
                    // The parameters that follow are part of the same operation.
                    let mut texts = vec![value.to_owned()];
                    while let Some((k, v)) = items.get(at)
                        && k == "param"
                    {
                        texts.push(v.clone());
                        at += 1;
                    }
                    let edits: Result<Vec<ParamEdit>, _> =
                        texts.iter().map(|t| ParamEdit::parse(t)).collect();
                    match edits {
                        Ok(edits) => self.set_material_params(loc, &edits),
                        Err(error) => self.preset_item_failed(key, &error.to_string()),
                    }
                }
                "language" => {
                    language = Some(value.to_owned());
                    true
                }
                "string" => match pair(value) {
                    Ok((entry, new)) => {
                        let hash = entry
                            .strip_prefix("0x")
                            .and_then(|h| u32::from_str_radix(h, 16).ok())
                            .unwrap_or_else(|| doctor_toolkit::hash::string_id(&entry));
                        self.edit_strings_everywhere(&[StringEdit {
                            hash,
                            text: new,
                            language: language.clone(),
                        }])
                    }
                    Err(error) => self.preset_item_failed(key, &error),
                },
                "add_string" => {
                    let mut translations = BTreeMap::new();
                    let mut bad = None;
                    while let Some((k, v)) = items.get(at)
                        && k == "tr"
                    {
                        at += 1;
                        match v.split_once(':') {
                            Some((code, t)) => {
                                translations.insert(code.trim().to_owned(), t.to_owned());
                            }
                            None => bad = Some(trf("resource.preset_pair", &[("item", &"tr")])),
                        }
                    }
                    match (pair(value), bad) {
                        (Ok((name, new)), None) => {
                            self.add_string_translated(&name, &new, &translations)
                        }
                        (Err(error), _) | (_, Some(error)) => self.preset_item_failed(key, &error),
                    }
                }
                "apx" => match pair(value) {
                    Ok((old, new)) => self.retarget_apx(loc, &old, &new),
                    Err(error) => self.preset_item_failed(key, &error),
                },
                "material" => match u32::from_str_radix(value.trim_start_matches("0x"), 16) {
                    Ok(hash) => self.open_material(hash),
                    Err(_) => self.preset_item_failed(key, tr("material.bad_hash")),
                },
                _ => self.preset_item_failed(key, tr("resource.preset_unknown")),
            };
            if !ok {
                return false;
            }
        }
        true
    }

    /// An item of the preset that cannot be read: an error of the run.
    fn preset_item_failed(&mut self, item: &str, why: &str) -> bool {
        self.notify(
            Level::Error,
            format!("DOCTOR_TOOLKIT_GUI_RESOURCE: {item}: {why}"),
        );
        false
    }
}

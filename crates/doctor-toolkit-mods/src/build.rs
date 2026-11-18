//! Building a mod from the workspace: the section of the editor's Mods
//! window only, and the fields of format version 3 ([`RelationsForm`]),
//! which the editor's "Package a mod" wizard shows too.

use std::path::PathBuf;

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::mods::{self, Incompatible, ModInfo, Preview, Relations};
use doctor_toolkit::workspace::{ResourceKey, Workspace};

use crate::ModsPanel;
use crate::i18n::{tr, trf, trn};

/// The relations of a mod being built, as typed: one mod per line in the
/// lists (`id condition`, for incompatible mods `id condition: reason`),
/// ids separated by commas or spaces in the rules of the order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RelationsForm {
    pub depends: String,
    pub recommends: String,
    pub incompatible: String,
    pub load_after: String,
    pub load_before: String,
    /// Editions separated by commas; empty (or `any`): any edition.
    pub editions: String,
    /// Empty: this program's version or newer; `none`: no condition.
    pub requires_manager: String,
    /// Executable code (format version 5): one DLL per line, a path or
    /// `name.asi=path`.
    pub native: String,
    /// The mod makes no sense without its code.
    pub native_required: bool,
    /// Every resource whole: no deltas (format version 6), for the mod
    /// managers before that version (`ModInfo::whole_resources`).
    pub whole_resources: bool,
}

/// `id condition` lines.
fn id_lines(text: &str) -> Vec<(String, String)> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|line| match line.split_once(char::is_whitespace) {
            Some((id, rest)) if !rest.trim().is_empty() => (id.to_owned(), rest.trim().to_owned()),
            _ => (
                line.split_whitespace().next().unwrap_or(line).to_owned(),
                "*".to_owned(),
            ),
        })
        .collect()
}

/// Ids separated by commas or spaces.
fn id_list(text: &str) -> Vec<String> {
    text.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

impl RelationsForm {
    /// The relations typed; the manifest checks them when the mod is built.
    pub fn relations(&self) -> Relations {
        let editions = id_list(&self.editions)
            .into_iter()
            .filter(|e| e != "any")
            .collect();
        let requires_manager = match self.requires_manager.trim() {
            "" => None,
            "none" => Some(String::new()),
            condition => Some(condition.to_owned()),
        };
        Relations {
            depends: id_lines(&self.depends).into_iter().collect(),
            recommends: id_lines(&self.recommends).into_iter().collect(),
            incompatible: id_lines(&self.incompatible)
                .into_iter()
                .map(|(id, rest)| {
                    let (versions, reason) = match rest.split_once(':') {
                        Some((v, r)) => (v.trim().to_owned(), r.trim().to_owned()),
                        None => (rest, String::new()),
                    };
                    let versions = if versions.is_empty() {
                        "*".to_owned()
                    } else {
                        versions
                    };
                    (id, Incompatible { versions, reason })
                })
                .collect(),
            load_after: id_list(&self.load_after),
            load_before: id_list(&self.load_before),
            editions,
            requires_manager,
            made_on: Default::default(),
            made_on_data: Default::default(),
            native: self.native(),
        }
    }

    /// The modules typed, read from the disk. A file that cannot be read
    /// becomes an empty module, which building the mod refuses by its name.
    fn native(&self) -> Option<mods::NativeBuild> {
        let modules: Vec<(String, Vec<u8>)> = self
            .native
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(|line| {
                let (name, path) = mods::native::module_spec(line);
                (name, std::fs::read(path).unwrap_or_default())
            })
            .collect();
        (!modules.is_empty()).then_some(mods::NativeBuild {
            modules,
            platform_abi: Some(mods::native::PLATFORM_ABI),
            required: self.native_required,
        })
    }

    /// Adds to the dependencies the mods written into `game` that the
    /// changes of `ws` were made on; returns how many were added.
    pub fn fill_suggested(
        &mut self,
        game: &GameInstance,
        ws: &Workspace,
        keys: Option<&[ResourceKey]>,
        own_id: &str,
    ) -> Result<usize, String> {
        let suggested =
            mods::suggested_dependencies(game, ws, keys, own_id).map_err(|e| e.to_string())?;
        let have: Vec<String> = id_lines(&self.depends)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        let mut added = 0;
        for (id, condition) in suggested.depends {
            if have.contains(&id) {
                continue;
            }
            if !self.depends.is_empty() && !self.depends.ends_with('\n') {
                self.depends.push('\n');
            }
            self.depends += &format!("{id} {condition}");
            added += 1;
        }
        Ok(added)
    }

    /// The fields in a grid of two columns; `edition` is this game's
    /// edition, which a button puts in the list of editions.
    pub fn ui(&mut self, ui: &mut egui::Ui, edition: Option<&str>) {
        let lines = |ui: &mut egui::Ui, text: &mut String, hint: &str| {
            ui.add(
                egui::TextEdit::multiline(text)
                    .desired_rows(2)
                    .hint_text(hint)
                    .font(egui::TextStyle::Monospace),
            );
        };
        egui::Grid::new("mod-relations-form")
            .num_columns(2)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label(tr("mods.field_depends"))
                    .on_hover_text(tr("mods.field_depends_hint"));
                lines(ui, &mut self.depends, "new-new-york >=1.1 <2");
                ui.end_row();
                ui.label(tr("mods.field_recommends"));
                lines(ui, &mut self.recommends, "hd-textures *");
                ui.end_row();
                ui.label(tr("mods.field_incompatible"))
                    .on_hover_text(tr("mods.field_incompatible_hint"));
                lines(ui, &mut self.incompatible, "old-nights <3: …");
                ui.end_row();
                ui.label(tr("mods.field_load_after"))
                    .on_hover_text(tr("mods.field_order_hint"));
                ui.add(egui::TextEdit::singleline(&mut self.load_after).hint_text("id, id"));
                ui.end_row();
                ui.label(tr("mods.field_load_before"))
                    .on_hover_text(tr("mods.field_order_hint"));
                ui.add(egui::TextEdit::singleline(&mut self.load_before).hint_text("id, id"));
                ui.end_row();
                ui.label(tr("mods.field_editions"))
                    .on_hover_text(tr("mods.field_editions_hint"));
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.editions)
                            .hint_text(tr("mods.edition_any")),
                    );
                    if let Some(edition) = edition
                        && ui
                            .small_button(tr("mods.only_this_edition"))
                            .on_hover_text(trf(
                                "mods.only_this_edition_hint",
                                &[("edition", &edition)],
                            ))
                            .clicked()
                    {
                        self.editions = edition.to_owned();
                    }
                });
                ui.end_row();
                ui.label(tr("mods.field_requires_manager"))
                    .on_hover_text(tr("mods.field_requires_manager_hint"));
                ui.add(
                    egui::TextEdit::singleline(&mut self.requires_manager)
                        .hint_text(mods::default_requires_manager()),
                );
                ui.end_row();
                ui.label(tr("mods.field_native"))
                    .on_hover_text(tr("mods.field_native_hint"));
                ui.vertical(|ui| {
                    lines(ui, &mut self.native, "target/…/my_mod.dll");
                    ui.checkbox(&mut self.native_required, tr("mods.field_native_required"));
                });
                ui.end_row();
                ui.label(tr("mods.field_deltas"))
                    .on_hover_text(tr("mods.field_deltas_hint"));
                ui.checkbox(&mut self.whole_resources, tr("mods.field_whole_resources"))
                    .on_hover_text(tr("mods.field_deltas_hint"));
                ui.end_row();
            });
    }
}

/// Metadata typed into the build form.
pub(crate) struct BuildForm {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub url: String,
    pub preview: Option<PathBuf>,
    pub relations: RelationsForm,
    /// The dependencies typed are all the mod has (those of the base of
    /// the editor's open project, stage 108): none is added by a guess.
    pub dependencies_named: bool,
}

impl Default for BuildForm {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            version: "1.0".into(),
            author: String::new(),
            description: String::new(),
            url: String::new(),
            preview: None,
            relations: RelationsForm::default(),
            dependencies_named: false,
        }
    }
}

impl BuildForm {
    pub fn info(&self) -> ModInfo {
        let id = if self.id.trim().is_empty() {
            mods::id_from_name(&self.name)
        } else {
            self.id.trim().to_owned()
        };
        ModInfo {
            id,
            name: self.name.trim().to_owned(),
            version: if self.version.trim().is_empty() {
                "1.0".to_owned()
            } else {
                self.version.trim().to_owned()
            },
            author: self.author.trim().to_owned(),
            description: self.description.clone(),
            url: Some(self.url.trim().to_owned()).filter(|u| !u.is_empty()),
            relations: self.relations.relations(),
            whole_resources: self.relations.whole_resources,
        }
    }

    /// [`Self::info`] with the dependencies on the mods written into the
    /// game that the changes were made on — or, when the dependencies are
    /// those of a project's base, with no more than the data of those mods
    /// the changes replace.
    fn info_for(&self, game: Option<&GameInstance>, ws: &Workspace) -> Result<ModInfo, String> {
        let mut info = self.info();
        let Some(game) = game else {
            return Ok(info);
        };
        if self.dependencies_named {
            let manager = mods::ModManager::open(game).map_err(|e| e.to_string())?;
            mods::add_made_on(&mut info, game, &manager, ws, None).map_err(|e| e.to_string())?;
        } else {
            mods::add_suggested_dependencies(&mut info, game, ws, None)
                .map_err(|e| e.to_string())?;
        }
        Ok(info)
    }
}

impl ModsPanel {
    pub(crate) fn build_section(&mut self, ui: &mut egui::Ui, ws: &Workspace) {
        egui::CollapsingHeader::new(tr("mods.build_title"))
            .id_salt("mods-build")
            .default_open(true)
            .show(ui, |ui| {
                if ws.is_empty() {
                    ui.weak(tr("mods.workspace_empty"));
                    return;
                }
                ui.label(trf(
                    "mods.workspace_changes",
                    &[
                        ("changes", &trn("count.changes", ws.len() as u64, &[])),
                        ("dir", &ws.dir().display()),
                    ],
                ));
                let form = &mut self.build;
                egui::Grid::new("mod-build-form")
                    .num_columns(2)
                    .spacing([8.0, 4.0])
                    .show(ui, |ui| {
                        ui.label(tr("mods.field_name"));
                        ui.text_edit_singleline(&mut form.name);
                        ui.end_row();
                        ui.label(tr("mods.field_id"));
                        ui.add(
                            egui::TextEdit::singleline(&mut form.id)
                                .hint_text(mods::id_from_name(&form.name)),
                        );
                        ui.end_row();
                        ui.label(tr("mods.field_version"));
                        ui.text_edit_singleline(&mut form.version);
                        ui.end_row();
                        ui.label(tr("mods.field_author"));
                        ui.text_edit_singleline(&mut form.author);
                        ui.end_row();
                        ui.label(tr("mods.field_url"));
                        ui.add(egui::TextEdit::singleline(&mut form.url).hint_text("https://…"));
                        ui.end_row();
                        ui.label(tr("mods.field_description"));
                        ui.text_edit_multiline(&mut form.description);
                        ui.end_row();
                        ui.label(tr("mods.field_preview"));
                        ui.horizontal(|ui| {
                            let name = form.preview.as_ref().map_or_else(
                                || tr("mods.no_preview").to_owned(),
                                |p| p.display().to_string(),
                            );
                            ui.label(name);
                            if ui.button(tr("prefs.choose")).clicked()
                                && let Some(path) = rfd::FileDialog::new()
                                    .add_filter(tr("mods.filter_pictures"), &["png", "jpg", "jpeg"])
                                    .pick_file()
                            {
                                form.preview = Some(path);
                            }
                            if form.preview.is_some() && ui.button(tr("mods.clear")).clicked() {
                                form.preview = None;
                            }
                        });
                        ui.end_row();
                    });
                let edition = self
                    .game
                    .as_ref()
                    .and_then(|g| doctor_toolkit::edition::game_edition(g.root()));
                egui::CollapsingHeader::new(tr("mods.relations_title"))
                    .id_salt("mods-build-relations")
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(tr("mods.relations_hint"))
                                .weak()
                                .small(),
                        );
                        self.build.relations.ui(ui, edition.as_deref());
                        if let Some(game) = self.game.clone()
                            && ui
                                .button(tr("mods.fill_depends"))
                                .on_hover_text(tr("mods.fill_depends_hint"))
                                .clicked()
                        {
                            let own = self.build.info().id;
                            self.message = Some(
                                match self.build.relations.fill_suggested(&game, ws, None, &own) {
                                    Ok(n) => (trf("mods.filled_depends", &[("count", &n)]), false),
                                    Err(e) => (e, true),
                                },
                            );
                        }
                    });
                let ready = !self.build.name.trim().is_empty() && self.running.is_none();
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(ready, egui::Button::new(tr("mods.save_file")))
                        .on_hover_text(tr("mods.save_file_hint"))
                        .clicked()
                    {
                        let info = self.build.info();
                        if let Some(out) = rfd::FileDialog::new()
                            .set_file_name(format!("{}-{}.zip", info.id, info.version))
                            .add_filter(tr("mods.filter_mod"), &["zip"])
                            .save_file()
                        {
                            self.message = Some(match self.build_file(ws, &out) {
                                Ok(message) => (message, false),
                                Err(error) => (error, true),
                            });
                        }
                    }
                    if ui
                        .add_enabled(
                            ready && self.manager.is_some(),
                            egui::Button::new(tr("mods.install_workspace")),
                        )
                        .on_hover_text(tr("mods.install_workspace_hint"))
                        .clicked()
                    {
                        let result = self.install_workspace(ws);
                        self.message = Some(match result {
                            Ok(message) => (message, false),
                            Err(error) => (error, true),
                        });
                        self.refresh(ui.ctx());
                    }
                });
            });
    }

    fn build_file(&self, ws: &Workspace, out: &std::path::Path) -> Result<String, String> {
        let info = self
            .build
            .info_for(self.game.as_deref(), ws)
            .map_err(|e| trf("mods.cannot_build", &[("error", &e)]))?;
        let preview = self.load_preview()?;
        let report = mods::build_from_workspace(ws, &info, None, preview.as_ref(), out)
            .map_err(|e| trf("mods.cannot_build", &[("error", &e)]))?;
        let mut message = trf(
            "mods.built",
            &[
                ("file", &out.display()),
                (
                    "resources",
                    &trn("count.resources", report.resources as u64, &[]),
                ),
                ("added", &report.added),
                (
                    "streams",
                    &trn("count.new_streams", report.new_streams as u64, &[]),
                ),
                ("kib", &(report.file_size / 1024)),
            ],
        );
        if report.delta_kinds > 0 && !info.whole_resources {
            message.push('\n');
            message.push_str(&trf(
                "mods.built_deltas",
                &[("deltas", &report.deltas), ("of", &report.delta_kinds)],
            ));
        }
        // What the author should know about the mod's size (a whole sound
        // for a retimed subtitle) and about the resources that could not be
        // changes: the library's text, as its errors are.
        for note in &report.notes {
            message.push('\n');
            message.push_str(note);
        }
        Ok(message)
    }

    fn load_preview(&self) -> Result<Option<Preview>, String> {
        self.build
            .preview
            .as_ref()
            .map(|path| {
                let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
                Preview::from_image(data).map_err(|e| e.to_string())
            })
            .transpose()
    }

    fn install_workspace(&mut self, ws: &Workspace) -> Result<String, String> {
        let (Some(game), Some(manager)) = (self.game.clone(), self.manager.as_mut()) else {
            return Err(tr("mods.no_game_open").to_owned());
        };
        let info = self
            .build
            .info_for(Some(&game), ws)
            .map_err(|e| trf("mods.cannot_install_workspace", &[("error", &e)]))?;
        let report = manager
            .install_workspace(&game, ws, &info, None)
            .map_err(|e| trf("mods.cannot_install_workspace", &[("error", &e)]))?;
        self.selected = Some(report.id.clone());
        let mut message = trf(
            "mods.installed_workspace",
            &[
                ("name", &report.name),
                (
                    "resources",
                    &trn("count.resources", report.resources as u64, &[]),
                ),
            ],
        );
        // A mod of edits made on other data than it is built on takes back
        // what the mods below changed since: said, in the library's words.
        for warning in &report.warnings {
            message.push('\n');
            message.push_str(warning);
        }
        Ok(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_build_form_fills_in_defaults() {
        let form = BuildForm {
            name: " Better Cars ".into(),
            url: "  ".into(),
            version: String::new(),
            ..BuildForm::default()
        };
        let info = form.info();
        assert_eq!(info.id, "better-cars");
        assert_eq!(info.name, "Better Cars");
        assert_eq!(info.version, "1.0");
        assert_eq!(info.url, None);
        mods::validate_id(&info.id).unwrap();
        assert_eq!(info.relations, Relations::default());
    }

    #[test]
    fn the_relations_form_reads_what_is_typed() {
        let form = RelationsForm {
            depends: "new-new-york >=1.1 <2\n\n  base\n".into(),
            recommends: "hd *".into(),
            incompatible: "old-nights <3: changes the same lamps\ndark ^1.0\nugly".into(),
            load_after: "a, b c".into(),
            load_before: String::new(),
            editions: "1.0.789.0, 1.0.764.0".into(),
            requires_manager: "none".into(),
            ..RelationsForm::default()
        };
        let r = form.relations();
        assert_eq!(r.depends["new-new-york"], ">=1.1 <2");
        assert_eq!(r.depends["base"], "*");
        assert_eq!(r.recommends["hd"], "*");
        assert_eq!(r.incompatible["old-nights"].versions, "<3");
        assert_eq!(
            r.incompatible["old-nights"].reason,
            "changes the same lamps"
        );
        assert_eq!(r.incompatible["dark"].versions, "^1.0");
        assert_eq!(r.incompatible["ugly"].versions, "*");
        assert_eq!(r.load_after, ["a", "b", "c"]);
        assert!(r.load_before.is_empty());
        assert_eq!(r.editions, ["1.0.789.0", "1.0.764.0"]);
        assert_eq!(r.requires_manager.as_deref(), Some(""));
        let empty = RelationsForm::default().relations();
        assert!(empty.editions.is_empty(), "any edition by default");
        assert_eq!(empty.requires_manager, None);
        let any = RelationsForm {
            editions: "any".into(),
            ..RelationsForm::default()
        };
        assert!(any.relations().editions.is_empty());
    }
}

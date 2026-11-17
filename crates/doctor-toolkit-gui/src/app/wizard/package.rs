//! Package a mod: choose changes of the workspace, give the name, author,
//! version, description and a preview picture, check the package against
//! the game and the installed mods, build the ZIP (`docs/mods.md`) and, if
//! wanted, install it in the mod manager to try it with Play.

use std::path::PathBuf;

use doctor_toolkit::mods::{self, ModInfo, Preview, manager::ModManager};
use doctor_toolkit::workspace::ResourceKey;
use doctor_toolkit_mods::RelationsForm;
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::{Color32, RichText};

use super::{Start, Task, Tx};
use crate::app::{BrowserApp, Level};

/// One change of the workspace to put in the mod or leave out.
pub(crate) struct Item {
    pub key: ResourceKey,
    pub label: String,
    pub added: bool,
    pub chosen: bool,
}

/// What the compatibility check found.
#[derive(Debug, Default, Clone)]
pub(crate) struct Checked {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub notes: Vec<String>,
}

pub(crate) struct Package {
    pub items: Vec<Item>,
    pub info: ModInfo,
    /// Dependencies, editions and the load order, as typed (format 3).
    pub relations: RelationsForm,
    /// The id follows the name until it is edited.
    pub id_edited: bool,
    pub preview: Option<PathBuf>,
    pub preview_error: Option<String>,
    pub checked: Option<Checked>,
    /// Where the ZIP goes; asked for when the mod is built.
    pub out: Option<PathBuf>,
    /// The file built and its size.
    pub built: Option<(PathBuf, u64)>,
    /// What the build tells the author about the mod (`BuildReport::notes`).
    pub build_notes: Vec<String>,
    pub installed: Option<Result<String, String>>,
}

impl Package {
    pub fn new(app: &mut BrowserApp, _start: &Start) -> Self {
        let mut p = Self {
            items: Vec::new(),
            info: ModInfo {
                version: "1.0".into(),
                ..Default::default()
            },
            relations: RelationsForm::default(),
            id_edited: false,
            preview: None,
            preview_error: None,
            checked: None,
            out: None,
            built: None,
            build_notes: Vec::new(),
            installed: None,
        };
        p.load(app);
        p
    }

    pub fn preset(&mut self, key: &str, value: &str, app: &mut BrowserApp) {
        match key {
            "name" => {
                self.info.name = value.to_owned();
                self.info.id = mods::id_from_name(value);
            }
            "author" => self.info.author = value.to_owned(),
            "description" => self.info.description = value.to_owned(),
            "out" => self.out = Some(PathBuf::from(value)),
            "preview" => self.set_preview(PathBuf::from(value)),
            "check" => self.run_checks(app),
            _ => {}
        }
    }

    /// The changes of the workspace.
    pub fn load(&mut self, app: &BrowserApp) {
        self.items = app
            .workspace
            .as_ref()
            .map(|ws| {
                ws.changes()
                    .map(|c| Item {
                        key: c.key.clone(),
                        label: c.label.clone(),
                        added: c.is_added(),
                        chosen: true,
                    })
                    .collect()
            })
            .unwrap_or_default();
    }

    fn keys(&self) -> Option<Vec<ResourceKey>> {
        if self.items.iter().all(|i| i.chosen) {
            return None;
        }
        Some(
            self.items
                .iter()
                .filter(|i| i.chosen)
                .map(|i| i.key.clone())
                .collect(),
        )
    }

    pub fn set_preview(&mut self, path: PathBuf) {
        self.preview_error = std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|d| {
                Preview::from_image(d)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            })
            .err();
        self.preview = Some(path);
    }

    /// The metadata with the relations typed and the dependencies on the
    /// mods written into the game that the chosen changes were made on;
    /// also the ids of those added by themselves.
    fn full_info(&self, app: &BrowserApp) -> Result<(ModInfo, Vec<String>), String> {
        let mut info = self.info.clone();
        info.relations = self.relations.relations();
        let mut added = Vec::new();
        if let (Some(game), Some(ws)) = (&app.disk_game(), &app.workspace) {
            let keys = self.keys();
            added = mods::add_suggested_dependencies(&mut info, game, ws, keys.as_deref())
                .map_err(|e| e.to_string())?;
        }
        Ok((info, added))
    }

    /// Checks the package: the manifest, the game it was made for, the
    /// state of the changes and the installed mods that change the same
    /// resources.
    pub fn run_checks(&mut self, app: &BrowserApp) {
        let mut c = Checked::default();
        let (Some(game), Some(ws)) = (&app.disk_game(), &app.workspace) else {
            c.errors.push(tr("wizard.package.no_changes").to_owned());
            self.checked = Some(c);
            return;
        };
        let keys = self.keys();
        // The check of the edits themselves (stage 103; the same as "Check
        // edits"): what it finds about the chosen changes.
        match app.validate_edits() {
            Ok(Some(report)) => {
                let chosen = |f: &doctor_toolkit::validate::Finding| match (&f.key, &keys) {
                    (Some(key), Some(keys)) => keys.iter().any(|k| k.to_string() == *key),
                    _ => true,
                };
                let line = |f: &doctor_toolkit::validate::Finding| match f.place() {
                    place if place.is_empty() => f.what.clone(),
                    place => format!("{place}: {}", f.what),
                };
                // Conflicts with the game files are told below.
                for f in report.errors.iter().filter(|f| chosen(f)) {
                    if !f.what.starts_with("conflict:") {
                        c.errors.push(line(f));
                    }
                }
                for f in report.warnings.iter().filter(|f| chosen(f)) {
                    if f.stream.is_none() {
                        c.warnings.push(line(f));
                    }
                }
            }
            Ok(None) => {}
            Err(error) => c.warnings.push(error),
        }
        let info = match self.full_info(app) {
            Ok((info, added)) => {
                for id in added {
                    c.notes.push(trf(
                        "wizard.package.depends_added",
                        &[("id", &id), ("versions", &info.relations.depends[&id])],
                    ));
                }
                info
            }
            Err(error) => {
                c.warnings.push(error);
                let mut info = self.info.clone();
                info.relations = self.relations.relations();
                info
            }
        };
        match mods::manifest_from_workspace(ws, &info, keys.as_deref()) {
            Ok(manifest) => {
                c.warnings.extend(manifest.warnings());
                c.notes.push(trf(
                    "wizard.package.contents",
                    &[
                        (
                            "resources",
                            &trn("count.resources", manifest.resources.len() as u64, &[]),
                        ),
                        ("added", &manifest.added().count()),
                        (
                            "streams",
                            &trn("count.new_streams", manifest.streams.len() as u64, &[]),
                        ),
                        ("bytes", &crate::app::human_size(manifest.payload_bytes())),
                    ],
                ));
                if let Ok(status) = ws.status(game) {
                    if !status.fingerprint_matches {
                        c.warnings.push(tr("wizard.package.other_game").to_owned());
                    }
                    for conflict in status.conflicts() {
                        c.warnings
                            .push(trf("wizard.package.conflict", &[("stream", &conflict)]));
                    }
                }
                match ModManager::open(game) {
                    Ok(manager) => {
                        for m in manager.mods() {
                            if m.id == manifest.id {
                                c.warnings
                                    .push(trf("wizard.package.same_id", &[("id", &m.id)]));
                            }
                            let shared = m
                                .manifest
                                .resources
                                .iter()
                                .filter(|r| manifest.resources.iter().any(|o| o.key == r.key))
                                .count();
                            if shared > 0 {
                                c.warnings.push(trf(
                                    "wizard.package.overlap",
                                    &[("id", &m.id), ("count", &shared)],
                                ));
                            }
                        }
                    }
                    Err(error) => c.warnings.push(error.to_string()),
                }
            }
            Err(error) => c.errors.push(error.to_string()),
        }
        if let Some(error) = &self.preview_error {
            c.errors.push(error.clone());
        }
        if c.errors.is_empty() && c.warnings.is_empty() {
            c.notes.push(tr("wizard.package.all_good").to_owned());
        }
        self.checked = Some(c);
    }

    fn changes_step(&mut self, ui: &mut egui::Ui, app: &mut BrowserApp) {
        if ui.button(tr("wizard.package.reload")).clicked() {
            self.load(app);
        }
        if self.items.is_empty() {
            ui.label(tr("wizard.package.no_changes"));
            return;
        }
        ui.horizontal(|ui| {
            if ui.small_button(tr("wizard.package.all")).clicked() {
                self.items.iter_mut().for_each(|i| i.chosen = true);
            }
            if ui.small_button(tr("wizard.package.none")).clicked() {
                self.items.iter_mut().for_each(|i| i.chosen = false);
            }
            let n = self.items.iter().filter(|i| i.chosen).count();
            ui.label(trf(
                "wizard.package.chosen",
                &[("count", &n), ("total", &self.items.len())],
            ));
        });
        egui::ScrollArea::vertical()
            .id_salt("package-items")
            .auto_shrink(false)
            .show(ui, |ui| {
                egui::Grid::new("package-grid")
                    .num_columns(2)
                    .striped(true)
                    .show(ui, |ui| {
                        for item in &mut self.items {
                            let key = if item.added {
                                format!("+ {}", item.key)
                            } else {
                                item.key.to_string()
                            };
                            ui.checkbox(&mut item.chosen, RichText::new(key).monospace());
                            ui.label(RichText::new(&item.label).weak());
                            ui.end_row();
                        }
                    });
            });
    }

    fn details_step(&mut self, ui: &mut egui::Ui, edition: Option<String>) {
        egui::Grid::new("package-info")
            .num_columns(2)
            .show(ui, |ui| {
                ui.label(tr("wizard.package.name"));
                if ui.text_edit_singleline(&mut self.info.name).changed() && !self.id_edited {
                    self.info.id = mods::id_from_name(&self.info.name);
                }
                ui.end_row();
                ui.label(tr("wizard.package.id"));
                if ui.text_edit_singleline(&mut self.info.id).changed() {
                    self.id_edited = true;
                }
                ui.end_row();
                ui.label(tr("wizard.package.author"));
                ui.text_edit_singleline(&mut self.info.author);
                ui.end_row();
                ui.label(tr("wizard.package.version"));
                ui.text_edit_singleline(&mut self.info.version);
                ui.end_row();
                ui.label(tr("wizard.package.url"));
                let mut url = self.info.url.clone().unwrap_or_default();
                if ui.text_edit_singleline(&mut url).changed() {
                    self.info.url = (!url.is_empty()).then_some(url);
                }
                ui.end_row();
                ui.label(tr("wizard.package.description"));
                ui.text_edit_multiline(&mut self.info.description);
                ui.end_row();
                ui.label(tr("wizard.package.preview"));
                ui.horizontal(|ui| {
                    if ui.button(tr("wizard.package.choose_preview")).clicked()
                        && let Some(path) = rfd::FileDialog::new()
                            .add_filter(tr("wizard.package.pictures"), &["png", "jpg", "jpeg"])
                            .pick_file()
                    {
                        self.set_preview(path);
                    }
                    if let Some(path) = &self.preview {
                        ui.monospace(path.display().to_string());
                        if ui.small_button("×").clicked() {
                            self.preview = None;
                            self.preview_error = None;
                        }
                    }
                });
                ui.end_row();
            });
        ui.label(
            RichText::new(tr("wizard.package.preview_hint"))
                .weak()
                .small(),
        );
        if let Some(error) = &self.preview_error {
            ui.colored_label(Color32::from_rgb(230, 90, 80), error);
        }
        egui::CollapsingHeader::new(tr("mods.relations_title"))
            .id_salt("package-relations")
            .show(ui, |ui| {
                ui.label(RichText::new(tr("mods.relations_hint")).weak().small());
                self.relations.ui(ui, edition.as_deref());
            });
    }

    fn check_step(&mut self, ui: &mut egui::Ui, app: &mut BrowserApp) {
        if self.checked.is_none() {
            self.run_checks(app);
        }
        if ui.button(tr("wizard.package.check_again")).clicked() {
            self.run_checks(app);
        }
        let Some(c) = &self.checked else { return };
        for e in &c.errors {
            ui.colored_label(
                Color32::from_rgb(230, 90, 80),
                trf("wizard.error", &[("text", e)]),
            );
        }
        for w in &c.warnings {
            ui.colored_label(Color32::YELLOW, trf("wizard.warning", &[("text", w)]));
        }
        for n in &c.notes {
            ui.label(n);
        }
    }

    /// Builds the ZIP into `out`.
    pub fn build(&mut self, app: &BrowserApp, out: &std::path::Path) -> Result<u64, String> {
        let ws = app.workspace.as_ref().ok_or(tr("wizard.no_workspace"))?;
        let preview = match &self.preview {
            Some(path) => Some(
                std::fs::read(path)
                    .map_err(|e| e.to_string())
                    .and_then(|d| Preview::from_image(d).map_err(|e| e.to_string()))?,
            ),
            None => None,
        };
        let keys = self.keys();
        let (info, _) = self.full_info(app)?;
        let report = mods::build_from_workspace(ws, &info, keys.as_deref(), preview.as_ref(), out)
            .map_err(|e| e.to_string())?;
        self.build_notes = report.notes;
        Ok(report.file_size)
    }

    /// The done step: the file, and installing it in the mod manager.
    pub fn done_ui(&mut self, ui: &mut egui::Ui, app: &mut BrowserApp) {
        let Some((path, size)) = self.built.clone() else {
            return;
        };
        ui.label(trf(
            "wizard.package.built",
            &[
                ("file", &path.display()),
                ("size", &crate::app::human_size(size)),
            ],
        ));
        for note in &self.build_notes {
            ui.colored_label(egui::Color32::YELLOW, note);
        }
        ui.horizontal_wrapped(|ui| {
            if let Some(dir) = path.parent()
                && ui.button(tr("wizard.package.show_folder")).clicked()
            {
                let _ = crate::app::shell::open_external(dir.as_os_str());
            }
            let installed = matches!(self.installed, Some(Ok(_)));
            if ui
                .add_enabled(!installed, egui::Button::new(tr("wizard.package.install")))
                .on_hover_text(tr("wizard.package.install_hint"))
                .clicked()
            {
                self.installed = Some(self.install(app, &path, ui.ctx()));
                if let Some(Ok(id)) = &self.installed {
                    app.notify(Level::Info, trf("wizard.package.installed", &[("id", id)]));
                    app.mods_open = true;
                }
            }
        });
        if let Some(Err(error)) = &self.installed {
            ui.colored_label(Color32::from_rgb(230, 90, 80), error);
        }
    }

    pub(super) fn install(
        &self,
        app: &mut BrowserApp,
        path: &std::path::Path,
        ctx: &egui::Context,
    ) -> Result<String, String> {
        let game = app.disk_game().ok_or(tr("edits.no_game"))?;
        if let Some(reason) = app.write_lock() {
            return Err(reason);
        }
        let mut manager = ModManager::open(&game).map_err(|e| e.to_string())?;
        let report = manager
            .install_file(&game, path, true)
            .map_err(|e| e.to_string())?;
        app.mods.refresh(ctx);
        Ok(report.id)
    }
}

impl Task for Package {
    fn steps(&self) -> &'static [&'static str] {
        &[
            "wizard.package.step.changes",
            "wizard.package.step.details",
            "wizard.package.step.check",
        ]
    }

    fn ui(&mut self, step: usize, ui: &mut egui::Ui, app: &mut BrowserApp, _tx: &mut Tx) {
        match step {
            0 => self.changes_step(ui, app),
            1 => {
                let edition = app
                    .game
                    .as_ref()
                    .and_then(|g| doctor_toolkit::edition::game_edition(g.root()));
                self.details_step(ui, edition);
            }
            _ => self.check_step(ui, app),
        }
    }

    fn check(&self, step: usize, _app: &BrowserApp) -> Result<(), String> {
        match step {
            0 if !self.items.iter().any(|i| i.chosen) => {
                Err(tr("wizard.package.need_changes").to_owned())
            }
            1 => {
                if self.info.name.trim().is_empty() {
                    return Err(tr("wizard.package.need_name").to_owned());
                }
                mods::validate_id(&self.info.id).map_err(|e| e.to_string())?;
                if let Some(e) = &self.preview_error {
                    return Err(e.clone());
                }
                Ok(())
            }
            2 => match &self.checked {
                Some(c) if !c.errors.is_empty() => Err(c.errors[0].clone()),
                None => Err(tr("wizard.package.need_check").to_owned()),
                _ => Ok(()),
            },
            _ => Ok(()),
        }
    }

    fn leave(&mut self, step: usize, app: &mut BrowserApp, _tx: &mut Tx) -> Result<(), String> {
        if step == 1 {
            self.run_checks(app);
        }
        Ok(())
    }

    fn summary(&self, _app: &BrowserApp) -> Vec<String> {
        let mut out = vec![
            format!(
                "{} ({}) {}",
                self.info.name, self.info.id, self.info.version
            ),
            trf("wizard.package.by", &[("author", &self.info.author)]),
        ];
        if let Some(c) = &self.checked {
            out.extend(c.notes.iter().cloned());
            out.extend(
                c.warnings
                    .iter()
                    .map(|w| trf("wizard.warning", &[("text", w)])),
            );
        }
        let n = self.items.iter().filter(|i| i.chosen).count();
        out.push(trf(
            "wizard.package.chosen",
            &[("count", &n), ("total", &self.items.len())],
        ));
        out
    }

    fn records_edits(&self) -> bool {
        false
    }

    fn record_label(&self) -> &'static str {
        tr("wizard.package.build")
    }

    fn record(&mut self, app: &mut BrowserApp, _tx: &mut Tx) -> Result<String, String> {
        let out = match self.out.clone() {
            Some(out) => out,
            None => {
                let mut dialog = rfd::FileDialog::new()
                    .add_filter(tr("wizard.package.zip"), &["zip"])
                    .set_file_name(format!("{}-{}.zip", self.info.id, self.info.version));
                if let Some(dir) = &app.settings.export_dirs.export {
                    dialog = dialog.set_directory(dir);
                }
                dialog
                    .save_file()
                    .ok_or_else(|| tr("wizard.package.no_file").to_owned())?
            }
        };
        let size = self.build(app, &out)?;
        self.built = Some((out.clone(), size));
        Ok(trf("wizard.package.label", &[("file", &out.display())]))
    }
}

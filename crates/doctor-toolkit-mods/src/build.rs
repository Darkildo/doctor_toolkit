//! Building a mod from the workspace: the section of the editor's Mods
//! window only.

use std::path::PathBuf;

use doctor_toolkit::mods::{self, ModInfo, Preview};
use doctor_toolkit::workspace::Workspace;

use crate::ModsPanel;
use crate::i18n::{tr, trf, trn};

/// Metadata typed into the build form.
pub(crate) struct BuildForm {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub url: String,
    pub preview: Option<PathBuf>,
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
        }
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
        let info = self.build.info();
        let preview = self.load_preview()?;
        let report = mods::build_from_workspace(ws, &info, None, preview.as_ref(), out)
            .map_err(|e| trf("mods.cannot_build", &[("error", &e)]))?;
        Ok(trf(
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
        ))
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
        let info = self.build.info();
        let report = manager
            .install_workspace(&game, ws, &info, None)
            .map_err(|e| trf("mods.cannot_install_workspace", &[("error", &e)]))?;
        self.selected = Some(report.id.clone());
        Ok(trf(
            "mods.installed_workspace",
            &[
                ("name", &report.name),
                (
                    "resources",
                    &trn("count.resources", report.resources as u64, &[]),
                ),
            ],
        ))
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
    }
}

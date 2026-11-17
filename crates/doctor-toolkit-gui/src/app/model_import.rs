//! "Import model…": a model file (glTF, FBX, OBJ, USD) over the selected
//! mesh or metamodel, or as a new model set; "Export model…" writes the
//! selected model in any of these formats. The window shows the plan (parts, records, bones mapped,
//! materials, warnings) and a GPU preview of the new mesh before anything is
//! recorded.

use std::path::PathBuf;

use doctor_toolkit::import::model;
use doctor_toolkit::resource::mesh::TYPE_MESH;
use doctor_toolkit::scene::{self, ModelFormat};
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::{Color32, RichText};

use super::BrowserApp;
use crate::model_view::ModelPreview;

/// State of the import window.
#[derive(Default)]
pub(crate) struct ModelImport {
    open: bool,
    /// `(stream game index, resource)` the import targets.
    target: Option<(usize, usize)>,
    path: Option<PathBuf>,
    as_new: bool,
    new_name: String,
    retarget: String,
    /// The stream a new model set goes into (of the game or new); the
    /// stream of the target when empty.
    into: String,
    /// New collision built from the imported geometry: `mesh`, `hull`,
    /// `box`; none when empty.
    collision: String,
    plan: Option<Result<model::ImportPlan, String>>,
    preview: Option<ModelPreview>,
    /// Counter that keeps preview meshes apart in the renderer cache.
    previews: u64,
    /// `DOCTOR_TOOLKIT_GUI_IMPORT` was used.
    preset_used: bool,
    /// Format of "Export model…".
    export_format: Option<ModelFormat>,
}

impl ModelImport {
    fn reset_plan(&mut self) {
        self.plan = None;
        self.preview = None;
    }
}

const TYPE_METAMODEL: u32 = doctor_toolkit::hash::sdbm(b"metamodel");

/// A file dialog for every model format the importer reads.
fn model_file_dialog() -> rfd::FileDialog {
    rfd::FileDialog::new()
        .add_filter(tr("import.filter_models"), &ModelFormat::READ_EXTENSIONS)
        .add_filter("glTF", &["glb", "gltf"])
        .add_filter("FBX", &["fbx"])
        .add_filter("Wavefront OBJ", &["obj"])
        .add_filter("USD", &["usd", "usda", "usdc", "usdz"])
}

impl BrowserApp {
    /// The format choice and "Export model…" button of a mesh resource.
    fn model_export_controls(&mut self, ui: &mut egui::Ui, loc: crate::nav::Location) {
        ui.horizontal(|ui| {
            let format = self
                .model_import
                .export_format
                .get_or_insert(ModelFormat::Glb);
            egui::ComboBox::from_id_salt("model_export_format")
                .selected_text(format.label())
                .show_ui(ui, |ui| {
                    for f in ModelFormat::WRITABLE {
                        ui.selectable_value(format, f, f.label());
                    }
                });
            let format = *format;
            let clicked = ui
                .button(tr("import.export_model"))
                .on_hover_text(tr("import.export_mesh_hint"))
                .clicked();
            if clicked {
                self.export_model(loc, format);
            }
        });
    }

    /// Writes the mesh at `loc` as a model file of `format`.
    fn export_model(&mut self, loc: crate::nav::Location, format: ModelFormat) {
        let Some(game) = self.game.clone() else {
            return;
        };
        let result = (|| -> Result<Option<String>, String> {
            let file = game.open_stream(loc.stream).map_err(|e| e.to_string())?;
            let layout = file.layout();
            let volatile = doctor_toolkit::resource::mesh::volatile_for(layout, loc.resource);
            let data = |r: usize| -> Result<Vec<u8>, String> {
                match &self.workspace {
                    Some(ws) => ws.current_data(&game, loc.stream, r),
                    None => file.resource_data(r),
                }
                .map_err(|e| e.to_string())
            };
            let mesh = data(loc.resource)?;
            let volatile = match volatile {
                Some(v) => data(v)?,
                None => Vec::new(),
            };
            let model = doctor_toolkit::resource::mesh::Model::parse(&mesh, &volatile)
                .map_err(|e| e.to_string())?;
            if self.assets.is_none()
                && let Some(index) = &self.index
            {
                self.assets = Some(doctor_toolkit::game::GameAssets::new(game.clone(), index));
            }
            let mut none = doctor_toolkit::export::gltf::NoAssets;
            let assets: &mut dyn doctor_toolkit::export::gltf::AssetSource = match &mut self.assets
            {
                Some(assets) => assets,
                None => &mut none,
            };
            let built = scene::game::model_scene(&model, assets).map_err(|e| e.to_string())?;
            let Some(path) = rfd::FileDialog::new()
                .set_title(tr("import.export_title"))
                .add_filter(format.label(), &[format.extension()])
                .set_file_name(format!(
                    "{}.{}",
                    scene::file_name(&model.name),
                    format.extension()
                ))
                .save_file()
            else {
                return Ok(None);
            };
            let written =
                scene::write_file(&built, &path, Some(format)).map_err(|e| e.to_string())?;
            let mut message = trf(
                "model.exported",
                &[
                    ("path", &path.display()),
                    ("files", &trn("count.files", written.len() as u64, &[])),
                ],
            );
            if !format.has_skeletons() && !built.joints.is_empty() {
                message += tr("import.bind_pose_note");
            }
            Ok(Some(message))
        })();
        match result {
            Ok(Some(message)) => self.status = message,
            Ok(None) => self.status = tr("model.export_cancelled").into(),
            Err(error) => self.notify(
                super::Level::Error,
                trf("model.cannot_export", &[("error", &error)]),
            ),
        }
    }

    /// The "Import model…" button next to the model preview of a mesh or
    /// metamodel resource.
    pub(super) fn model_import_button(
        &mut self,
        ui: &mut egui::Ui,
        loc: crate::nav::Location,
        type_id: u32,
    ) {
        if type_id != TYPE_MESH && type_id != TYPE_METAMODEL {
            return;
        }
        let target = (loc.stream, loc.resource);
        // `DOCTOR_TOOLKIT_GUI_IMPORT=<file>` opens the window once, for screenshots.
        let preset = std::env::var_os("DOCTOR_TOOLKIT_GUI_IMPORT")
            .filter(|_| !self.model_import.preset_used)
            .map(PathBuf::from);
        let clicked = ui
            .button(tr("import.import_model"))
            .on_hover_text(tr("import.import_model_hint"))
            .clicked();
        if type_id == TYPE_MESH {
            self.model_export_controls(ui, loc);
        }
        if clicked || preset.is_some() {
            if self.model_import.target != Some(target) {
                self.model_import = ModelImport {
                    previews: self.model_import.previews,
                    preset_used: self.model_import.preset_used,
                    ..Default::default()
                };
            }
            self.model_import.target = Some(target);
            self.model_import.open = true;
            self.model_import.preset_used = true;
            let path = preset.or_else(|| model_file_dialog().pick_file());
            if let Some(path) = path {
                self.model_import.path = Some(path);
                self.model_import.reset_plan();
                self.plan_model_import();
            }
        }
    }

    /// Works out the import with the current options.
    fn plan_model_import(&mut self) {
        let (Some(game), Some(index)) = (self.game.clone(), self.index.clone()) else {
            return;
        };
        let state = &self.model_import;
        let (Some((stream, resource)), Some(path)) = (state.target, state.path.clone()) else {
            return;
        };
        let retarget: Result<Vec<usize>, _> = state
            .retarget
            .split([',', ' '])
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.trim().parse::<usize>())
            .collect();
        let options = model::ImportOptions {
            new_name: (state.as_new && !state.new_name.trim().is_empty())
                .then(|| state.new_name.trim().to_owned()),
            retarget: retarget.unwrap_or_default(),
            format: None,
            collision: match doctor_toolkit::zone_ops::parse_collision(Some(&state.collision)) {
                Ok(kind) => kind,
                Err(error) => {
                    self.model_import.plan = Some(Err(error.to_string()));
                    return;
                }
            },
        };
        let into =
            (state.as_new && !state.into.trim().is_empty()).then(|| state.into.trim().to_owned());
        if self.assets.is_none() {
            self.assets = Some(doctor_toolkit::game::GameAssets::new(game.clone(), &index));
        }
        // The plan reads the workspace version of the target.
        if let Err(error) = self.workspace_mut() {
            self.model_import.plan = Some(Err(error));
            return;
        }
        let result = scene::read_file(&path)
            .map_err(|e| e.to_string())
            .and_then(|gltf| {
                let ws = self.workspace.as_ref().ok_or(tr("import.no_workspace"))?;
                let assets = self.assets.as_mut().expect("created above");
                let mut plan =
                    model::plan(&game, ws, &index, assets, stream, resource, &gltf, &options)
                        .map_err(|e| e.to_string())?;
                if let Some(into) = &into {
                    let into = doctor_toolkit::zone_ops::any_stream(&game, ws, into)
                        .map_err(|e| e.to_string())?;
                    plan.put_into(&game, ws, &into).map_err(|e| e.to_string())?;
                }
                Ok(plan)
            });
        self.model_import.preview = None;
        if let Ok(plan) = &result {
            match plan.preview() {
                Ok(parsed) => {
                    self.model_import.previews += 1;
                    let key = u64::MAX - self.model_import.previews;
                    let game_assets = self.assets.as_mut().expect("created above");
                    // New materials and textures of the plan show in the preview.
                    let mut assets = plan.assets(game_assets);
                    self.model_import.preview = Some(ModelPreview::from_model(
                        &parsed,
                        key,
                        &mut assets,
                        self.gpu.as_ref(),
                    ));
                }
                Err(error) => {
                    self.model_import.plan =
                        Some(Err(trf("import.preview_error", &[("error", &error)])));
                    return;
                }
            }
        }
        self.model_import.plan = Some(result);
    }

    /// The import window.
    pub(super) fn show_model_import(&mut self, ctx: &egui::Context) {
        if !self.model_import.open {
            return;
        }
        let mut open = true;
        let mut replan = false;
        let mut record = false;
        let new_streams = self.new_stream_names();
        egui::Window::new(tr("import.title"))
            .open(&mut open)
            .default_size([760.0, 720.0])
            .resizable(true)
            .show(ctx, |ui| {
                let state = &mut self.model_import;
                ui.horizontal(|ui| {
                    ui.label(tr("import.file"));
                    ui.monospace(
                        state
                            .path
                            .as_ref()
                            .map_or(tr("import.no_file").into(), |p| p.display().to_string()),
                    );
                    if ui.button(tr("prefs.choose")).clicked()
                        && let Some(path) = model_file_dialog().pick_file()
                    {
                        state.path = Some(path);
                        replan = true;
                    }
                });
                ui.horizontal(|ui| {
                    replan |= ui
                        .radio_value(&mut state.as_new, false, tr("import.replace"))
                        .on_hover_text(tr("import.replace_hint"))
                        .changed();
                    replan |= ui
                        .radio_value(&mut state.as_new, true, tr("import.new_set"))
                        .on_hover_text(tr("import.new_set_hint"))
                        .changed();
                });
                if state.as_new {
                    ui.horizontal(|ui| {
                        ui.label(tr("import.name"));
                        replan |= ui.text_edit_singleline(&mut state.new_name).lost_focus();
                        ui.label(tr("import.retarget"));
                        replan |= ui
                            .text_edit_singleline(&mut state.retarget)
                            .on_hover_text(tr("import.retarget_hint"))
                            .lost_focus();
                    });
                    if state.new_name.trim().is_empty() {
                        ui.colored_label(Color32::YELLOW, tr("import.need_name"));
                    }
                    ui.horizontal(|ui| {
                        ui.label(tr("import.into"));
                        replan |= ui
                            .add(
                                egui::TextEdit::singleline(&mut state.into)
                                    .hint_text(tr("import.into_default"))
                                    .desired_width(260.0),
                            )
                            .on_hover_text(tr("import.into_hint"))
                            .lost_focus();
                        if !new_streams.is_empty() {
                            ui.menu_button("…", |ui| {
                                for name in &new_streams {
                                    if ui.button(name).clicked() {
                                        state.into = name.clone();
                                        replan = true;
                                        ui.close();
                                    }
                                }
                            });
                        }
                    });
                }
                ui.horizontal(|ui| {
                    ui.label(tr("import.collision"));
                    let label = |kind: &str| match kind {
                        "mesh" => tr("import.collision_mesh"),
                        "hull" => tr("import.collision_hull"),
                        "box" => tr("import.collision_box"),
                        _ => tr("import.collision_none"),
                    };
                    egui::ComboBox::from_id_salt("model_import_collision")
                        .selected_text(label(&state.collision))
                        .show_ui(ui, |ui| {
                            for kind in ["", "mesh", "hull", "box"] {
                                replan |= ui
                                    .selectable_value(
                                        &mut state.collision,
                                        kind.to_owned(),
                                        label(kind),
                                    )
                                    .changed();
                            }
                        })
                        .response
                        .on_hover_text(tr("import.collision_hint"));
                });
                ui.separator();
                match &state.plan {
                    None => {
                        ui.label(RichText::new(tr("import.choose_file")).weak());
                    }
                    Some(Err(error)) => {
                        ui.colored_label(Color32::RED, error);
                    }
                    Some(Ok(plan)) => {
                        summary(ui, plan);
                        ui.horizontal(|ui| {
                            let ready = !state.as_new || !state.new_name.trim().is_empty();
                            record = ui
                                .add_enabled(ready, egui::Button::new(tr("import.record")))
                                .on_hover_text(tr("import.record_hint"))
                                .clicked();
                            ui.label(RichText::new(&plan.label).weak().small());
                        });
                    }
                }
                if let Some(preview) = &mut state.preview {
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .id_salt("model import preview")
                        .show(ui, |ui| preview.show(ui));
                }
            });
        if !open {
            self.model_import.open = false;
        }
        if replan && self.model_import.path.is_some() {
            self.plan_model_import();
        }
        if record {
            self.record_model_import();
        }
    }

    fn record_model_import(&mut self) {
        let Some(game) = self.game.clone() else {
            return;
        };
        let Some(Ok(plan)) = self.model_import.plan.take() else {
            return;
        };
        let result = self
            .workspace_mut()
            .and_then(|ws| plan.commit(ws, &game).map_err(|e| e.to_string()));
        self.status = match result {
            Ok(Some(_)) => trf("wizard.recorded", &[("label", &plan.label)]),
            Ok(None) => tr("import.no_change").to_owned(),
            Err(error) => trf("import.cannot_record", &[("error", &error)]),
        };
        self.model_import.open = false;
        self.model_import.preview = None;
        self.after_edit(self.model_import.target.map(|(stream, _)| stream));
    }
}

fn summary(ui: &mut egui::Ui, plan: &model::ImportPlan) {
    let s = &plan.summary;
    let mut header = trf(
        "import.summary",
        &[
            ("mode", &s.mode),
            ("parts", &trn("count.parts", s.parts.len() as u64, &[])),
            (
                "records",
                &trn("import.draw_records", s.records as u64, &[]),
            ),
            ("reused", &s.materials_reused),
            ("new", &s.materials_new),
            (
                "textures",
                &trn("count.textures", s.textures_imported as u64, &[]),
            ),
        ],
    );
    if let Some(skeleton) = &s.skeleton {
        header += &trf(
            "import.summary_skeleton",
            &[
                ("name", skeleton),
                ("bones", &trn("count.bones", s.bones_mapped as u64, &[])),
            ],
        );
    }
    ui.label(header);
    egui::Grid::new("model import parts")
        .striped(true)
        .num_columns(5)
        .show(ui, |ui| {
            for title in [
                "model.part",
                "import.kind",
                "import.vertices",
                "import.triangles",
                "model.material",
            ] {
                ui.strong(tr(title));
            }
            ui.end_row();
            for p in &s.parts {
                ui.label(&p.name);
                ui.label(&p.kind);
                ui.label(p.vertices.to_string());
                ui.label(p.triangles.to_string());
                if p.new_material {
                    ui.colored_label(Color32::LIGHT_BLUE, &p.material);
                } else {
                    ui.label(&p.material);
                }
                ui.end_row();
            }
        });
    egui::CollapsingHeader::new(trf(
        "import.resources",
        &[
            ("meshes", &trn("count.meshes", s.meshes.len() as u64, &[])),
            ("added", &s.added.len()),
            ("retargeted", &s.retargeted.len()),
        ],
    ))
    .id_salt("model import resources")
    .show(ui, |ui| {
        for line in s
            .meshes
            .iter()
            .map(|m| format!("mesh {m}"))
            .chain(
                s.added
                    .iter()
                    .map(|a| trf("import.line_add", &[("what", a)])),
            )
            .chain(
                s.retargeted
                    .iter()
                    .map(|r| trf("import.line_retarget", &[("what", r)])),
            )
        {
            ui.monospace(line);
        }
    });
    for w in &s.warnings {
        ui.colored_label(Color32::YELLOW, format!("⚠ {w}"));
    }
}

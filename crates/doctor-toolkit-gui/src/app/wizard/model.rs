//! Replace a model (a weapon, a prop, a car): pick the model, export it as
//! a reference for the 3D program, choose the new file, read the import
//! plan (parts, bones, materials, warnings, collision) next to a preview,
//! and record it (`doctor_toolkit::import::model`, `docs/model-import.md`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use doctor_toolkit::external::{self, EditorRole, Format, Target};
use doctor_toolkit::import::model::{self, ImportOptions, ImportPlan};
use doctor_toolkit::resource::hkx::collision::CollisionKind;
use doctor_toolkit::resource::mesh::TYPE_MESH;
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::{Color32, RichText};

use super::retexture::{TYPE_METAMODEL, find_metamodel};
use super::{Picker, Start, Task, Tx, Watched};
use crate::app::BrowserApp;
use crate::model_view::ModelPreview;
use crate::nav::Location;

/// Keeps the preview meshes of wizards apart in the renderer cache.
static PREVIEWS: AtomicU64 = AtomicU64::new(0);

/// A renderer cache key for a new preview mesh.
pub(super) fn preview_key() -> u64 {
    u64::MAX / 2 - PREVIEWS.fetch_add(1, Ordering::Relaxed)
}

/// File extensions of the model files `model-import` reads.
pub(super) const MODEL_EXTENSIONS: &[&str] = &["glb", "gltf"];

/// Reads a model file for the importer.
pub(super) fn read_model(path: &Path) -> Result<doctor_toolkit::import::gltf::GltfModel, String> {
    doctor_toolkit::import::gltf::read_file(path).map_err(|e| e.to_string())
}

/// Asks for a model file.
pub(super) fn pick_model_file(app: &BrowserApp) -> Option<PathBuf> {
    let mut dialog =
        rfd::FileDialog::new().add_filter(tr("wizard.model.files_filter"), MODEL_EXTENSIONS);
    if let Some(dir) = &app.settings.export_dirs.import {
        dialog = dialog.set_directory(dir);
    }
    dialog.pick_file()
}

/// What a plan says, for the plan step and the summary.
pub(super) fn plan_lines(plan: &ImportPlan) -> Vec<String> {
    let s = &plan.summary;
    let mut out = vec![trf(
        "wizard.model.plan_head",
        &[
            ("parts", &trn("count.parts", s.parts.len() as u64, &[])),
            ("records", &trn("count.draw_records", s.records as u64, &[])),
            ("reused", &s.materials_reused),
            ("new", &s.materials_new),
            (
                "textures",
                &trn("count.textures", s.textures_imported as u64, &[]),
            ),
        ],
    )];
    if let Some(skeleton) = &s.skeleton {
        out.push(trf(
            "wizard.model.plan_skeleton",
            &[
                ("skeleton", skeleton),
                ("bones", &trn("count.bones", s.bones_mapped as u64, &[])),
            ],
        ));
    }
    for p in &s.parts {
        out.push(trf(
            "wizard.model.plan_part",
            &[
                ("name", &p.name),
                ("kind", &p.kind),
                ("vertices", &trn("count.vertices", p.vertices as u64, &[])),
                (
                    "triangles",
                    &trn("count.triangles", p.triangles as u64, &[]),
                ),
                ("material", &p.material),
            ],
        ));
    }
    if let Some(c) = &s.collision {
        out.push(trf(
            "wizard.model.plan_collision",
            &[
                ("kind", &format!("{:?}", c.kind).to_lowercase()),
                (
                    "triangles",
                    &trn("count.triangles", c.triangles as u64, &[]),
                ),
            ],
        ));
    }
    if !s.copies.is_empty() {
        const SHOWN: usize = 4;
        let mut streams = s.copies[..s.copies.len().min(SHOWN)].join(", ");
        if s.copies.len() > SHOWN {
            streams.push_str(", …");
        }
        out.push(trf(
            "wizard.model.plan_copies",
            &[("count", &s.copies.len()), ("streams", &streams)],
        ));
    }
    let mut keys: Vec<&String> = Vec::new();
    for m in s.meshes.iter().chain(&s.added).chain(&s.retargeted) {
        if !keys.contains(&m) {
            keys.push(m);
            out.push(format!("  {m}"));
        }
    }
    out
}

/// A plan's warnings, in yellow.
pub(super) fn plan_ui(ui: &mut egui::Ui, plan: &ImportPlan) {
    for line in plan_lines(plan) {
        ui.label(line);
    }
    for w in &plan.summary.warnings {
        ui.colored_label(Color32::YELLOW, trf("wizard.warning", &[("text", w)]));
    }
}

/// The collision choices of the importer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Collision {
    Keep,
    Build(CollisionKind),
}

pub(crate) struct ReplaceModel {
    pub picker: Picker,
    pub target: Option<Location>,
    /// The reference model written for the 3D program.
    pub reference: Option<Watched>,
    pub file: Option<PathBuf>,
    pub collision: Collision,
    pub plan: Option<Result<ImportPlan, String>>,
    preview: Option<ModelPreview>,
    pub message: Option<String>,
}

impl ReplaceModel {
    pub fn new(app: &mut BrowserApp, start: &Start) -> Self {
        let target = start
            .loc
            .filter(|&l| matches!(app.type_of(l), Some(TYPE_MESH | TYPE_METAMODEL)))
            .or_else(|| start.model.and_then(|id| find_metamodel(app, id)));
        Self {
            picker: Picker {
                text: "weapon".into(),
                ..Default::default()
            },
            target,
            reference: None,
            file: None,
            collision: Collision::Keep,
            plan: None,
            preview: None,
            message: None,
        }
    }

    pub fn preset(&mut self, key: &str, value: &str, app: &mut BrowserApp) {
        match key {
            "file" => self.set_file(app, PathBuf::from(value)),
            "collision" => {
                self.collision = value
                    .parse()
                    .map(Collision::Build)
                    .unwrap_or(Collision::Keep);
            }
            "search" => self.picker.text = value.to_owned(),
            _ => {}
        }
    }

    pub fn set_file(&mut self, app: &mut BrowserApp, path: PathBuf) {
        if let Some(dir) = path.parent() {
            app.settings.export_dirs.import = Some(dir.to_owned());
        }
        self.file = Some(path);
        self.plan(app);
    }

    /// Works the import out with the current options.
    pub fn plan(&mut self, app: &mut BrowserApp) {
        self.preview = None;
        let (Some(loc), Some(path)) = (self.target, self.file.clone()) else {
            self.plan = None;
            return;
        };
        let options = ImportOptions {
            collision: match self.collision {
                Collision::Keep => None,
                Collision::Build(kind) => Some(kind),
            },
            ..Default::default()
        };
        let result = make_plan(app, loc, &path, &options);
        if let Ok(plan) = &result {
            self.preview = preview(app, plan);
        }
        self.plan = Some(result);
    }

    fn target_step(&mut self, ui: &mut egui::Ui, app: &mut BrowserApp) {
        ui.label(tr("wizard.model.pick"));
        let mut target = self.target;
        if app.picker_ui(
            ui,
            &mut self.picker,
            &[TYPE_METAMODEL, TYPE_MESH],
            &mut target,
        ) {
            self.target = target;
            self.reference = None;
            self.plan = None;
        }
        ui.label(RichText::new(tr("wizard.model.pick_hint")).weak().small());
    }

    fn reference_step(&mut self, ui: &mut egui::Ui, app: &mut BrowserApp) {
        ui.label(tr("wizard.model.reference"));
        ui.horizontal_wrapped(|ui| {
            if ui.button(tr("wizard.model.export_reference")).clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("glTF", &["glb"])
                    .set_file_name("reference.glb")
                    .save_file()
            {
                self.message = Some(match self.export_reference(app, path) {
                    Ok(path) => trf("wizard.model.exported", &[("file", &path.display())]),
                    Err(e) => e,
                });
            }
            if ui
                .button(tr("wizard.model.open_blender"))
                .on_hover_text(tr("wizard.model.open_blender_hint"))
                .clicked()
            {
                let path = super::work_dir("model").join("reference.glb");
                match self.export_reference(app, path) {
                    Ok(path) => app.wizard_open_editor(EditorRole::Model, &path),
                    Err(e) => self.message = Some(e),
                }
            }
        });
        if let Some(message) = &self.message {
            ui.label(RichText::new(message).weak());
        }
        ui.label(
            RichText::new(tr("wizard.model.reference_hint"))
                .weak()
                .small(),
        );
    }

    /// Writes the target as GLB to `path` and watches it: a save becomes the new file.
    pub fn export_reference(
        &mut self,
        app: &mut BrowserApp,
        path: PathBuf,
    ) -> Result<PathBuf, String> {
        let loc = self.target.ok_or(tr("wizard.no_model"))?;
        let (Some(game), Some(index)) = (app.game.clone(), app.index.clone()) else {
            return Err(tr("edits.no_game").into());
        };
        let target = Target::at(&game, loc.stream, loc.resource, Some(Format::Glb))
            .map_err(|e| e.to_string())?;
        let assets = app
            .assets
            .get_or_insert_with(|| doctor_toolkit::game::GameAssets::new(game.clone(), &index));
        let glb = external::export(&game, app.workspace.as_ref(), &target, Some(assets))
            .map_err(|e| e.to_string())?;
        let watched = Watched::write(path, &glb)?;
        let path = watched.path.clone();
        self.reference = Some(watched);
        Ok(path)
    }

    fn file_step(&mut self, ui: &mut egui::Ui, app: &mut BrowserApp) {
        if let Some(bytes) = self.reference.as_mut().and_then(Watched::poll)
            && !bytes.is_empty()
            && let Some(path) = self.reference.as_ref().map(|r| r.path.clone())
        {
            self.set_file(app, path);
        }
        if self.reference.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(500));
        }
        ui.label(tr("wizard.model.file"));
        ui.horizontal_wrapped(|ui| {
            if ui.button(tr("wizard.model.choose_file")).clicked()
                && let Some(path) = pick_model_file(app)
            {
                self.set_file(app, path);
            }
            if let Some(file) = &self.file {
                ui.monospace(file.display().to_string());
            }
        });
        let is_metamodel = self.target.and_then(|l| app.type_of(l)) == Some(TYPE_METAMODEL);
        ui.add_enabled_ui(is_metamodel, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(tr("wizard.model.collision"));
                let before = self.collision;
                ui.radio_value(
                    &mut self.collision,
                    Collision::Keep,
                    tr("wizard.model.collision_keep"),
                );
                for (kind, key) in [
                    (CollisionKind::Mesh, "wizard.model.collision_mesh"),
                    (CollisionKind::Hull, "wizard.model.collision_hull"),
                    (CollisionKind::Box, "wizard.model.collision_box"),
                ] {
                    ui.radio_value(&mut self.collision, Collision::Build(kind), tr(key));
                }
                if before != self.collision && self.file.is_some() {
                    self.plan(app);
                }
            });
        });
        if !is_metamodel {
            ui.label(
                RichText::new(tr("wizard.model.collision_mesh_only"))
                    .weak()
                    .small(),
            );
        }
        if let Some(Err(error)) = &self.plan {
            ui.colored_label(Color32::from_rgb(230, 90, 80), error);
        }
    }

    fn plan_step(&mut self, ui: &mut egui::Ui) {
        let Some(Ok(plan)) = &self.plan else {
            ui.label(tr("wizard.model.no_plan"));
            return;
        };
        egui::Panel::left("wizard-model-plan")
            .resizable(true)
            .default_size(360.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("wizard-model-plan-lines")
                    .show(ui, |ui| plan_ui(ui, plan));
            });
        egui::CentralPanel::default().show(ui, |ui| match &mut self.preview {
            Some(preview) => preview.show(ui),
            None => {
                ui.label(RichText::new(tr("wizard.model.no_preview")).weak());
            }
        });
    }
}

/// Plans the import of the file at `path` over resource `loc`.
pub(super) fn make_plan(
    app: &mut BrowserApp,
    loc: Location,
    path: &Path,
    options: &ImportOptions,
) -> Result<ImportPlan, String> {
    let (Some(game), Some(index)) = (app.game.clone(), app.index.clone()) else {
        return Err(tr("edits.no_game").into());
    };
    let gltf = read_model(path)?;
    // The plan reads the workspace version of the target.
    app.workspace_mut()?;
    let ws = app.workspace.as_ref().ok_or(tr("wizard.no_workspace"))?;
    let assets = app
        .assets
        .get_or_insert_with(|| doctor_toolkit::game::GameAssets::new(game.clone(), &index));
    model::plan(
        &game,
        ws,
        &index,
        assets,
        loc.stream,
        loc.resource,
        &gltf,
        options,
    )
    .map_err(|e| e.to_string())
}

/// A preview of the new mesh of a plan, with its new materials.
pub(super) fn preview(app: &mut BrowserApp, plan: &ImportPlan) -> Option<ModelPreview> {
    let parsed = plan.preview().ok()?;
    let gpu = app.gpu.clone();
    let assets = app.assets.as_mut()?;
    let mut assets = plan.assets(assets);
    Some(ModelPreview::from_model(
        &parsed,
        preview_key(),
        &mut assets,
        gpu.as_ref(),
    ))
}

impl Task for ReplaceModel {
    fn steps(&self) -> &'static [&'static str] {
        &[
            "wizard.model.step.target",
            "wizard.model.step.reference",
            "wizard.model.step.file",
            "wizard.model.step.plan",
        ]
    }

    fn ui(&mut self, step: usize, ui: &mut egui::Ui, app: &mut BrowserApp, _tx: &mut Tx) {
        match step {
            0 => self.target_step(ui, app),
            1 => self.reference_step(ui, app),
            2 => self.file_step(ui, app),
            _ => self.plan_step(ui),
        }
    }

    fn check(&self, step: usize, _app: &BrowserApp) -> Result<(), String> {
        match step {
            0 if self.target.is_none() => Err(tr("wizard.model.need_target").to_owned()),
            2 => match &self.plan {
                None => Err(tr("wizard.model.need_file").to_owned()),
                Some(Err(e)) => Err(e.clone()),
                Some(Ok(_)) => Ok(()),
            },
            _ => Ok(()),
        }
    }

    fn summary(&self, app: &BrowserApp) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(loc) = self.target {
            out.push(trf(
                "wizard.model.summary_target",
                &[("model", &app.location_label(loc))],
            ));
        }
        if let Some(file) = &self.file {
            out.push(trf(
                "wizard.model.summary_file",
                &[("file", &file.display())],
            ));
        }
        if let Some(Ok(plan)) = &self.plan {
            out.extend(plan_lines(plan));
            out.extend(
                plan.summary
                    .warnings
                    .iter()
                    .map(|w| trf("wizard.warning", &[("text", w)])),
            );
        }
        out
    }

    fn record(&mut self, app: &mut BrowserApp, _tx: &mut Tx) -> Result<String, String> {
        let Some(Ok(plan)) = &self.plan else {
            return Err(tr("wizard.model.need_file").to_owned());
        };
        let game = app.game.clone().ok_or(tr("edits.no_game"))?;
        plan.commit(app.workspace_mut()?, &game)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| tr("wizard.model.no_change").to_owned())?;
        Ok(plan.label.clone())
    }
}

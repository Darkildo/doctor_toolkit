//! Place an object in a level: choose a model of the game (or import a file
//! as a new model set), then load the zone in the level view of the wizard,
//! add the object in front of the camera and move it with the gizmo. The
//! level view records as it goes; the wizard joins it all into one operation
//! (Cancel takes it back).

use std::path::PathBuf;

use doctor_toolkit::import::model::{ImportOptions, ImportPlan};
use doctor_toolkit::workspace::ResourceKey;
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::{Color32, RichText};

use super::model::{make_plan, pick_model_file, plan_ui};
use super::retexture::{TYPE_METAMODEL, find_metamodel};
use super::{Picker, Start, Task, Tx};
use crate::app::BrowserApp;
use crate::nav::Location;

pub(crate) struct Place {
    pub picker: Picker,
    /// The metamodel to show (or the template of an imported model).
    pub model: Option<Location>,
    /// A model file to import as a new model set.
    pub file: Option<PathBuf>,
    pub new_name: String,
    pub plan: Option<Result<ImportPlan, String>>,
    /// Clone the metamodel into the zone (needed when it is elsewhere).
    pub clone: bool,
    /// Give the clone its own collision built from its meshes.
    pub collision: bool,
    /// Id of the level view of the wizard (in `BrowserApp::levels`).
    pub level: u64,
    /// History length when the task started recording, and after the
    /// import, before anything was placed.
    pub started: Option<usize>,
    pub placed_from: Option<usize>,
    /// The model to fill into the add panel of the level view.
    prefill: Option<u32>,
    /// Zones to load in the level view (screenshot preset).
    zones: Vec<String>,
}

impl Place {
    pub fn new(app: &mut BrowserApp, start: &Start) -> Self {
        let model = start
            .loc
            .filter(|&l| app.type_of(l) == Some(TYPE_METAMODEL))
            .or_else(|| start.model.and_then(|id| find_metamodel(app, id)));
        Self {
            picker: Picker::default(),
            model,
            file: None,
            new_name: String::new(),
            plan: None,
            clone: true,
            collision: false,
            level: app.new_pane_id(),
            started: None,
            placed_from: None,
            prefill: None,
            zones: Vec::new(),
        }
    }

    pub fn preset(&mut self, key: &str, value: &str, app: &mut BrowserApp) {
        match key {
            "model" => {
                if let Ok(id) = u32::from_str_radix(value, 16) {
                    self.model = find_metamodel(app, id);
                }
            }
            "search" => self.picker.text = value.to_owned(),
            "file" => self.set_file(app, PathBuf::from(value)),
            "collision" => self.collision = true,
            "zones" => self.zones = value.split('+').map(str::to_owned).collect(),
            _ => {}
        }
    }

    fn set_file(&mut self, app: &mut BrowserApp, path: PathBuf) {
        if self.new_name.is_empty() {
            self.new_name = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_else(|| "new_model".into());
        }
        self.file = Some(path);
        self.plan(app);
    }

    fn plan(&mut self, app: &mut BrowserApp) {
        let (Some(loc), Some(path)) = (self.model, self.file.clone()) else {
            self.plan = None;
            return;
        };
        let options = ImportOptions {
            new_name: Some(self.new_name.clone()),
            ..Default::default()
        };
        self.plan = Some(make_plan(app, loc, &path, &options));
    }

    /// Id of the metamodel the new object shows: the new one of the import, or
    /// the chosen one.
    fn model_id(&self, app: &BrowserApp) -> Option<u32> {
        if let Some(Ok(plan)) = &self.plan {
            return plan
                .summary
                .added
                .iter()
                .filter_map(|k| k.parse::<ResourceKey>().ok())
                .find(|k| k.type_id == TYPE_METAMODEL)
                .map(|k| k.ids[3]);
        }
        let loc = self.model?;
        let index = app.index.as_ref()?;
        index
            .entries
            .iter()
            .find(|e| e.stream as usize == loc.stream && e.resource as usize == loc.resource)
            .map(|e| e.header.ids[3])
    }

    fn model_step(&mut self, ui: &mut egui::Ui, app: &mut BrowserApp) {
        ui.label(tr("wizard.place.pick"));
        let mut model = self.model;
        if app.picker_ui(ui, &mut self.picker, &[TYPE_METAMODEL], &mut model) {
            self.model = model;
            if self.file.is_some() {
                self.plan(app);
            }
        }
        ui.separator();
        ui.label(tr("wizard.place.import"));
        ui.horizontal_wrapped(|ui| {
            if ui.button(tr("wizard.model.choose_file")).clicked()
                && let Some(path) = pick_model_file(app)
            {
                self.set_file(app, path);
            }
            if let Some(file) = &self.file {
                ui.monospace(file.display().to_string());
                if ui.small_button("×").clicked() {
                    self.file = None;
                    self.plan = None;
                }
            }
        });
        if self.file.is_some() {
            ui.horizontal(|ui| {
                ui.label(tr("wizard.place.new_name"));
                if ui.text_edit_singleline(&mut self.new_name).lost_focus() {
                    self.plan(app);
                }
            });
        }
        match &self.plan {
            Some(Ok(plan)) => {
                egui::ScrollArea::vertical()
                    .id_salt("place-plan")
                    .max_height(200.0)
                    .show(ui, |ui| plan_ui(ui, plan));
            }
            Some(Err(error)) => {
                ui.colored_label(Color32::from_rgb(230, 90, 80), error);
            }
            None => {}
        }
        ui.separator();
        ui.add_enabled_ui(self.file.is_none(), |ui| {
            ui.checkbox(&mut self.clone, tr("wizard.place.clone"))
                .on_hover_text(tr("wizard.place.clone_hint"));
            ui.add_enabled(
                self.clone,
                egui::Checkbox::new(&mut self.collision, tr("wizard.place.collision")),
            )
            .on_hover_text(tr("wizard.place.collision_hint"));
        });
    }

    fn place_step(&mut self, ui: &mut egui::Ui, app: &mut BrowserApp) {
        ui.label(RichText::new(tr("wizard.place.how")).weak());
        self.prepare_level(app, ui.ctx());
        let placed = self.placed(app);
        if placed > 0 {
            ui.label(RichText::new(trn("wizard.place.placed_n", placed as u64, &[])).strong());
        }
        app.level_with_panels(ui, self.level);
    }

    /// Edits recorded in the level view since the model was ready.
    pub fn placed(&self, app: &BrowserApp) -> usize {
        match (self.placed_from, &app.workspace) {
            (Some(from), Some(ws)) => ws.head().saturating_sub(from),
            _ => 0,
        }
    }

    /// Makes the level view of the wizard and fills in its add panel.
    fn prepare_level(&mut self, app: &mut BrowserApp, ctx: &egui::Context) {
        if !app.levels.contains_key(&self.level) {
            let mut level = crate::level_view::LevelView::new(app.gpu.clone(), ctx);
            level.forget_preset();
            if !self.zones.is_empty() {
                level.load_on_open(std::mem::take(&mut self.zones));
            }
            if let (Some(game), Some(index)) = (&app.game, &app.index) {
                level.open_game(game.clone(), index.clone());
            }
            app.levels.insert(self.level, level);
        }
        if let Some(model) = self.prefill.take() {
            let clone = self.clone && self.file.is_none();
            if let Some(level) = app.levels.get_mut(&self.level) {
                level.prefill_add(model, clone, self.collision);
            }
        }
    }
}

impl Task for Place {
    fn steps(&self) -> &'static [&'static str] {
        &["wizard.place.step.model", "wizard.place.step.place"]
    }

    fn ui(&mut self, step: usize, ui: &mut egui::Ui, app: &mut BrowserApp, _tx: &mut Tx) {
        match step {
            0 => self.model_step(ui, app),
            _ => self.place_step(ui, app),
        }
    }

    fn check(&self, step: usize, app: &BrowserApp) -> Result<(), String> {
        match step {
            0 => {
                if self.model.is_none() {
                    return Err(tr("wizard.place.need_model").to_owned());
                }
                if let Some(Err(e)) = &self.plan {
                    return Err(e.clone());
                }
                Ok(())
            }
            _ if self.placed(app) == 0 => Err(tr("wizard.place.need_place").to_owned()),
            _ => Ok(()),
        }
    }

    fn leave(&mut self, step: usize, app: &mut BrowserApp, tx: &mut Tx) -> Result<(), String> {
        if step != 0 || self.placed_from.is_some() {
            return Ok(());
        }
        let model = self
            .model_id(app)
            .ok_or_else(|| tr("wizard.place.need_model").to_owned())?;
        // From here on the level view records; Cancel takes it back.
        tx.begin(app)?;
        self.started = tx.mark;
        if let Some(Ok(plan)) = &self.plan {
            let game = app.game.clone().ok_or(tr("edits.no_game"))?;
            plan.commit(app.workspace_mut()?, &game)
                .map_err(|e| e.to_string())?;
            app.after_edit(None);
        }
        self.placed_from = Some(app.workspace_mut()?.head());
        self.prefill = Some(model);
        Ok(())
    }

    fn summary(&self, app: &BrowserApp) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(id) = self.model_id(app) {
            out.push(trf(
                "wizard.place.summary_model",
                &[("id", &format!("{id:08x}"))],
            ));
        }
        if let (Some(ws), Some(from)) = (&app.workspace, self.started) {
            for op in ws.operations().iter().take(ws.head()).skip(from) {
                out.push(op.label.clone());
            }
        }
        out
    }

    fn record(&mut self, app: &mut BrowserApp, _tx: &mut Tx) -> Result<String, String> {
        let placed = self.placed(app);
        if placed == 0 {
            return Err(tr("wizard.place.need_place").to_owned());
        }
        let id = self.model_id(app).unwrap_or_default();
        Ok(trf(
            "wizard.place.label",
            &[
                ("count", &trn("count.edits", placed as u64, &[])),
                ("id", &format!("{id:08x}")),
            ],
        ))
    }
}

/// The level view ids wizards use.
pub(super) fn level_of(state: &super::State) -> Option<u64> {
    match state {
        super::State::Place(p) => Some(p.level),
        _ => None,
    }
}

//! New outfit: pick a parts database, a slot and the sets that may wear it;
//! import a mesh skinned to the human skeleton as a new model set (cloned
//! from a part of that slot); give its new materials tint colours; name the
//! part and its variant; look at the character wearing it; record the mesh
//! and the parts database edit as one operation (`partsdb add-part`).

use std::collections::HashMap;
use std::path::PathBuf;

use doctor_toolkit::character::MeshLocator;
use doctor_toolkit::import::model::{ImportOptions, ImportPlan};
use doctor_toolkit::resource::partsdb::{MODEL_SLOTS, Part, PartsDb};
use doctor_toolkit::resource::simgroup::Hash32;
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::{Color32, RichText};

use super::model::{make_plan, pick_model_file, plan_ui};
use super::retexture::TYPE_PARTSDB;
use super::{Picker, Start, Task, Tx};
use crate::app::BrowserApp;
use crate::character_view::{CharacterView, Extra};
use crate::nav::Location;

/// Tint parameters of the character shaders (`docs/formats/characters.md` §3).
const TINTS: [&str; 3] = ["g_TintColor0", "g_TintColor1", "g_TintColor2"];

/// A new material and its tint colours.
pub(crate) struct Tint {
    pub hash: u32,
    pub name: String,
    /// `(parameter, value)` of the tint parameters it has.
    pub colours: Vec<(&'static str, Vec<f32>)>,
}

pub(crate) struct Outfit {
    pub picker: Picker,
    pub db_at: Option<Location>,
    pub db: Option<PartsDb>,
    pub slot: usize,
    /// Sets that may wear the outfit.
    pub sets: Vec<bool>,
    pub set_filter: String,
    /// The part whose mesh the new one is cloned from.
    pub template: Option<usize>,
    pub file: Option<PathBuf>,
    pub plan: Option<Result<ImportPlan, String>>,
    pub tints: Vec<Tint>,
    pub part_name: String,
    pub variant_name: String,
    pub tags: String,
    pub only: bool,
    preview: Option<CharacterView>,
    pub message: Option<String>,
}

impl Outfit {
    pub fn new(app: &mut BrowserApp, start: &Start) -> Self {
        let mut o = Self {
            picker: Picker {
                text: "allcharacters".into(),
                ..Default::default()
            },
            db_at: None,
            db: None,
            slot: 0,
            sets: Vec::new(),
            set_filter: String::new(),
            template: None,
            file: None,
            plan: None,
            tints: Vec::new(),
            part_name: String::new(),
            variant_name: String::new(),
            tags: String::new(),
            only: false,
            preview: None,
            message: None,
        };
        if let Some(loc) = start.loc.filter(|&l| app.type_of(l) == Some(TYPE_PARTSDB)) {
            o.set_db(app, loc);
        }
        o
    }

    pub fn preset(&mut self, key: &str, value: &str, app: &mut BrowserApp) {
        match key {
            "slot" => {
                if let Some(s) = self.db.as_ref().and_then(|db| db.slot_index(value)) {
                    self.set_slot(app, s);
                }
            }
            "set" => {
                if let Some(s) = self.db.as_ref().and_then(|db| db.set_index(value)) {
                    self.sets.iter_mut().for_each(|b| *b = false);
                    self.sets[s] = true;
                    self.pick_template(app);
                }
            }
            "file" => self.set_file(app, PathBuf::from(value)),
            "name" => {
                self.part_name = value.to_owned();
                self.variant_name = value.to_owned();
            }
            "search" => self.picker.text = value.to_owned(),
            _ => {}
        }
    }

    pub fn set_db(&mut self, app: &mut BrowserApp, loc: Location) {
        self.db_at = Some(loc);
        self.plan = None;
        self.preview = None;
        match app
            .current_data(loc)
            .and_then(|d| PartsDb::parse(&d).map_err(|e| e.to_string()))
        {
            Ok(db) => {
                self.sets = vec![false; db.sets.len()];
                if let Some(all) = db.set_index("_all") {
                    self.sets[all] = true;
                } else if !self.sets.is_empty() {
                    self.sets[0] = true;
                }
                self.db = Some(db);
                self.message = None;
                self.set_slot(app, 0);
            }
            Err(error) => {
                self.db = None;
                self.message = Some(error);
            }
        }
    }

    pub fn set_slot(&mut self, app: &mut BrowserApp, slot: usize) {
        self.slot = slot;
        self.pick_template(app);
    }

    /// The first set chosen.
    fn first_set(&self) -> Option<usize> {
        self.sets.iter().position(|&b| b)
    }

    /// Parts that can be worn in the slot by the first chosen set, with a mesh.
    fn slot_parts(&self) -> Vec<usize> {
        let (Some(db), Some(set)) = (&self.db, self.first_set()) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for choice in db.sets[set].choices.iter().filter(|c| c.slot == self.slot) {
            for &v in &choice.variants {
                for &p in &db.variants[v].parts {
                    let p = p as usize;
                    if db.parts[p].mesh_ids().next().is_some() && !out.contains(&p) {
                        out.push(p);
                    }
                }
            }
        }
        out
    }

    fn pick_template(&mut self, app: &mut BrowserApp) {
        let parts = self.slot_parts();
        if !self.template.is_some_and(|t| parts.contains(&t)) {
            self.template = parts.first().copied();
        }
        if self.file.is_some() {
            self.plan(app);
        }
    }

    /// The mesh of the template part.
    fn template_mesh(&self, app: &BrowserApp) -> Result<Location, String> {
        let (Some(db), Some(at), Some(t)) = (&self.db, self.db_at, self.template) else {
            return Err(tr("wizard.outfit.need_template").to_owned());
        };
        let index = app.index.as_ref().ok_or(tr("edits.no_game"))?;
        let id = db.parts[t]
            .mesh_ids()
            .next()
            .ok_or_else(|| tr("wizard.outfit.need_template").to_owned())?;
        let (stream, resource) = MeshLocator::new(index).find(id, at.stream).ok_or_else(|| {
            trf(
                "wizard.outfit.mesh_missing",
                &[("id", &format!("{id:08x}"))],
            )
        })?;
        Ok(Location { stream, resource })
    }

    pub fn set_file(&mut self, app: &mut BrowserApp, path: PathBuf) {
        if let Some(dir) = path.parent() {
            app.settings.export_dirs.import = Some(dir.to_owned());
        }
        if self.part_name.is_empty() {
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_else(|| "outfit".into());
            let stem: String = stem
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                .collect();
            self.part_name = format!("mod_{stem}");
            self.variant_name = self.part_name.clone();
        }
        self.file = Some(path);
        self.plan(app);
    }

    /// Plans the import of the file as a new model set cloned from the template.
    pub fn plan(&mut self, app: &mut BrowserApp) {
        self.preview = None;
        let Some(path) = self.file.clone() else {
            self.plan = None;
            return;
        };
        let result = self.template_mesh(app).and_then(|loc| {
            let options = ImportOptions {
                new_name: Some(self.part_name.clone()),
                ..Default::default()
            };
            make_plan(app, loc, &path, &options)
        });
        self.tints = match &result {
            Ok(plan) => plan
                .new_materials()
                .into_iter()
                .map(|m| Tint {
                    hash: m.name_hash,
                    colours: TINTS
                        .iter()
                        .filter_map(|&t| Some((t, m.floats(t)?.to_vec())))
                        .collect(),
                    name: m.name,
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        self.plan = Some(result);
    }

    /// Ids of the new mesh.
    fn new_mesh(&self) -> Option<u32> {
        match &self.plan {
            Some(Ok(plan)) => plan.mesh_ids().first().copied(),
            _ => None,
        }
    }

    /// The parts database with the new part and variant; returns it with the
    /// index of the variant.
    pub fn edited_db(&self, current: &PartsDb) -> Result<(PartsDb, usize, usize), String> {
        let mesh = self
            .new_mesh()
            .ok_or_else(|| tr("wizard.model.need_file").to_owned())?;
        let mut db = current.clone();
        if db.part_index(&self.part_name).is_some() {
            return Err(trf(
                "wizard.outfit.part_exists",
                &[("name", &self.part_name)],
            ));
        }
        let mut models = [Hash32(0); MODEL_SLOTS];
        models[0] = Hash32(mesh);
        let tags = self
            .tags
            .split([',', ' '])
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_owned)
            .collect();
        let part = db
            .add_part(Part {
                name: self.part_name.clone(),
                slot: Some(self.slot),
                models,
                label: String::new(),
                tags,
            })
            .map_err(|e| e.to_string())?;
        let name = if self.variant_name.trim().is_empty() {
            self.part_name.clone()
        } else {
            self.variant_name.trim().to_owned()
        };
        let variant = db
            .add_variant(&name, vec![part as u16])
            .map_err(|e| e.to_string())?;
        let mut changed = 0;
        for (s, _) in self.sets.iter().enumerate().filter(|(_, b)| **b) {
            changed += db
                .allow_variant(s, self.slot, variant, self.only)
                .map_err(|e| e.to_string())? as usize;
        }
        Ok((db, variant, changed))
    }

    fn set_step(&mut self, ui: &mut egui::Ui, app: &mut BrowserApp) {
        ui.label(tr("wizard.outfit.pick"));
        let mut at = self.db_at;
        if app.picker_ui(ui, &mut self.picker, &[TYPE_PARTSDB], &mut at)
            && let Some(loc) = at
        {
            self.set_db(app, loc);
        }
        if let Some(message) = &self.message {
            ui.colored_label(Color32::YELLOW, message);
        }
        let Some(db) = &self.db else { return };
        let mut slot = self.slot;
        ui.horizontal(|ui| {
            egui::ComboBox::from_label(tr("wizard.outfit.slot"))
                .selected_text(db.slots.get(slot).cloned().unwrap_or_default())
                .show_ui(ui, |ui| {
                    for (i, name) in db.slots.iter().enumerate() {
                        ui.selectable_value(&mut slot, i, name);
                    }
                });
        });
        ui.label(tr("wizard.outfit.sets"));
        ui.horizontal(|ui| {
            ui.label(tr("wizard.filter"));
            ui.text_edit_singleline(&mut self.set_filter);
        });
        let filter = self.set_filter.to_lowercase();
        let mut changed = false;
        egui::ScrollArea::vertical()
            .id_salt("outfit-sets")
            .max_height(260.0)
            .show(ui, |ui| {
                for (i, set) in db.sets.iter().enumerate() {
                    if !filter.is_empty() && !set.name.to_lowercase().contains(&filter) {
                        continue;
                    }
                    changed |= ui.checkbox(&mut self.sets[i], &set.name).changed();
                }
            });
        if slot != self.slot {
            self.set_slot(app, slot);
        } else if changed {
            self.pick_template(app);
        }
    }

    fn mesh_step(&mut self, ui: &mut egui::Ui, app: &mut BrowserApp) {
        let parts = self.slot_parts();
        if let Some(db) = &self.db {
            let mut template = self.template;
            egui::ComboBox::from_label(tr("wizard.outfit.template"))
                .selected_text(
                    template
                        .map(|t| db.parts[t].name.clone())
                        .unwrap_or_default(),
                )
                .show_ui(ui, |ui| {
                    for &p in &parts {
                        ui.selectable_value(&mut template, Some(p), &db.parts[p].name);
                    }
                });
            if template != self.template {
                self.template = template;
                if self.file.is_some() {
                    self.plan(app);
                }
            }
        }
        ui.label(
            RichText::new(tr("wizard.outfit.template_hint"))
                .weak()
                .small(),
        );
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
        match &self.plan {
            Some(Ok(plan)) => {
                if plan.summary.skeleton.is_none() {
                    ui.colored_label(Color32::YELLOW, tr("wizard.outfit.not_skinned"));
                }
                egui::ScrollArea::vertical()
                    .id_salt("outfit-plan")
                    .show(ui, |ui| plan_ui(ui, plan));
            }
            Some(Err(error)) => {
                ui.colored_label(Color32::from_rgb(230, 90, 80), error);
            }
            None => {}
        }
    }

    fn materials_step(&mut self, ui: &mut egui::Ui) {
        let Some(Ok(plan)) = &mut self.plan else {
            return;
        };
        for p in &plan.summary.parts {
            let kind = if p.new_material {
                tr("wizard.outfit.material_new")
            } else {
                tr("wizard.outfit.material_game")
            };
            ui.label(format!("{}: {} ({kind})", p.name, p.material));
        }
        ui.separator();
        if self.tints.iter().all(|t| t.colours.is_empty()) {
            ui.label(RichText::new(tr("wizard.outfit.no_tints")).weak());
        }
        let mut changed = Vec::new();
        for tint in &mut self.tints {
            if tint.colours.is_empty() {
                continue;
            }
            ui.horizontal_wrapped(|ui| {
                ui.strong(&tint.name);
                for (param, values) in &mut tint.colours {
                    let mut rgb = [
                        values.first().copied().unwrap_or(1.0),
                        values.get(1).copied().unwrap_or(1.0),
                        values.get(2).copied().unwrap_or(1.0),
                    ];
                    ui.label(*param);
                    if ui.color_edit_button_rgb(&mut rgb).changed() {
                        for (v, c) in values.iter_mut().zip(rgb) {
                            *v = c;
                        }
                        changed.push((tint.hash, *param, values.clone()));
                    }
                }
            });
        }
        for (hash, param, values) in changed {
            if let Err(error) = plan.set_material_floats(hash, param, &values) {
                self.message = Some(error.to_string());
            }
            self.preview = None;
        }
        ui.label(RichText::new(tr("wizard.outfit.tint_hint")).weak().small());
    }

    fn part_step(&mut self, ui: &mut egui::Ui) {
        egui::Grid::new("outfit-part")
            .num_columns(2)
            .show(ui, |ui| {
                ui.label(tr("wizard.outfit.part_name"));
                ui.text_edit_singleline(&mut self.part_name);
                ui.end_row();
                ui.label(tr("wizard.outfit.variant_name"));
                ui.text_edit_singleline(&mut self.variant_name);
                ui.end_row();
                ui.label(tr("wizard.outfit.tags"));
                ui.text_edit_singleline(&mut self.tags)
                    .on_hover_text(tr("wizard.outfit.tags_hint"));
                ui.end_row();
            });
        ui.checkbox(&mut self.only, tr("wizard.outfit.only"));
        if let Some(db) = &self.db {
            match self.edited_db(db) {
                Ok((_, _, changed)) => {
                    ui.label(trn("wizard.outfit.sets_offer", changed as u64, &[]));
                }
                Err(error) => {
                    ui.colored_label(Color32::YELLOW, error);
                }
            }
        }
    }

    fn preview_step(&mut self, ui: &mut egui::Ui, app: &mut BrowserApp) {
        if self.preview.is_none() {
            self.preview = self.make_preview(app);
        }
        match &mut self.preview {
            Some(view) => {
                if let Err(error) = view.parts() {
                    ui.colored_label(Color32::YELLOW, error);
                }
                view.show(ui);
            }
            None => {
                ui.label(RichText::new(tr("wizard.outfit.no_preview")).weak());
            }
        }
    }

    fn make_preview(&self, app: &BrowserApp) -> Option<CharacterView> {
        let (Some(game), Some(at), Some(db), Some(Ok(plan))) =
            (app.game.clone(), self.db_at, &self.db, &self.plan)
        else {
            return None;
        };
        let (edited, variant, _) = self.edited_db(db).ok()?;
        let bytes = edited.encode().ok()?;
        let mut view =
            CharacterView::load(&game, at.stream, TYPE_PARTSDB, &bytes, app.gpu.as_ref())?;
        let mut meshes = HashMap::new();
        meshes.insert(self.new_mesh()?, plan.preview().ok()?);
        let extra = Extra {
            meshes,
            materials: plan.new_materials(),
            textures: plan.new_textures(),
        };
        view.preview_with(extra, self.first_set()?, self.slot, variant);
        Some(view)
    }
}

impl Task for Outfit {
    fn steps(&self) -> &'static [&'static str] {
        &[
            "wizard.outfit.step.set",
            "wizard.outfit.step.mesh",
            "wizard.outfit.step.materials",
            "wizard.outfit.step.part",
            "wizard.outfit.step.preview",
        ]
    }

    fn ui(&mut self, step: usize, ui: &mut egui::Ui, app: &mut BrowserApp, _tx: &mut Tx) {
        match step {
            0 => self.set_step(ui, app),
            1 => self.mesh_step(ui, app),
            2 => self.materials_step(ui),
            3 => self.part_step(ui),
            _ => self.preview_step(ui, app),
        }
    }

    fn check(&self, step: usize, _app: &BrowserApp) -> Result<(), String> {
        match step {
            0 if self.db.is_none() => Err(tr("wizard.outfit.need_db").to_owned()),
            0 if self.first_set().is_none() => Err(tr("wizard.outfit.need_set").to_owned()),
            0 if self.template.is_none() => Err(tr("wizard.outfit.need_template").to_owned()),
            1 => match &self.plan {
                None => Err(tr("wizard.model.need_file").to_owned()),
                Some(Err(e)) => Err(e.clone()),
                Some(Ok(p)) if p.summary.skeleton.is_none() => {
                    Err(tr("wizard.outfit.not_skinned").to_owned())
                }
                Some(Ok(_)) => Ok(()),
            },
            3 => {
                if !doctor_toolkit::text::valid_name(&self.part_name) {
                    return Err(tr("wizard.outfit.bad_name").to_owned());
                }
                let db = self.db.as_ref().ok_or(tr("wizard.no_parts_db"))?;
                self.edited_db(db).map(|_| ())
            }
            _ => Ok(()),
        }
    }

    fn summary(&self, app: &BrowserApp) -> Vec<String> {
        let mut out = Vec::new();
        if let (Some(db), Some(at)) = (&self.db, self.db_at) {
            let sets: Vec<&str> = self
                .sets
                .iter()
                .enumerate()
                .filter(|(_, b)| **b)
                .map(|(i, _)| db.sets[i].name.as_str())
                .collect();
            out.push(trf(
                "wizard.outfit.summary_part",
                &[
                    ("part", &self.part_name),
                    (
                        "slot",
                        &db.slots.get(self.slot).cloned().unwrap_or_default(),
                    ),
                    ("db", &app.location_label(at)),
                ],
            ));
            out.push(trf(
                "wizard.outfit.summary_sets",
                &[("sets", &sets.join(", "))],
            ));
        }
        if let Some(Ok(plan)) = &self.plan {
            out.extend(super::model::plan_lines(plan));
        }
        out
    }

    fn record(&mut self, app: &mut BrowserApp, _tx: &mut Tx) -> Result<String, String> {
        let Some(Ok(plan)) = &self.plan else {
            return Err(tr("wizard.model.need_file").to_owned());
        };
        let at = self.db_at.ok_or(tr("wizard.no_parts_db"))?;
        let game = app.game.clone().ok_or(tr("edits.no_game"))?;
        plan.commit(app.workspace_mut()?, &game)
            .map_err(|e| e.to_string())?;
        // The database as the workspace has it now.
        let current = app
            .current_data(at)
            .and_then(|d| PartsDb::parse(&d).map_err(|e| e.to_string()))?;
        let (db, _, _) = self.edited_db(&current)?;
        let data = db.encode().map_err(|e| e.to_string())?;
        let label = trf("wizard.outfit.label", &[("part", &self.part_name)]);
        app.workspace_mut()?
            .set_resource(&game, at.stream, at.resource, data, &label)
            .map_err(|e| e.to_string())?;
        Ok(label)
    }
}

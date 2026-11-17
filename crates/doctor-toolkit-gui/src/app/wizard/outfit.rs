//! New outfit: pick a parts database, a slot and the sets that may wear it;
//! import a mesh skinned to the human skeleton as a new model set (cloned
//! from a part of that slot); give its new materials tint colours; name the
//! part and its variant; look at the character wearing it; record the mesh
//! and the parts database edit as one operation (`partsdb add-part`).

use std::collections::HashMap;
use std::path::PathBuf;

use doctor_toolkit::character::MeshLocator;
use doctor_toolkit::import::model::{ImportOptions, ImportPlan};
use doctor_toolkit::outfit::{self, PartSpec};
use doctor_toolkit::resource::partsdb::{MODEL_SLOTS, PartsDb};
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

impl Tint {
    /// Every tint colour is black: the clothing would be drawn black.
    pub fn is_black(&self) -> bool {
        !self.colours.is_empty()
            && self
                .colours
                .iter()
                .all(|(_, v)| v.iter().take(3).all(|c| c.abs() < 1e-3))
    }
}

/// A placeholder material of the game's clothing (`…_mat`): its own tints
/// are debug colours or black, the real ones come from the presets of the
/// parts database (`docs/formats/characters.md` §1).
pub(crate) fn is_stub_material(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with("_mat")
}

/// Width of the character preview that keeps it inside a tab with `room`
/// left: the view is 0.9 of its width tall, below about 200 points of
/// controls.
pub(crate) fn preview_width(room: egui::Vec2) -> f32 {
    const CONTROLS: f32 = 200.0;
    ((room.y - CONTROLS) / 0.9).clamp(220.0, room.x.max(220.0))
}

/// A part that can be the sample of the new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Template {
    pub part: usize,
    /// A set that wears it in the slot (none: no set does).
    pub set: Option<usize>,
    /// That set is ticked.
    pub ticked: bool,
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
    /// A second, lower level of detail of the part (optional): its file and
    /// what the import of it would do. It is planned again when recorded,
    /// after the main mesh is in the workspace, so that it shares the new
    /// materials.
    pub lod_file: Option<PathBuf>,
    pub lod_plan: Option<Result<ImportPlan, String>>,
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
            lod_file: None,
            lod_plan: None,
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
            "lod" => self.set_lod_file(app, Some(PathBuf::from(value))),
            "template" => {
                let found = self.templates().into_iter().find(|t| {
                    self.db
                        .as_ref()
                        .is_some_and(|db| db.parts[t.part].name == value)
                });
                match found {
                    Some(t) => {
                        self.template = Some(t.part);
                        if self.file.is_some() {
                            self.plan(app);
                        }
                    }
                    None => {
                        self.template = None;
                        self.message =
                            Some(trf("wizard.outfit.no_such_template", &[("name", &value)]));
                    }
                }
            }
            // `tint=<0..2>:<r>/<g>/<b>`: that tint colour of every new material.
            "tint" => {
                if let Err(error) = self.set_tint(value) {
                    self.message = Some(error.clone());
                    self.plan = Some(Err(error));
                }
            }
            "tags" => self.tags = value.to_owned(),
            "only" => self.only = true,
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

    /// Parts with a mesh that can serve as the sample of the new one, the
    /// best first: those the ticked sets wear in the slot, then those of the
    /// other sets of the database, then parts of the slot no set wears.
    /// (A set such as `_all` of the player may have nothing with a mesh in
    /// a slot: its hat is an empty part.)
    pub(crate) fn templates(&self) -> Vec<Template> {
        let Some(db) = &self.db else {
            return Vec::new();
        };
        let mut out: Vec<Template> = Vec::new();
        let has_mesh = |p: usize| db.parts[p].mesh_ids().next().is_some();
        for ticked in [true, false] {
            for (s, set) in db.sets.iter().enumerate() {
                if self.sets.get(s).copied().unwrap_or(false) != ticked {
                    continue;
                }
                for choice in set.choices.iter().filter(|c| c.slot == self.slot) {
                    for &v in &choice.variants {
                        for &p in &db.variants[v].parts {
                            let p = p as usize;
                            if has_mesh(p) && !out.iter().any(|t| t.part == p) {
                                out.push(Template {
                                    part: p,
                                    set: Some(s),
                                    ticked,
                                });
                            }
                        }
                    }
                }
            }
        }
        for (p, part) in db.parts.iter().enumerate() {
            if part.slot == Some(self.slot) && has_mesh(p) && !out.iter().any(|t| t.part == p) {
                out.push(Template {
                    part: p,
                    set: None,
                    ticked: false,
                });
            }
        }
        out
    }

    /// Where the sample part comes from, for the label next to it.
    fn template_source(&self, t: &Template) -> String {
        let (Some(db), Some(set)) = (&self.db, t.set) else {
            return tr("wizard.outfit.template_no_set").to_owned();
        };
        let key = if t.ticked {
            "wizard.outfit.template_ticked"
        } else {
            "wizard.outfit.template_other"
        };
        trf(key, &[("set", &db.sets[set].name)])
    }

    fn pick_template(&mut self, app: &mut BrowserApp) {
        let parts = self.templates();
        if !self
            .template
            .is_some_and(|t| parts.iter().any(|p| p.part == t))
        {
            self.template = parts.first().map(|t| t.part);
        }
        if self.file.is_some() {
            self.plan(app);
        }
    }

    /// The mesh of the template part: its first one, or the one of level of
    /// detail `lod` when the part has it.
    fn template_mesh(&self, app: &BrowserApp, lod: usize) -> Result<Location, String> {
        let (Some(db), Some(at), Some(t)) = (&self.db, self.db_at, self.template) else {
            return Err(tr("wizard.outfit.need_template").to_owned());
        };
        let index = app.index.as_ref().ok_or(tr("edits.no_game"))?;
        // The mesh of that level of detail first, then the part's others
        // (a part may name a far mesh the game does not ship).
        let wanted = db.parts[t].models[lod.min(MODEL_SLOTS - 1)].0;
        let ids: Vec<u32> = std::iter::once(wanted)
            .chain(db.parts[t].mesh_ids())
            .filter(|&id| id != 0)
            .collect();
        let first = *ids
            .first()
            .ok_or_else(|| tr("wizard.outfit.need_template").to_owned())?;
        let locator = MeshLocator::new(index);
        let (stream, resource) = ids
            .iter()
            .find_map(|&id| locator.find(id, at.stream))
            .ok_or_else(|| {
                trf(
                    "wizard.outfit.mesh_missing",
                    &[("id", &format!("{first:08x}"))],
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

    /// The file of the second level of detail, or none.
    pub fn set_lod_file(&mut self, app: &mut BrowserApp, path: Option<PathBuf>) {
        self.lod_file = path;
        self.plan_lod(app);
    }

    /// Name of the model set of the second level of detail.
    fn lod_name(&self) -> String {
        format!("{}_lod2", self.part_name)
    }

    /// Plans the import of the second level of detail (a check before the
    /// record plans it again over the recorded main mesh).
    fn plan_lod(&mut self, app: &mut BrowserApp) {
        self.lod_plan = self.lod_file.clone().map(|path| {
            let loc = self.template_mesh(app, 1)?;
            let options = ImportOptions {
                new_name: Some(self.lod_name()),
                ..Default::default()
            };
            make_plan(app, loc, &path, &options)
        });
    }

    /// Plans the import of the file as a new model set cloned from the template.
    pub fn plan(&mut self, app: &mut BrowserApp) {
        self.preview = None;
        self.plan_lod(app);
        let Some(path) = self.file.clone() else {
            self.plan = None;
            return;
        };
        let result = self.template_mesh(app, 0).and_then(|loc| {
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

    /// Sets tint colour `<n>:<r>/<g>/<b>` of every new material that has it.
    fn set_tint(&mut self, spec: &str) -> Result<(), String> {
        let bad = || trf("wizard.outfit.bad_tint", &[("value", &spec)]);
        let (n, rgb) = spec.split_once(':').ok_or_else(bad)?;
        let param = *n
            .parse::<usize>()
            .ok()
            .and_then(|n| TINTS.get(n))
            .ok_or_else(bad)?;
        let rgb: Vec<f32> = rgb.split('/').filter_map(|v| v.parse().ok()).collect();
        if rgb.len() != 3 {
            return Err(bad());
        }
        let Some(Ok(plan)) = &mut self.plan else {
            return Err(tr("wizard.model.need_file").to_owned());
        };
        let mut set = 0;
        for tint in &mut self.tints {
            for (name, values) in &mut tint.colours {
                if *name != param {
                    continue;
                }
                for (v, c) in values.iter_mut().zip(&rgb) {
                    *v = *c;
                }
                plan.set_material_floats(tint.hash, param, values)
                    .map_err(|e| e.to_string())?;
                set += 1;
            }
        }
        self.preview = None;
        if set == 0 {
            return Err(tr("wizard.outfit.no_tints").to_owned());
        }
        Ok(())
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
        let lod = match &self.lod_plan {
            Some(Ok(plan)) => plan.mesh_ids().first().copied(),
            _ => None,
        };
        self.edited_db_with(current, lod)
    }

    /// [`Self::edited_db`] with `lod` as the mesh of the second level of detail.
    fn edited_db_with(
        &self,
        current: &PartsDb,
        lod: Option<u32>,
    ) -> Result<(PartsDb, usize, usize), String> {
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
        if let Some(lod) = lod {
            models[1] = Hash32(lod);
        }
        let tags = self
            .tags
            .split([',', ' '])
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_owned)
            .collect();
        // The operation itself is the library's (`partsdb add-part`, the
        // MCP tool `partsdb_add_part`).
        let added = outfit::add_part(
            &mut db,
            &PartSpec {
                name: self.part_name.clone(),
                slot: self.slot,
                models,
                tags,
                variant: self.variant_name.clone(),
                sets: (0..self.sets.len()).filter(|&s| self.sets[s]).collect(),
                only: self.only,
            },
        )
        .map_err(|e| e.to_string())?;
        Ok((db, added.variant, added.sets_changed))
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
        self.template_note(ui);
    }

    /// The sample part and where it comes from; a warning when it is not of
    /// a ticked set.
    fn template_note(&self, ui: &mut egui::Ui) {
        let Some(db) = &self.db else { return };
        let templates = self.templates();
        let Some(t) = templates.iter().find(|t| Some(t.part) == self.template) else {
            return;
        };
        let text = trf(
            "wizard.outfit.template_is",
            &[
                ("part", &db.parts[t.part].name),
                ("source", &self.template_source(t)),
            ],
        );
        if t.ticked {
            ui.label(RichText::new(text).weak());
        } else {
            ui.colored_label(Color32::YELLOW, text);
        }
    }

    fn mesh_step(&mut self, ui: &mut egui::Ui, app: &mut BrowserApp) {
        let parts = self.templates();
        if let Some(db) = &self.db {
            let mut template = self.template;
            let label =
                |t: &Template| format!("{} — {}", db.parts[t.part].name, self.template_source(t));
            egui::ComboBox::from_label(tr("wizard.outfit.template"))
                .width(420.0)
                .selected_text(
                    parts
                        .iter()
                        .find(|t| Some(t.part) == template)
                        .map(label)
                        .unwrap_or_default(),
                )
                .show_ui(ui, |ui| {
                    for t in &parts {
                        ui.selectable_value(&mut template, Some(t.part), label(t));
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
        ui.horizontal_wrapped(|ui| {
            if ui
                .button(tr("wizard.outfit.choose_lod"))
                .on_hover_text(tr("wizard.outfit.lod_hint"))
                .clicked()
                && let Some(path) = pick_model_file(app)
            {
                self.set_lod_file(app, Some(path));
            }
            if let Some(file) = &self.lod_file {
                ui.monospace(file.display().to_string());
                if ui.small_button(tr("wizard.outfit.no_lod")).clicked() {
                    self.set_lod_file(app, None);
                }
            } else {
                ui.label(RichText::new(tr("wizard.outfit.lod_none")).weak());
            }
        });
        match &self.lod_plan {
            Some(Ok(plan)) => {
                let s = &plan.summary;
                let (vertices, triangles) = s
                    .parts
                    .iter()
                    .fold((0, 0), |(v, t), p| (v + p.vertices, t + p.triangles));
                ui.label(trf(
                    "wizard.outfit.lod_plan",
                    &[
                        ("name", &self.lod_name()),
                        ("vertices", &vertices),
                        ("triangles", &triangles),
                    ],
                ));
            }
            Some(Err(error)) => {
                ui.colored_label(Color32::from_rgb(230, 90, 80), error);
            }
            None => {}
        }
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
            if !p.new_material && is_stub_material(&p.material) {
                ui.colored_label(
                    Color32::YELLOW,
                    trf("wizard.outfit.stub_material", &[("name", &p.material)]),
                );
            }
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
            if tint.is_black() {
                ui.colored_label(
                    Color32::YELLOW,
                    trf("wizard.outfit.black_tints", &[("name", &tint.name)]),
                );
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
                // The view is as tall as it is wide: keep it inside the tab
                // (and scroll when the tab is small).
                let room = ui.available_size();
                let width = preview_width(room);
                egui::ScrollArea::vertical()
                    .id_salt("outfit-preview")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_max_width(width);
                        view.show(ui);
                    });
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
            0 if self.template.is_none() => Err(self
                .message
                .clone()
                .unwrap_or_else(|| tr("wizard.outfit.need_template").to_owned())),
            1 if matches!(&self.lod_plan, Some(Err(_))) => match &self.lod_plan {
                Some(Err(e)) => Err(trf("wizard.outfit.lod_error", &[("error", e)])),
                _ => Ok(()),
            },
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
        if let (Some(db), Some(t)) = (&self.db, self.template)
            && let Some(source) = self.templates().iter().find(|s| s.part == t)
        {
            out.push(trf(
                "wizard.outfit.template_is",
                &[
                    ("part", &db.parts[t].name),
                    ("source", &self.template_source(source)),
                ],
            ));
        }
        if let Some(Ok(plan)) = &self.plan {
            out.extend(super::model::plan_lines(plan));
        }
        if let Some(file) = &self.lod_file {
            out.push(trf(
                "wizard.outfit.summary_lod",
                &[
                    ("name", &self.lod_name()),
                    ("file", &file.display().to_string()),
                ],
            ));
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
        // The second level of detail, planned over the recorded main mesh:
        // it uses the materials that one added.
        let mut lod = None;
        if let Some(path) = self.lod_file.clone() {
            let loc = self.template_mesh(app, 1)?;
            let options = ImportOptions {
                new_name: Some(self.lod_name()),
                ..Default::default()
            };
            let plan = make_plan(app, loc, &path, &options)
                .map_err(|e| trf("wizard.outfit.lod_error", &[("error", &e)]))?;
            lod = plan.mesh_ids().first().copied();
            plan.commit(app.workspace_mut()?, &game)
                .map_err(|e| e.to_string())?;
        }
        // The database as the workspace has it now.
        let current = app
            .current_data(at)
            .and_then(|d| PartsDb::parse(&d).map_err(|e| e.to_string()))?;
        let (db, _, _) = self.edited_db_with(&current, lod)?;
        let data = db.encode().map_err(|e| e.to_string())?;
        let label = trf("wizard.outfit.label", &[("part", &self.part_name)]);
        app.workspace_mut()?
            .set_resource(&game, at.stream, at.resource, data, &label)
            .map_err(|e| e.to_string())?;
        Ok(label)
    }
}

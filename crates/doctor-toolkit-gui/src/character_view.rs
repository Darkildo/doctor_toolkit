//! Parts databases (`partsdb`): the tree of sets, slots, variants and parts,
//! and a character preview assembled from the choices of one set, with the
//! game's material swaps, body morphs and shading; export as glTF.
//!
//! `DOCTOR_TOOLKIT_GUI_CHARACTER=<options>` presets the preview for
//! screenshots: comma-separated `set=<name>`, `lod=<n>`, `yaw=<rad>`,
//! `pitch=<rad>`, `zoom=<factor>`, `select=<part>` and `choose=<KEY=VALUE>`
//! (as `doctor-toolkit partsdb character --choose`; repeatable).
//!
//! In a tab of its own (stage 56; a floating window before) the choices
//! and parts are on the left, the preview fills the rest, Random, Export
//! and the shading are on the bar of the tab, and the materials of the
//! selected part are in the Details panel.

use std::collections::BTreeMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;

use doctor_toolkit::character::{Character, MeshLocator, parse_selection};
use doctor_toolkit::game::{GameAssets, GameInstance, ResourceIndex};
use doctor_toolkit::resource::partsdb::{PartsDb, Selection};
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::{Color32, Pos2, Rect, RichText, Sense, Vec2};

use crate::model_view::{LoadOptions, LoadedModel, load_with, material_panel};
use crate::render::{
    Gpu, Instance, MeshKey, OrbitCamera, Pick, Scene, SceneOptions, Viewport, math,
};

const TYPE_PARTSDB: u32 = doctor_toolkit::hash::sdbm(b"partsdb");

/// Meshes, materials and textures that are not in the game files yet (an
/// outfit about to be imported), drawn as if they were.
#[derive(Default)]
pub struct Extra {
    /// Meshes by id.
    pub meshes: std::collections::HashMap<u32, doctor_toolkit::resource::mesh::Model>,
    pub materials: Vec<doctor_toolkit::resource::material::Material>,
    pub textures: Vec<doctor_toolkit::resource::texture::Texture>,
}

/// The assets of the game with [`Extra`] in front.
struct Overlay<'a> {
    extra: &'a Extra,
    game: &'a mut GameAssets,
}

impl doctor_toolkit::export::gltf::AssetSource for Overlay<'_> {
    fn material(&mut self, hash: u32) -> Option<doctor_toolkit::resource::material::Material> {
        self.extra
            .materials
            .iter()
            .find(|m| m.name_hash == hash)
            .cloned()
            .or_else(|| self.game.material(hash))
    }

    fn texture(&mut self, hash: u32) -> Option<doctor_toolkit::resource::texture::Texture> {
        self.extra
            .textures
            .iter()
            .find(|t| t.header.name_hash == hash)
            .cloned()
            .or_else(|| self.game.texture(hash))
    }

    fn skeleton(
        &mut self,
        bones: &[(u32, u32)],
    ) -> Option<doctor_toolkit::resource::rcb::Skeleton> {
        self.game.skeleton(bones)
    }

    fn rigs(&mut self) -> &[doctor_toolkit::resource::rcb::Rig] {
        GameAssets::rigs(self.game)
    }
}

/// A loaded character: the library's assembly and one renderer model per part.
struct Loaded {
    character: Character,
    models: Vec<LoadedModel>,
    bounds: math::Aabb,
}

#[derive(Clone, PartialEq)]
struct RenderKey {
    camera: OrbitCamera,
    size: [u32; 2],
    options: SceneOptions,
    highlight: Option<Pick>,
    hidden: Vec<bool>,
    generation: u64,
}

pub struct CharacterView {
    gpu: Option<Gpu>,
    /// Game directory; the game and its resource index are opened on the
    /// first assembly and shared by every view of the session.
    root: PathBuf,
    game: Option<(Arc<GameInstance>, Arc<ResourceIndex>)>,
    stream: usize,
    db: PartsDb,
    meshes: Option<MeshLocator>,
    assets: Option<GameAssets>,
    set: usize,
    selection: Selection,
    lod: usize,
    loaded: Option<Result<Loaded, String>>,
    generation: u64,
    camera: OrbitCamera,
    scene: Scene,
    viewport: Viewport,
    hidden: Vec<bool>,
    selected: Option<usize>,
    hovered: Option<usize>,
    last_render: Option<RenderKey>,
    filter: String,
    status: String,
    preview_open: bool,
    /// Meshes and materials not in the game files.
    extra: Extra,
}

impl CharacterView {
    /// A view of the parts database `data` (resource `resource` of stream
    /// `stream`), or `None` for other resource types.
    pub fn load(
        game: &GameInstance,
        stream: usize,
        type_id: u32,
        data: &[u8],
        gpu: Option<&Gpu>,
    ) -> Option<Self> {
        if type_id != TYPE_PARTSDB {
            return None;
        }
        let db = PartsDb::parse(data).ok()?;
        let mut view = Self {
            gpu: gpu.cloned(),
            root: game.root().to_owned(),
            game: None,
            stream,
            db,
            meshes: None,
            assets: None,
            set: 0,
            selection: Selection::default(),
            lod: 0,
            loaded: None,
            generation: 0,
            camera: OrbitCamera::default(),
            scene: Scene::default(),
            viewport: Viewport::new(),
            hidden: Vec::new(),
            selected: None,
            hovered: None,
            last_render: None,
            filter: String::new(),
            status: String::new(),
            preview_open: true,
            extra: Extra::default(),
        };
        if let Ok(preset) = std::env::var("DOCTOR_TOOLKIT_GUI_CHARACTER") {
            view.apply_preset(&preset);
        }
        Some(view)
    }

    fn apply_preset(&mut self, preset: &str) {
        let mut choices = Vec::new();
        let mut camera = Vec::new();
        for item in preset.split(',').map(str::trim) {
            let (name, value) = item.split_once('=').unwrap_or((item, ""));
            match name {
                "set" => {
                    if let Some(s) = self.db.set_index(value) {
                        self.set = s;
                    }
                }
                "lod" => self.lod = value.parse().unwrap_or(0),
                "choose" => choices.push(value.to_owned()),
                "select" => self.selected = value.parse().ok(),
                _ => camera.push((name.to_owned(), value.parse::<f32>().ok())),
            }
        }
        if let Ok(selection) = parse_selection(&self.db, self.set, &choices) {
            self.selection = selection;
        }
        self.rebuild();
        for (name, number) in camera {
            match name.as_str() {
                "yaw" => self.camera.yaw = number.unwrap_or(self.camera.yaw),
                "pitch" => self.camera.pitch = number.unwrap_or(self.camera.pitch),
                "zoom" => self.camera.distance /= number.unwrap_or(1.0).max(0.01),
                _ => {}
            }
        }
    }

    /// The game and its resource index, opened once per game directory.
    fn game(&mut self) -> Result<(Arc<GameInstance>, Arc<ResourceIndex>), String> {
        if let Some(g) = &self.game {
            return Ok(g.clone());
        }
        // The editor's index once it has one (shared with the cut-scenes).
        let pair = crate::cutscene_view::preview::open_game(&self.root)?;
        self.game = Some(pair.clone());
        Ok(pair)
    }

    fn assets(&mut self) -> &mut GameAssets {
        let (game, index) = self.game.clone().expect("opened by rebuild");
        self.assets
            .get_or_insert_with(|| GameAssets::new(game, &index))
    }

    /// Assembles the current choices and uploads the parts.
    fn rebuild(&mut self) {
        self.generation += 1;
        self.last_render = None;
        let Some(gpu) = self.gpu.clone() else {
            self.loaded = Some(Err(tr("character.no_gpu").into()));
            return;
        };
        let (game, index) = match self.game() {
            Ok(pair) => pair,
            Err(e) => {
                self.loaded = Some(Err(e));
                return;
            }
        };
        if self.meshes.is_none() {
            self.meshes = Some(MeshLocator::new(&index));
        }
        let character = Character::load_with(
            &game,
            self.meshes.as_ref().expect("built above"),
            self.stream,
            &self.db,
            self.set,
            &self.selection,
            self.lod,
            &self.extra.meshes,
        );
        let character = match character {
            Ok(c) => c,
            Err(e) => {
                self.loaded = Some(Err(e.to_string()));
                return;
            }
        };
        let mut models = Vec::new();
        let mut bounds = math::Aabb::EMPTY;
        let mut errors = Vec::new();
        let morphs = character.morphs.clone();
        for part in &character.parts {
            let options = LoadOptions {
                swaps: part.materials.clone(),
                morphs: morphs.clone(),
            };
            let key = part_key(part.location, &options);
            let mut renderer = gpu.lock();
            self.assets();
            let mut assets = Overlay {
                extra: &self.extra,
                game: self.assets.as_mut().expect("made above"),
            };
            match load_with(&part.model, key, &mut assets, &mut renderer, &options) {
                Ok(model) => {
                    bounds.union(&model.mesh.bounds());
                    models.push(model);
                }
                Err(e) => errors.push(format!("{}: {e}", part.name)),
            }
        }
        // Every part of the character stays on the GPU.
        gpu.lock().set_mesh_budget(48.max(models.len() * 2));
        self.status = errors.join("; ");
        self.scene.instances = models
            .iter()
            .map(|m| Instance::new(m.key, math::IDENTITY))
            .collect();
        if self.hidden.len() != models.len() {
            self.hidden = vec![false; models.len()];
            self.selected = self.selected.filter(|&s| s < models.len());
            if !bounds.is_empty() {
                self.camera = OrbitCamera::framing(&bounds);
            }
        }
        self.loaded = Some(Ok(Loaded {
            character,
            models,
            bounds,
        }));
    }

    /// Draws `extra` as part of the game (a preview of an import) and
    /// assembles set `set` with variant `variant` in slot `slot`.
    pub fn preview_with(&mut self, extra: Extra, set: usize, slot: usize, variant: usize) {
        self.extra = extra;
        self.set = set.min(self.db.sets.len().saturating_sub(1));
        self.selection = Selection::default();
        self.selection.variants.insert(slot, variant);
        self.hidden.clear();
        self.rebuild();
    }

    /// The assembled character's part names, or why it did not assemble.
    pub fn parts(&self) -> Result<Vec<String>, String> {
        match &self.loaded {
            Some(Ok(l)) => Ok(l.character.parts.iter().map(|p| p.name.clone()).collect()),
            Some(Err(e)) => Err(e.clone()),
            None => Ok(Vec::new()),
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        let db = &self.db;
        ui.horizontal_wrapped(|ui| {
            ui.strong(trf("character.database", &[("name", &db.name)]));
            let kind = if db.kind == 4 {
                tr("character.kind_vehicle")
            } else {
                tr("character.kind_characters")
            };
            ui.label(
                RichText::new(trf(
                    "character.database_counts",
                    &[
                        ("kind", &kind),
                        ("slots", &db.slots.len()),
                        ("parts", &db.parts.len()),
                        ("variants", &db.variants.len()),
                        ("presets", &db.presets.len()),
                        ("blenders", &db.blenders.len()),
                        ("sets", &db.sets.len()),
                        ("groups", &db.groups.len()),
                    ],
                ))
                .weak(),
            );
        });
        egui::CollapsingHeader::new(tr("character.preview"))
            .id_salt("character preview")
            .default_open(self.preview_open)
            .show(ui, |ui| {
                if self.loaded.is_none() {
                    if ui.button(tr("character.assemble")).clicked() {
                        self.rebuild();
                    }
                    ui.label(RichText::new(tr("character.assemble_hint")).weak().small());
                } else {
                    let changed = self.controls(ui);
                    if changed {
                        self.rebuild();
                    }
                    if self.view_ready(ui) {
                        let width = ui.available_width().max(64.0);
                        let height = (width * 0.9).clamp(200.0, 720.0);
                        self.viewport_ui(ui, egui::vec2(width, height));
                        self.part_list(ui, true);
                    }
                }
            });
        egui::CollapsingHeader::new(tr("character.tree"))
            .id_salt("partsdb tree")
            .show(ui, |ui| self.tree(ui));
    }

    /// The preview in a tab of its own: the choices, the parts and the tree
    /// on the left, the preview in the rest; `details`: the materials of
    /// the selected part are in the Details panel.
    pub fn show_tab(&mut self, ui: &mut egui::Ui, details: bool) {
        egui::Panel::left(ui.scope_id().with("character controls"))
            .resizable(true)
            .default_size(420.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("character tab scroll")
                    .show(ui, |ui| {
                        if self.choices(ui) {
                            self.rebuild();
                        }
                        if !self.status.is_empty() {
                            ui.colored_label(Color32::YELLOW, &self.status);
                        }
                        if self.loaded.is_some() {
                            self.part_list(ui, !details);
                        }
                        egui::CollapsingHeader::new(tr("character.tree"))
                            .id_salt("partsdb tree")
                            .show(ui, |ui| self.tree(ui));
                    });
            });
        if self.loaded.is_none() {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                if ui.button(tr("character.assemble")).clicked() {
                    self.rebuild();
                }
                ui.label(RichText::new(tr("character.assemble_hint")).weak().small());
            });
        } else if self.view_ready(ui) {
            let size = ui.available_size().max(Vec2::splat(64.0));
            self.viewport_ui(ui, size);
        }
    }

    /// Random, Export, "Shading ▾" and Frame on the bar of the tab.
    pub fn toolbar_ui(&mut self, ui: &mut egui::Ui) {
        use crate::app::theme::{self, Icon, Tone};
        if theme::tool_button(ui, None, tr("character.random"), false, Tone::Plain)
            .on_hover_text(tr("character.random_hint"))
            .clicked()
        {
            self.randomise();
            self.rebuild();
        }
        if theme::tool_button(
            ui,
            Some(Icon::Export),
            tr("import.export_model"),
            false,
            Tone::Plain,
        )
        .on_hover_text(tr("import.export_model_hint"))
        .clicked()
        {
            self.export();
        }
        theme::menu_button(ui, None, tr("model.shading"), "", |ui| {
            let o = &mut self.scene.options;
            ui.checkbox(&mut o.surfaces, tr("model.game_shading"));
            ui.checkbox(&mut o.textures, tr("model.textures"));
            ui.checkbox(&mut o.lighting, tr("model.lighting"));
            ui.checkbox(&mut o.backface_culling, tr("model.cull"));
        });
        if theme::icon_button(ui, Icon::Frame, tr("model.frame_hint")) {
            self.frame_selected();
        }
    }

    /// F: frames the character.
    pub fn frame_selected(&mut self) {
        if let Some(Ok(loaded)) = &self.loaded
            && !loaded.bounds.is_empty()
        {
            self.camera.frame(&loaded.bounds);
        }
    }

    /// The materials of the selected part, for the Details panel.
    pub fn inspector_ui(&mut self, ui: &mut egui::Ui) {
        let Some(Ok(loaded)) = &self.loaded else {
            return;
        };
        let Some(i) = self.selected else { return };
        let (Some(model), Some(part)) = (loaded.models.get(i), loaded.character.parts.get(i))
        else {
            return;
        };
        ui.strong(trf("character.materials_of", &[("name", &part.name)]));
        for (k, info) in model.parts.iter().enumerate() {
            ui.separator();
            ui.label(
                RichText::new(trf(
                    "character.record",
                    &[("index", &k), ("name", &info.name)],
                ))
                .small(),
            );
            material_panel::show(ui, info, ("character details", i, k));
        }
    }

    fn choices(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        let db = &self.db;
        egui::Grid::new("character choices")
            .num_columns(2)
            .show(ui, |ui| {
                ui.label(tr("character.set"));
                let current = db.sets[self.set].name.clone();
                egui::ComboBox::from_id_salt("character set")
                    .selected_text(current)
                    .height(400.0)
                    .show_ui(ui, |ui| {
                        for (i, set) in db.sets.iter().enumerate() {
                            if ui.selectable_label(i == self.set, &set.name).clicked() {
                                self.set = i;
                                self.selection = Selection::default();
                                changed = true;
                            }
                        }
                    });
                ui.end_row();
                ui.label(if db.kind == 4 {
                    tr("character.damage")
                } else {
                    tr("character.detail")
                });
                ui.horizontal(|ui| {
                    let labels: &[&str] = if db.kind == 4 {
                        &["dmg0", "dmg1", "dmg2", "dmg3", "lod2"]
                    } else {
                        &["lod1", "lod2"]
                    };
                    for (i, label) in labels.iter().enumerate() {
                        changed |= ui.radio_value(&mut self.lod, i, *label).changed();
                    }
                });
                ui.end_row();
                let set = &db.sets[self.set];
                for choice in &set.choices {
                    if choice.variants.len() < 2 {
                        continue;
                    }
                    ui.label(&db.slots[choice.slot]);
                    let current = self
                        .selection
                        .variants
                        .get(&choice.slot)
                        .copied()
                        .filter(|v| choice.variants.contains(v))
                        .unwrap_or(choice.variants[0]);
                    egui::ComboBox::from_id_salt(("slot", choice.slot))
                        .selected_text(&db.variants[current].name)
                        .height(400.0)
                        .show_ui(ui, |ui| {
                            for &v in &choice.variants {
                                if ui
                                    .selectable_label(v == current, &db.variants[v].name)
                                    .clicked()
                                {
                                    self.selection.variants.insert(choice.slot, v);
                                    changed = true;
                                }
                            }
                        });
                    ui.end_row();
                }
                for (k, choice) in set.presets.iter().enumerate() {
                    if choice.items.len() < 2 {
                        continue;
                    }
                    let preset = &db.presets[choice.preset];
                    ui.label(trf("character.look", &[("name", &preset.name)]));
                    let current = self.selection.items.get(&k).copied().unwrap_or(0);
                    let name = |i: usize| preset.items[choice.items[i]].name.clone();
                    egui::ComboBox::from_id_salt(("preset", k))
                        .selected_text(name(current.min(choice.items.len() - 1)))
                        .height(400.0)
                        .show_ui(ui, |ui| {
                            for i in 0..choice.items.len() {
                                if ui.selectable_label(i == current, name(i)).clicked() {
                                    self.selection.items.insert(k, i);
                                    changed = true;
                                }
                            }
                        });
                    ui.end_row();
                }
                for m in &set.materials {
                    for c in &m.materials {
                        if c.candidates.len() < 2 {
                            continue;
                        }
                        ui.label(trf("character.colour", &[("part", &db.parts[m.part].name)]));
                        let key = (m.part, c.material.0);
                        let current = self.selection.materials.get(&key).copied().unwrap_or(0);
                        ui.horizontal(|ui| {
                            for i in 0..c.candidates.len() {
                                if ui
                                    .selectable_label(i == current, format!("{}", i + 1))
                                    .on_hover_text(trf(
                                        "model.material_line",
                                        &[("name", &c.candidates[i])],
                                    ))
                                    .clicked()
                                {
                                    self.selection.materials.insert(key, i);
                                    changed = true;
                                }
                            }
                        });
                        ui.end_row();
                    }
                }
            });
        changed
    }

    /// Choices, buttons and status; returns whether the character must be rebuilt.
    fn controls(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = self.choices(ui);
        ui.horizontal_wrapped(|ui| {
            if ui
                .button(tr("character.random"))
                .on_hover_text(tr("character.random_hint"))
                .clicked()
            {
                self.randomise();
                changed = true;
            }
            if ui
                .button(tr("import.export_model"))
                .on_hover_text(tr("import.export_model_hint"))
                .clicked()
            {
                self.export();
            }
            let o = &mut self.scene.options;
            ui.checkbox(&mut o.surfaces, tr("model.game_shading"));
            ui.checkbox(&mut o.textures, tr("model.textures"));
            ui.checkbox(&mut o.lighting, tr("model.lighting"));
            ui.checkbox(&mut o.backface_culling, tr("model.cull"));
            if ui.button(tr("model.frame")).clicked()
                && let Some(Ok(loaded)) = &self.loaded
                && !loaded.bounds.is_empty()
            {
                self.camera.frame(&loaded.bounds);
            }
        });
        if !self.status.is_empty() {
            ui.colored_label(Color32::YELLOW, &self.status);
        }
        changed
    }

    /// Whether a character is loaded; shows the error otherwise.
    fn view_ready(&self, ui: &mut egui::Ui) -> bool {
        match &self.loaded {
            Some(Err(error)) => {
                ui.colored_label(Color32::RED, error);
                false
            }
            None => false,
            Some(Ok(_)) => true,
        }
    }

    fn randomise(&mut self) {
        let mut seed = DefaultHasher::new();
        std::time::SystemTime::now().hash(&mut seed);
        let mut state = seed.finish() | 1;
        let mut next = |n: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % n.max(1) as u64) as usize
        };
        let set = &self.db.sets[self.set];
        let mut selection = Selection::default();
        for c in &set.choices {
            if !c.variants.is_empty() {
                selection
                    .variants
                    .insert(c.slot, c.variants[next(c.variants.len())]);
            }
        }
        for (k, c) in set.presets.iter().enumerate() {
            selection.items.insert(k, next(c.items.len()));
        }
        for m in &set.materials {
            for c in &m.materials {
                selection
                    .materials
                    .insert((m.part, c.material.0), next(c.candidates.len()));
            }
        }
        self.selection = selection;
    }

    fn export(&mut self) {
        let Some(Ok(loaded)) = &self.loaded else {
            return;
        };
        use doctor_toolkit::scene::{self, ModelFormat};
        let mut dialog =
            rfd::FileDialog::new().set_file_name(format!("{}.glb", loaded.character.name));
        for format in ModelFormat::WRITABLE {
            dialog = dialog.add_filter(format.label(), &[format.extension()]);
        }
        let Some(path) = dialog.save_file() else {
            return;
        };
        let character = loaded.character.clone();
        let format = ModelFormat::from_path(&path).unwrap_or(ModelFormat::Glb);
        let result = character
            .to_scene(self.assets())
            .and_then(|built| scene::write_file(&built, &path, Some(format)))
            .map_err(|e| e.to_string());
        self.status = match result {
            Ok(files) => trf(
                "model.exported",
                &[
                    ("path", &path.display()),
                    ("files", &trn("count.files", files.len() as u64, &[])),
                ],
            ),
            Err(e) => trf("model.cannot_export", &[("error", &e)]),
        };
    }

    fn viewport_ui(&mut self, ui: &mut egui::Ui, size: Vec2) {
        let Some(gpu) = self.gpu.clone() else { return };
        let Some(Ok(loaded)) = &self.loaded else {
            return;
        };
        let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
        self.camera.handle_input(ui, &response);
        let ppp = ui.ctx().pixels_per_point();
        let pixels = [
            (rect.width() * ppp).round().max(1.0) as u32,
            (rect.height() * ppp).round().max(1.0) as u32,
        ];
        let highlight = self.hovered.or(self.selected).map(|i| Pick {
            instance: i as u32,
            part: Pick::ALL_PARTS,
        });
        self.scene.highlight = highlight;
        self.scene.hidden_parts = loaded
            .models
            .iter()
            .zip(&self.hidden)
            .filter(|(_, h)| **h)
            .flat_map(|(m, _)| (0..m.mesh.parts.len() as u32).map(move |p| (m.key, p)))
            .collect();
        self.scene.options.game = crate::game_light::studio();
        let view = self.camera.view(rect.aspect_ratio());
        let key = RenderKey {
            camera: self.camera,
            size: pixels,
            options: self.scene.options,
            highlight,
            hidden: self.hidden.clone(),
            generation: self.generation,
        };
        {
            let mut renderer = gpu.lock();
            let mut missing = false;
            for m in &loaded.models {
                if !renderer.has_mesh(m.key) {
                    renderer.upload_mesh(m.key, m.mesh.clone());
                    missing = true;
                }
            }
            if missing || self.last_render.as_ref() != Some(&key) {
                renderer.render(&mut self.viewport, pixels, &self.scene, &view);
                self.last_render = Some(key);
            }
            if response.clicked()
                && let Some(pos) = response.interact_pointer_pos()
            {
                let local = (pos - rect.min) * ppp;
                let pixel = [local.x.max(0.0) as u32, local.y.max(0.0) as u32];
                let pick = renderer.pick(&mut self.viewport, &self.scene, &view, pixel);
                self.selected = pick.map(|p| p.instance as usize);
            }
        }
        if let Some(id) = self.viewport.texture_id() {
            ui.painter().image(
                id,
                rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }
    }

    fn part_list(&mut self, ui: &mut egui::Ui, materials: bool) {
        let Some(Ok(loaded)) = &self.loaded else {
            return;
        };
        let character = &loaded.character;
        self.hovered = None;
        let mut hovered = None;
        let mut clicked = None;
        egui::CollapsingHeader::new(trf("model.parts", &[("count", &loaded.models.len())]))
            .id_salt("character parts")
            .default_open(true)
            .show(ui, |ui| {
                for (i, (part, model)) in character.parts.iter().zip(&loaded.models).enumerate() {
                    ui.horizontal(|ui| {
                        let mut visible = !self.hidden[i];
                        if ui.checkbox(&mut visible, "").changed() {
                            self.hidden[i] = !visible;
                        }
                        let slot = self.db.parts[part.part]
                            .slot
                            .map_or("", |s| self.db.slots[s].as_str());
                        let label = format!("{:<12} {}  ({})", slot, part.name, model.name);
                        let response = ui.selectable_label(
                            self.selected == Some(i),
                            RichText::new(label).monospace(),
                        );
                        if response.hovered() {
                            hovered = Some(i);
                        }
                        if response.clicked() {
                            clicked = Some(i);
                        }
                    });
                }
                for &p in &character.assembly.culled {
                    ui.label(
                        RichText::new(trf(
                            "character.hidden_by_tags",
                            &[
                                ("name", &self.db.parts[p].name),
                                ("tags", &format!("{:?}", self.db.parts[p].tags)),
                            ],
                        ))
                        .weak(),
                    );
                }
                for (name, id) in &character.missing {
                    ui.colored_label(
                        Color32::YELLOW,
                        trf(
                            "character.mesh_missing",
                            &[("name", name), ("id", &format!("{id:08x}"))],
                        ),
                    );
                }
                let morphs: Vec<String> = character
                    .assembly
                    .morphs
                    .iter()
                    .filter(|m| m.1 != 0.0)
                    .map(|(n, w)| format!("{n} {w:.2}"))
                    .collect();
                if !morphs.is_empty() {
                    ui.label(
                        RichText::new(trf("character.morphs", &[("list", &morphs.join(", "))]))
                            .small(),
                    );
                }
            });
        if let Some(c) = clicked {
            self.selected = (self.selected != Some(c)).then_some(c);
        }
        self.hovered = hovered;
        if materials
            && let Some(i) = self.selected
            && let Some(model) = loaded.models.get(i)
        {
            egui::CollapsingHeader::new(trf(
                "character.materials_of",
                &[("name", &character.parts[i].name)],
            ))
            .id_salt("character materials")
            .default_open(true)
            .show(ui, |ui| {
                for (k, info) in model.parts.iter().enumerate() {
                    ui.separator();
                    ui.label(
                        RichText::new(trf(
                            "character.record",
                            &[("index", &k), ("name", &info.name)],
                        ))
                        .small(),
                    );
                    material_panel::show(ui, info, ("character", i, k));
                }
            });
        }
    }

    fn tree(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(tr("character.filter"));
            ui.text_edit_singleline(&mut self.filter);
        });
        let filter = self.filter.to_ascii_lowercase();
        let db = &self.db;
        let mut groups_of: BTreeMap<usize, Vec<&str>> = BTreeMap::new();
        for g in &db.groups {
            for &s in &g.sets {
                groups_of.entry(s).or_default().push(&g.name);
            }
        }
        let mut open_set = None;
        egui::ScrollArea::vertical()
            .id_salt("partsdb tree scroll")
            .max_height(480.0)
            .show(ui, |ui| {
                for (s, set) in db.sets.iter().enumerate() {
                    let matches = filter.is_empty()
                        || set.name.to_ascii_lowercase().contains(&filter)
                        || set.choices.iter().any(|c| {
                            c.variants.iter().any(|&v| {
                                db.variants[v].name.to_ascii_lowercase().contains(&filter)
                                    || db.variants[v].parts.iter().any(|&p| {
                                        db.parts[p as usize]
                                            .name
                                            .to_ascii_lowercase()
                                            .contains(&filter)
                                    })
                            })
                        });
                    if !matches {
                        continue;
                    }
                    let groups = groups_of.get(&s).map(|g| g.join(", ")).unwrap_or_default();
                    let header = egui::CollapsingHeader::new(trf(
                        "character.tree_set",
                        &[("name", &set.name), ("groups", &groups)],
                    ))
                    .id_salt(("tree set", s))
                    .show(ui, |ui| {
                        if ui.small_button(tr("character.preview_set")).clicked() {
                            open_set = Some(s);
                        }
                        for c in &set.choices {
                            egui::CollapsingHeader::new(format!(
                                "{} ({})",
                                db.slots[c.slot],
                                trn("count.variants", c.variants.len() as u64, &[])
                            ))
                            .id_salt(("tree slot", s, c.slot))
                            .show(ui, |ui| {
                                for &v in &c.variants {
                                    let variant = &db.variants[v];
                                    ui.label(RichText::new(&variant.name).strong());
                                    for &p in &variant.parts {
                                        let part = &db.parts[p as usize];
                                        let ids: Vec<String> =
                                            part.mesh_ids().map(|id| format!("{id:08x}")).collect();
                                        let tags = if part.tags.is_empty() {
                                            String::new()
                                        } else {
                                            format!(" [{}]", part.tags.join(", "))
                                        };
                                        ui.monospace(format!(
                                            "    {}{tags}  {}",
                                            part.name,
                                            ids.join(" ")
                                        ));
                                    }
                                }
                            });
                        }
                        for p in &set.presets {
                            let preset = &db.presets[p.preset];
                            let items: Vec<&str> = p
                                .items
                                .iter()
                                .map(|&i| preset.items[i].name.as_str())
                                .collect();
                            ui.label(trf(
                                "character.looks",
                                &[("name", &preset.name), ("items", &items.join(", "))],
                            ));
                        }
                    });
                    header.header_response.on_hover_text(trf(
                        "character.tree_set_hint",
                        &[
                            ("choices", &set.choices.len()),
                            ("looks", &set.presets.len()),
                            ("colours", &set.materials.len()),
                        ],
                    ));
                }
            });
        if let Some(s) = open_set {
            self.set = s;
            self.selection = Selection::default();
            self.rebuild();
        }
    }
}

/// Mesh key of a part drawn with `options`: resource location, swaps and morphs.
fn part_key(location: (usize, usize), options: &LoadOptions) -> MeshKey {
    let mut h = DefaultHasher::new();
    "character part".hash(&mut h);
    location.hash(&mut h);
    let swaps: BTreeMap<u32, u32> = options.swaps.iter().map(|(a, b)| (*a, *b)).collect();
    swaps.hash(&mut h);
    for (hash, w) in &options.morphs {
        hash.hash(&mut h);
        w.to_bits().hash(&mut h);
    }
    h.finish()
}

pub const VIEW: crate::views::ViewEntry = crate::views::ViewEntry {
    id: "character",
    load: load_view,
};

fn load_view(
    source: &mut crate::views::ViewSource<'_>,
) -> Option<Box<dyn crate::views::ResourceViewer>> {
    if source.type_id != TYPE_PARTSDB {
        return None;
    }
    let data = source.data(source.index).ok()?;
    let view = CharacterView::load(
        source.game,
        source.stream_index,
        source.type_id,
        &data,
        source.gpu,
    )?;
    Some(Box::new(view))
}

/// Choices and camera kept when the database is reloaded after an edit.
struct SavedState {
    set: usize,
    selection: Selection,
    lod: usize,
    camera: OrbitCamera,
    assembled: bool,
}

impl crate::views::ResourceViewer for CharacterView {
    fn show(
        &mut self,
        ui: &mut egui::Ui,
        cx: &mut crate::views::ViewContext<'_>,
    ) -> Option<crate::views::ViewAction> {
        if cx.in_tab() {
            self.show_tab(ui, cx.details);
        } else {
            CharacterView::show(self, ui);
        }
        None
    }

    fn tab_body(&self) -> crate::views::TabBody {
        crate::views::TabBody::Fill
    }

    fn toolbar(
        &mut self,
        ui: &mut egui::Ui,
        _cx: &mut crate::views::ViewContext<'_>,
    ) -> Option<crate::views::ViewAction> {
        self.toolbar_ui(ui);
        None
    }

    fn has_inspector(&self) -> bool {
        self.selected.is_some() && matches!(self.loaded, Some(Ok(_)))
    }

    fn inspector(
        &mut self,
        ui: &mut egui::Ui,
        _cx: &mut crate::views::ViewContext<'_>,
    ) -> Option<crate::views::ViewAction> {
        self.inspector_ui(ui);
        None
    }

    fn frame_selection(&mut self) -> bool {
        self.frame_selected();
        true
    }

    fn keys(&self) -> &'static [(&'static str, &'static str)] {
        crate::model_view::KEYS
    }

    fn state(&self) -> Option<Box<dyn std::any::Any>> {
        Some(Box::new(SavedState {
            set: self.set,
            selection: self.selection.clone(),
            lod: self.lod,
            camera: self.camera,
            assembled: matches!(self.loaded, Some(Ok(_))),
        }))
    }

    fn restore(&mut self, state: Box<dyn std::any::Any>) {
        let Ok(state) = state.downcast::<SavedState>() else {
            return;
        };
        if state.set >= self.db.sets.len() {
            return;
        }
        self.set = state.set;
        self.selection = state.selection;
        self.lod = state.lod;
        if state.assembled {
            self.rebuild();
            self.camera = state.camera;
        }
    }
}

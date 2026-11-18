//! Model preview: a GPU viewer with parts, materials and the skeleton, and the
//! software rasteriser as a fallback when no GPU renderer is available.
//!
//! `DOCTOR_TOOLKIT_GUI_MODEL=<options>` presets the viewer for screenshots:
//! comma-separated `wireframe`, `normals`, `skeleton`, `flat`
//! (no textures), `unlit`, `cull`, `weights=<bone>`, `select=<part>`,
//! `hide=<first>-<last>`, `yaw=<rad>`, `pitch=<rad>`, `zoom=<factor>`,
//! `collision` (Havok collision wireframe) and `spin` (turn every frame,
//! print frame intervals to stderr).
//!
//! In a tab of its own (stage 56; a floating window before) the viewer is
//! an asset editor: its switches are "Shading ▾" and "Show ▾" on the bar of
//! the tab, the parts and bones on the left, the viewport fills the rest,
//! and the material of the selected part is in the Details panel.

mod load;
pub mod material_panel;
mod software;

use std::time::{Duration, Instant};

use doctor_toolkit::export::gltf::AssetSource;
use doctor_toolkit::game::StreamFile;
use doctor_toolkit::resource::mesh::{Model, TYPE_MESH};
use doctor_toolkit::resource::treemodel::{TYPE_TREE, TreeModel};
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::{Color32, Pos2, Rect, RichText, Sense, Stroke, Vec2};

use crate::render::{
    Gpu, MeshKey, OrbitCamera, Pick, RenderStats, Scene, SceneOptions, View, Viewport, math,
};
use load::PartInfo;

pub use load::{LoadOptions, LoadedModel, load_limited, load_model, load_tree, load_with};

pub enum ModelPreview {
    Gpu(Box<GpuModelView>),
    Software(software::ModelPreview),
}

impl ModelPreview {
    /// Loads mesh resource `index`, or returns `None` if it is not a mesh.
    pub fn load(
        stream: &StreamFile,
        index: usize,
        assets: &mut dyn AssetSource,
        gpu: Option<&Gpu>,
    ) -> Option<Self> {
        if stream.layout().resources[index].header.type_id != TYPE_MESH {
            return None;
        }
        Some(match gpu {
            Some(gpu) => Self::Gpu(Box::new(GpuModelView::load(stream, index, assets, gpu))),
            None => Self::Software(software::ModelPreview::load(stream, index, assets)?),
        })
    }

    /// Loads tree model resource `index` (`treemodel`); drawn on the GPU only.
    pub fn load_tree(
        stream: &StreamFile,
        index: usize,
        assets: &mut dyn AssetSource,
        gpu: &Gpu,
    ) -> Option<Self> {
        if stream.layout().resources[index].header.type_id != TYPE_TREE {
            return None;
        }
        let started = Instant::now();
        let loaded = stream
            .resource_data(index)
            .and_then(|data| TreeModel::parse(&data))
            .map(|tree| {
                let name = tree
                    .source_path()
                    .rsplit(['\\', '/'])
                    .nth(1)
                    .unwrap_or("tree");
                let name = name.to_owned();
                load::load_tree(
                    &tree,
                    &name,
                    load::mesh_key(stream, index),
                    assets,
                    &mut gpu.lock(),
                    None,
                )
            })
            .map_err(|e| e.to_string());
        Some(Self::Gpu(Box::new(GpuModelView::new(
            loaded,
            started.elapsed(),
            gpu,
        ))))
    }

    /// A preview of a model that is not in a stream; `key` must be unique to it.
    pub fn from_model(
        model: &Model,
        key: MeshKey,
        assets: &mut dyn AssetSource,
        gpu: Option<&Gpu>,
    ) -> Self {
        match gpu {
            Some(gpu) => {
                let started = Instant::now();
                let loaded = load::load_parsed(model, key, assets, &mut gpu.lock())
                    .map_err(|e| e.to_string());
                Self::Gpu(Box::new(GpuModelView::new(loaded, started.elapsed(), gpu)))
            }
            None => Self::Software(software::ModelPreview::from_model(model, assets)),
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        match self {
            Self::Gpu(view) => view.show(ui),
            Self::Software(view) => {
                ui.label(RichText::new(tr("model.software_note")).weak());
                view.show(ui);
            }
        }
    }
}

/// The keys of the model viewer, for the "?" of its tab.
pub const KEYS: &[(&str, &str)] = &[
    ("LMB drag", "keys.orbit"),
    ("RMB / MMB drag, Shift", "keys.pan"),
    ("Wheel", "keys.zoom"),
    ("Click", "keys.select_part"),
    ("Double click", "keys.frame_part"),
];

/// Everything that changes the rendered image; the view is redrawn only when it changes.
#[derive(Clone, PartialEq)]
struct RenderKey {
    camera: OrbitCamera,
    size: [u32; 2],
    options: SceneOptions,
    highlight: Option<Pick>,
    hidden: Vec<bool>,
    collision: bool,
}

pub struct GpuModelView {
    gpu: Gpu,
    model: Result<LoadedModel, String>,
    load_time: Duration,
    camera: OrbitCamera,
    scene: Scene,
    viewport: Viewport,
    hidden: Vec<bool>,
    show_skeleton: bool,
    /// Collision of the model (through a metamodel of its stream), and whether it is shown.
    collision: Option<Result<crate::collision_view::Wire, String>>,
    show_collision: bool,
    selected_part: Option<u32>,
    /// Part under the pointer in the viewport (from the id pass).
    hovered_part: Option<u32>,
    /// Part under the pointer in the part list.
    list_hover: Option<u32>,
    hovered_bone: Option<usize>,
    /// Bone whose weights are shown while nothing else is hovered.
    weight_bone: Option<usize>,
    last_render: Option<RenderKey>,
    last_pick: Option<Pos2>,
    stats: RenderStats,
    /// Turn the camera every frame and report frame intervals on stderr.
    spin: Option<Vec<f32>>,
}

impl GpuModelView {
    pub fn load(
        stream: &StreamFile,
        index: usize,
        assets: &mut dyn AssetSource,
        gpu: &Gpu,
    ) -> Self {
        let started = Instant::now();
        let model = load_model(stream, index, assets, &mut gpu.lock()).map_err(|e| e.to_string());
        let mut view = Self::new(model, started.elapsed(), gpu);
        view.collision = crate::collision_view::for_mesh(stream, index);
        if let Ok(preset) = std::env::var("DOCTOR_TOOLKIT_GUI_MODEL") {
            view.show_collision = preset.split(',').any(|o| o.trim() == "collision");
        }
        view
    }

    fn new(model: Result<LoadedModel, String>, load_time: Duration, gpu: &Gpu) -> Self {
        let (camera, parts, key) = match &model {
            Ok(model) => (
                OrbitCamera::framing(&model.mesh.bounds()),
                model.parts.len(),
                model.key,
            ),
            Err(_) => (OrbitCamera::default(), 0, 0),
        };
        let mut view = Self {
            gpu: gpu.clone(),
            model,
            load_time,
            camera,
            scene: Scene::single(key),
            viewport: Viewport::new(),
            hidden: vec![false; parts],
            show_skeleton: false,
            collision: None,
            show_collision: false,
            selected_part: None,
            hovered_part: None,
            list_hover: None,
            hovered_bone: None,
            weight_bone: None,
            last_render: None,
            last_pick: None,
            stats: RenderStats::default(),
            spin: None,
        };
        if let Ok(preset) = std::env::var("DOCTOR_TOOLKIT_GUI_MODEL") {
            view.apply_preset(&preset);
        }
        view
    }

    fn apply_preset(&mut self, preset: &str) {
        for item in preset.split(',').map(str::trim) {
            let (name, value) = item.split_once('=').unwrap_or((item, ""));
            let number = value.parse::<f32>().ok();
            let options = &mut self.scene.options;
            match name {
                "spin" => self.spin = Some(Vec::new()),
                "wireframe" => options.wireframe = true,
                "normals" => options.normals = true,
                "skeleton" => {
                    self.show_skeleton = true;
                    self.frame(None);
                }
                "flat" => options.textures = false,
                "unlit" => options.lighting = false,
                "cull" => options.backface_culling = true,
                "weights" => self.weight_bone = value.parse().ok(),
                "select" => self.selected_part = value.parse().ok(),
                "hide" => {
                    if let Some((a, b)) = value.split_once('-')
                        && let (Ok(a), Ok(b)) = (a.parse::<usize>(), b.parse::<usize>())
                    {
                        for i in a..=b.min(self.hidden.len().saturating_sub(1)) {
                            self.hidden[i] = true;
                        }
                    }
                }
                "yaw" => self.camera.yaw = number.unwrap_or(self.camera.yaw),
                "pitch" => self.camera.pitch = number.unwrap_or(self.camera.pitch),
                "zoom" => self.camera.distance /= number.unwrap_or(1.0).max(0.01),
                _ => {}
            }
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        if let Err(error) = &self.model {
            ui.colored_label(Color32::RED, trf("model.cannot_load", &[("error", error)]));
            return;
        }
        self.header(ui);
        self.controls(ui);
        let width = ui.available_width().max(64.0);
        let height = (width * 0.75).clamp(160.0, 640.0);
        self.viewport_ui(ui, egui::vec2(width, height));
        self.hover_line(ui);
        self.lists(ui, false, true);
    }

    /// The viewer in a tab of its own: the parts and bones on the left, the
    /// viewport in the rest; `details`: the selected part's material is in
    /// the Details panel, not under the lists.
    pub fn show_tab(&mut self, ui: &mut egui::Ui, details: bool) {
        if let Err(error) = &self.model {
            ui.colored_label(Color32::RED, trf("model.cannot_load", &[("error", error)]));
            return;
        }
        egui::Panel::left(ui.scope_id().with("model parts"))
            .resizable(true)
            .default_size(300.0)
            .max_size(560.0)
            .show(ui, |ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                self.header(ui);
                egui::ScrollArea::vertical()
                    .id_salt("model tab lists")
                    .show(ui, |ui| self.lists(ui, true, !details));
            });
        let size = ui.available_size().max(Vec2::splat(64.0));
        self.viewport_ui(ui, size);
    }

    /// The switches of the viewer on the bar of its tab: "Shading ▾",
    /// "Show ▾" and Frame.
    pub fn toolbar_ui(&mut self, ui: &mut egui::Ui) {
        use crate::app::theme::{self, Icon};
        let has_bones = self.model.as_ref().is_ok_and(|m| !m.bones.is_empty());
        theme::menu_button(ui, None, tr("model.shading"), "", |ui| {
            let o = &mut self.scene.options;
            ui.checkbox(&mut o.textures, tr("model.textures"));
            ui.checkbox(&mut o.surfaces, tr("model.game_shading"))
                .on_hover_text(tr("model.game_shading_hint"));
            ui.checkbox(&mut o.lighting, tr("model.lighting"));
            ui.checkbox(&mut o.alpha_test, tr("model.alpha_test"))
                .on_hover_text(tr("model.alpha_test_hint"));
            ui.checkbox(&mut o.vertex_colors, tr("model.vertex_colors"));
            ui.checkbox(&mut o.backface_culling, tr("model.cull"))
                .on_hover_text(tr("model.cull_hint"));
            ui.separator();
            ui.checkbox(&mut o.wireframe, tr("model.wireframe"));
            ui.checkbox(&mut o.normals, tr("model.normals"));
        });
        let mut reframe = false;
        theme::menu_button(ui, None, tr("view.show"), "", |ui| {
            reframe = ui
                .add_enabled(
                    has_bones,
                    egui::Checkbox::new(&mut self.show_skeleton, tr("model.skeleton")),
                )
                .on_disabled_hover_text(tr("model.no_skeleton"))
                .changed();
            ui.add_enabled(
                matches!(self.collision, Some(Ok(_))),
                egui::Checkbox::new(&mut self.show_collision, tr("model.collision")),
            );
            ui.separator();
            if ui.button(tr("model.show_all")).clicked() {
                self.hidden.fill(false);
            }
            if ui.button(tr("model.hide_all")).clicked() {
                self.hidden.fill(true);
            }
        });
        if reframe {
            self.frame(None);
        }
        if theme::icon_button(ui, Icon::Frame, tr("model.frame_hint")) {
            self.frame(self.selected_part);
        }
    }

    /// The selected part, for the Details panel.
    pub fn has_selection(&self) -> bool {
        self.selected_part.is_some() && self.model.is_ok()
    }

    /// The selected part's material and details, for the Details panel.
    pub fn inspector_ui(&mut self, ui: &mut egui::Ui) {
        let Ok(model) = &self.model else { return };
        let Some(part) = self.selected_part.and_then(|p| model.parts.get(p as usize)) else {
            return;
        };
        ui.strong(trf("model.material_of", &[("name", &part.name)]));
        material_panel::show(ui, part, ("model", "details"));
        egui::CollapsingHeader::new(tr("model.part"))
            .id_salt("model part details")
            .show(ui, |ui| part_details(ui, part, model));
    }

    /// F: frames the selected part, or the model.
    pub fn frame_selected(&mut self) {
        self.frame(self.selected_part);
    }

    fn header(&self, ui: &mut egui::Ui) {
        let Ok(model) = &self.model else { return };
        ui.horizontal_wrapped(|ui| {
            ui.strong(&model.name);
            let mut text = trf(
                "model.summary",
                &[
                    ("parts", &trn("count.parts", model.parts.len() as u64, &[])),
                    (
                        "triangles",
                        &trn("count.triangles", model.mesh.triangle_count() as u64, &[]),
                    ),
                    (
                        "materials",
                        &trn(
                            "model.textured_materials",
                            model.textured_materials as u64,
                            &[],
                        ),
                    ),
                ],
            );
            match &model.skeleton_name {
                Some(name) => {
                    text += &trf(
                        "model.summary_skeleton",
                        &[
                            ("name", name),
                            ("bones", &trn("count.bones", model.bones.len() as u64, &[])),
                        ],
                    )
                }
                None if model.bone_table > 0 => {
                    text += &trf(
                        "model.summary_no_skeleton",
                        &[("bones", &trn("count.bones", model.bone_table as u64, &[]))],
                    )
                }
                None => {}
            }
            ui.label(RichText::new(text).weak());
        });
        ui.label(
            RichText::new(trf(
                "model.stats",
                &[
                    (
                        "load",
                        &format!("{:.0}", self.load_time.as_secs_f64() * 1000.0),
                    ),
                    ("gpu", &self.gpu.summary()),
                    (
                        "frame",
                        &format!("{:.2}", self.stats.cpu_time.as_secs_f64() * 1000.0),
                    ),
                    (
                        "calls",
                        &trn("model.draw_calls", self.stats.draw_calls as u64, &[]),
                    ),
                    ("msaa", &self.gpu.lock().sample_count()),
                ],
            ))
            .weak()
            .small(),
        );
    }

    fn controls(&mut self, ui: &mut egui::Ui) {
        let has_bones = self.model.as_ref().is_ok_and(|m| !m.bones.is_empty());
        ui.horizontal_wrapped(|ui| {
            let o = &mut self.scene.options;
            ui.checkbox(&mut o.textures, tr("model.textures"));
            ui.checkbox(&mut o.surfaces, tr("model.game_shading"))
                .on_hover_text(tr("model.game_shading_hint"));
            ui.checkbox(&mut o.lighting, tr("model.lighting"));
            ui.checkbox(&mut o.alpha_test, tr("model.alpha_test"))
                .on_hover_text(tr("model.alpha_test_hint"));
            ui.checkbox(&mut o.vertex_colors, tr("model.vertex_colors"));
            ui.checkbox(&mut o.backface_culling, tr("model.cull"))
                .on_hover_text(tr("model.cull_hint"));
            ui.checkbox(&mut o.wireframe, tr("model.wireframe"));
            ui.checkbox(&mut o.normals, tr("model.normals"));
            if ui
                .add_enabled(
                    has_bones,
                    egui::Checkbox::new(&mut self.show_skeleton, tr("model.skeleton")),
                )
                .on_disabled_hover_text(tr("model.no_skeleton"))
                .changed()
            {
                self.frame(None);
            }
            let collision = ui.add_enabled(
                matches!(self.collision, Some(Ok(_))),
                egui::Checkbox::new(&mut self.show_collision, tr("model.collision")),
            );
            match &self.collision {
                Some(Ok(wire)) => collision.on_hover_text(trf(
                    "model.collision_hint",
                    &[(
                        "triangles",
                        &trn("count.triangles", wire.triangles as u64, &[]),
                    )],
                )),
                Some(Err(e)) => collision.on_disabled_hover_text(e.as_str()),
                None => collision.on_disabled_hover_text(tr("model.no_collision")),
            };
            if ui
                .button(tr("model.frame"))
                .on_hover_text(tr("model.frame_hint"))
                .clicked()
            {
                self.frame(None);
            }
        });
        ui.label(RichText::new(tr("model.mouse_hint")).weak().small());
    }

    /// Frames a part, or the visible model.
    fn frame(&mut self, part: Option<u32>) {
        let Ok(model) = &self.model else { return };
        let mut bounds = math::Aabb::EMPTY;
        for (i, p) in model.mesh.parts.iter().enumerate() {
            if part.map_or(!self.hidden[i], |only| only as usize == i) {
                bounds.union(&p.bounds);
            }
        }
        if bounds.is_empty() {
            bounds = model.mesh.bounds();
        }
        if part.is_none() && self.show_skeleton {
            for bone in &model.bones {
                bounds.add_point(bone.position);
            }
        }
        self.camera.frame(&bounds);
        // Keep the whole model inside the clip planes when framing a part.
        self.camera.scene_radius = self.camera.scene_radius.max(model.mesh.bounds().radius());
    }

    /// Turns the camera and prints frame interval statistics every 300 frames.
    fn spin(&mut self, ui: &egui::Ui) {
        let Some(intervals) = &mut self.spin else {
            return;
        };
        self.camera.yaw += 0.01;
        intervals.push(ui.input(|i| i.unstable_dt));
        if intervals.len() == 300 {
            let mut sorted = intervals.clone();
            sorted.sort_by(f32::total_cmp);
            eprintln!(
                "model view: {} frames, interval median {:.2} ms, p99 {:.2} ms, \
                 last render {:.2} ms CPU",
                sorted.len(),
                sorted[sorted.len() / 2] * 1000.0,
                sorted[sorted.len() * 99 / 100] * 1000.0,
                self.stats.cpu_time.as_secs_f64() * 1000.0
            );
            intervals.clear();
        }
        ui.ctx().request_repaint();
    }

    fn viewport_ui(&mut self, ui: &mut egui::Ui, size: Vec2) {
        self.spin(ui);
        let (model_key, mesh) = match &self.model {
            Ok(model) => (model.key, model.mesh.clone()),
            Err(_) => return,
        };
        let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
        self.camera.handle_input(ui, &response);
        if response.hovered() && ui.input(|i| i.key_pressed(egui::Key::F)) {
            self.frame(None);
        }
        let ppp = ui.ctx().pixels_per_point();
        let pixels = [
            (rect.width() * ppp).round().max(1.0) as u32,
            (rect.height() * ppp).round().max(1.0) as u32,
        ];
        let highlight = self
            .list_hover
            .or(self.hovered_part)
            .or(self.selected_part)
            .map(|part| Pick { instance: 0, part });
        self.scene.highlight = highlight;
        let weights = self.hovered_bone.or(self.weight_bone);
        self.scene.options.bone_weights = weights.and_then(|b| u8::try_from(b).ok());
        self.scene.hidden_parts = self
            .hidden
            .iter()
            .enumerate()
            .filter(|(_, h)| **h)
            .map(|(i, _)| (model_key, i as u32))
            .collect();
        self.scene.options.game = crate::game_light::studio();
        let view = self.camera.view(rect.aspect_ratio());
        let key = RenderKey {
            camera: self.camera,
            size: pixels,
            options: self.scene.options,
            highlight,
            hidden: self.hidden.clone(),
            collision: self.show_collision,
        };

        {
            let mut renderer = self.gpu.lock();
            if !renderer.has_mesh(model_key) {
                renderer.upload_mesh(model_key, mesh.clone());
                self.last_render = None;
            }
            if self.last_render.as_ref() != Some(&key) {
                self.scene.overlay_lines = match (&self.collision, self.show_collision) {
                    (Some(Ok(wire)), true) => wire.lines(None),
                    _ => Vec::new(),
                };
                renderer.render(&mut self.viewport, pixels, &self.scene, &view);
                self.stats = renderer.stats;
                self.last_render = Some(key);
            }
            // Pick the part under a resting pointer.
            match response.hover_pos() {
                Some(pos) if !response.dragged() => {
                    if self.last_pick != Some(pos) {
                        self.last_pick = Some(pos);
                        let local = (pos - rect.min) * ppp;
                        let pixel = [local.x.max(0.0) as u32, local.y.max(0.0) as u32];
                        self.hovered_part = if renderer.is_software() {
                            let uv = (pos - rect.min) / rect.size();
                            let meshes = std::collections::HashMap::from([(model_key, &*mesh)]);
                            let ray = view.ray([uv.x, uv.y]);
                            self.scene.ray_pick(&ray, &meshes).map(|p| p.part)
                        } else {
                            renderer
                                .pick(&mut self.viewport, &self.scene, &view, pixel)
                                .map(|p| p.part)
                        };
                    }
                }
                _ => {
                    self.hovered_part = None;
                    self.last_pick = None;
                }
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
        self.hovered_bone = None;
        if self.show_skeleton {
            self.skeleton_overlay(ui, rect, &view, response.hover_pos());
        }
        if response.clicked() {
            self.selected_part = self.hovered_part;
        }
        if response.double_clicked() {
            self.frame(self.hovered_part);
        }
        self.viewport_label(ui, rect);
    }

    /// Bones as lines between joints, the joint under the pointer named.
    fn skeleton_overlay(&mut self, ui: &egui::Ui, rect: Rect, view: &View, pointer: Option<Pos2>) {
        let Ok(model) = &self.model else { return };
        let painter = ui.painter_at(rect);
        let screen: Vec<Option<Pos2>> = model
            .bones
            .iter()
            .map(|b| {
                view.project(b.position)
                    .map(|(uv, _)| rect.min + Vec2::new(uv[0], uv[1]) * rect.size())
            })
            .collect();
        let line = Stroke::new(1.5, Color32::from_rgb(255, 225, 120));
        for (i, bone) in model.bones.iter().enumerate() {
            if let (Some(parent), Some(p)) = (bone.parent, screen[i])
                && let Some(q) = screen[parent]
            {
                painter.line_segment([q, p], line);
            }
        }
        let hovered = pointer.and_then(|pointer| {
            screen
                .iter()
                .enumerate()
                .filter_map(|(i, p)| p.map(|p| (i, p.distance(pointer))))
                .filter(|(_, d)| *d < 7.0)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i)
        });
        for (i, p) in screen.iter().enumerate() {
            let Some(p) = *p else { continue };
            let active = Some(i) == hovered || Some(i) == self.weight_bone;
            painter.circle(
                p,
                if active { 5.0 } else { 3.0 },
                if active {
                    Color32::from_rgb(255, 90, 30)
                } else {
                    Color32::from_rgb(255, 160, 40)
                },
                Stroke::new(1.0, Color32::BLACK),
            );
        }
        if let (Some(i), Some(p)) = (hovered, hovered.and_then(|i| screen[i])) {
            let bone = &model.bones[i];
            let parent = bone
                .parent
                .map_or(tr("model.root").to_owned(), |p| model.bones[p].name.clone());
            let text = trf(
                "model.bone_label",
                &[("index", &i), ("name", &bone.name), ("parent", &parent)],
            );
            let galley =
                painter.layout_no_wrap(text, egui::FontId::proportional(13.0), Color32::WHITE);
            let at = p + Vec2::new(8.0, -8.0 - galley.size().y);
            painter.rect_filled(
                Rect::from_min_size(at, galley.size()).expand(3.0),
                3.0,
                Color32::from_black_alpha(190),
            );
            painter.galley(at, galley, Color32::WHITE);
            self.hovered_bone = Some(i);
        }
    }

    /// Names of the part under the pointer, drawn into the viewport corner.
    fn viewport_label(&self, ui: &egui::Ui, rect: Rect) {
        let Ok(model) = &self.model else { return };
        let Some(part) = self.hovered_part.or(self.selected_part) else {
            return;
        };
        let Some(info) = model.parts.get(part as usize) else {
            return;
        };
        let mut text = format!("{part}: {}", info.name);
        text += "\n";
        text += &trf("model.material_line", &[("name", &material_name(info))]);
        for (role, name) in &info.textures {
            text += &format!("\n{role}: {name}");
        }
        let painter = ui.painter_at(rect);
        let galley = painter.layout_no_wrap(text, egui::FontId::monospace(12.0), Color32::WHITE);
        let at = rect.left_top() + Vec2::splat(6.0);
        painter.rect_filled(
            Rect::from_min_size(at, galley.size()).expand(4.0),
            3.0,
            Color32::from_black_alpha(170),
        );
        painter.galley(at, galley, Color32::WHITE);
    }

    fn hover_line(&self, ui: &mut egui::Ui) {
        let text = match (&self.model, self.hovered_bone.or(self.weight_bone)) {
            (Ok(model), Some(bone)) if self.scene.options.bone_weights.is_some() => trf(
                "model.weights_of",
                &[("index", &bone), ("name", &model.bones[bone].name)],
            ),
            _ => String::new(),
        };
        if !text.is_empty() {
            ui.label(RichText::new(text).small());
        }
    }

    /// The part list with visibility, materials and skinning, and the bone list.
    fn lists(&mut self, ui: &mut egui::Ui, in_window: bool, material: bool) {
        let Ok(model) = &self.model else { return };
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        self.list_hover = None;
        let mut list_hover = None;
        let mut clicked = None;
        let mut weight_click = None;
        let mut bone_hover = None;
        egui::CollapsingHeader::new(trf("model.parts", &[("count", &model.parts.len())]))
            .id_salt(("model parts", in_window))
            .default_open(true)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui.small_button(tr("model.show_all")).clicked() {
                        self.hidden.fill(false);
                    }
                    if ui.small_button(tr("model.hide_all")).clicked() {
                        self.hidden.fill(true);
                    }
                    if let Some(selected) = self.selected_part
                        && ui.small_button(tr("model.isolate")).clicked()
                    {
                        for (i, h) in self.hidden.iter_mut().enumerate() {
                            *h = i != selected as usize;
                        }
                    }
                });
                let row_height = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
                let scroll = egui::ScrollArea::vertical()
                    .id_salt(("model part rows", in_window))
                    .auto_shrink([false, true]);
                let scroll = if in_window {
                    scroll
                } else {
                    scroll.max_height(260.0)
                };
                scroll.show_rows(ui, row_height, model.parts.len(), |ui, rows| {
                    for i in rows {
                        let info = &model.parts[i];
                        ui.horizontal(|ui| {
                            let mut visible = !self.hidden[i];
                            if ui.checkbox(&mut visible, "").changed() {
                                self.hidden[i] = !visible;
                            }
                            let label = format!(
                                "{i:3} {}  {}",
                                info.name,
                                info.material.as_deref().unwrap_or("?")
                            );
                            let selected = self.selected_part == Some(i as u32)
                                || self.hovered_part == Some(i as u32);
                            let response = ui
                                .selectable_label(selected, RichText::new(label).monospace())
                                .on_hover_ui(|ui| part_details(ui, info, model));
                            if response.hovered() {
                                list_hover = Some(i as u32);
                            }
                            if response.clicked() {
                                clicked = Some(i as u32);
                            }
                        });
                    }
                });
            });
        if !material {
        } else if let Some(part) = self.selected_part.and_then(|p| model.parts.get(p as usize)) {
            egui::CollapsingHeader::new(trf("model.material_of", &[("name", &part.name)]))
                .id_salt(("model material", in_window))
                .default_open(true)
                .show(ui, |ui| {
                    material_panel::show(ui, part, ("model", in_window))
                });
        } else {
            ui.label(RichText::new(tr("model.select_part")).weak());
        }
        if !model.bones.is_empty() {
            egui::CollapsingHeader::new(trf("model.bones", &[("count", &model.bones.len())]))
                .id_salt(("model bones", in_window))
                .show(ui, |ui| {
                    ui.label(RichText::new(tr("model.bones_hint")).weak().small());
                    let row_height = ui.text_style_height(&egui::TextStyle::Body) + 2.0;
                    egui::ScrollArea::vertical()
                        .id_salt(("model bone rows", in_window))
                        .max_height(if in_window { f32::INFINITY } else { 220.0 })
                        .show_rows(ui, row_height, model.bones.len(), |ui, rows| {
                            for i in rows {
                                let bone = &model.bones[i];
                                let depth = depth_of(&model.bones, i);
                                let text = format!("{}{i:3} {}", "  ".repeat(depth), bone.name);
                                let response = ui.selectable_label(
                                    self.weight_bone == Some(i),
                                    RichText::new(text).monospace(),
                                );
                                if response.hovered() {
                                    bone_hover = Some(i);
                                }
                                if response.clicked() {
                                    weight_click = Some(i);
                                }
                            }
                        });
                });
        }
        self.list_hover = list_hover;
        if let Some(part) = clicked {
            self.selected_part = (self.selected_part != Some(part)).then_some(part);
        }
        if let Some(bone) = weight_click {
            self.weight_bone = (self.weight_bone != Some(bone)).then_some(bone);
        }
        if bone_hover.is_some() {
            self.hovered_bone = bone_hover;
        }
    }
}

/// The name of a part's material, or its hash when no material has it.
fn material_name(info: &PartInfo) -> String {
    info.material.clone().unwrap_or_else(|| {
        trf(
            "model.not_found",
            &[("hash", &format!("{:08x}", info.material_hash))],
        )
    })
}

fn depth_of(bones: &[load::BoneInfo], mut bone: usize) -> usize {
    let mut depth = 0;
    while let Some(parent) = bones[bone].parent {
        bone = parent;
        depth += 1;
        if depth > 64 {
            break;
        }
    }
    depth
}

fn part_details(ui: &mut egui::Ui, info: &PartInfo, model: &LoadedModel) {
    egui::Grid::new("part details")
        .num_columns(2)
        .show(ui, |ui| {
            let bone_name = |b: u8| {
                model
                    .bones
                    .get(b as usize)
                    .map_or(trf("model.bone_n", &[("index", &b)]), |bone| {
                        format!("{b} {}", bone.name)
                    })
            };
            let mut rows = vec![
                (tr("model.part"), info.name.clone()),
                (tr("model.material"), material_name(info)),
            ];
            for (role, name) in &info.textures {
                rows.push((tr("model.texture"), format!("{role}: {name}")));
            }
            rows.push((tr("model.vertex_format"), info.format.to_owned()));
            rows.push((
                tr("model.size"),
                format!(
                    "{}, {}",
                    trn("count.vertices", info.vertices as u64, &[]),
                    trn("count.triangles", info.triangles as u64, &[])
                ),
            ));
            let skinning = tr("model.skinning");
            match (info.influences, info.rigid_bone) {
                (_, Some(bone)) => rows.push((
                    skinning,
                    trf("model.skinning_rigid", &[("bone", &bone_name(bone))]),
                )),
                (Some(n), None) => {
                    rows.push((skinning, trn("model.skinning_up_to", n as u64, &[])))
                }
                (None, None) => rows.push((skinning, tr("model.skinning_static").to_owned())),
            }
            if !info.palette.is_empty() {
                let names: Vec<String> = info.palette.iter().map(|&b| bone_name(b)).collect();
                rows.push((tr("model.bone_palette"), names.join("\n")));
            }
            for (name, value) in rows {
                ui.label(name);
                ui.monospace(value);
                ui.end_row();
            }
        });
}

// ------------------------------------------------------------ view registry

/// Models (`mesh`): the GPU viewer, or the software preview without wgpu.
pub const VIEW: crate::views::ViewEntry = crate::views::ViewEntry {
    id: "model",
    load: load_view,
};

fn load_view(
    source: &mut crate::views::ViewSource<'_>,
) -> Option<Box<dyn crate::views::ResourceViewer>> {
    if source.type_id == TYPE_TREE {
        return ModelPreview::load_tree(source.stream, source.index, source.assets, source.gpu?)
            .map(|v| Box::new(v) as Box<dyn crate::views::ResourceViewer>);
    }
    if source.type_id != TYPE_MESH {
        return None;
    }
    if let Some(view) = load_edited(source) {
        return Some(Box::new(view));
    }
    ModelPreview::load(source.stream, source.index, source.assets, source.gpu)
        .map(|v| Box::new(v) as Box<dyn crate::views::ResourceViewer>)
}

impl crate::views::ResourceViewer for ModelPreview {
    fn show(
        &mut self,
        ui: &mut egui::Ui,
        cx: &mut crate::views::ViewContext<'_>,
    ) -> Option<crate::views::ViewAction> {
        match self {
            Self::Gpu(view) if cx.in_tab() => view.show_tab(ui, cx.details),
            _ => ModelPreview::show(self, ui),
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
        if let Self::Gpu(view) = self {
            view.toolbar_ui(ui);
        }
        None
    }

    fn has_inspector(&self) -> bool {
        matches!(self, Self::Gpu(view) if view.has_selection())
    }

    fn inspector(
        &mut self,
        ui: &mut egui::Ui,
        _cx: &mut crate::views::ViewContext<'_>,
    ) -> Option<crate::views::ViewAction> {
        if let Self::Gpu(view) = self {
            view.inspector_ui(ui);
        }
        None
    }

    fn frame_selection(&mut self) -> bool {
        match self {
            Self::Gpu(view) => {
                view.frame_selected();
                true
            }
            Self::Software(_) => false,
        }
    }

    fn keys(&self) -> &'static [(&'static str, &'static str)] {
        KEYS
    }
}

/// The mesh as the workspace has it, with the workspace's materials and
/// textures: an imported model shows before it is written into the game.
/// `None` when the workspace changes nothing a model is drawn with (the
/// game's data is shown then).
fn load_edited(source: &mut crate::views::ViewSource<'_>) -> Option<ModelPreview> {
    use std::hash::{Hash, Hasher};

    let workspace = source.workspace?;
    let index = source.index;
    let volatile = doctor_toolkit::resource::mesh::volatile_for(source.stream.layout(), index);
    let edited =
        source.edited(index).is_some() || volatile.is_some_and(|v| source.edited(v).is_some());
    let started = Instant::now();
    let data = source.data(index);
    let volatile_data = match volatile {
        Some(v) => source.data(v),
        None => Ok(Vec::new()),
    };
    let mut assets = doctor_toolkit::workspace::EditedAssets::new(workspace, &mut *source.assets);
    if !edited && assets.is_empty() {
        return None;
    }
    let model = data
        .and_then(|mesh| Ok((mesh, volatile_data?)))
        .and_then(|(mesh, volatile)| {
            // The renderer keeps meshes by key: this version has its own.
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            load::mesh_key(source.stream, index).hash(&mut hasher);
            mesh.hash(&mut hasher);
            volatile.hash(&mut hasher);
            Model::parse(&mesh, &volatile)
                .map(|model| (model, hasher.finish()))
                .map_err(|e| e.to_string())
        });
    Some(match (model, source.gpu) {
        (Ok((model, mut key)), Some(gpu)) => {
            // Materials are part of the uploaded mesh: a changed material
            // or texture of the workspace makes another one.
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            key.hash(&mut hasher);
            for record in &model.records {
                if let Some(material) = assets.material(record.material_hash) {
                    material.name.hash(&mut hasher);
                    for (_, texture, param) in material.textures() {
                        param.name.hash(&mut hasher);
                        texture.hash(&mut hasher);
                        assets.texture_version(texture).hash(&mut hasher);
                    }
                }
            }
            key = hasher.finish();
            let loaded = load::load_parsed(&model, key, &mut assets, &mut gpu.lock())
                .map_err(|e| e.to_string());
            let mut view = GpuModelView::new(loaded, started.elapsed(), gpu);
            view.collision = crate::collision_view::for_mesh(source.stream, index);
            if let Ok(preset) = std::env::var("DOCTOR_TOOLKIT_GUI_MODEL") {
                view.show_collision = preset.split(',').any(|o| o.trim() == "collision");
            }
            ModelPreview::Gpu(Box::new(view))
        }
        (Ok((model, _)), None) => {
            ModelPreview::Software(software::ModelPreview::from_model(&model, &mut assets))
        }
        (Err(error), Some(gpu)) => ModelPreview::Gpu(Box::new(GpuModelView::new(
            Err(error),
            started.elapsed(),
            gpu,
        ))),
        // Without the renderer the game's data is shown, with its error.
        (Err(_), None) => return None,
    })
}

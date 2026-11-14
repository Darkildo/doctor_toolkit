//! A cut-scene over the level (stage 62): the actors of the shot at the
//! play head (posed by their clips once the characters are loaded), the
//! camera of the shot as a pyramid, the camera paths of every shot with
//! points at their keys, and the path of the selected actor.
//!
//! The camera and the actors are picked in the view (or in the Outliner's
//! "Cut-scene" branch) and moved and turned by the level's gizmo; the edits
//! go back to the app as [`CineAction`]s, which writes keys of the shot at
//! the play head (`doctor_toolkit::cutscene::edit`). The play head is the
//! app's (`app/cinema.rs`): the Timeline panel, the cut-scene tabs and this
//! view share it.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use doctor_toolkit::cutscene::edit::{Target, key_at, key_times};
use doctor_toolkit::cutscene::world::{matrix_to_world, to_world};
use doctor_toolkit::cutscene::{Cutscene, actor_state, camera_state};
use doctor_toolkit::level::RowMatrix;
use doctor_toolkit::resource::trinity::{Command, Guid, eval_positions};
use doctor_toolkit_mods::i18n::{tr, trf};
use egui::{Color32, Pos2, Rect, RichText};

use crate::app::theme::{self, Tone};
use crate::cutscene_view::preview::{
    self, ActorOptions, CAMERA_COLOUR, Loaded, PATH_COLOUR, camera_path, frustum_lines, push_path,
};
use crate::render::{Gpu, LineVertex, Scene, View, math};

/// What of the cut-scene is selected: the camera of the shot at the play
/// head, or an actor (by GUID: the same actor in every shot it plays in).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Item {
    Camera,
    Actor([u8; 16]),
}

impl Item {
    /// What is keyed for it.
    pub fn target(self) -> Target {
        match self {
            Self::Camera => Target::Camera,
            Self::Actor(g) => Target::Actor(Guid(g)),
        }
    }
}

/// An edit or a command of the cut-scene for the app.
#[derive(Debug, Clone, PartialEq)]
pub enum CineAction {
    /// The item placed by this matrix (anchor space to world, column-major)
    /// at the play head: the gizmo let go.
    Transform { item: Item, matrix: [f32; 16] },
    /// The camera at the play head: x, y, z and its angles in degrees.
    Camera([f32; 6]),
    /// An actor at the play head: position and (or) rotation.
    Actor {
        guid: [u8; 16],
        position: Option<[f32; 3]>,
        rotation: Option<[f32; 4]>,
    },
    /// The focal length of the camera at the play head, millimetres.
    Focal(f32),
    /// A camera key from the view's camera: camera to world, vertical
    /// field of view in degrees.
    KeyFromView { matrix: [f32; 16], fov: f32 },
    /// Remove the keys of the item at the play head.
    DeleteKey(Item),
    /// Move the play head (seconds from the start of the cut-scene).
    Seek(f32),
    /// Play or pause.
    Play,
    /// Take the cut-scene off the view.
    Close,
}

/// Colour of the paths of the other shots.
const OTHER_PATH: [u8; 4] = [60, 110, 140, 255];
/// Colour of a selected item.
const SELECTED: [u8; 4] = [235, 170, 60, 255];
/// Colour of the keys; the key at the play head is white.
const KEY: [u8; 4] = [120, 220, 255, 255];
/// Colour of the path of the selected actor.
const ACTOR_PATH: [u8; 4] = [235, 200, 90, 255];
/// How near (points) a click must be to pick a camera or an actor.
const PICK_RADIUS: f32 = 14.0;

/// The cut-scene shown over a level view.
pub struct Layer {
    pub cutscene: Arc<Cutscene>,
    /// The play head, seconds from the start of the cut-scene (the app's).
    pub time: f32,
    pub playing: bool,
    pub selected: Option<Item>,
    /// The view looks through the camera of the shot.
    pub through_shot: bool,
    /// The item the gizmo drags and where it is now.
    pub dragged: Option<(Item, [f32; 16])>,
    /// The characters of the actors, once loaded (`Load characters`).
    pub characters: Option<Result<Loaded, String>>,
    /// Load characters was pressed: the app opens them.
    pub want_characters: bool,
    /// Put the view's camera on the shot's once the zones are loaded.
    pub frame: bool,
    /// Or here: eye and the point looked at (screenshots).
    pub frame_view: Option<([f32; 3], [f32; 3])>,
    /// What the app is doing for the cut-scene (choosing zones…).
    pub status: String,
}

impl Layer {
    pub fn new(cutscene: Arc<Cutscene>) -> Self {
        Self {
            cutscene,
            time: 0.0,
            playing: false,
            selected: None,
            through_shot: false,
            dragged: None,
            characters: None,
            want_characters: false,
            frame: true,
            frame_view: None,
            status: String::new(),
        }
    }

    /// The shot at the play head.
    pub fn shot(&self) -> Option<usize> {
        self.cutscene.shot_at(self.time)
    }

    /// The play head in the shot, seconds.
    pub fn local_time(&self) -> f32 {
        self.shot()
            .map_or(0.0, |s| self.time - self.cutscene.shots[s].start)
    }

    /// Where an item is at the play head (anchor space to world,
    /// column-major); the dragged one where the gizmo has it.
    pub fn matrix(&self, item: Item) -> Option<[f32; 16]> {
        if let Some((i, m)) = self.dragged
            && i == item
        {
            return Some(m);
        }
        let s = self.shot()?;
        let shot = &self.cutscene.shots[s];
        let anchor = self.cutscene.anchor(s);
        let local = self.time - shot.start;
        match item {
            Item::Camera => {
                let c = camera_state(shot, local)?;
                Some(matrix_to_world(&anchor, &c.matrix()))
            }
            Item::Actor(g) => {
                let a = actor_state(shot, &Guid(g), local)?;
                Some(matrix_to_world(&anchor, &a.matrix()))
            }
        }
    }

    /// The gizmo's matrix of an item (rows: axes, then the position).
    pub fn row_matrix(&self, item: Item) -> Option<RowMatrix> {
        let m = self.matrix(item)?;
        Some(std::array::from_fn(|r| {
            std::array::from_fn(|c| m[r * 4 + c])
        }))
    }

    /// The camera of the shot at the play head: camera to world and its
    /// vertical field of view in degrees.
    pub fn shot_camera(&self) -> Option<([f32; 16], f32)> {
        let c = self.cutscene.camera_at(self.time)?;
        Some((
            matrix_to_world(&self.cutscene.anchor(c.shot), &c.matrix()),
            c.vertical_fov(),
        ))
    }

    /// The items of the shot at the play head and the points that pick them.
    pub fn items(&self) -> Vec<(Item, Vec<[f32; 3]>)> {
        let Some(s) = self.shot() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if !self.through_shot
            && let Some(m) = self.matrix(Item::Camera)
        {
            out.push((Item::Camera, vec![[m[12], m[13], m[14]]]));
        }
        for actor in &self.cutscene.shots[s].actors {
            let item = Item::Actor(actor.guid.0);
            if let Some(m) = self.matrix(item) {
                let p = [m[12], m[13], m[14]];
                // Feet and chest: people are drawn about 1.9 m high.
                out.push((item, vec![p, math::add(p, [0.0, 1.0, 0.0])]));
            }
        }
        out
    }

    /// The item nearest to a point of the view (within [`PICK_RADIUS`]).
    pub fn pick(&self, view: &View, rect: Rect, pos: Pos2) -> Option<Item> {
        let mut best: Option<(f32, Item)> = None;
        for (item, points) in self.items() {
            for p in points {
                let Some((uv, depth)) = view.project(p) else {
                    continue;
                };
                if !(0.0..=1.0).contains(&depth) {
                    continue;
                }
                let at = Pos2::new(
                    rect.min.x + uv[0] * rect.width(),
                    rect.min.y + uv[1] * rect.height(),
                );
                let d = at.distance(pos);
                if d < PICK_RADIUS && best.is_none_or(|(b, _)| d < b) {
                    best = Some((d, item));
                }
            }
        }
        best.map(|(_, i)| i)
    }

    /// Name of an item: "Camera" or the actor's name.
    pub fn name(&self, item: Item) -> String {
        match item {
            Item::Camera => tr("cine.camera").to_owned(),
            Item::Actor(g) => self
                .cutscene
                .shots
                .iter()
                .flat_map(|s| &s.actors)
                .find(|a| a.guid.0 == g)
                .map_or_else(|| "?".to_owned(), |a| a.name.clone()),
        }
    }

    /// Whether the play head is on a key of the item.
    pub fn on_key(&self, item: Item) -> Option<f32> {
        let s = self.shot()?;
        key_at(
            self.cutscene.shots[s].block(),
            item.target(),
            self.local_time(),
        )
    }

    /// What changes the picture: a hash for the view's render key.
    pub fn revision(&self) -> u64 {
        let mut h = DefaultHasher::new();
        (Arc::as_ptr(&self.cutscene) as usize).hash(&mut h);
        self.time.to_bits().hash(&mut h);
        self.selected.hash(&mut h);
        self.through_shot.hash(&mut h);
        if let Some((i, m)) = &self.dragged {
            i.hash(&mut h);
            m.map(f32::to_bits).hash(&mut h);
        }
        match &self.characters {
            Some(Ok(l)) => l.actor_count().hash(&mut h),
            Some(Err(_)) => 1.hash(&mut h),
            None => 0.hash(&mut h),
        }
        h.finish()
    }

    /// Adds the actors to `scene.instances`, the paths to `lines` (tested
    /// against depth) and the cameras and keys to `overlay`.
    pub fn fill(
        &mut self,
        scene: &mut Scene,
        lines: &mut Vec<LineVertex>,
        overlay: &mut Vec<LineVertex>,
        gpu: Option<&Gpu>,
    ) {
        let cutscene = self.cutscene.clone();
        let Some(s) = cutscene.shot_at(self.time) else {
            return;
        };
        if let Some(Ok(loaded)) = &mut self.characters {
            preview::ensure_shot(loaded, gpu, true, &cutscene, s);
        }
        let loaded = match &self.characters {
            Some(Ok(l)) => Some(l),
            _ => None,
        };
        let options = ActorOptions {
            moved: match self.dragged {
                Some((Item::Actor(g), m)) => Some((g, m)),
                _ => None,
            },
            selected: match self.selected {
                Some(Item::Actor(g)) => Some(g),
                _ => None,
            },
        };
        let mut actors = Scene::default();
        preview::draw_actors(
            &mut actors,
            gpu,
            loaded,
            None,
            &cutscene,
            self.time,
            &options,
        );
        scene.instances.extend(actors.instances);
        overlay.extend(actors.overlay_lines);
        // Camera paths: every shot, the one at the play head brighter.
        for shot in 0..cutscene.shots.len() {
            let colour = if shot == s { PATH_COLOUR } else { OTHER_PATH };
            push_path(lines, &camera_path(&cutscene, shot, 10.0), colour);
        }
        // The keys of the camera of this shot.
        let shot = &cutscene.shots[s];
        let anchor = cutscene.anchor(s);
        let local = self.time - shot.start;
        for k in key_times(shot.block(), Target::Camera) {
            if let Some(c) = camera_state(shot, k) {
                let p = to_world(&anchor, [c.values[0], c.values[1], c.values[2]]);
                let colour = if (k - local).abs() < doctor_toolkit::cutscene::edit::KEY_TOLERANCE {
                    [255, 255, 255, 255]
                } else {
                    KEY
                };
                diamond(overlay, p, 0.12, colour);
            }
        }
        // The camera, seen from outside.
        if !self.through_shot
            && let (Some(m), Some(c)) = (self.matrix(Item::Camera), camera_state(shot, local))
        {
            let colour = if self.selected == Some(Item::Camera) {
                SELECTED
            } else {
                CAMERA_COLOUR
            };
            overlay.extend(frustum_lines(&m, c.vertical_fov(), c.aspect, 1.5, colour));
        }
        // The path of the selected actor and its keys.
        if let Some(Item::Actor(g)) = self.selected {
            let times = shot.block().times();
            for (command, &at) in shot.block().commands.iter().zip(&times) {
                let Command::ActorTransform {
                    actor, positions, ..
                } = command
                else {
                    continue;
                };
                if actor.0 != g || positions.is_empty() {
                    continue;
                }
                let end = positions.last().map_or(0.0, |k| k.time);
                let steps = ((end * 10.0).ceil() as usize).max(1);
                let path: Vec<[f32; 3]> = (0..=steps)
                    .filter_map(|i| {
                        let t = end * i as f32 / steps as f32;
                        Some(to_world(&anchor, eval_positions(positions, t)?))
                    })
                    .collect();
                push_path(overlay, &path, ACTOR_PATH);
                for k in positions {
                    let colour = if (at + k.time - local).abs()
                        < doctor_toolkit::cutscene::edit::KEY_TOLERANCE
                    {
                        [255, 255, 255, 255]
                    } else {
                        ACTOR_PATH
                    };
                    diamond(overlay, to_world(&anchor, k.position), 0.1, colour);
                }
            }
        }
    }

    /// The bar of the cut-scene at the bottom of the view: its name and
    /// shot, play, the play head, through the shot camera, characters, a
    /// key from the view and close.
    pub fn bar_ui(&mut self, ui: &mut egui::Ui, rect: Rect) -> Option<CineAction> {
        let mut action = None;
        let area = Rect::from_min_max(
            egui::pos2(rect.left() + 6.0, rect.bottom() - 34.0),
            egui::pos2(rect.right() - 6.0, rect.bottom() - 6.0),
        );
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(area)
                .layout(egui::Layout::left_to_right(egui::Align::Max)),
            |ui| {
                egui::Frame::new()
                    .fill(Color32::from_black_alpha(160))
                    .corner_radius(4)
                    .inner_margin(egui::Margin::symmetric(4, 2))
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        ui.horizontal(|ui| action = self.bar_buttons(ui));
                    });
            },
        );
        action
    }

    fn bar_buttons(&mut self, ui: &mut egui::Ui) -> Option<CineAction> {
        let mut action = None;
        let cutscene = self.cutscene.clone();
        let shot = self
            .shot()
            .map(|s| cutscene.shots[s].seq.source_name().to_owned())
            .unwrap_or_default();
        ui.label(RichText::new(trf("cine.bar_title", &[("name", &cutscene.name)])).strong())
            .on_hover_text(&cutscene.stream_name);
        ui.label(RichText::new(shot).weak());
        let icon = (!self.playing).then_some(theme::Icon::Play);
        let label = if self.playing {
            tr("cutscene.pause")
        } else {
            tr("cutscene.play")
        };
        if theme::tool_button(ui, icon, label, self.playing, Tone::Play).clicked() {
            action = Some(CineAction::Play);
        }
        let mut t = self.time;
        if ui
            .add(
                egui::DragValue::new(&mut t)
                    .range(0.0..=cutscene.duration())
                    .speed(0.02)
                    .max_decimals(2)
                    .suffix(tr("cutscene.unit_s")),
            )
            .changed()
        {
            action = Some(CineAction::Seek(t));
        }
        ui.checkbox(&mut self.through_shot, tr("cine.through_shot"))
            .on_hover_text(tr("cine.through_shot_hint"));
        match &self.characters {
            None if self.want_characters => {
                ui.spinner();
            }
            None => {
                // The app opens them: it has the game and its index.
                self.want_characters = ui
                    .button(tr("cutscene.load_characters"))
                    .on_hover_text(tr("cutscene.load_characters_hint"))
                    .clicked();
            }
            Some(Err(e)) => {
                ui.colored_label(Color32::YELLOW, tr("cine.no_characters"))
                    .on_hover_text(e);
            }
            Some(Ok(_)) => {}
        }
        if ui
            .button(tr("cine.key_from_view"))
            .on_hover_text(tr("cine.key_from_view_hint"))
            .clicked()
        {
            // The view fills in its camera.
            action = Some(CineAction::KeyFromView {
                matrix: math::IDENTITY,
                fov: 0.0,
            });
        }
        if !self.status.is_empty() {
            ui.spinner();
            ui.label(RichText::new(&self.status).weak());
        }
        if ui
            .small_button("×")
            .on_hover_text(tr("cine.close_hint"))
            .clicked()
        {
            action = Some(CineAction::Close);
        }
        action
    }
}

/// A small diamond (three crossed lines) at `p`.
fn diamond(out: &mut Vec<LineVertex>, p: [f32; 3], size: f32, colour: [u8; 4]) {
    for axis in 0..3 {
        let mut a = p;
        let mut b = p;
        a[axis] -= size;
        b[axis] += size;
        out.push(LineVertex::new(a, colour));
        out.push(LineVertex::new(b, colour));
    }
}

impl super::LevelView {
    /// Selects a camera or actor of the cut-scene (and no object of the
    /// level), or nothing of it.
    pub fn cine_select(&mut self, item: Option<Item>) {
        let Some(layer) = &mut self.cine else { return };
        if layer.selected == item {
            return;
        }
        layer.selected = item;
        if item.is_some() {
            self.filter.selected = None;
            self.filter.selection.clear();
            self.refill = true;
        }
        self.selection_serial += 1;
    }

    /// Puts the view's camera at `eye` looking at `look` (screenshots).
    pub fn place_camera(&mut self, eye: [f32; 3], look: [f32; 3]) {
        if let Some(layer) = &mut self.cine
            && (layer.frame || self.job.is_some() || self.level.is_none())
        {
            // Once the zones are loaded (their load frames the view).
            layer.frame = true;
            layer.frame_view = Some((eye, look));
            return;
        }
        self.camera
            .set_projection(super::camera::Projection::Perspective, 20.0);
        self.camera.eye = eye;
        self.camera.turn_to(look);
        self.camera_moved = true;
        self.refill = true;
    }

    /// Shows or hides the markers of objects without a model (screenshots).
    pub fn show_markers(&mut self, on: bool) {
        self.filter.markers = on;
        self.refill = true;
    }

    /// The selected camera or actor of the cut-scene.
    pub fn cine_selected(&self) -> Option<Item> {
        self.cine.as_ref()?.selected
    }

    /// The view's camera as a camera of a shot: camera to world
    /// (column-major, looking down −Z) and the vertical field of view in
    /// degrees.
    pub fn view_camera(&self) -> ([f32; 16], f32) {
        let f = self.camera.forward();
        let r = self.camera.right();
        let u = math::cross(r, f);
        let e = self.camera.eye;
        (
            [
                r[0], r[1], r[2], 0.0, //
                u[0], u[1], u[2], 0.0, //
                -f[0], -f[1], -f[2], 0.0, //
                e[0], e[1], e[2], 1.0,
            ],
            self.camera.fov_y.to_degrees(),
        )
    }

    /// A command of the cut-scene's bar with what the view knows: the
    /// view's camera for a key from the view.
    pub(super) fn fill_cine_action(&self, action: CineAction) -> CineAction {
        match action {
            CineAction::KeyFromView { .. } => {
                let (matrix, fov) = self.view_camera();
                CineAction::KeyFromView { matrix, fov }
            }
            other => other,
        }
    }

    /// The camera through the shot's, or on it once the zones are loaded.
    pub(super) fn cine_camera(&mut self) {
        let Some(layer) = &mut self.cine else { return };
        let frame = layer.frame && self.job.is_none() && self.level.is_some();
        if !(layer.through_shot || frame) {
            return;
        }
        let Some((m, fov)) = layer.shot_camera() else {
            return;
        };
        let forward = [-m[8], -m[9], -m[10]];
        let len = math::length(forward);
        if len <= 1e-6 {
            return;
        }
        let f = math::scale(forward, 1.0 / len);
        let before = self.camera;
        self.camera
            .set_projection(super::camera::Projection::Perspective, 20.0);
        self.camera.pitch = f[1].clamp(-1.0, 1.0).asin().clamp(-1.55, 1.55);
        // FlyCamera: yaw 0 looks along −Z, forward = (−sin, ·, −cos).
        self.camera.yaw = (-f[0]).atan2(-f[2]);
        let eye = [m[12], m[13], m[14]];
        if layer.through_shot {
            self.camera.eye = eye;
            self.camera.fov_y = fov.to_radians();
        } else {
            // Framing: where the shot camera is, seeing what it sees (flying
            // back shows the camera itself).
            self.camera.eye = eye;
            self.camera.fov_y = fov.to_radians().max(20f32.to_radians());
            layer.frame = false;
            self.camera_moved = true;
        }
        if let Some((eye, look)) = layer.frame_view.take().filter(|_| !layer.through_shot) {
            self.camera.eye = eye;
            self.camera.turn_to(look);
        }
        if self.camera != before {
            self.refill = true;
        }
    }

    /// Draws the cut-scene into the scene after the level's instances;
    /// returns its revision for the render key.
    pub(super) fn cine_scene(&mut self) -> u64 {
        self.scene.instances.truncate(self.base_instances);
        let Some(layer) = &mut self.cine else {
            self.cine_cache = None;
            return 0;
        };
        let revision = layer.revision();
        // Meshes the renderer dropped (posed actors) are made again.
        let dropped = match (&self.cine_cache, &self.gpu) {
            (Some((_, drawn)), Some(gpu)) => {
                let mut renderer = gpu.lock();
                drawn.instances.iter().any(|i| !renderer.has_mesh(i.mesh))
            }
            _ => false,
        };
        if dropped || self.cine_cache.as_ref().is_none_or(|(r, _)| *r != revision) {
            let mut scene = Scene::default();
            let mut drawn = super::CineDrawn::default();
            layer.fill(
                &mut scene,
                &mut drawn.lines,
                &mut drawn.overlay,
                self.gpu.as_ref(),
            );
            drawn.instances = scene.instances;
            self.cine_cache = Some((revision, drawn));
        }
        if let Some((_, drawn)) = &self.cine_cache {
            self.scene.instances.extend(drawn.instances.iter().cloned());
            self.scene.lines.extend(drawn.lines.iter().copied());
            self.scene
                .overlay_lines
                .extend(drawn.overlay.iter().copied());
        }
        revision
    }
}

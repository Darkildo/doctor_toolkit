//! What the keys and the mouse do in the level view (stage 54), as in
//! Unreal Engine: Q W E R and the space bar choose the gizmo (while the
//! right button is not held: then the letters fly the camera), F flies to
//! the selection, End puts it on the ground, Ctrl+A selects everything
//! shown, Esc nothing, Ctrl+D duplicates and Del removes it (one operation
//! each); a frame drawn with the left button selects what is inside; one
//! gizmo moves, turns or scales the whole selection about its centre, and
//! with Alt it moves copies; the right button without a drag opens the
//! context menu.

use std::collections::{BTreeSet, HashSet};
use std::time::{Duration, Instant};

use doctor_toolkit::level::RowMatrix;
use doctor_toolkit_mods::i18n::{tr, trn};
use egui::{Pos2, Rect};

use super::camera::FlyCamera;
use super::{Drag, LevelAction, LevelView, ObjRef, gizmo, math, nav, scene};
use crate::render::{Instance, LineVertex, View};

/// Objects Ctrl+A selects at most: more make every edit of the selection
/// slow and the Details of all of them useless.
pub const SELECT_ALL_LIMIT: usize = 1000;
/// How long a note shows in the view.
const NOTE_TIME: Duration = Duration::from_secs(4);
/// The tint of the copies an Alt + drag moves.
const GHOST_TINT: [f32; 4] = [0.6, 1.1, 1.4, 1.0];

/// A frame drawn with the left button over empty space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Marquee {
    pub start: Pos2,
    pub now: Pos2,
    pub op: MarqueeOp,
}

/// What a frame does with what is inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarqueeOp {
    /// Selects it alone.
    Replace,
    /// Shift: adds it to the selection.
    Add,
    /// Ctrl: takes it from the selection.
    Remove,
}

impl Marquee {
    pub fn new(start: Pos2, ctrl: bool, shift: bool) -> Self {
        let op = if shift {
            MarqueeOp::Add
        } else if ctrl {
            MarqueeOp::Remove
        } else {
            MarqueeOp::Replace
        };
        Self {
            start,
            now: start,
            op,
        }
    }

    pub fn rect(&self) -> Rect {
        Rect::from_two_pos(self.start, self.now)
    }
}

/// What the context menu was opened on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Context {
    /// The object under the pointer (it is selected).
    pub object: Option<ObjRef>,
    /// The object's origin, or the camera: "Play from here" starts on the
    /// ground below it.
    pub at: [f32; 3],
}

/// The commands of the context menu of the view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextItem {
    FlyTo,
    Duplicate,
    Delete,
    DropToGround,
    ShowInOutliner,
    ShowLogic,
    CopyGuid,
    PlayHere,
    SelectAll,
}

/// Where the gizmo is and what it moves.
pub(super) struct Target {
    pub origin: math::Vec3,
    pub axes: [math::Vec3; 3],
    pub mode: gizmo::Mode,
    /// The objects it moves and their transforms now.
    pub objects: Vec<(ObjRef, RowMatrix)>,
    /// A node of a navigation graph instead.
    pub node: Option<nav::NodeRef>,
    /// A camera or actor of the cut-scene instead, and where it is.
    pub cine: Option<(super::cine::Item, RowMatrix)>,
}

/// One edit, or one operation of several.
pub(super) fn many(mut actions: Vec<LevelAction>, label: String) -> Option<LevelAction> {
    match actions.len() {
        0 => None,
        1 => actions.pop(),
        _ => Some(LevelAction::Many { actions, label }),
    }
}

impl LevelView {
    /// The selected objects the gizmo moves: placed and not locked.
    pub(super) fn gizmo_objects(&self) -> Vec<(ObjRef, RowMatrix)> {
        let mut objects: Vec<ObjRef> = self
            .filter
            .selection
            .iter()
            .copied()
            .filter(|&r| !self.is_object_locked(r))
            .collect();
        objects.sort_unstable();
        objects
            .into_iter()
            .filter_map(|r| Some((r, self.object_transform(r)?)))
            .collect()
    }

    /// The gizmo: at the centre of the selected objects (the axes of the
    /// primary one for local axes), at the selected node of a navigation
    /// graph, or where a drag has it.
    pub(super) fn gizmo_target(&self) -> Option<Target> {
        if let Some(drag) = &self.drag {
            return Some(Target {
                origin: drag.origin(),
                axes: drag.axes,
                mode: drag.mode,
                objects: Vec::new(),
                node: drag.node,
                cine: None,
            });
        }
        let objects = self.gizmo_objects();
        if !objects.is_empty() {
            let mut bounds = math::Aabb::EMPTY;
            for (_, m) in &objects {
                bounds.add_point([m[3][0], m[3][1], m[3][2]]);
            }
            let main = objects
                .iter()
                .find(|(r, _)| Some(*r) == self.filter.selected)
                .unwrap_or(&objects[0])
                .1;
            return Some(Target {
                origin: bounds.center(),
                axes: gizmo::axes(self.gizmo, self.space, &main),
                mode: self.gizmo,
                objects,
                node: None,
                cine: None,
            });
        }
        // A camera or actor of the cut-scene: moved or turned (scaling
        // means nothing to it).
        if let Some(layer) = &self.cine
            && let Some(item) = layer.selected
            && let Some(m) = layer.row_matrix(item)
            && self.filter.selected.is_none()
        {
            let mode = match self.gizmo {
                gizmo::Mode::Scale => gizmo::Mode::Move,
                mode => mode,
            };
            return Some(Target {
                origin: [m[3][0], m[3][1], m[3][2]],
                axes: gizmo::axes(mode, self.space, &m),
                mode,
                objects: Vec::new(),
                node: None,
                cine: Some((item, m)),
            });
        }
        let node = self
            .nav
            .selected
            .filter(|_| self.filter.selected.is_none() && self.nav.visible)?;
        Some(Target {
            origin: self.nav.position(node)?,
            axes: gizmo::WORLD,
            mode: gizmo::Mode::Move,
            objects: Vec::new(),
            node: Some(node),
            cine: None,
        })
    }

    /// The keys over the view.
    pub(super) fn view_keys(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
    ) -> Option<LevelAction> {
        let over =
            response.hovered() || response.has_focus() || response.is_pointer_button_down_on();
        if !over || ui.ctx().egui_wants_keyboard_input() {
            return None;
        }
        let right = FlyCamera::right_held(ui, response);
        struct Keys {
            focus: bool,
            escape: bool,
            delete: bool,
            duplicate: bool,
            all: bool,
            ground: bool,
            modes: [bool; 4],
            space: bool,
            digits: [bool; 3],
        }
        let keys = ui.input(|i| {
            use egui::Key;
            let plain = |key| i.key_pressed(key) && !i.modifiers.any();
            let command = |key| {
                i.key_pressed(key) && i.modifiers.command && !i.modifiers.alt && !i.modifiers.shift
            };
            Keys {
                focus: plain(Key::F),
                escape: i.key_pressed(Key::Escape),
                delete: i.key_pressed(Key::Delete),
                duplicate: command(Key::D),
                all: command(Key::A),
                ground: plain(Key::End),
                modes: [Key::Q, Key::W, Key::E, Key::R].map(plain),
                space: plain(Key::Space),
                digits: [Key::Num1, Key::Num2, Key::Num3].map(plain),
            }
        });
        let mut action = None;
        if keys.focus {
            self.focus_selection();
        }
        if keys.escape {
            if self.drag.is_some() {
                self.cancel_drag();
            } else if self.marquee.is_some() {
                self.marquee = None;
            } else {
                self.clear_selection();
                self.nav.select(None);
                self.cine_select(None);
            }
        }
        if keys.delete {
            action = match self.cine.as_ref().and_then(|l| l.selected) {
                Some(item) if self.filter.selected.is_none() => Some(LevelAction::Cutscene(
                    super::cine::CineAction::DeleteKey(item),
                )),
                _ => self.edit_selection(true),
            };
        }
        if keys.duplicate {
            action = self.edit_selection(false);
        }
        if keys.all {
            self.select_all();
        }
        if keys.ground {
            self.request_drop();
        }
        // Q W E R and the space bar as in UE; the letters fly while the
        // right button is held, and the camera as before flies with them.
        if !self.classic && !right {
            let modes = [
                gizmo::Mode::Select,
                gizmo::Mode::Move,
                gizmo::Mode::Rotate,
                gizmo::Mode::Scale,
            ];
            for (pressed, mode) in keys.modes.iter().zip(modes) {
                if *pressed {
                    self.set_gizmo_mode(mode);
                }
            }
            if keys.space {
                self.set_gizmo_mode(self.gizmo.next());
            }
        }
        let modes = [gizmo::Mode::Move, gizmo::Mode::Rotate, gizmo::Mode::Scale];
        for (pressed, mode) in keys.digits.iter().zip(modes) {
            if *pressed {
                self.set_gizmo_mode(mode);
            }
        }
        action
    }

    /// Esc during a drag: everything goes back.
    fn cancel_drag(&mut self) {
        let Some(drag) = self.drag.take() else { return };
        if let Some(layer) = &mut self.cine {
            layer.dragged = None;
        }
        if drag.node.is_some() {
            self.nav.preview = None;
            self.nav.generation += 1;
        } else if !drag.copy {
            for (r, m) in &drag.objects {
                if let Some(indices) = self.by_object.get(r) {
                    scene::move_items(&mut self.items, indices, m);
                }
            }
        }
        self.refill = true;
    }

    /// The edit a finished drag of the gizmo makes: one operation for every
    /// object it changed (copies with Alt).
    pub(super) fn finish_drag(&mut self, drag: Drag) -> Option<LevelAction> {
        if let Some((item, start, current)) = drag.cine {
            if let Some(layer) = &mut self.cine {
                layer.dragged = None;
            }
            return (current != start).then(|| {
                LevelAction::Cutscene(super::cine::CineAction::Transform {
                    item,
                    matrix: scene::flatten(&current),
                })
            });
        }
        if let Some(n) = drag.node {
            let to = drag.origin();
            if to != drag.pivot {
                match self.nav.moved(n, to) {
                    Ok(a) => return Some(a),
                    Err(e) => self.status = e,
                }
            } else {
                self.nav.preview = None;
            }
            return None;
        }
        let level = self.level.as_ref()?;
        let mut actions = Vec::new();
        for ((r, start), m) in drag.objects.iter().zip(&drag.current) {
            if m == start {
                continue;
            }
            let group = level.groups.get(r.group)?.location;
            actions.push(if drag.copy {
                LevelAction::DuplicateTo {
                    group,
                    object: r.object,
                    matrix: *m,
                }
            } else {
                LevelAction::SetTransform {
                    group,
                    object: r.object,
                    matrix: *m,
                }
            });
        }
        let n = actions.len() as u64;
        let label = if drag.copy {
            trn("view.copied_label", n, &[])
        } else {
            trn("view.transformed_label", n, &[])
        };
        many(actions, label)
    }

    /// Selects what a frame holds: the objects drawn (not hidden, not
    /// locked) whose centre is inside it.
    pub(super) fn select_in(&mut self, marquee: &Marquee, view: &View, rect: Rect) {
        let frame = marquee.rect();
        let mut inside: BTreeSet<ObjRef> = BTreeSet::new();
        for &i in &self.visible {
            let Some(item) = self.items.get(i) else {
                continue;
            };
            let Some(r) = item.object else { continue };
            if inside.contains(&r) || self.locked.contains(r, item.class) {
                continue;
            }
            let centre = self
                .spheres
                .get(&item.mesh)
                .map(|(c, _)| math::transform_point(&item.world, *c))
                .unwrap_or([item.world[12], item.world[13], item.world[14]]);
            if let Some((uv, depth)) = view.project(centre)
                && (0.0..=1.0).contains(&depth)
            {
                let p = Pos2::new(
                    rect.min.x + uv[0] * rect.width(),
                    rect.min.y + uv[1] * rect.height(),
                );
                if frame.contains(p) {
                    inside.insert(r);
                }
            }
        }
        let mut set = self.selection();
        match marquee.op {
            MarqueeOp::Replace => set = inside,
            MarqueeOp::Add => set.extend(inside),
            MarqueeOp::Remove => set.retain(|r| !inside.contains(r)),
        }
        let primary = self.filter.selected.filter(|r| set.contains(r));
        self.nav.select(None);
        self.set_selection(primary, set);
    }

    /// Ctrl+A: every object of the loaded zones that is shown and not
    /// locked, up to [`SELECT_ALL_LIMIT`].
    pub fn select_all(&mut self) {
        let Some(level) = &self.level else { return };
        let mut all: Vec<ObjRef> = self
            .by_object
            .keys()
            .copied()
            .filter(|&r| {
                level
                    .groups
                    .get(r.group)
                    .and_then(|g| g.objects.get(r.object))
                    .is_some_and(|o| {
                        !self.filter.hidden.contains(r, o.class)
                            && !self.filter.hidden_classes.contains(&o.class)
                            && !self.locked.contains(r, o.class)
                    })
            })
            .collect();
        all.sort_unstable();
        if all.len() > SELECT_ALL_LIMIT {
            self.show_note(trn(
                "view.select_limit",
                all.len() as u64,
                &[("limit", &SELECT_ALL_LIMIT)],
            ));
            all.truncate(SELECT_ALL_LIMIT);
        }
        let primary = self
            .filter
            .selected
            .filter(|r| all.binary_search(r).is_ok());
        self.set_selection(primary, all.into_iter().collect());
    }

    /// End: puts the selected objects on what is below them (the
    /// collision of the level, built first when it is not).
    pub fn request_drop(&mut self) {
        if !self.gizmo_objects().is_empty() {
            self.drop_pending = true;
        }
    }

    /// The edit of End once the collision is there.
    pub(super) fn drop_to_ground(&mut self) -> Option<LevelAction> {
        if !self.drop_pending {
            return None;
        }
        let objects = self.gizmo_objects();
        if objects.is_empty() {
            self.drop_pending = false;
            return None;
        }
        let ready = {
            let (level, input) = (self.level.as_deref()?, self.input.as_ref()?);
            if !self.walk.ready_for(level) {
                let mut bounds = math::Aabb::EMPTY;
                for (_, m) in &objects {
                    bounds.add_point([m[3][0], m[3][1], m[3][2]]);
                }
                self.walk
                    .prepare(level, &self.catalog, &input.parents, bounds.center());
                self.walk.poll_build();
            }
            self.walk.ready_for(level)
        };
        if !ready {
            if self.walk.building() {
                self.show_note(tr("view.ground_building").to_owned());
                self.ctx.request_repaint_after(Duration::from_millis(100));
            } else {
                self.drop_pending = false;
                self.show_note(tr("view.ground_failed").to_owned());
            }
            return None;
        }
        self.drop_pending = false;
        let skip: HashSet<(usize, usize)> =
            objects.iter().map(|(r, _)| (r.group, r.object)).collect();
        let renderer = self.gpu.as_ref().map(|gpu| gpu.lock());
        let mut actions = Vec::new();
        for (r, m) in &objects {
            // The lowest point of the object's meshes, or its origin.
            let mut bounds = math::Aabb::EMPTY;
            if let (Some(indices), Some(renderer)) = (self.by_object.get(r), &renderer) {
                for &i in indices {
                    let item = &self.items[i];
                    if let Some(mesh) = renderer.mesh_data(item.mesh) {
                        bounds.union(&mesh.bounds().transformed(&item.world));
                    }
                }
            }
            let (x, bottom, z) = if bounds.is_empty() {
                (m[3][0], m[3][1], m[3][2])
            } else {
                let c = bounds.center();
                (c[0], bounds.min[1], c[2])
            };
            let pass = |o: (usize, usize)| skip.contains(&o);
            let ground = self
                .walk
                .ground_below([x, bottom + 0.05, z], &pass)
                .or_else(|| self.walk.ground_below([x, bottom + 1000.0, z], &pass));
            let Some(ground) = ground else { continue };
            if (ground - bottom).abs() < 1e-3 {
                continue;
            }
            let mut moved = *m;
            moved[3][1] += ground - bottom;
            let Some((group, object)) = self.location_of(*r) else {
                continue;
            };
            actions.push(LevelAction::SetTransform {
                group,
                object,
                matrix: moved,
            });
        }
        drop(renderer);
        if actions.is_empty() {
            self.show_note(tr("view.ground_none").to_owned());
        }
        let n = actions.len() as u64;
        many(actions, trn("view.dropped_label", n, &[]))
    }

    /// The right button was clicked: the object under the pointer is
    /// selected (unless it is already) and the menu is for it.
    pub(super) fn open_context(&mut self, picked: Option<ObjRef>) {
        if let Some(r) = picked
            && !self.is_selected(r)
        {
            self.select(Some(r));
        }
        let at = picked
            .or(self.filter.selected)
            .and_then(|r| self.object_transform(r))
            .map_or(self.camera.eye, |m| [m[3][0], m[3][1], m[3][2]]);
        self.context = Some(Context { object: picked, at });
    }

    /// Whether the context menu was opened (and for what).
    #[cfg(test)]
    pub fn context(&self) -> Option<Context> {
        self.context
    }

    /// The context menu of the view.
    pub(super) fn context_ui(&mut self, ui: &mut egui::Ui) -> Option<LevelAction> {
        ui.set_min_width(220.0);
        let count = self.selection().len();
        let many = |key: &str| {
            if count > 1 {
                format!("{} ({count})", tr(key))
            } else {
                tr(key).to_owned()
            }
        };
        let mut chosen = None;
        let mut item = |ui: &mut egui::Ui, item: ContextItem, text: &str, keys: &str, on: bool| {
            let mut button = egui::Button::new(text);
            if !keys.is_empty() {
                button = button.shortcut_text(keys);
            }
            if ui.add_enabled(on, button).clicked() {
                chosen = Some(item);
            }
        };
        let selected = count > 0;
        item(ui, ContextItem::FlyTo, tr("outliner.focus"), "F", true);
        item(
            ui,
            ContextItem::Duplicate,
            &many("outliner.duplicate"),
            "Ctrl+D",
            selected,
        );
        item(
            ui,
            ContextItem::Delete,
            &many("outliner.delete"),
            "Del",
            selected,
        );
        item(
            ui,
            ContextItem::DropToGround,
            &many("view.drop_to_ground"),
            "End",
            selected,
        );
        ui.separator();
        item(
            ui,
            ContextItem::ShowInOutliner,
            tr("view.show_in_outliner"),
            "",
            selected,
        );
        item(
            ui,
            ContextItem::ShowLogic,
            tr("outliner.show_logic"),
            "",
            self.filter.selected.is_some(),
        );
        item(
            ui,
            ContextItem::CopyGuid,
            &many("outliner.copy_guid"),
            "",
            selected,
        );
        item(
            ui,
            ContextItem::SelectAll,
            tr("view.select_all"),
            "Ctrl+A",
            true,
        );
        ui.separator();
        item(ui, ContextItem::PlayHere, tr("gametab.play_here"), "", true);
        let item = chosen?;
        ui.close();
        let ctx = ui.ctx().clone();
        self.context_command(item, &ctx)
    }

    /// A command of the context menu.
    pub fn context_command(
        &mut self,
        item: ContextItem,
        ctx: &egui::Context,
    ) -> Option<LevelAction> {
        let context = self.context.take();
        match item {
            ContextItem::FlyTo => match context.and_then(|c| c.object) {
                Some(r) => self.focus_on(r),
                None => self.focus_selection(),
            },
            ContextItem::Duplicate => return self.edit_selection(false),
            ContextItem::Delete => return self.edit_selection(true),
            ContextItem::DropToGround => self.request_drop(),
            ContextItem::ShowInOutliner => return Some(LevelAction::ShowInOutliner),
            ContextItem::ShowLogic => {
                let (group, object) = self.selected_location()?;
                return Some(LevelAction::ShowLogic { group, object });
            }
            ContextItem::CopyGuid => {
                let guids: Vec<String> = self
                    .selection()
                    .into_iter()
                    .filter_map(|r| Some(self.object(r)?.guid.to_string()))
                    .collect();
                ctx.copy_text(guids.join("\n"));
            }
            ContextItem::SelectAll => self.select_all(),
            ContextItem::PlayHere => {
                // The ground below the object or the camera (the next
                // frame, the collision may still be built).
                self.play_here = true;
                self.play_here_at = context.map(|c| c.at);
            }
        }
        None
    }

    /// The copies an Alt + drag moves, drawn where they go.
    pub(super) fn add_ghosts(&mut self) {
        let Some(drag) = self.drag.as_ref().filter(|d| d.copy) else {
            return;
        };
        for ((r, _), m) in drag.objects.iter().zip(&drag.current) {
            let Some(indices) = self.by_object.get(r) else {
                continue;
            };
            for &i in indices {
                let item = &self.items[i];
                let world = scene::flatten(&doctor_toolkit::level::compose(&item.local, m));
                let mut tint = item.tint;
                for (t, g) in tint.iter_mut().zip(GHOST_TINT) {
                    *t *= g;
                }
                self.scene.instances.push(Instance {
                    mesh: item.mesh,
                    transform: world,
                    tint,
                });
            }
        }
    }

    /// The grid: at the height of the selected object (or low among the
    /// objects) under where the camera looks, or in the plane of a front or
    /// side view; a line every 1, 10 or 100 m as the camera is near or far,
    /// every tenth brighter.
    pub(super) fn grid_lines(&self) -> Vec<LineVertex> {
        let camera = &self.camera;
        let forward = camera.forward();
        let ground = self
            .filter
            .selected
            .and_then(|r| self.object_transform(r))
            .map_or(self.ground_y, |m| m[3][1]);
        let (normal, centre, distance) = match camera.projection {
            super::Projection::Front => ([0.0, 0.0, 1.0], camera.eye, camera.ortho_half),
            super::Projection::Side => ([1.0, 0.0, 0.0], camera.eye, camera.ortho_half),
            super::Projection::Top => (
                [0.0, 1.0, 0.0],
                [camera.eye[0], ground, camera.eye[2]],
                camera.ortho_half,
            ),
            super::Projection::Perspective => {
                let height = camera.eye[1] - ground;
                let at = if forward[1] < -0.05 {
                    let t = (height / -forward[1]).min(camera.far);
                    math::add(camera.eye, math::scale(forward, t))
                } else {
                    let ahead = math::add(camera.eye, math::scale(forward, 50.0));
                    [ahead[0], ground, ahead[2]]
                };
                let d = math::length(math::sub(at, camera.eye)).max(height.abs());
                ([0.0, 1.0, 0.0], [at[0], ground, at[2]], d)
            }
        };
        let cell = 10f32
            .powf((distance.max(1.0)).log10().floor() - 1.0)
            .max(0.1);
        let (u, v): (math::Vec3, math::Vec3) = if normal[1] == 1.0 {
            ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0])
        } else if normal[2] == 1.0 {
            ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0])
        } else {
            ([0.0, 0.0, 1.0], [0.0, 1.0, 0.0])
        };
        const HALF: i32 = 50;
        // Lines at whole cells, so they stay put as the camera moves.
        let snap = |x: f32| (x / cell).round() as i32;
        let cu = snap(math::dot(centre, u));
        let cv = snap(math::dot(centre, v));
        let base = math::scale(normal, math::dot(centre, normal));
        let point = |a: i32, b: i32| {
            math::add(
                base,
                math::add(
                    math::scale(u, a as f32 * cell),
                    math::scale(v, b as f32 * cell),
                ),
            )
        };
        let color = |k: i32| {
            if k % 10 == 0 {
                [150, 150, 160, 255]
            } else {
                [80, 82, 90, 255]
            }
        };
        let mut out = Vec::with_capacity((4 * HALF as usize + 2) * 2);
        for k in -HALF..=HALF {
            let c = color(cu + k);
            out.push(LineVertex::new(point(cu + k, cv - HALF), c));
            out.push(LineVertex::new(point(cu + k, cv + HALF), c));
            let c = color(cv + k);
            out.push(LineVertex::new(point(cu - HALF, cv + k), c));
            out.push(LineVertex::new(point(cu + HALF, cv + k), c));
        }
        out
    }

    /// Where on the screen axis `axis` of the gizmo is, `along` its length
    /// from the centre (tests drag it).
    #[cfg(test)]
    pub fn gizmo_point(&self, axis: usize, along: f32) -> Option<Pos2> {
        let t = self.gizmo_target()?;
        let view = self.camera.view(self.view_rect.aspect_ratio());
        let size = gizmo::size(&view, t.origin);
        self.screen_of(math::add(t.origin, math::scale(t.axes[axis], size * along)))
    }

    /// The note shown now (tests).
    #[cfg(test)]
    pub fn note(&self) -> Option<&str> {
        self.current_note()
    }

    /// Whether End waits for the collision (tests).
    #[cfg(test)]
    pub fn drop_waits(&self) -> bool {
        self.drop_pending
    }

    /// The camera (tests).
    #[cfg(test)]
    pub fn camera(&self) -> super::camera::FlyCamera {
        self.camera
    }

    /// The camera, to change (tests).
    #[cfg(test)]
    pub fn camera_mut(&mut self) -> &mut super::camera::FlyCamera {
        &mut self.camera
    }

    /// Shows a short message in the view.
    pub(super) fn show_note(&mut self, text: String) {
        self.note = Some((text, Instant::now()));
        self.ctx.request_repaint_after(NOTE_TIME);
    }

    /// The note, while it shows.
    pub(super) fn current_note(&self) -> Option<&str> {
        self.note
            .as_ref()
            .filter(|(_, at)| at.elapsed() < NOTE_TIME)
            .map(|(text, _)| text.as_str())
    }
}

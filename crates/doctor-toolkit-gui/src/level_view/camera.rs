//! The camera of the level view: a fly camera in perspective, or a look
//! straight from above, the front or the side (orthographic, stage 54).
//!
//! Two ways to drive it ([`FlyCamera::handle_input`]):
//!
//! - **as in Unreal Engine** (stage 54): the right button held looks round
//!   and W A S D Q E fly (the wheel then changes the speed); without it the
//!   wheel moves forward and back, the arrows and Page Up / Down move;
//!   middle drag pans; left drag in empty space moves forward and turns;
//!   Alt + left drag orbits the pivot, Alt + right drag moves to it;
//! - **as before** (the setting "Camera: as before"): right or middle drag
//!   looks round, W A S D Q E always fly, the wheel moves, Alt + left drag
//!   orbits.
//!
//! The orthographic views pan with the right or middle button and zoom
//! with the wheel either way.

use crate::render::View;
use crate::render::math::{self, Aabb, Vec3};

/// How the view projects the level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Projection {
    #[default]
    Perspective,
    /// From above, north (−Z) up.
    Top,
    /// From the front (looking along −Z).
    Front,
    /// From the side (looking along −X).
    Side,
}

impl Projection {
    pub const ALL: [Projection; 4] = [Self::Perspective, Self::Top, Self::Front, Self::Side];

    pub fn is_ortho(self) -> bool {
        self != Self::Perspective
    }

    /// Where an orthographic view looks and its up direction.
    fn axes(self) -> Option<(Vec3, Vec3)> {
        match self {
            Self::Perspective => None,
            Self::Top => Some(([0.0, -1.0, 0.0], [0.0, 0.0, -1.0])),
            Self::Front => Some(([0.0, 0.0, -1.0], [0.0, 1.0, 0.0])),
            Self::Side => Some(([-1.0, 0.0, 0.0], [0.0, 1.0, 0.0])),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlyCamera {
    /// The eye in perspective; the centre of the view in the orthographic
    /// projections.
    pub eye: Vec3,
    /// Rotation about the vertical axis, radians; 0 looks along −Z.
    pub yaw: f32,
    /// Elevation, radians, within ±89°.
    pub pitch: f32,
    pub fov_y: f32,
    /// Movement speed in metres per second.
    pub speed: f32,
    /// Far clip distance.
    pub far: f32,
    pub projection: Projection,
    /// Half the height of an orthographic view, metres.
    pub ortho_half: f32,
}

impl Default for FlyCamera {
    fn default() -> Self {
        Self {
            eye: [0.0, 50.0, 100.0],
            yaw: 0.0,
            pitch: -0.4,
            fov_y: 60f32.to_radians(),
            speed: 30.0,
            far: 6000.0,
            projection: Projection::Perspective,
            ortho_half: 50.0,
        }
    }
}

const PITCH_LIMIT: f32 = 1.55;
/// How far behind the centre of an orthographic view its eye is.
const ORTHO_BACK: f32 = 5000.0;
/// Camera speeds, metres per second.
pub const SPEED_RANGE: std::ops::RangeInclusive<f32> = 0.5..=2000.0;

/// How the mouse drives the camera this frame (the view decides what a
/// left drag in empty space does).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Controls {
    /// Unreal Engine's controls; off: the controls before stage 54.
    pub ue: bool,
    /// A left drag moves the camera (it started in empty space and the
    /// gizmo mode is not Select).
    pub left_moves: bool,
}

impl FlyCamera {
    pub fn forward(&self) -> Vec3 {
        if let Some((f, _)) = self.projection.axes() {
            return f;
        }
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        [-sy * cp, sp, -cy * cp]
    }

    /// Right vector (horizontal in perspective).
    pub fn right(&self) -> Vec3 {
        if let Some((f, up)) = self.projection.axes() {
            return math::normalize_or(math::cross(f, up), [1.0, 0.0, 0.0]);
        }
        let (sy, cy) = self.yaw.sin_cos();
        [cy, 0.0, -sy]
    }

    /// Up on the screen.
    pub fn up(&self) -> Vec3 {
        math::normalize_or(math::cross(self.right(), self.forward()), [0.0, 1.0, 0.0])
    }

    pub fn view(&self, aspect: f32) -> View {
        let aspect = aspect.max(1e-3);
        if let Some((f, up)) = self.projection.axes() {
            let eye = math::sub(self.eye, math::scale(f, ORTHO_BACK));
            let half = self.ortho_half.max(0.01);
            return View {
                view: math::look_at(eye, self.eye, up),
                projection: orthographic(half * aspect, half, 1.0, ORTHO_BACK * 2.0),
                eye,
            };
        }
        let near = 0.25;
        View {
            view: math::look_at(
                self.eye,
                math::add(self.eye, self.forward()),
                [0.0, 1.0, 0.0],
            ),
            projection: math::perspective(self.fov_y, aspect, near, self.far.max(10.0)),
            eye: self.eye,
        }
    }

    /// Where levels of detail and the size of things are measured from: the
    /// eye, or for an orthographic view the eye of a perspective view
    /// showing as much.
    pub fn lod_eye(&self) -> Vec3 {
        if self.projection.is_ortho() {
            let d = self.ortho_half / (self.fov_y / 2.0).tan();
            math::sub(self.eye, math::scale(self.forward(), d))
        } else {
            self.eye
        }
    }

    /// Looks at `target` from `distance` away, keeping the direction (an
    /// orthographic view centres it and shows as much).
    pub fn look_at(&mut self, target: Vec3, distance: f32) {
        if self.projection.is_ortho() {
            self.eye = target;
            self.ortho_half = (distance * (self.fov_y / 2.0).tan()).max(1.0);
        } else {
            self.eye = math::sub(target, math::scale(self.forward(), distance));
        }
    }

    /// Shows the whole box, looking down at it from the south-east.
    pub fn frame(&mut self, bounds: &Aabb) {
        if bounds.is_empty() {
            return;
        }
        let radius = bounds.radius().max(1.0);
        self.yaw = 0.6;
        self.pitch = -0.6;
        self.look_at(bounds.center(), radius / (self.fov_y / 2.0).tan());
        self.far = (radius * 4.0).clamp(2000.0, 20000.0);
        self.speed = (radius / 10.0).clamp(10.0, 400.0);
    }

    /// Turns the view to look at `target` from where the camera is.
    pub fn turn_to(&mut self, target: Vec3) {
        if self.projection.is_ortho() {
            self.eye = target;
            return;
        }
        let d = math::sub(target, self.eye);
        let len = math::length(d);
        if len < 1e-4 {
            return;
        }
        self.pitch = (d[1] / len).asin().clamp(-PITCH_LIMIT, PITCH_LIMIT);
        self.yaw = (-d[0]).atan2(-d[2]);
    }

    /// Switches the projection; `distance` is how far the point looked at
    /// is (what an orthographic view centres on).
    pub fn set_projection(&mut self, projection: Projection, distance: f32) {
        if projection == self.projection {
            return;
        }
        let tan = (self.fov_y / 2.0).tan();
        let (centre, half) = if self.projection.is_ortho() {
            (self.eye, self.ortho_half)
        } else {
            let d = distance.max(1.0);
            (math::add(self.eye, math::scale(self.forward(), d)), d * tan)
        };
        self.projection = projection;
        if projection.is_ortho() {
            self.eye = centre;
            self.ortho_half = half.max(1.0);
        } else {
            // Back to the direction it had, as far as showed as much.
            self.eye = math::sub(centre, math::scale(self.forward(), half / tan));
        }
    }

    /// Multiplies the speed (the wheel with the right button held).
    pub fn change_speed(&mut self, factor: f32) {
        self.speed = (self.speed * factor).clamp(*SPEED_RANGE.start(), *SPEED_RANGE.end());
    }

    /// Mouse and keyboard input on the viewport; `pivot` is what Alt + drag
    /// orbits. Returns whether the camera moved.
    pub fn handle_input(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        pivot: Vec3,
        controls: Controls,
    ) -> bool {
        let before = *self;
        if self.projection.is_ortho() {
            self.ortho_input(ui, response);
        } else if controls.ue {
            self.ue_input(ui, response, pivot, controls.left_moves);
        } else {
            self.classic_input(ui, response, pivot);
        }
        *self != before
    }

    /// Whether the right button is held on the view (flying, as in UE).
    pub fn right_held(ui: &egui::Ui, response: &egui::Response) -> bool {
        response.dragged_by(egui::PointerButton::Secondary)
            || (ui.input(|i| i.pointer.secondary_down()) && response.is_pointer_button_down_on())
    }

    /// Movement keys: forward, right, up (−1, 0, 1 each).
    fn keys(ui: &egui::Ui, letters: bool) -> [f32; 3] {
        ui.input(|i| {
            use egui::Key;
            let k = |key| i.key_down(key) as i32 as f32;
            let l = |key| if letters { k(key) } else { 0.0 };
            [
                l(Key::W) + k(Key::ArrowUp) - l(Key::S) - k(Key::ArrowDown),
                l(Key::D) + k(Key::ArrowRight) - l(Key::A) - k(Key::ArrowLeft),
                l(Key::E) + k(Key::PageUp) - l(Key::Q) - k(Key::PageDown),
            ]
        })
    }

    /// Flies by the keys: `fast` (Shift) four times as fast, `slow` a
    /// quarter.
    fn fly(&mut self, ui: &egui::Ui, keys: [f32; 3], fast: bool, slow: bool) {
        if keys == [0.0; 3] {
            return;
        }
        let dt = ui.input(|i| i.stable_dt.min(0.1));
        let factor = if fast {
            4.0
        } else if slow {
            0.25
        } else {
            1.0
        };
        let step = self.speed * factor * dt;
        let mut move_by = math::scale(self.forward(), keys[0] * step);
        move_by = math::add(move_by, math::scale(self.right(), keys[1] * step));
        move_by = math::add(move_by, [0.0, keys[2] * step, 0.0]);
        self.eye = math::add(self.eye, move_by);
        ui.ctx().request_repaint();
    }

    fn look(&mut self, delta: egui::Vec2, rate: f32) {
        self.yaw -= delta.x * rate;
        self.pitch = (self.pitch - delta.y * rate).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }

    fn orbit(&mut self, delta: egui::Vec2, pivot: Vec3) {
        let distance = math::length(math::sub(pivot, self.eye)).max(1.0);
        self.look(delta, 0.006);
        self.look_at(pivot, distance);
    }

    /// Moves along the screen so that the point at `distance` follows the
    /// pointer.
    fn pan(&mut self, delta: egui::Vec2, distance: f32, height: f32) {
        let per_point = 2.0 * distance * (self.fov_y / 2.0).tan() / height.max(1.0);
        let by = math::add(
            math::scale(self.right(), -delta.x * per_point),
            math::scale(self.up(), delta.y * per_point),
        );
        self.eye = math::add(self.eye, by);
    }

    /// The controls before stage 54.
    fn classic_input(&mut self, ui: &egui::Ui, response: &egui::Response, pivot: Vec3) {
        let delta = response.drag_delta();
        let alt = ui.input(|i| i.modifiers.alt);
        if response.dragged_by(egui::PointerButton::Secondary)
            || response.dragged_by(egui::PointerButton::Middle)
        {
            self.look(delta, 0.005);
        } else if alt && response.dragged_by(egui::PointerButton::Primary) {
            self.orbit(delta, pivot);
        }
        let active = response.hovered() || response.has_focus();
        if active {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.eye = math::add(
                    self.eye,
                    math::scale(self.forward(), scroll * self.speed * 0.01),
                );
            }
        }
        if active && !ui.ctx().egui_wants_keyboard_input() {
            let keys = Self::keys(ui, true);
            let (fast, slow) = ui.input(|i| (i.modifiers.shift, i.modifiers.ctrl));
            self.fly(ui, keys, fast, slow);
        }
    }

    /// Unreal Engine's controls.
    fn ue_input(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        pivot: Vec3,
        left_moves: bool,
    ) {
        use egui::PointerButton::{Middle, Primary, Secondary};
        let delta = response.drag_delta();
        let alt = ui.input(|i| i.modifiers.alt);
        let right = Self::right_held(ui, response);
        let to_pivot = math::length(math::sub(pivot, self.eye)).max(1.0);
        if response.dragged_by(Middle) {
            self.pan(delta, to_pivot, response.rect.height());
        } else if alt && response.dragged_by(Primary) {
            self.orbit(delta, pivot);
        } else if alt && response.dragged_by(Secondary) {
            // Towards the pivot (right or up) or away from it.
            let distance = (to_pivot * (1.0 - (delta.x - delta.y) * 0.005)).max(0.5);
            let target = math::add(self.eye, math::scale(self.forward(), to_pivot));
            self.eye = math::sub(target, math::scale(self.forward(), distance));
        } else if response.dragged_by(Secondary) {
            self.look(delta, 0.005);
        } else if left_moves && response.dragged_by(Primary) {
            // Forward and back along the ground, turning left and right.
            let (sy, cy) = self.yaw.sin_cos();
            let ahead = [-sy, 0.0, -cy];
            let step = -delta.y * (self.speed * 0.02).max(0.05);
            self.eye = math::add(self.eye, math::scale(ahead, step));
            self.yaw -= delta.x * 0.005;
        }
        let active = response.hovered() || response.has_focus() || right;
        if active {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                if right {
                    self.change_speed(1.004f32.powf(scroll));
                } else {
                    self.eye = math::add(
                        self.eye,
                        math::scale(self.forward(), scroll * self.speed * 0.01),
                    );
                }
            }
        }
        if active && !ui.ctx().egui_wants_keyboard_input() {
            // Letters fly only while the right button is held; without it
            // Q W E R choose the gizmo.
            let keys = Self::keys(ui, right);
            let fast = ui.input(|i| i.modifiers.shift);
            self.fly(ui, keys, fast, false);
        }
    }

    /// Orthographic views: the right or middle button pans, the wheel zooms,
    /// the arrows move.
    fn ortho_input(&mut self, ui: &egui::Ui, response: &egui::Response) {
        let delta = response.drag_delta();
        let height = response.rect.height().max(1.0);
        if response.dragged_by(egui::PointerButton::Secondary)
            || response.dragged_by(egui::PointerButton::Middle)
        {
            let per_point = 2.0 * self.ortho_half / height;
            self.eye = math::add(
                self.eye,
                math::add(
                    math::scale(self.right(), -delta.x * per_point),
                    math::scale(self.up(), delta.y * per_point),
                ),
            );
        }
        let right = Self::right_held(ui, response);
        if response.hovered() || right {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.ortho_half = (self.ortho_half * 0.998f32.powf(scroll)).clamp(0.5, 20000.0);
            }
            if !ui.ctx().egui_wants_keyboard_input() {
                let keys = Self::keys(ui, right);
                if keys != [0.0; 3] {
                    let dt = ui.input(|i| i.stable_dt.min(0.1));
                    let step = self.ortho_half * dt;
                    self.eye = math::add(
                        self.eye,
                        math::add(
                            math::scale(self.right(), keys[1] * step),
                            math::scale(self.up(), (keys[0] + keys[2]) * step),
                        ),
                    );
                    ui.ctx().request_repaint();
                }
            }
        }
    }
}

/// Right-handed orthographic projection with depth in 0..1 (wgpu clip
/// space), `half_width` and `half_height` metres either side of the centre.
fn orthographic(half_width: f32, half_height: f32, near: f32, far: f32) -> math::Mat4 {
    let depth = far - near;
    [
        1.0 / half_width,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0 / half_height,
        0.0,
        0.0,
        0.0,
        0.0,
        -1.0 / depth,
        0.0,
        0.0,
        0.0,
        -near / depth,
        1.0,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_at_targets_and_frames_boxes() {
        let mut camera = FlyCamera::default();
        camera.turn_to([10.0, 50.0, 100.0]);
        let f = camera.forward();
        assert!((f[0] - 1.0).abs() < 1e-5 && f[1].abs() < 1e-5, "{f:?}");
        camera.look_at([0.0; 3], 10.0);
        let view = camera.view(1.5);
        let (uv, _) = view.project([0.0; 3]).unwrap();
        assert!((uv[0] - 0.5).abs() < 1e-4 && (uv[1] - 0.5).abs() < 1e-4);
        let bounds = Aabb {
            min: [-100.0, 0.0, -100.0],
            max: [100.0, 20.0, 100.0],
        };
        camera.frame(&bounds);
        let view = camera.view(1.0);
        // The framing is tight: corners of the box may touch the edges.
        for corner in [bounds.min, bounds.max, bounds.center()] {
            let (uv, depth) = view.project(corner).unwrap();
            assert!(uv.iter().all(|c| (-0.2..=1.2).contains(c)), "{uv:?}");
            assert!((0.0..=1.0).contains(&depth));
        }
        let (uv, _) = view.project(bounds.center()).unwrap();
        assert!((uv[0] - 0.5).abs() < 1e-3 && (uv[1] - 0.5).abs() < 1e-3);
        // Right is perpendicular to forward.
        assert!(math::dot(camera.right(), camera.forward()).abs() < 1e-5);
    }

    #[test]
    fn orthographic_views_look_along_the_axes_and_come_back() {
        let mut camera = FlyCamera::default();
        camera.look_at([10.0, 0.0, 20.0], 40.0);
        let persp = camera;
        camera.set_projection(Projection::Top, 40.0);
        assert_eq!(camera.forward(), [0.0, -1.0, 0.0]);
        assert!((camera.eye[0] - 10.0).abs() < 1e-3 && (camera.eye[2] - 20.0).abs() < 1e-3);
        let view = camera.view(2.0);
        // The centre is in the middle; east is right, north is up.
        let (uv, depth) = view.project([10.0, 0.0, 20.0]).unwrap();
        assert!(
            (uv[0] - 0.5).abs() < 1e-4 && (uv[1] - 0.5).abs() < 1e-4,
            "{uv:?}"
        );
        assert!((0.0..=1.0).contains(&depth));
        let (east, _) = view.project([15.0, 0.0, 20.0]).unwrap();
        let (north, _) = view.project([10.0, 0.0, 15.0]).unwrap();
        assert!(east[0] > 0.5 && north[1] < 0.5, "{east:?} {north:?}");
        // Size is the same at any height: no perspective.
        let (high, _) = view.project([15.0, 100.0, 20.0]).unwrap();
        assert!((high[0] - east[0]).abs() < 1e-4);
        // A ray through the centre goes straight down.
        let ray = view.ray([0.5, 0.5]);
        assert!((ray.dir[1] + 1.0).abs() < 1e-4, "{:?}", ray.dir);
        assert!(
            (camera.lod_eye()[1] - 40.0).abs() < 1e-2,
            "as far as the view shows"
        );
        // The front and the side.
        camera.set_projection(Projection::Front, 0.0);
        assert_eq!(camera.forward(), [0.0, 0.0, -1.0]);
        camera.set_projection(Projection::Side, 0.0);
        assert_eq!(camera.forward(), [-1.0, 0.0, 0.0]);
        assert!(math::dot(camera.right(), camera.up()).abs() < 1e-5);
        // Back to perspective: the same direction and eye.
        camera.set_projection(Projection::Perspective, 0.0);
        for (a, b) in camera.eye.iter().zip(persp.eye) {
            assert!((a - b).abs() < 1e-2, "{:?} {:?}", camera.eye, persp.eye);
        }
        camera.change_speed(1e6);
        assert_eq!(camera.speed, *SPEED_RANGE.end());
    }
}

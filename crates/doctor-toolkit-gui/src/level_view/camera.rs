//! Fly camera for levels: look with the right mouse button, move with
//! W A S D (Q / E down and up), orbit a pivot with Alt + left drag.

use crate::render::View;
use crate::render::math::{self, Aabb, Vec3};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlyCamera {
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
        }
    }
}

const PITCH_LIMIT: f32 = 1.55;

impl FlyCamera {
    pub fn forward(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        [-sy * cp, sp, -cy * cp]
    }

    /// Horizontal right vector.
    pub fn right(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        [cy, 0.0, -sy]
    }

    pub fn view(&self, aspect: f32) -> View {
        let near = 0.25;
        View {
            view: math::look_at(
                self.eye,
                math::add(self.eye, self.forward()),
                [0.0, 1.0, 0.0],
            ),
            projection: math::perspective(self.fov_y, aspect.max(1e-3), near, self.far.max(10.0)),
            eye: self.eye,
        }
    }

    /// Looks at `target` from `distance` away, keeping the direction.
    pub fn look_at(&mut self, target: Vec3, distance: f32) {
        self.eye = math::sub(target, math::scale(self.forward(), distance));
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
        let d = math::sub(target, self.eye);
        let len = math::length(d);
        if len < 1e-4 {
            return;
        }
        self.pitch = (d[1] / len).asin().clamp(-PITCH_LIMIT, PITCH_LIMIT);
        self.yaw = (-d[0]).atan2(-d[2]);
    }

    /// Mouse and keyboard input on the viewport; `pivot` is what Alt + drag
    /// orbits. Returns whether the camera moved.
    pub fn handle_input(&mut self, ui: &egui::Ui, response: &egui::Response, pivot: Vec3) -> bool {
        let before = *self;
        let delta = response.drag_delta();
        let alt = ui.input(|i| i.modifiers.alt);
        if response.dragged_by(egui::PointerButton::Secondary)
            || response.dragged_by(egui::PointerButton::Middle)
        {
            self.yaw -= delta.x * 0.005;
            self.pitch = (self.pitch - delta.y * 0.005).clamp(-PITCH_LIMIT, PITCH_LIMIT);
        } else if alt && response.dragged_by(egui::PointerButton::Primary) {
            let distance = math::length(math::sub(pivot, self.eye)).max(1.0);
            self.yaw -= delta.x * 0.006;
            self.pitch = (self.pitch - delta.y * 0.006).clamp(-PITCH_LIMIT, PITCH_LIMIT);
            self.look_at(pivot, distance);
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
            let (dt, keys, fast, slow) = ui.input(|i| {
                use egui::Key;
                let k = |key| i.key_down(key) as i32 as f32;
                (
                    i.stable_dt.min(0.1),
                    [
                        k(Key::W) + k(Key::ArrowUp) - k(Key::S) - k(Key::ArrowDown),
                        k(Key::D) + k(Key::ArrowRight) - k(Key::A) - k(Key::ArrowLeft),
                        k(Key::E) + k(Key::PageUp) - k(Key::Q) - k(Key::PageDown),
                    ],
                    i.modifiers.shift,
                    i.modifiers.ctrl,
                )
            });
            if keys != [0.0; 3] {
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
        }
        *self != before
    }
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
}

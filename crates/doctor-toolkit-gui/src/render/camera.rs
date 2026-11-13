//! Orbit camera: rotate around a target, pan, zoom, frame a bounding box.

use super::math::{self, Aabb, Mat4, Ray, Vec3};

/// The matrices the renderer needs for one view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub view: Mat4,
    pub projection: Mat4,
    pub eye: Vec3,
}

impl View {
    pub fn view_projection(&self) -> Mat4 {
        math::mul(&self.projection, &self.view)
    }

    /// The ray through a point of the viewport given in 0..1 (y down).
    pub fn ray(&self, uv: [f32; 2]) -> Ray {
        let inv = math::inverse(&self.view_projection()).unwrap_or(math::IDENTITY);
        let ndc = [uv[0] * 2.0 - 1.0, 1.0 - uv[1] * 2.0];
        let unproject = |z: f32| {
            let p = math::mul_vec4(&inv, [ndc[0], ndc[1], z, 1.0]);
            [p[0] / p[3], p[1] / p[3], p[2] / p[3]]
        };
        let near = unproject(0.0);
        let far = unproject(1.0);
        Ray {
            origin: near,
            dir: math::normalize_or(math::sub(far, near), [0.0, 0.0, -1.0]),
        }
    }

    /// A world point in viewport coordinates 0..1 (y down) with its view depth;
    /// `None` behind the camera.
    pub fn project(&self, p: Vec3) -> Option<([f32; 2], f32)> {
        let clip = math::mul_vec4(&self.view_projection(), [p[0], p[1], p[2], 1.0]);
        if clip[3] <= 1e-6 {
            return None;
        }
        let ndc = [clip[0] / clip[3], clip[1] / clip[3]];
        Some((
            [(ndc[0] + 1.0) / 2.0, (1.0 - ndc[1]) / 2.0],
            clip[2] / clip[3],
        ))
    }
}

/// Camera orbiting a target point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrbitCamera {
    pub target: Vec3,
    pub distance: f32,
    /// Rotation about the vertical axis, radians.
    pub yaw: f32,
    /// Elevation, radians, within ±89°.
    pub pitch: f32,
    pub fov_y: f32,
    /// Radius of the content: sets the clip planes and the pan speed.
    pub scene_radius: f32,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            target: [0.0; 3],
            distance: 3.0,
            yaw: 0.6,
            pitch: 0.35,
            fov_y: 45f32.to_radians(),
            scene_radius: 1.0,
        }
    }
}

const PITCH_LIMIT: f32 = 1.55;

impl OrbitCamera {
    /// A camera that shows the whole box.
    pub fn framing(bounds: &Aabb) -> Self {
        let mut camera = Self::default();
        camera.frame(bounds);
        camera
    }

    /// Centres the box and moves back until it fits; keeps the angles.
    pub fn frame(&mut self, bounds: &Aabb) {
        let radius = bounds.radius().max(1e-3);
        self.target = bounds.center();
        self.scene_radius = radius;
        self.distance = radius / (self.fov_y / 2.0).sin() * 1.05;
    }

    pub fn eye(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        math::add(
            self.target,
            math::scale([cp * sy, sp, cp * cy], self.distance),
        )
    }

    pub fn view(&self, aspect: f32) -> View {
        let eye = self.eye();
        let radius = self.scene_radius.max(1e-3);
        let near = (self.distance - 2.0 * radius)
            .max(self.distance * 0.005)
            .max(1e-4);
        let far = (self.distance + 2.0 * radius).max(near * 10.0);
        View {
            view: math::look_at(eye, self.target, [0.0, 1.0, 0.0]),
            projection: math::perspective(self.fov_y, aspect.max(1e-3), near, far),
            eye,
        }
    }

    pub fn rotate(&mut self, dx: f32, dy: f32) {
        self.yaw -= dx * 0.008;
        self.pitch = (self.pitch + dy * 0.008).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }

    /// Moves the target in the view plane by a drag of `dx, dy` points in a
    /// viewport `height` points tall.
    pub fn pan(&mut self, dx: f32, dy: f32, height: f32) {
        let view = math::look_at(self.eye(), self.target, [0.0, 1.0, 0.0]);
        let right = [view[0], view[4], view[8]];
        let up = [view[1], view[5], view[9]];
        // World units per point at the target distance.
        let unit = 2.0 * self.distance * (self.fov_y / 2.0).tan() / height.max(1.0);
        self.target = math::add(
            self.target,
            math::add(math::scale(right, -dx * unit), math::scale(up, dy * unit)),
        );
    }

    /// Scales the distance; positive `steps` move closer.
    pub fn zoom(&mut self, steps: f32) {
        let min = self.scene_radius * 1e-3;
        let max = self.scene_radius * 1e3;
        self.distance = (self.distance * (-steps * 0.0015).exp()).clamp(min, max);
    }

    /// Applies mouse input to a viewport response: left drag rotates, right or
    /// middle drag (or Shift + left drag) pans, the wheel zooms. Returns whether
    /// the camera moved.
    pub fn handle_input(&mut self, ui: &egui::Ui, response: &egui::Response) -> bool {
        let before = *self;
        let shift = ui.input(|i| i.modifiers.shift);
        let delta = response.drag_delta();
        if response.dragged_by(egui::PointerButton::Primary) && !shift {
            self.rotate(delta.x, delta.y);
        } else if response.dragged_by(egui::PointerButton::Secondary)
            || response.dragged_by(egui::PointerButton::Middle)
            || (response.dragged_by(egui::PointerButton::Primary) && shift)
        {
            self.pan(delta.x, delta.y, response.rect.height());
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.zoom(scroll);
                // The wheel zooms the view; a scroll area around it must not scroll too.
                ui.ctx()
                    .input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
            }
        }
        *self != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framed_box_is_inside_the_view() {
        let bounds = Aabb {
            min: [-10.0, 0.0, -2.0],
            max: [30.0, 5.0, 2.0],
        };
        let camera = OrbitCamera::framing(&bounds);
        let view = camera.view(1.0);
        for i in 0..8 {
            let corner = [
                if i & 1 == 0 {
                    bounds.min[0]
                } else {
                    bounds.max[0]
                },
                if i & 2 == 0 {
                    bounds.min[1]
                } else {
                    bounds.max[1]
                },
                if i & 4 == 0 {
                    bounds.min[2]
                } else {
                    bounds.max[2]
                },
            ];
            let (uv, depth) = view.project(corner).unwrap();
            assert!(uv.iter().all(|c| (0.0..=1.0).contains(c)), "{uv:?}");
            assert!((0.0..=1.0).contains(&depth), "{depth}");
        }
    }

    #[test]
    fn centre_ray_hits_the_target() {
        let camera = OrbitCamera::default();
        let view = camera.view(1.6);
        let ray = view.ray([0.5, 0.5]);
        let to_target = math::sub(camera.target, ray.origin);
        let along = math::dot(to_target, ray.dir);
        let closest = ray.at(along);
        assert!(math::length(math::sub(closest, camera.target)) < 1e-3);
        // A projected point maps back onto the ray through it.
        let p = [0.3, 0.2, -0.1];
        let (uv, _) = view.project(p).unwrap();
        let ray = view.ray(uv);
        let along = math::dot(math::sub(p, ray.origin), ray.dir);
        assert!(math::length(math::sub(ray.at(along), p)) < 1e-3);
    }

    #[test]
    fn pan_and_zoom() {
        let mut camera = OrbitCamera::default();
        let start = camera;
        camera.pan(100.0, 0.0, 500.0);
        assert!(math::length(math::sub(camera.target, start.target)) > 0.0);
        assert_eq!(camera.distance, start.distance);
        camera.zoom(100.0);
        assert!(camera.distance < start.distance);
        camera.rotate(0.0, 10_000.0);
        assert!(camera.pitch <= PITCH_LIMIT);
    }
}

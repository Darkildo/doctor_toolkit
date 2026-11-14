//! Move, rotate and scale gizmo: three axes (move, scale) or three rings
//! (rotate) drawn as overlay lines at the selected object, hit-tested on
//! the screen and dragged along the axis or around it. The axes are the
//! world's or the object's own (stage 52); snapping rounds the drag to a
//! step, always when it is switched on and with Ctrl otherwise.

use doctor_toolkit::level::RowMatrix;

use crate::render::math::{self, Ray, Vec3};
use crate::render::{LineVertex, View};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Clicks select; no gizmo.
    Select,
    Move,
    Rotate,
    /// Scale along an axis of the object.
    Scale,
}

/// The axes the gizmo works along.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Space {
    #[default]
    World,
    /// The axes of the selected object.
    Local,
}

/// Snapping of the gizmo.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Snap {
    /// Snap always (Ctrl then drags freely); off: only with Ctrl.
    pub on: bool,
    /// Metres of a move step.
    pub step: f32,
    /// Degrees of a rotation step.
    pub angle: f32,
    /// Step of the scale factor.
    pub scale: f32,
}

impl Default for Snap {
    fn default() -> Self {
        Self {
            on: false,
            step: 0.5,
            angle: 15.0,
            scale: 0.25,
        }
    }
}

/// Steps offered for moves, rotations and scale.
pub const STEPS: [f32; 6] = [0.1, 0.25, 0.5, 1.0, 2.0, 5.0];
pub const ANGLES: [f32; 5] = [5.0, 10.0, 15.0, 45.0, 90.0];
pub const SCALES: [f32; 4] = [0.05, 0.1, 0.25, 0.5];

/// The world axes.
pub const WORLD: [Vec3; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
// X red, Y green, Z blue (the colours of the shell's theme).
const COLORS: [[u8; 4]; 3] = [
    crate::app::theme::AXIS_X.to_array(),
    crate::app::theme::AXIS_Y.to_array(),
    crate::app::theme::AXIS_Z.to_array(),
];
const HOVER: [u8; 4] = [255, 230, 60, 255];
const RING_SEGMENTS: usize = 48;
/// Pointer distance in points within which an axis or ring is hit.
pub const HIT_DISTANCE: f32 = 9.0;

/// Size of the gizmo at `origin` for a constant size on the screen.
pub fn size(view: &View, origin: Vec3) -> f32 {
    math::length(math::sub(origin, view.eye)).max(0.5) * 0.15
}

/// The axes of `space` for an object with the transform `m`: its rows,
/// normalised, for the local space (the world's when they are degenerate).
/// Scaling always uses the object's axes.
pub fn axes(mode: Mode, space: Space, m: &RowMatrix) -> [Vec3; 3] {
    if space == Space::World && mode != Mode::Scale {
        return WORLD;
    }
    let mut out = WORLD;
    for (axis, row) in out.iter_mut().zip(m.iter()) {
        let v = [row[0], row[1], row[2]];
        if math::length(v) > 1e-6 {
            *axis = math::normalize_or(v, *axis);
        } else {
            return WORLD;
        }
    }
    out
}

/// Two unit vectors spanning the plane perpendicular to axis `axis`.
fn plane(axes: &[Vec3; 3], axis: usize) -> (Vec3, Vec3) {
    (axes[(axis + 1) % 3], axes[(axis + 2) % 3])
}

fn ring_point(origin: Vec3, axes: &[Vec3; 3], axis: usize, radius: f32, angle: f32) -> Vec3 {
    let (u, v) = plane(axes, axis);
    let (s, c) = angle.sin_cos();
    math::add(
        origin,
        math::add(math::scale(u, c * radius), math::scale(v, s * radius)),
    )
}

/// Overlay lines of the gizmo.
pub fn lines(
    mode: Mode,
    origin: Vec3,
    axes: &[Vec3; 3],
    size: f32,
    hovered: Option<usize>,
) -> Vec<LineVertex> {
    let mut out = Vec::new();
    for axis in 0..3 {
        let color = if hovered == Some(axis) {
            HOVER
        } else {
            COLORS[axis]
        };
        let dir = axes[axis];
        let (u, v) = plane(axes, axis);
        match mode {
            Mode::Select => {}
            Mode::Move => {
                let tip = math::add(origin, math::scale(dir, size));
                out.push(LineVertex::new(origin, color));
                out.push(LineVertex::new(tip, color));
                // A small arrow head.
                let back = math::add(origin, math::scale(dir, size * 0.85));
                for side in [-1.0, 1.0] {
                    out.push(LineVertex::new(tip, color));
                    out.push(LineVertex::new(
                        math::add(back, math::scale(u, side * size * 0.06)),
                        color,
                    ));
                }
            }
            Mode::Scale => {
                let tip = math::add(origin, math::scale(dir, size));
                out.push(LineVertex::new(origin, color));
                out.push(LineVertex::new(tip, color));
                // A small square at the end.
                let h = size * 0.05;
                let corner = |a: f32, b: f32| {
                    math::add(tip, math::add(math::scale(u, a * h), math::scale(v, b * h)))
                };
                let square = [
                    corner(-1.0, -1.0),
                    corner(1.0, -1.0),
                    corner(1.0, 1.0),
                    corner(-1.0, 1.0),
                ];
                for i in 0..4 {
                    out.push(LineVertex::new(square[i], color));
                    out.push(LineVertex::new(square[(i + 1) % 4], color));
                }
            }
            Mode::Rotate => {
                for i in 0..RING_SEGMENTS {
                    let a = i as f32 / RING_SEGMENTS as f32 * std::f32::consts::TAU;
                    let b = (i + 1) as f32 / RING_SEGMENTS as f32 * std::f32::consts::TAU;
                    out.push(LineVertex::new(
                        ring_point(origin, axes, axis, size, a),
                        color,
                    ));
                    out.push(LineVertex::new(
                        ring_point(origin, axes, axis, size, b),
                        color,
                    ));
                }
            }
        }
    }
    out
}

fn segment_distance(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let ap = [p[0] - a[0], p[1] - a[1]];
    let len2 = ab[0] * ab[0] + ab[1] * ab[1];
    let t = if len2 > 0.0 {
        ((ap[0] * ab[0] + ap[1] * ab[1]) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let d = [ap[0] - ab[0] * t, ap[1] - ab[1] * t];
    (d[0] * d[0] + d[1] * d[1]).sqrt()
}

/// The axis under the pointer: `screen` maps a world point to screen points.
pub fn hit(
    mode: Mode,
    origin: Vec3,
    axes: &[Vec3; 3],
    size: f32,
    pointer: [f32; 2],
    screen: &dyn Fn(Vec3) -> Option<[f32; 2]>,
) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (axis, &dir) in axes.iter().enumerate() {
        let points: Vec<Vec3> = match mode {
            Mode::Select => return None,
            Mode::Move | Mode::Scale => (0..=8)
                .map(|i| math::add(origin, math::scale(dir, size * i as f32 / 8.0)))
                .collect(),
            Mode::Rotate => (0..=RING_SEGMENTS)
                .map(|i| {
                    let a = i as f32 / RING_SEGMENTS as f32 * std::f32::consts::TAU;
                    ring_point(origin, axes, axis, size, a)
                })
                .collect(),
        };
        let projected: Vec<Option<[f32; 2]>> = points.iter().map(|&p| screen(p)).collect();
        for pair in projected.windows(2) {
            if let [Some(a), Some(b)] = pair {
                let d = segment_distance(pointer, *a, *b);
                if d < HIT_DISTANCE && best.is_none_or(|(_, bd)| d < bd) {
                    best = Some((axis, d));
                }
            }
        }
    }
    best.map(|(axis, _)| axis)
}

/// The drag parameter under a ray: the position along the axis (move,
/// scale) or the angle around it (rotate).
pub fn parameter(
    mode: Mode,
    origin: Vec3,
    axes: &[Vec3; 3],
    axis: usize,
    ray: &Ray,
) -> Option<f32> {
    let a = axes[axis];
    match mode {
        Mode::Select => None,
        Mode::Move | Mode::Scale => {
            let w = math::sub(origin, ray.origin);
            let b = math::dot(a, ray.dir);
            let c = math::dot(ray.dir, ray.dir);
            let d = math::dot(a, w);
            let e = math::dot(ray.dir, w);
            let denom = c - b * b;
            if denom.abs() < 1e-6 {
                return None;
            }
            Some((b * e - c * d) / denom)
        }
        Mode::Rotate => {
            let n = math::dot(ray.dir, a);
            if n.abs() < 1e-6 {
                return None;
            }
            let t = math::dot(math::sub(origin, ray.origin), a) / n;
            if t <= 0.0 {
                return None;
            }
            let v = math::sub(ray.at(t), origin);
            let (u, w) = plane(axes, axis);
            Some(math::dot(v, w).atan2(math::dot(v, u)))
        }
    }
}

/// Rotates `v` about the unit axis `axis` by `angle` radians.
fn rotate(v: Vec3, axis: Vec3, angle: f32) -> Vec3 {
    let (s, c) = angle.sin_cos();
    let cross = math::cross(axis, v);
    let along = math::scale(axis, math::dot(axis, v) * (1.0 - c));
    math::add(math::add(math::scale(v, c), math::scale(cross, s)), along)
}

/// `start` moved by `amount` along the axis (move), turned by `amount`
/// radians about it through the object's origin (rotate), or stretched by
/// the factor `1 + amount` along the object's own axis (scale).
pub fn apply(
    mode: Mode,
    start: &RowMatrix,
    axes: &[Vec3; 3],
    axis: usize,
    amount: f32,
) -> RowMatrix {
    let mut m = *start;
    let dir = axes[axis];
    match mode {
        Mode::Select => {}
        Mode::Move => {
            for (k, d) in dir.iter().enumerate() {
                m[3][k] += d * amount;
            }
        }
        Mode::Rotate => {
            for row in m.iter_mut().take(3) {
                let r = rotate([row[0], row[1], row[2]], dir, amount);
                row[..3].copy_from_slice(&r);
            }
        }
        Mode::Scale => {
            let factor = (1.0 + amount).max(0.01);
            for value in m[axis].iter_mut().take(3) {
                *value *= factor;
            }
        }
    }
    m
}

/// Rounds a drag amount to the step of the mode.
pub fn snap(mode: Mode, amount: f32, snap: &Snap) -> f32 {
    let step = match mode {
        Mode::Select => return amount,
        Mode::Move => snap.step,
        Mode::Rotate => snap.angle.to_radians(),
        Mode::Scale => snap.scale,
    };
    if step <= 0.0 {
        return amount;
    }
    (amount / step).round() * step
}

#[cfg(test)]
mod tests {
    use super::*;
    use doctor_toolkit::level::IDENTITY;

    #[test]
    fn move_parameter_follows_the_ray() {
        let origin = [10.0, 0.0, 0.0];
        // A ray straight down onto x = 13 hits the X axis 3 units along.
        let ray = Ray {
            origin: [13.0, 10.0, 0.0],
            dir: [0.0, -1.0, 0.0],
        };
        let t = parameter(Mode::Move, origin, &WORLD, 0, &ray).unwrap();
        assert!((t - 3.0).abs() < 1e-4, "{t}");
        let m = apply(Mode::Move, &IDENTITY, &WORLD, 0, t);
        assert!((m[3][0] - 3.0).abs() < 1e-4);
        // A ray along the axis has no parameter.
        let along = Ray {
            origin: [0.0; 3],
            dir: [1.0, 0.0, 0.0],
        };
        assert!(parameter(Mode::Move, origin, &WORLD, 0, &along).is_none());
        assert!(parameter(Mode::Select, origin, &WORLD, 0, &ray).is_none());
    }

    #[test]
    fn rotation_turns_the_rows_about_the_world_axis() {
        let origin = [0.0; 3];
        let ray = |x: f32, z: f32| Ray {
            origin: [x, 5.0, z],
            dir: [0.0, -1.0, 0.0],
        };
        // Y axis: plane spanned by Z (u) and X (w).
        let a0 = parameter(Mode::Rotate, origin, &WORLD, 1, &ray(0.0, 1.0)).unwrap();
        let a1 = parameter(Mode::Rotate, origin, &WORLD, 1, &ray(1.0, 0.0)).unwrap();
        let delta = a1 - a0;
        assert!((delta.abs() - std::f32::consts::FRAC_PI_2).abs() < 1e-4);
        let mut start = IDENTITY;
        start[3] = [5.0, 6.0, 7.0, 1.0];
        let m = apply(Mode::Rotate, &start, &WORLD, 1, std::f32::consts::FRAC_PI_2);
        assert_eq!(m[3], start[3], "the position stays");
        // 90° about +Y turns +X into −Z.
        assert!(
            (m[0][2] + 1.0).abs() < 1e-5 && m[0][0].abs() < 1e-5,
            "{:?}",
            m[0]
        );
        let snapping = Snap::default();
        assert_eq!(snap(Mode::Move, 1.2, &snapping), 1.0);
        assert!((snap(Mode::Rotate, 0.3, &snapping) - 15f32.to_radians()).abs() < 1e-6);
        assert_eq!(snap(Mode::Scale, 0.3, &snapping), 0.25);
        let fine = Snap {
            step: 0.1,
            angle: 5.0,
            ..snapping
        };
        assert!((snap(Mode::Move, 1.23, &fine) - 1.2).abs() < 1e-5);
        assert!((snap(Mode::Rotate, 0.1, &fine) - 5f32.to_radians()).abs() < 1e-6);
    }

    #[test]
    fn local_axes_follow_the_object() {
        // An object turned 90° about Y: its X axis points along world −Z.
        let mut m = apply(
            Mode::Rotate,
            &IDENTITY,
            &WORLD,
            1,
            std::f32::consts::FRAC_PI_2,
        );
        m[3] = [1.0, 2.0, 3.0, 1.0];
        assert_eq!(axes(Mode::Move, Space::World, &m), WORLD);
        let local = axes(Mode::Move, Space::Local, &m);
        assert!((local[0][2] + 1.0).abs() < 1e-5, "{:?}", local[0]);
        // Moving 2 m along the local X goes 2 m along world −Z.
        let moved = apply(Mode::Move, &m, &local, 0, 2.0);
        assert!((moved[3][2] - 1.0).abs() < 1e-5 && (moved[3][0] - 1.0).abs() < 1e-5);
        // Scale works on the object's axes even in world space.
        let scaled = apply(
            Mode::Scale,
            &m,
            &axes(Mode::Scale, Space::World, &m),
            0,
            1.0,
        );
        let len = math::length([scaled[0][0], scaled[0][1], scaled[0][2]]);
        assert!((len - 2.0).abs() < 1e-5, "{len}");
        assert_eq!(scaled[1], m[1], "the other axes stay");
        assert_eq!(scaled[3], m[3], "the position stays");
        // A degenerate matrix falls back to the world axes.
        let flat = [[0.0; 4]; 4];
        assert_eq!(axes(Mode::Move, Space::Local, &flat), WORLD);
    }

    #[test]
    fn hits_axes_on_the_screen() {
        // An orthographic-like mapping: x, y of the world are the screen.
        let screen = |p: Vec3| Some([p[0] * 100.0, -p[1] * 100.0]);
        let origin = [0.0; 3];
        let hit_at = |mode, p| hit(mode, origin, &WORLD, 1.0, p, &screen);
        assert_eq!(hit_at(Mode::Move, [50.0, 3.0]), Some(0));
        assert_eq!(hit_at(Mode::Move, [2.0, -60.0]), Some(1));
        assert_eq!(hit_at(Mode::Move, [60.0, -60.0]), None);
        assert_eq!(hit_at(Mode::Scale, [50.0, 3.0]), Some(0));
        assert_eq!(hit_at(Mode::Select, [50.0, 3.0]), None);
        assert_eq!(lines(Mode::Move, origin, &WORLD, 1.0, None).len(), 18);
        assert_eq!(lines(Mode::Scale, origin, &WORLD, 1.0, None).len(), 30);
        assert!(lines(Mode::Select, origin, &WORLD, 1.0, None).is_empty());
        assert_eq!(
            lines(Mode::Rotate, origin, &WORLD, 1.0, Some(1)).len(),
            3 * RING_SEGMENTS * 2
        );
    }
}

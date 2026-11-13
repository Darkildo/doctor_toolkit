//! Move and rotate gizmo: three world axes (move) or three rings (rotate)
//! drawn as overlay lines at the selected object, hit-tested on the screen
//! and dragged along the axis or around it.

use doctor_toolkit::level::RowMatrix;

use crate::render::math::{self, Ray, Vec3};
use crate::render::{LineVertex, View};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Move,
    Rotate,
}

const AXES: [Vec3; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
const COLORS: [[u8; 4]; 3] = [[230, 60, 60, 255], [80, 210, 80, 255], [70, 120, 255, 255]];
const HOVER: [u8; 4] = [255, 230, 60, 255];
const RING_SEGMENTS: usize = 48;
/// Pointer distance in points within which an axis or ring is hit.
pub const HIT_DISTANCE: f32 = 9.0;

/// Size of the gizmo at `origin` for a constant size on the screen.
pub fn size(view: &View, origin: Vec3) -> f32 {
    math::length(math::sub(origin, view.eye)).max(0.5) * 0.15
}

/// Two unit vectors spanning the plane perpendicular to axis `axis`.
fn plane(axis: usize) -> (Vec3, Vec3) {
    (AXES[(axis + 1) % 3], AXES[(axis + 2) % 3])
}

fn ring_point(origin: Vec3, axis: usize, radius: f32, angle: f32) -> Vec3 {
    let (u, v) = plane(axis);
    let (s, c) = angle.sin_cos();
    math::add(
        origin,
        math::add(math::scale(u, c * radius), math::scale(v, s * radius)),
    )
}

/// Overlay lines of the gizmo.
pub fn lines(mode: Mode, origin: Vec3, size: f32, hovered: Option<usize>) -> Vec<LineVertex> {
    let mut out = Vec::new();
    for axis in 0..3 {
        let color = if hovered == Some(axis) {
            HOVER
        } else {
            COLORS[axis]
        };
        match mode {
            Mode::Move => {
                let tip = math::add(origin, math::scale(AXES[axis], size));
                out.push(LineVertex::new(origin, color));
                out.push(LineVertex::new(tip, color));
                // A small arrow head.
                let (u, _) = plane(axis);
                let back = math::add(origin, math::scale(AXES[axis], size * 0.85));
                for side in [-1.0, 1.0] {
                    out.push(LineVertex::new(tip, color));
                    out.push(LineVertex::new(
                        math::add(back, math::scale(u, side * size * 0.06)),
                        color,
                    ));
                }
            }
            Mode::Rotate => {
                for i in 0..RING_SEGMENTS {
                    let a = i as f32 / RING_SEGMENTS as f32 * std::f32::consts::TAU;
                    let b = (i + 1) as f32 / RING_SEGMENTS as f32 * std::f32::consts::TAU;
                    out.push(LineVertex::new(ring_point(origin, axis, size, a), color));
                    out.push(LineVertex::new(ring_point(origin, axis, size, b), color));
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
    size: f32,
    pointer: [f32; 2],
    screen: &dyn Fn(Vec3) -> Option<[f32; 2]>,
) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (axis, &dir) in AXES.iter().enumerate() {
        let points: Vec<Vec3> = match mode {
            Mode::Move => (0..=8)
                .map(|i| math::add(origin, math::scale(dir, size * i as f32 / 8.0)))
                .collect(),
            Mode::Rotate => (0..=RING_SEGMENTS)
                .map(|i| {
                    let a = i as f32 / RING_SEGMENTS as f32 * std::f32::consts::TAU;
                    ring_point(origin, axis, size, a)
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

/// The drag parameter under a ray: the position along the axis (move) or
/// the angle around it (rotate).
pub fn parameter(mode: Mode, origin: Vec3, axis: usize, ray: &Ray) -> Option<f32> {
    let a = AXES[axis];
    match mode {
        Mode::Move => {
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
            let (u, w) = plane(axis);
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

/// `start` moved by `delta` along `axis` (move) or turned by `delta`
/// radians about the world axis through its origin (rotate).
pub fn apply(mode: Mode, start: &RowMatrix, axis: usize, delta: f32) -> RowMatrix {
    let mut m = *start;
    match mode {
        Mode::Move => m[3][axis] += delta,
        Mode::Rotate => {
            for row in m.iter_mut().take(3) {
                let r = rotate([row[0], row[1], row[2]], AXES[axis], delta);
                row[..3].copy_from_slice(&r);
            }
        }
    }
    m
}

/// Rounds a drag delta to the snap step: 0.5 m or 15°.
pub fn snap(mode: Mode, delta: f32) -> f32 {
    let step = match mode {
        Mode::Move => 0.5,
        Mode::Rotate => 15f32.to_radians(),
    };
    (delta / step).round() * step
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
        let t = parameter(Mode::Move, origin, 0, &ray).unwrap();
        assert!((t - 3.0).abs() < 1e-4, "{t}");
        let m = apply(Mode::Move, &IDENTITY, 0, t);
        assert!((m[3][0] - 3.0).abs() < 1e-4);
        // A ray along the axis has no parameter.
        let along = Ray {
            origin: [0.0; 3],
            dir: [1.0, 0.0, 0.0],
        };
        assert!(parameter(Mode::Move, origin, 0, &along).is_none());
    }

    #[test]
    fn rotation_turns_the_rows_about_the_world_axis() {
        let origin = [0.0; 3];
        let ray = |x: f32, z: f32| Ray {
            origin: [x, 5.0, z],
            dir: [0.0, -1.0, 0.0],
        };
        // Y axis: plane spanned by Z (u) and X (w).
        let a0 = parameter(Mode::Rotate, origin, 1, &ray(0.0, 1.0)).unwrap();
        let a1 = parameter(Mode::Rotate, origin, 1, &ray(1.0, 0.0)).unwrap();
        let delta = a1 - a0;
        assert!((delta.abs() - std::f32::consts::FRAC_PI_2).abs() < 1e-4);
        let mut start = IDENTITY;
        start[3] = [5.0, 6.0, 7.0, 1.0];
        let m = apply(Mode::Rotate, &start, 1, std::f32::consts::FRAC_PI_2);
        assert_eq!(m[3], start[3], "the position stays");
        // 90° about +Y turns +X into −Z.
        assert!(
            (m[0][2] + 1.0).abs() < 1e-5 && m[0][0].abs() < 1e-5,
            "{:?}",
            m[0]
        );
        assert_eq!(snap(Mode::Move, 1.2), 1.0);
        assert!((snap(Mode::Rotate, 0.3) - 15f32.to_radians()).abs() < 1e-6);
    }

    #[test]
    fn hits_axes_on_the_screen() {
        // An orthographic-like mapping: x, y of the world are the screen.
        let screen = |p: Vec3| Some([p[0] * 100.0, -p[1] * 100.0]);
        let origin = [0.0; 3];
        assert_eq!(hit(Mode::Move, origin, 1.0, [50.0, 3.0], &screen), Some(0));
        assert_eq!(hit(Mode::Move, origin, 1.0, [2.0, -60.0], &screen), Some(1));
        assert_eq!(hit(Mode::Move, origin, 1.0, [60.0, -60.0], &screen), None);
        assert_eq!(lines(Mode::Move, origin, 1.0, None).len(), 18);
        assert_eq!(
            lines(Mode::Rotate, origin, 1.0, Some(1)).len(),
            3 * RING_SEGMENTS * 2
        );
    }
}

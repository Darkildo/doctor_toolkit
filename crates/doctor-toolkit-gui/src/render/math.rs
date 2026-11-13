//! Small vector and matrix helpers for the renderer.
//!
//! Matrices are column-major `[f32; 16]` for column vectors, the convention of
//! glTF, WGSL and `doctor_toolkit::resource::rcb::Mat4`.

pub type Vec3 = [f32; 3];
pub type Mat4 = [f32; 16];

pub const IDENTITY: Mat4 = [
    1.0, 0.0, 0.0, 0.0, //
    0.0, 1.0, 0.0, 0.0, //
    0.0, 0.0, 1.0, 0.0, //
    0.0, 0.0, 0.0, 1.0,
];

pub fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn scale(a: Vec3, s: f32) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

pub fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub fn length(a: Vec3) -> f32 {
    dot(a, a).sqrt()
}

/// `a` scaled to unit length; `fallback` for (near) zero vectors.
pub fn normalize_or(a: Vec3, fallback: Vec3) -> Vec3 {
    let len = length(a);
    if len > 1e-12 {
        scale(a, 1.0 / len)
    } else {
        fallback
    }
}

/// `a × b`.
pub fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [0f32; 16];
    for col in 0..4 {
        for row in 0..4 {
            out[col * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[col * 4 + k]).sum();
        }
    }
    out
}

/// `m × (x, y, z, w)`.
pub fn mul_vec4(m: &Mat4, v: [f32; 4]) -> [f32; 4] {
    let mut out = [0f32; 4];
    for (row, value) in out.iter_mut().enumerate() {
        *value = (0..4).map(|k| m[k * 4 + row] * v[k]).sum();
    }
    out
}

/// Transforms a point (w = 1) without the perspective divide.
pub fn transform_point(m: &Mat4, p: Vec3) -> Vec3 {
    let v = mul_vec4(m, [p[0], p[1], p[2], 1.0]);
    [v[0], v[1], v[2]]
}

pub fn translation(t: Vec3) -> Mat4 {
    let mut m = IDENTITY;
    m[12..15].copy_from_slice(&t);
    m
}

/// Rotation about the Y axis by `angle` radians.
pub fn rotation_y(angle: f32) -> Mat4 {
    let (s, c) = angle.sin_cos();
    [
        c, 0.0, -s, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        s, 0.0, c, 0.0, //
        0.0, 0.0, 0.0, 1.0,
    ]
}

pub fn uniform_scale(s: f32) -> Mat4 {
    let mut m = IDENTITY;
    m[0] = s;
    m[5] = s;
    m[10] = s;
    m
}

/// Largest scale factor of the upper 3×3 part, for bounding spheres.
pub fn max_scale(m: &Mat4) -> f32 {
    (0..3)
        .map(|c| length([m[c * 4], m[c * 4 + 1], m[c * 4 + 2]]))
        .fold(0.0, f32::max)
}

/// Right-handed view matrix looking from `eye` at `target`.
pub fn look_at(eye: Vec3, target: Vec3, up: Vec3) -> Mat4 {
    let f = normalize_or(sub(target, eye), [0.0, 0.0, -1.0]);
    let s = normalize_or(cross(f, up), [1.0, 0.0, 0.0]);
    let u = cross(s, f);
    [
        s[0],
        u[0],
        -f[0],
        0.0,
        s[1],
        u[1],
        -f[1],
        0.0,
        s[2],
        u[2],
        -f[2],
        0.0,
        -dot(s, eye),
        -dot(u, eye),
        dot(f, eye),
        1.0,
    ]
}

/// Right-handed perspective projection with depth in 0..1 (wgpu clip space).
pub fn perspective(fov_y: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let f = 1.0 / (fov_y / 2.0).tan();
    let range = far / (near - far);
    [
        f / aspect,
        0.0,
        0.0,
        0.0,
        0.0,
        f,
        0.0,
        0.0,
        0.0,
        0.0,
        range,
        -1.0,
        0.0,
        0.0,
        range * near,
        0.0,
    ]
}

/// General 4×4 inverse; `None` for singular matrices.
pub fn inverse(m: &Mat4) -> Option<Mat4> {
    let mut inv = [0f32; 16];
    inv[0] = m[5] * m[10] * m[15] - m[5] * m[11] * m[14] - m[9] * m[6] * m[15]
        + m[9] * m[7] * m[14]
        + m[13] * m[6] * m[11]
        - m[13] * m[7] * m[10];
    inv[4] = -m[4] * m[10] * m[15] + m[4] * m[11] * m[14] + m[8] * m[6] * m[15]
        - m[8] * m[7] * m[14]
        - m[12] * m[6] * m[11]
        + m[12] * m[7] * m[10];
    inv[8] = m[4] * m[9] * m[15] - m[4] * m[11] * m[13] - m[8] * m[5] * m[15]
        + m[8] * m[7] * m[13]
        + m[12] * m[5] * m[11]
        - m[12] * m[7] * m[9];
    inv[12] = -m[4] * m[9] * m[14] + m[4] * m[10] * m[13] + m[8] * m[5] * m[14]
        - m[8] * m[6] * m[13]
        - m[12] * m[5] * m[10]
        + m[12] * m[6] * m[9];
    inv[1] = -m[1] * m[10] * m[15] + m[1] * m[11] * m[14] + m[9] * m[2] * m[15]
        - m[9] * m[3] * m[14]
        - m[13] * m[2] * m[11]
        + m[13] * m[3] * m[10];
    inv[5] = m[0] * m[10] * m[15] - m[0] * m[11] * m[14] - m[8] * m[2] * m[15]
        + m[8] * m[3] * m[14]
        + m[12] * m[2] * m[11]
        - m[12] * m[3] * m[10];
    inv[9] = -m[0] * m[9] * m[15] + m[0] * m[11] * m[13] + m[8] * m[1] * m[15]
        - m[8] * m[3] * m[13]
        - m[12] * m[1] * m[11]
        + m[12] * m[3] * m[9];
    inv[13] = m[0] * m[9] * m[14] - m[0] * m[10] * m[13] - m[8] * m[1] * m[14]
        + m[8] * m[2] * m[13]
        + m[12] * m[1] * m[10]
        - m[12] * m[2] * m[9];
    inv[2] = m[1] * m[6] * m[15] - m[1] * m[7] * m[14] - m[5] * m[2] * m[15]
        + m[5] * m[3] * m[14]
        + m[13] * m[2] * m[7]
        - m[13] * m[3] * m[6];
    inv[6] = -m[0] * m[6] * m[15] + m[0] * m[7] * m[14] + m[4] * m[2] * m[15]
        - m[4] * m[3] * m[14]
        - m[12] * m[2] * m[7]
        + m[12] * m[3] * m[6];
    inv[10] = m[0] * m[5] * m[15] - m[0] * m[7] * m[13] - m[4] * m[1] * m[15]
        + m[4] * m[3] * m[13]
        + m[12] * m[1] * m[7]
        - m[12] * m[3] * m[5];
    inv[14] = -m[0] * m[5] * m[14] + m[0] * m[6] * m[13] + m[4] * m[1] * m[14]
        - m[4] * m[2] * m[13]
        - m[12] * m[1] * m[6]
        + m[12] * m[2] * m[5];
    inv[3] = -m[1] * m[6] * m[11] + m[1] * m[7] * m[10] + m[5] * m[2] * m[11]
        - m[5] * m[3] * m[10]
        - m[9] * m[2] * m[7]
        + m[9] * m[3] * m[6];
    inv[7] = m[0] * m[6] * m[11] - m[0] * m[7] * m[10] - m[4] * m[2] * m[11]
        + m[4] * m[3] * m[10]
        + m[8] * m[2] * m[7]
        - m[8] * m[3] * m[6];
    inv[11] = -m[0] * m[5] * m[11] + m[0] * m[7] * m[9] + m[4] * m[1] * m[11]
        - m[4] * m[3] * m[9]
        - m[8] * m[1] * m[7]
        + m[8] * m[3] * m[5];
    inv[15] = m[0] * m[5] * m[10] - m[0] * m[6] * m[9] - m[4] * m[1] * m[10]
        + m[4] * m[2] * m[9]
        + m[8] * m[1] * m[6]
        - m[8] * m[2] * m[5];
    let det = m[0] * inv[0] + m[1] * inv[4] + m[2] * inv[8] + m[3] * inv[12];
    if det.abs() < 1e-30 {
        return None;
    }
    let inv_det = 1.0 / det;
    Some(inv.map(|x| x * inv_det))
}

/// Axis-aligned bounding box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub const EMPTY: Self = Self {
        min: [f32::INFINITY; 3],
        max: [f32::NEG_INFINITY; 3],
    };

    pub fn is_empty(&self) -> bool {
        (0..3).any(|i| self.min[i] > self.max[i])
    }

    pub fn add_point(&mut self, p: Vec3) {
        for (i, value) in p.into_iter().enumerate() {
            self.min[i] = self.min[i].min(value);
            self.max[i] = self.max[i].max(value);
        }
    }

    pub fn union(&mut self, other: &Aabb) {
        if !other.is_empty() {
            self.add_point(other.min);
            self.add_point(other.max);
        }
    }

    pub fn center(&self) -> Vec3 {
        if self.is_empty() {
            return [0.0; 3];
        }
        scale(add(self.min, self.max), 0.5)
    }

    /// Radius of the bounding sphere around [`Self::center`].
    pub fn radius(&self) -> f32 {
        if self.is_empty() {
            return 0.0;
        }
        length(sub(self.max, self.min)) / 2.0
    }

    /// The box around the eight transformed corners.
    pub fn transformed(&self, m: &Mat4) -> Aabb {
        let mut out = Aabb::EMPTY;
        if self.is_empty() {
            return out;
        }
        for i in 0..8 {
            let corner = [
                if i & 1 == 0 { self.min[0] } else { self.max[0] },
                if i & 2 == 0 { self.min[1] } else { self.max[1] },
                if i & 4 == 0 { self.min[2] } else { self.max[2] },
            ];
            out.add_point(transform_point(m, corner));
        }
        out
    }
}

/// A ray with a unit direction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ray {
    pub origin: Vec3,
    pub dir: Vec3,
}

impl Ray {
    pub fn at(&self, t: f32) -> Vec3 {
        add(self.origin, scale(self.dir, t))
    }

    /// Distance along the ray to a triangle (Möller–Trumbore), both sides.
    pub fn triangle(&self, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
        let e1 = sub(b, a);
        let e2 = sub(c, a);
        let p = cross(self.dir, e2);
        let det = dot(e1, p);
        if det.abs() < 1e-12 {
            return None;
        }
        let inv = 1.0 / det;
        let s = sub(self.origin, a);
        let u = dot(s, p) * inv;
        if !(0.0..=1.0).contains(&u) {
            return None;
        }
        let q = cross(s, e1);
        let v = dot(self.dir, q) * inv;
        if v < 0.0 || u + v > 1.0 {
            return None;
        }
        let t = dot(e2, q) * inv;
        (t > 0.0).then_some(t)
    }

    /// Entry distance into a box (slab test); `None` if missed.
    pub fn aabb(&self, b: &Aabb) -> Option<f32> {
        let mut near = 0f32;
        let mut far = f32::INFINITY;
        for i in 0..3 {
            let inv = 1.0 / self.dir[i];
            let mut t0 = (b.min[i] - self.origin[i]) * inv;
            let mut t1 = (b.max[i] - self.origin[i]) * inv;
            if t0 > t1 {
                std::mem::swap(&mut t0, &mut t1);
            }
            near = near.max(t0);
            far = far.min(t1);
            if near > far {
                return None;
            }
        }
        Some(near)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: &[f32], b: &[f32]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4)
    }

    #[test]
    fn inverse_undoes_transforms() {
        let m = mul(
            &translation([1.0, -2.0, 3.0]),
            &mul(&rotation_y(0.7), &uniform_scale(2.5)),
        );
        let inv = inverse(&m).unwrap();
        assert!(close(&mul(&m, &inv), &IDENTITY));
        let p = [0.3, 0.4, -5.0];
        assert!(close(&transform_point(&inv, transform_point(&m, p)), &p));
        assert!((max_scale(&m) - 2.5).abs() < 1e-5);
        assert!(inverse(&[0.0; 16]).is_none());
    }

    #[test]
    fn view_projection_maps_target_to_centre() {
        let view = look_at([0.0, 0.0, 10.0], [0.0; 3], [0.0, 1.0, 0.0]);
        let proj = perspective(1.0, 1.5, 0.1, 100.0);
        let clip = mul_vec4(&mul(&proj, &view), [0.0, 0.0, 0.0, 1.0]);
        let ndc = [clip[0] / clip[3], clip[1] / clip[3], clip[2] / clip[3]];
        assert!(close(&ndc[..2], &[0.0, 0.0]));
        assert!(ndc[2] > 0.0 && ndc[2] < 1.0);
        // Near and far planes map to depth 0 and 1.
        let near = mul_vec4(&mul(&proj, &view), [0.0, 0.0, 9.9, 1.0]);
        let far = mul_vec4(&mul(&proj, &view), [0.0, 0.0, -90.0, 1.0]);
        assert!((near[2] / near[3]).abs() < 1e-4);
        assert!((far[2] / far[3] - 1.0).abs() < 1e-4);
        // +X stays to the right, +Y up.
        let right = mul_vec4(&mul(&proj, &view), [1.0, 1.0, 0.0, 1.0]);
        assert!(right[0] > 0.0 && right[1] > 0.0);
    }

    #[test]
    fn ray_hits() {
        let ray = Ray {
            origin: [0.25, 0.25, 5.0],
            dir: [0.0, 0.0, -1.0],
        };
        let t = ray
            .triangle([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0])
            .unwrap();
        assert!((t - 5.0).abs() < 1e-5);
        assert!(
            ray.triangle([2.0, 0.0, 0.0], [3.0, 0.0, 0.0], [2.0, 1.0, 0.0])
                .is_none()
        );
        let b = Aabb {
            min: [-1.0; 3],
            max: [1.0; 3],
        };
        assert!((ray.aabb(&b).unwrap() - 4.0).abs() < 1e-5);
        let mut boxed = Aabb::EMPTY;
        assert!(boxed.is_empty());
        boxed.add_point([1.0, 2.0, 3.0]);
        boxed.add_point([-1.0, 0.0, 1.0]);
        assert_eq!(boxed.center(), [0.0, 1.0, 2.0]);
        let moved = boxed.transformed(&translation([1.0, 0.0, 0.0]));
        assert_eq!(moved.min, [0.0, 0.0, 1.0]);
    }
}

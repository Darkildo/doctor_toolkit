//! Small linear algebra for scenes: column-major 4×4 matrices (column
//! vectors, as glTF and [`crate::resource::rcb`]), quaternions `[x, y, z, w]`,
//! Euler angles, interpolation and polygon triangulation.

use crate::resource::mesh_build::PartGeometry;
use crate::resource::rcb::Mat4;

pub const IDENTITY: Mat4 = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

/// `a × b`.
pub fn multiply(a: &Mat4, b: &Mat4) -> Mat4 {
    crate::resource::rcb::multiply(a, b)
}

/// Inverse of an affine matrix.
pub fn invert(m: &Mat4) -> Mat4 {
    crate::resource::rcb::invert_affine(m)
}

/// Applies a matrix to a point.
pub fn point(m: &Mat4, v: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|r| m[r] * v[0] + m[4 + r] * v[1] + m[8 + r] * v[2] + m[12 + r])
}

/// Applies the 3×3 part of a matrix to a direction.
pub fn vector(m: &Mat4, v: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|r| m[r] * v[0] + m[4 + r] * v[1] + m[8 + r] * v[2])
}

pub fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 1e-20 && len.is_finite() {
        v.map(|x| x / len)
    } else {
        v
    }
}

pub fn determinant3(m: &Mat4) -> f32 {
    let a = |r: usize, c: usize| m[c * 4 + r];
    a(0, 0) * (a(1, 1) * a(2, 2) - a(1, 2) * a(2, 1))
        - a(0, 1) * (a(1, 0) * a(2, 2) - a(1, 2) * a(2, 0))
        + a(0, 2) * (a(1, 0) * a(2, 1) - a(1, 1) * a(2, 0))
}

pub fn translation(t: [f32; 3]) -> Mat4 {
    let mut m = IDENTITY;
    m[12..15].copy_from_slice(&t);
    m
}

/// The translation of a matrix.
pub fn translation_of(m: &Mat4) -> [f32; 3] {
    [m[12], m[13], m[14]]
}

pub fn scaling(s: [f32; 3]) -> Mat4 {
    let mut m = IDENTITY;
    m[0] = s[0];
    m[5] = s[1];
    m[10] = s[2];
    m
}

/// Rotation matrix of a unit quaternion `[x, y, z, w]`.
pub fn quat_matrix(q: [f32; 4]) -> Mat4 {
    let [x, y, z, w] = q;
    let (xx, yy, zz) = (x * x, y * y, z * z);
    let (xy, xz, yz) = (x * y, x * z, y * z);
    let (wx, wy, wz) = (w * x, w * y, w * z);
    [
        1.0 - 2.0 * (yy + zz),
        2.0 * (xy + wz),
        2.0 * (xz - wy),
        0.0,
        2.0 * (xy - wz),
        1.0 - 2.0 * (xx + zz),
        2.0 * (yz + wx),
        0.0,
        2.0 * (xz + wy),
        2.0 * (yz - wx),
        1.0 - 2.0 * (xx + yy),
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
    ]
}

/// `T × R × S`.
pub fn compose(t: [f32; 3], r: [f32; 4], s: [f32; 3]) -> Mat4 {
    let mut m = quat_matrix(r);
    for c in 0..3 {
        for row in 0..3 {
            m[c * 4 + row] *= s[c];
        }
    }
    m[12..15].copy_from_slice(&t);
    m
}

/// Translation, rotation and scale of an affine matrix (shear is lost; a
/// mirror goes into a negative x scale).
pub fn decompose(m: &Mat4) -> ([f32; 3], [f32; 4], [f32; 3]) {
    let t = [m[12], m[13], m[14]];
    let col = |c: usize| [m[c * 4], m[c * 4 + 1], m[c * 4 + 2]];
    let len = |v: [f32; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    let mut s = [len(col(0)), len(col(1)), len(col(2))];
    if determinant3(m) < 0.0 {
        s[0] = -s[0];
    }
    let mut r = IDENTITY;
    for c in 0..3 {
        let v = col(c);
        let k = if s[c].abs() > 1e-20 { 1.0 / s[c] } else { 0.0 };
        for row in 0..3 {
            r[c * 4 + row] = v[row] * k;
        }
    }
    (t, matrix_quat(&r), s)
}

/// Unit quaternion of the rotation part of a matrix (assumed orthonormal).
pub fn matrix_quat(m: &Mat4) -> [f32; 4] {
    let a = |r: usize, c: usize| m[c * 4 + r];
    let trace = a(0, 0) + a(1, 1) + a(2, 2);
    let q = if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        [
            (a(2, 1) - a(1, 2)) / s,
            (a(0, 2) - a(2, 0)) / s,
            (a(1, 0) - a(0, 1)) / s,
            0.25 * s,
        ]
    } else if a(0, 0) > a(1, 1) && a(0, 0) > a(2, 2) {
        let s = (1.0 + a(0, 0) - a(1, 1) - a(2, 2)).max(0.0).sqrt() * 2.0;
        [
            0.25 * s,
            (a(0, 1) + a(1, 0)) / s,
            (a(0, 2) + a(2, 0)) / s,
            (a(2, 1) - a(1, 2)) / s,
        ]
    } else if a(1, 1) > a(2, 2) {
        let s = (1.0 + a(1, 1) - a(0, 0) - a(2, 2)).max(0.0).sqrt() * 2.0;
        [
            (a(0, 1) + a(1, 0)) / s,
            0.25 * s,
            (a(1, 2) + a(2, 1)) / s,
            (a(0, 2) - a(2, 0)) / s,
        ]
    } else {
        let s = (1.0 + a(2, 2) - a(0, 0) - a(1, 1)).max(0.0).sqrt() * 2.0;
        [
            (a(0, 2) + a(2, 0)) / s,
            (a(1, 2) + a(2, 1)) / s,
            0.25 * s,
            (a(1, 0) - a(0, 1)) / s,
        ]
    };
    quat_normalize(q)
}

pub fn quat_normalize(q: [f32; 4]) -> [f32; 4] {
    let len = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if len > 1e-20 && len.is_finite() {
        q.map(|x| x / len)
    } else {
        [0.0, 0.0, 0.0, 1.0]
    }
}

/// `a × b` (apply `b` first).
pub fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

pub fn quat_conjugate(q: [f32; 4]) -> [f32; 4] {
    [-q[0], -q[1], -q[2], q[3]]
}

/// Spherical interpolation along the shorter arc.
pub fn slerp(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let mut dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    let mut b = b;
    if dot < 0.0 {
        dot = -dot;
        b = b.map(|x| -x);
    }
    if dot > 0.9995 {
        let q = [0, 1, 2, 3].map(|i| a[i] + (b[i] - a[i]) * t);
        return quat_normalize(q);
    }
    let theta = dot.clamp(-1.0, 1.0).acos();
    let sin = theta.sin();
    let wa = ((1.0 - t) * theta).sin() / sin;
    let wb = (t * theta).sin() / sin;
    quat_normalize([0, 1, 2, 3].map(|i| a[i] * wa + b[i] * wb))
}

pub fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t)
}

/// Quaternion of a rotation of `angle` radians about axis 0 (x), 1 (y) or 2 (z).
pub fn axis_quat(axis: usize, angle: f32) -> [f32; 4] {
    let (s, c) = (angle * 0.5).sin_cos();
    let mut q = [0.0, 0.0, 0.0, c];
    q[axis] = s;
    q
}

/// Euler rotation order: the axes in the order they apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EulerOrder(pub [usize; 3]);

impl EulerOrder {
    pub const XYZ: Self = Self([0, 1, 2]);

    /// FBX `RotationOrder` enum: 0 XYZ, 1 XZY, 2 YZX, 3 YXZ, 4 ZXY, 5 ZYX
    /// (6 spheric XYZ is treated as XYZ).
    pub fn fbx(value: i64) -> Self {
        Self(match value {
            1 => [0, 2, 1],
            2 => [1, 2, 0],
            3 => [1, 0, 2],
            4 => [2, 0, 1],
            5 => [2, 1, 0],
            _ => [0, 1, 2],
        })
    }
}

/// Quaternion of Euler angles in degrees; the first axis of `order` applies first.
pub fn euler_quat(degrees: [f32; 3], order: EulerOrder) -> [f32; 4] {
    let mut q = [0.0, 0.0, 0.0, 1.0];
    for &axis in &order.0 {
        q = quat_mul(axis_quat(axis, degrees[axis].to_radians()), q);
    }
    q
}

/// Euler angles in degrees (XYZ order: X applies first) of a quaternion.
pub fn quat_euler_xyz(q: [f32; 4]) -> [f32; 3] {
    // R = Rz Ry Rx.
    let m = quat_matrix(q);
    let a = |r: usize, c: usize| m[c * 4 + r];
    let sy = -a(2, 0);
    let (x, y, z) = if sy.abs() < 0.999_999 {
        (a(2, 1).atan2(a(2, 2)), sy.asin(), a(1, 0).atan2(a(0, 0)))
    } else {
        // Gimbal lock: put everything into x.
        let y = std::f32::consts::FRAC_PI_2.copysign(sy);
        ((-a(1, 2)).atan2(a(1, 1)), y, 0.0)
    };
    [x.to_degrees(), y.to_degrees(), z.to_degrees()]
}

/// Applies `m` to the geometry of a part (positions, normals, tangents);
/// a mirroring transform reverses the triangles.
pub fn transform_geometry(m: &Mat4, g: &mut PartGeometry) {
    if *m == IDENTITY {
        return;
    }
    let inv = invert(m);
    let det = determinant3(m);
    for p in &mut g.positions {
        *p = point(m, *p);
    }
    if let Some(normals) = &mut g.normals {
        for n in normals.iter_mut() {
            // Inverse transpose.
            let v = [0, 1, 2]
                .map(|r| inv[r * 4] * n[0] + inv[r * 4 + 1] * n[1] + inv[r * 4 + 2] * n[2]);
            *n = normalize3(v);
        }
    }
    if let Some(tangents) = &mut g.tangents {
        for t in tangents.iter_mut() {
            let v = normalize3(vector(m, [t[0], t[1], t[2]]));
            *t = [v[0], v[1], v[2], if det < 0.0 { -t[3] } else { t[3] }];
        }
    }
    if det < 0.0 {
        for t in &mut g.triangles {
            t.swap(1, 2);
        }
    }
}

/// Triangles of a polygon (indices into `points`): a fan for convex
/// polygons, ear clipping otherwise.
pub fn triangulate(points: &[[f32; 3]]) -> Vec<[usize; 3]> {
    let n = points.len();
    if n < 3 {
        return Vec::new();
    }
    if n == 3 {
        return vec![[0, 1, 2]];
    }
    // Newell normal and the dominant axis to project on.
    let mut normal = [0f32; 3];
    for i in 0..n {
        let a = points[i];
        let b = points[(i + 1) % n];
        normal[0] += (a[1] - b[1]) * (a[2] + b[2]);
        normal[1] += (a[2] - b[2]) * (a[0] + b[0]);
        normal[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    let axis = (0..3)
        .max_by(|&a, &b| normal[a].abs().total_cmp(&normal[b].abs()))
        .unwrap_or(2);
    let (u, v) = match axis {
        0 => (1, 2),
        1 => (2, 0),
        _ => (0, 1),
    };
    let flip = normal[axis] < 0.0;
    let p2: Vec<[f32; 2]> = points
        .iter()
        .map(|p| if flip { [p[v], p[u]] } else { [p[u], p[v]] })
        .collect();
    let cross = |o: [f32; 2], a: [f32; 2], b: [f32; 2]| {
        (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    };
    let convex = (0..n).all(|i| cross(p2[i], p2[(i + 1) % n], p2[(i + 2) % n]) >= -1e-12);
    let fan = || (1..n - 1).map(|i| [0, i, i + 1]).collect::<Vec<_>>();
    // Ear clipping is quadratic: large polygons are fanned.
    if convex || n > 64 || normal.iter().all(|x| x.abs() < 1e-20) {
        return fan();
    }
    // Ear clipping.
    let mut remaining: Vec<usize> = (0..n).collect();
    let mut out = Vec::with_capacity(n - 2);
    let mut guard = 0;
    while remaining.len() > 3 && guard < n * n {
        guard += 1;
        let m = remaining.len();
        let mut clipped = false;
        for k in 0..m {
            let (a, b, c) = (
                remaining[(k + m - 1) % m],
                remaining[k],
                remaining[(k + 1) % m],
            );
            if cross(p2[a], p2[b], p2[c]) <= 1e-12 {
                continue;
            }
            let inside = remaining.iter().any(|&p| {
                p != a
                    && p != b
                    && p != c
                    && cross(p2[a], p2[b], p2[p]) >= 0.0
                    && cross(p2[b], p2[c], p2[p]) >= 0.0
                    && cross(p2[c], p2[a], p2[p]) >= 0.0
            });
            if inside {
                continue;
            }
            out.push([a, b, c]);
            remaining.remove(k);
            clipped = true;
            break;
        }
        if !clipped {
            // Degenerate or self-intersecting: fan the rest.
            break;
        }
    }
    for i in 1..remaining.len().saturating_sub(1) {
        out.push([remaining[0], remaining[i], remaining[i + 1]]);
    }
    out
}

/// Value of a uniformly sampled track at a time in frames (clamped).
pub fn sample_vec3(keys: &[[f32; 3]], frame: f32) -> Option<[f32; 3]> {
    let (i, t) = position(keys.len(), frame)?;
    Some(if t == 0.0 {
        keys[i]
    } else {
        lerp3(keys[i], keys[i + 1], t)
    })
}

/// See [`sample_vec3`].
pub fn sample_quat(keys: &[[f32; 4]], frame: f32) -> Option<[f32; 4]> {
    let (i, t) = position(keys.len(), frame)?;
    Some(if t == 0.0 {
        keys[i]
    } else {
        slerp(keys[i], keys[i + 1], t)
    })
}

fn position(len: usize, frame: f32) -> Option<(usize, f32)> {
    if len == 0 {
        return None;
    }
    let f = frame.clamp(0.0, (len - 1) as f32);
    let i = (f.floor() as usize).min(len - 1);
    let t = f - i as f32;
    if i + 1 >= len || t <= 1e-6 {
        Some((i, 0.0))
    } else {
        Some((i, t))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: &[f32], b: &[f32], eps: f32) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < eps)
    }

    #[test]
    fn compose_and_decompose_are_inverse() {
        let q = quat_normalize([0.1, 0.7, -0.2, 0.6]);
        let m = compose([1.0, 2.0, 3.0], q, [2.0, 2.0, 2.0]);
        let (t, r, s) = decompose(&m);
        assert!(close(&t, &[1.0, 2.0, 3.0], 1e-5));
        assert!(close(&s, &[2.0, 2.0, 2.0], 1e-5));
        let same = close(&r, &q, 1e-5) || close(&r, &q.map(|x| -x), 1e-5);
        assert!(same, "{r:?} {q:?}");
        assert!(close(&multiply(&m, &invert(&m)), &IDENTITY, 1e-5));
    }

    #[test]
    fn euler_round_trip() {
        let angles = [30.0, -45.0, 60.0];
        let q = euler_quat(angles, EulerOrder::XYZ);
        assert!(close(&quat_euler_xyz(q), &angles, 1e-3));
        // XYZ: x applies first, so R = Rz Ry Rx.
        let rx = quat_matrix(axis_quat(0, 30f32.to_radians()));
        let ry = quat_matrix(axis_quat(1, (-45f32).to_radians()));
        let rz = quat_matrix(axis_quat(2, 60f32.to_radians()));
        let m = multiply(&rz, &multiply(&ry, &rx));
        assert!(close(&quat_matrix(q), &m, 1e-5));
    }

    #[test]
    fn triangulates_concave_polygons() {
        // An L shape.
        let l = [
            [0., 0., 0.],
            [2., 0., 0.],
            [2., 1., 0.],
            [1., 1., 0.],
            [1., 2., 0.],
            [0., 2., 0.],
        ];
        let tris = triangulate(&l);
        assert_eq!(tris.len(), 4);
        let area: f32 = tris
            .iter()
            .map(|t| {
                let (a, b, c) = (l[t[0]], l[t[1]], l[t[2]]);
                ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])) / 2.0
            })
            .sum();
        assert!((area - 3.0).abs() < 1e-5, "{area}");
        assert_eq!(triangulate(&l[..4]).len(), 2);
    }

    #[test]
    fn samples_between_keys() {
        assert_eq!(sample_vec3(&[[0.0; 3], [2.0; 3]], 0.5), Some([1.0; 3]));
        assert_eq!(sample_vec3(&[[1.0; 3]], 7.0), Some([1.0; 3]));
        let q = sample_quat(&[[0., 0., 0., 1.], axis_quat(1, 1.0)], 0.5).unwrap();
        assert!(close(&q, &axis_quat(1, 0.5), 1e-5));
    }
}

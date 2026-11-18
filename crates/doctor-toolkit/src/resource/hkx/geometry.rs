//! Collision shapes as triangles: for viewing, export and bounds.
//!
//! Spheres, capsules and cylinders are tessellated; convex vertex shapes are
//! rebuilt from their plane equations (each face is the set of vertices on
//! its plane). The convex radius (the thin shell Havok keeps around convex
//! shapes) is not added.

use super::physics::{Physics, Shape, ShapeKind};

type Vec3 = [f32; 3];

/// Triangles in one coordinate frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TriangleMesh {
    pub positions: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
}

impl TriangleMesh {
    fn push(&mut self, positions: impl IntoIterator<Item = Vec3>, triangles: &[[u32; 3]]) {
        let base = self.positions.len() as u32;
        self.positions.extend(positions);
        self.triangles.extend(
            triangles
                .iter()
                .map(|t| [t[0] + base, t[1] + base, t[2] + base]),
        );
    }

    fn append(&mut self, other: TriangleMesh) {
        let TriangleMesh {
            positions,
            triangles,
        } = other;
        self.push(positions, &triangles);
    }

    fn transformed(mut self, f: impl Fn(Vec3) -> Vec3) -> Self {
        for p in &mut self.positions {
            *p = f(*p);
        }
        self
    }

    /// Smallest and largest corner, `None` when empty.
    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let mut it = self.positions.iter();
        let first = *it.next()?;
        Some(it.fold((first, first), |(lo, hi), p| {
            (
                [lo[0].min(p[0]), lo[1].min(p[1]), lo[2].min(p[2])],
                [hi[0].max(p[0]), hi[1].max(p[1]), hi[2].max(p[2])],
            )
        }))
    }

    /// Unique edges, for wireframe drawing.
    pub fn edges(&self) -> Vec<[u32; 2]> {
        let mut edges: Vec<[u32; 2]> = self
            .triangles
            .iter()
            .flat_map(|t| [[t[0], t[1]], [t[1], t[2]], [t[2], t[0]]])
            .map(|[a, b]| if a < b { [a, b] } else { [b, a] })
            .collect();
        edges.sort_unstable();
        edges.dedup();
        edges
    }

    /// Binary glTF (`.glb`) with one triangle mesh, no material.
    pub fn to_glb(&self, name: &str) -> Vec<u8> {
        let mut bin = Vec::new();
        for p in &self.positions {
            for v in p {
                bin.extend_from_slice(&v.to_le_bytes());
            }
        }
        let positions_len = bin.len();
        for t in &self.triangles {
            for i in t {
                bin.extend_from_slice(&i.to_le_bytes());
            }
        }
        let indices_len = bin.len() - positions_len;
        let (lo, hi) = self.bounds().unwrap_or(([0.0; 3], [0.0; 3]));
        let json = serde_json::json!({
            "asset": { "version": "2.0", "generator": "doctor_toolkit collision" },
            "scene": 0,
            "scenes": [{ "nodes": [0] }],
            "nodes": [{ "mesh": 0, "name": name }],
            "meshes": [{ "name": name, "primitives": [{
                "attributes": { "POSITION": 0 }, "indices": 1, "mode": 4
            }]}],
            "buffers": [{ "byteLength": bin.len() }],
            "bufferViews": [
                { "buffer": 0, "byteOffset": 0, "byteLength": positions_len, "target": 34962 },
                { "buffer": 0, "byteOffset": positions_len, "byteLength": indices_len, "target": 34963 }
            ],
            "accessors": [
                { "bufferView": 0, "componentType": 5126, "count": self.positions.len(),
                  "type": "VEC3", "min": lo, "max": hi },
                { "bufferView": 1, "componentType": 5125, "count": 3 * self.triangles.len(),
                  "type": "SCALAR" }
            ]
        });
        let mut json = serde_json::to_vec(&json).expect("JSON of plain values");
        while !json.len().is_multiple_of(4) {
            json.push(b' ');
        }
        let total = 12 + 8 + json.len() + 8 + bin.len();
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&(total as u32).to_le_bytes());
        out.extend_from_slice(&(json.len() as u32).to_le_bytes());
        out.extend_from_slice(b"JSON");
        out.extend_from_slice(&json);
        out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        out.extend_from_slice(b"BIN\0");
        out.extend_from_slice(&bin);
        out
    }

    /// Wavefront OBJ text.
    pub fn to_obj(&self, name: &str) -> String {
        use std::fmt::Write;
        let mut out = format!("# doctor_toolkit collision\no {name}\n");
        for p in &self.positions {
            let _ = writeln!(out, "v {} {} {}", p[0], p[1], p[2]);
        }
        for t in &self.triangles {
            let _ = writeln!(out, "f {} {} {}", t[0] + 1, t[1] + 1, t[2] + 1);
        }
        out
    }
}

const SEGMENTS: usize = 12;

fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(a: Vec3, s: f32) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn length(a: Vec3) -> f32 {
    dot(a, a).sqrt()
}

fn normalize(a: Vec3) -> Vec3 {
    let l = length(a);
    if l > 0.0 {
        scale(a, 1.0 / l)
    } else {
        [0.0, 1.0, 0.0]
    }
}

fn xyz(v: [f32; 4]) -> Vec3 {
    [v[0], v[1], v[2]]
}

fn basis(axis: Vec3) -> (Vec3, Vec3) {
    let n = normalize(axis);
    let helper = if n[0].abs() < 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 0.0, 1.0]
    };
    let u = normalize(cross(n, helper));
    (u, cross(n, u))
}

/// A tube between `a` and `b` with radius `r`, closed by caps; `round` gives
/// hemispherical caps (capsules), otherwise flat ones (cylinders).
fn tube(a: Vec3, b: Vec3, r: f32, round: bool) -> TriangleMesh {
    let axis = sub(b, a);
    let (u, v) = basis(axis);
    let dir = normalize(axis);
    let rings = if round { SEGMENTS / 4 } else { 0 };
    // Profile: (distance along the axis from the end, radius) for each ring.
    let mut profile: Vec<(Vec3, f32, f32)> = Vec::new();
    for i in (0..=rings).rev() {
        let t = i as f32 / (rings.max(1)) as f32 * std::f32::consts::FRAC_PI_2;
        profile.push((a, -r * t.sin(), r * t.cos()));
    }
    for i in 0..=rings {
        let t = i as f32 / (rings.max(1)) as f32 * std::f32::consts::FRAC_PI_2;
        profile.push((b, r * t.sin(), r * t.cos()));
    }
    let mut mesh = TriangleMesh::default();
    for &(centre, along, radius) in &profile {
        for s in 0..SEGMENTS {
            let angle = s as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
            let offset = add(
                scale(u, angle.cos() * radius),
                scale(v, angle.sin() * radius),
            );
            mesh.positions
                .push(add(add(centre, scale(dir, along)), offset));
        }
    }
    let ring = |k: usize, s: usize| (k * SEGMENTS + s % SEGMENTS) as u32;
    for k in 0..profile.len() - 1 {
        for s in 0..SEGMENTS {
            mesh.triangles
                .push([ring(k, s), ring(k + 1, s), ring(k + 1, s + 1)]);
            mesh.triangles
                .push([ring(k, s), ring(k + 1, s + 1), ring(k, s + 1)]);
        }
    }
    // Caps (a fan around the first and last ring).
    let last = profile.len() - 1;
    for s in 1..SEGMENTS - 1 {
        mesh.triangles
            .push([ring(0, 0), ring(0, s + 1), ring(0, s)]);
        mesh.triangles
            .push([ring(last, 0), ring(last, s), ring(last, s + 1)]);
    }
    mesh
}

fn sphere(r: f32) -> TriangleMesh {
    tube([0.0, 0.0, 0.0], [0.0, 0.0, 0.0], r, true)
}

fn box_mesh(h: Vec3) -> TriangleMesh {
    let mut mesh = TriangleMesh::default();
    for i in 0..8 {
        mesh.positions.push([
            if i & 1 == 0 { -h[0] } else { h[0] },
            if i & 2 == 0 { -h[1] } else { h[1] },
            if i & 4 == 0 { -h[2] } else { h[2] },
        ]);
    }
    for [a, b, c, d] in [
        [0, 2, 6, 4],
        [1, 5, 7, 3],
        [0, 4, 5, 1],
        [2, 3, 7, 6],
        [0, 1, 3, 2],
        [4, 6, 7, 5],
    ] {
        mesh.triangles.push([a, b, c]);
        mesh.triangles.push([a, c, d]);
    }
    mesh
}

/// Faces of a convex hull given by its vertices and planes `n·p + d = 0`
/// (each face: the vertices furthest along a plane's normal).
pub fn convex_faces(vertices: &[Vec3], planes: &[[f32; 4]]) -> TriangleMesh {
    let mut mesh = TriangleMesh::default();
    let size = vertices
        .iter()
        .map(|v| length(*v))
        .fold(0.0f32, f32::max)
        .max(1e-3);
    for plane in planes {
        let n = xyz(*plane);
        if length(n) < 1e-6 {
            continue;
        }
        // The game's planes lie outside the vertices by the convex radius:
        // the face is the set of vertices furthest along the normal.
        let level = vertices
            .iter()
            .map(|&v| dot(n, v) + plane[3])
            .fold(f32::MIN, f32::max);
        let on: Vec<Vec3> = vertices
            .iter()
            .copied()
            .filter(|&v| (dot(n, v) + plane[3] - level).abs() <= 1e-3 * size.max(1.0))
            .collect();
        if on.len() < 3 {
            continue;
        }
        let centre = scale(
            on.iter().fold([0.0; 3], |s, &v| add(s, v)),
            1.0 / on.len() as f32,
        );
        let (u, v) = basis(n);
        let mut sorted: Vec<(f32, Vec3)> = on
            .iter()
            .map(|&p| {
                let d = sub(p, centre);
                (dot(d, v).atan2(dot(d, u)), p)
            })
            .collect();
        sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
        let ring: Vec<Vec3> = sorted.into_iter().map(|(_, p)| p).collect();
        let tris: Vec<[u32; 3]> = (1..ring.len() as u32 - 1).map(|k| [0, k, k + 1]).collect();
        mesh.push(ring, &tris);
    }
    mesh
}

/// The shape as triangles in its own frame.
pub fn shape_mesh(shape: &Shape) -> TriangleMesh {
    match &shape.kind {
        ShapeKind::Sphere { radius } => sphere(*radius),
        ShapeKind::Box { half_extents, .. } => box_mesh(xyz(*half_extents)),
        ShapeKind::Capsule { radius, a, b } => tube(xyz(*a), xyz(*b), *radius, true),
        ShapeKind::Cylinder {
            cylinder_radius,
            a,
            b,
            ..
        } => tube(xyz(*a), xyz(*b), *cylinder_radius, false),
        ShapeKind::ConvexVertices {
            vertices, planes, ..
        } => convex_faces(vertices, planes),
        ShapeKind::ConvexTranslate {
            translation, child, ..
        } => shape_mesh(child).transformed(|p| add(p, xyz(*translation))),
        ShapeKind::ConvexTransform {
            transform, child, ..
        } => shape_mesh(child).transformed(|p| apply(transform, p)),
        ShapeKind::List { children, .. } => {
            let mut mesh = TriangleMesh::default();
            for child in children {
                mesh.append(shape_mesh(child));
            }
            mesh
        }
        ShapeKind::Mopp { child, .. } => shape_mesh(child),
        ShapeKind::SimpleMesh {
            vertices,
            triangles,
            ..
        } => {
            let n = vertices.len() as u32;
            let mut mesh = TriangleMesh::default();
            let tris: Vec<[u32; 3]> = triangles
                .iter()
                .copied()
                .filter(|t| t.iter().all(|&i| i < n))
                .collect();
            mesh.push(vertices.iter().map(|v| xyz(*v)), &tris);
            mesh
        }
        ShapeKind::ExtendedMesh(m) => {
            let mut mesh = TriangleMesh::default();
            let s = xyz(m.scaling);
            for part in &m.triangle_subparts {
                let n = part.vertices.len() as u32;
                let tris: Vec<[u32; 3]> = part
                    .indices
                    .iter()
                    .map(|i| [i[0], i[1], i[2]])
                    .filter(|t| t.iter().all(|&i| i < n))
                    .collect();
                mesh.push(
                    part.vertices
                        .iter()
                        .map(|v| [v[0] * s[0], v[1] * s[1], v[2] * s[2]]),
                    &tris,
                );
            }
            for part in &m.shape_subparts {
                let q = part.rotation;
                let t = xyz(part.translation);
                for child in &part.shapes {
                    mesh.append(shape_mesh(child).transformed(|p| add(rotate(q, p), t)));
                }
            }
            mesh
        }
    }
}

fn apply(m: &[[f32; 4]; 4], p: Vec3) -> Vec3 {
    let c = |k: usize| xyz(m[k]);
    add(
        add(add(scale(c(0), p[0]), scale(c(1), p[1])), scale(c(2), p[2])),
        c(3),
    )
}

/// Rotates `p` by the quaternion `q` (`x, y, z, w`).
fn rotate(q: [f32; 4], p: Vec3) -> Vec3 {
    let u = [q[0], q[1], q[2]];
    let w = q[3];
    let t = scale(cross(u, p), 2.0);
    add(add(p, scale(t, w)), cross(u, t))
}

/// Radius of the shape around its origin, including the convex radius.
pub fn bounding_radius(shape: &Shape) -> f32 {
    let mesh = shape_mesh(shape);
    let r = mesh
        .positions
        .iter()
        .map(|&p| length(p))
        .fold(0.0f32, f32::max);
    r + convex_radius(shape)
}

fn convex_radius(shape: &Shape) -> f32 {
    match &shape.kind {
        ShapeKind::Box { radius, .. }
        | ShapeKind::ConvexVertices { radius, .. }
        | ShapeKind::ConvexTranslate { radius, .. }
        | ShapeKind::ConvexTransform { radius, .. }
        | ShapeKind::SimpleMesh { radius, .. } => *radius,
        ShapeKind::Cylinder { radius, .. } => *radius,
        ShapeKind::ExtendedMesh(m) => m.aabb_half_extents[3],
        _ => 0.0,
    }
}

/// All bodies' shapes in model space (each body's transform applied).
pub fn physics_mesh(physics: &Physics) -> TriangleMesh {
    let mut mesh = TriangleMesh::default();
    for body in physics.bodies() {
        let r = body.rotation;
        let t = body.translation;
        mesh.append(shape_mesh(&body.shape).transformed(|p| {
            add(
                add(add(scale(r[0], p[0]), scale(r[1], p[1])), scale(r[2], p[2])),
                t,
            )
        }));
    }
    mesh
}

/// Most points a hull is built of. A model of more is represented by its
/// outermost points in [`HULL_DIRECTIONS`] directions: a collision hull of
/// thousands of vertices is of no use to the game, and the points of a
/// detailed model that lie in one plane or close to it (the flat sides of a
/// prop) make the incremental build below lose track of what is visible —
/// its list of faces grew without end, and a model of three thousand
/// vertices took minutes.
const HULL_POINTS: usize = 256;
/// Directions the outermost points are taken in: evenly over the sphere.
const HULL_DIRECTIONS: usize = 252;

/// The points a hull is built of: `points` without repeats, and of more
/// than [`HULL_POINTS`] — the outermost one in each of the axes, the
/// diagonals and [`HULL_DIRECTIONS`] more directions (every one of them is
/// a vertex of the true hull, so the hull of these lies within it and
/// touches it in every such direction).
fn hull_points(points: &[Vec3]) -> Vec<Vec3> {
    let mut seen = std::collections::HashSet::new();
    let unique: Vec<Vec3> = points
        .iter()
        .copied()
        .filter(|p| seen.insert(p.map(f32::to_bits)))
        .collect();
    if unique.len() <= HULL_POINTS {
        return unique;
    }
    let mut directions: Vec<Vec3> = Vec::new();
    for axis in 0..3 {
        for sign in [1.0, -1.0] {
            let mut d = [0.0; 3];
            d[axis] = sign;
            directions.push(d);
        }
    }
    for corner in 0..8 {
        directions.push([0, 1, 2].map(|a| if corner >> a & 1 == 0 { 1.0 } else { -1.0 }));
    }
    for i in 0..HULL_DIRECTIONS {
        // A Fibonacci lattice: the golden angle between neighbours.
        let z = 1.0 - 2.0 * (i as f32 + 0.5) / HULL_DIRECTIONS as f32;
        let (r, a) = ((1.0 - z * z).sqrt(), 2.399_963_2 * i as f32);
        directions.push([r * a.cos(), r * a.sin(), z]);
    }
    let mut chosen: Vec<usize> = directions
        .iter()
        .filter_map(|d| {
            (0..unique.len()).max_by(|&a, &b| dot(*d, unique[a]).total_cmp(&dot(*d, unique[b])))
        })
        .collect();
    chosen.sort_unstable();
    chosen.dedup();
    chosen.into_iter().map(|i| unique[i]).collect()
}

/// The convex hull of `points`: vertices and outward planes `n·p + d = 0`.
/// Returns `None` for fewer than four points not in one plane. Of more than
/// [`HULL_POINTS`] distinct points the hull is that of their outermost
/// points ([`hull_points`]).
pub fn convex_hull(points: &[Vec3]) -> Option<(Vec<Vec3>, Vec<[f32; 4]>)> {
    let points = &hull_points(points)[..];
    let (lo, hi) = TriangleMesh {
        positions: points.to_vec(),
        triangles: Vec::new(),
    }
    .bounds()?;
    let eps = 1e-5 * length(sub(hi, lo)).max(1e-3);
    // Initial tetrahedron.
    let i0 = 0;
    let i1 = (0..points.len()).max_by(|&a, &b| {
        length(sub(points[a], points[i0])).total_cmp(&length(sub(points[b], points[i0])))
    })?;
    let line = sub(points[i1], points[i0]);
    let i2 = (0..points.len()).max_by(|&a, &b| {
        length(cross(line, sub(points[a], points[i0])))
            .total_cmp(&length(cross(line, sub(points[b], points[i0]))))
    })?;
    let normal = cross(line, sub(points[i2], points[i0]));
    if length(normal) <= eps * eps {
        return None;
    }
    let i3 = (0..points.len()).max_by(|&a, &b| {
        dot(normal, sub(points[a], points[i0]))
            .abs()
            .total_cmp(&dot(normal, sub(points[b], points[i0])).abs())
    })?;
    if dot(normalize(normal), sub(points[i3], points[i0])).abs() <= eps {
        return None;
    }
    let mut faces: Vec<[usize; 3]> = vec![[i0, i1, i2], [i0, i2, i3], [i0, i3, i1], [i1, i3, i2]];
    let centre = scale(
        [i0, i1, i2, i3]
            .iter()
            .fold([0.0; 3], |s, &i| add(s, points[i])),
        0.25,
    );
    let orient = |f: [usize; 3]| -> [usize; 3] {
        let n = cross(
            sub(points[f[1]], points[f[0]]),
            sub(points[f[2]], points[f[0]]),
        );
        if dot(n, sub(points[f[0]], centre)) < 0.0 {
            [f[0], f[2], f[1]]
        } else {
            f
        }
    };
    for f in &mut faces {
        *f = orient(*f);
    }
    let above = |f: &[usize; 3], p: Vec3| -> f32 {
        let n = normalize(cross(
            sub(points[f[1]], points[f[0]]),
            sub(points[f[2]], points[f[0]]),
        ));
        dot(n, sub(p, points[f[0]]))
    };
    for (i, &p) in points.iter().enumerate() {
        if [i0, i1, i2, i3].contains(&i) {
            continue;
        }
        let visible: Vec<bool> = faces.iter().map(|f| above(f, p) > eps).collect();
        if !visible.contains(&true) {
            continue;
        }
        // Horizon: edges of visible faces whose twin belongs to a hidden face.
        // The edges of the visible faces are put into a set first: looking
        // every edge up among all faces made a model of a few thousand
        // vertices take minutes.
        let seen: std::collections::HashSet<(usize, usize)> = faces
            .iter()
            .zip(&visible)
            .filter(|(_, v)| **v)
            .flat_map(|(f, _)| (0..3).map(|k| (f[k], f[(k + 1) % 3])))
            .collect();
        let mut horizon = Vec::new();
        for (f, _) in faces.iter().zip(&visible).filter(|(_, v)| **v) {
            for k in 0..3 {
                let (a, b) = (f[k], f[(k + 1) % 3]);
                if !seen.contains(&(b, a)) {
                    horizon.push((a, b));
                }
            }
        }
        let mut kept: Vec<[usize; 3]> = faces
            .iter()
            .zip(&visible)
            .filter(|(_, v)| !**v)
            .map(|(f, _)| *f)
            .collect();
        for (a, b) in horizon {
            kept.push([a, b, i]);
        }
        faces = kept;
        // A hull of n vertices has no more than 2n - 4 faces: more means
        // the build went wrong on points close to one plane. Rather no hull
        // than a build that never ends.
        if faces.len() > 4 * points.len() + 16 {
            return None;
        }
    }
    // Vertices used by the hull, and one plane per distinct face normal.
    let mut used: Vec<usize> = faces.iter().flatten().copied().collect();
    used.sort_unstable();
    used.dedup();
    let vertices: Vec<Vec3> = used.iter().map(|&i| points[i]).collect();
    let mut planes: Vec<[f32; 4]> = Vec::new();
    for f in &faces {
        let n = normalize(cross(
            sub(points[f[1]], points[f[0]]),
            sub(points[f[2]], points[f[0]]),
        ));
        let d = -dot(n, points[f[0]]);
        let same = planes
            .iter()
            .any(|p| dot(xyz(*p), n) > 1.0 - 1e-5 && (p[3] - d).abs() <= eps * 10.0);
        if !same {
            planes.push([n[0], n[1], n[2], d]);
        }
    }
    Some((vertices, planes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hull_of_a_few_thousand_points_is_built_at_once() {
        // Points on a sphere: every one of them is a vertex of the hull,
        // the worst case for the number of faces.
        let n = 3000;
        let points: Vec<Vec3> = (0..n)
            .map(|i| {
                let t = (i as f32 + 0.5) / n as f32;
                let (z, a) = (1.0 - 2.0 * t, 2.399_963 * i as f32);
                let r = (1.0 - z * z).sqrt();
                [r * a.cos(), r * a.sin(), z]
            })
            .collect();
        let started = std::time::Instant::now();
        let (vertices, planes) = convex_hull(&points).unwrap();
        assert!(
            started.elapsed() < std::time::Duration::from_secs(20),
            "{:?}",
            started.elapsed()
        );
        // The outermost points in the directions looked at: every one is
        // a point of the sphere, and the hull reaches close to it everywhere.
        assert!(
            vertices.len() > 200 && vertices.len() <= 266,
            "{}",
            vertices.len()
        );
        assert!(vertices.iter().all(|v| points.contains(v)));
        for p in &points {
            for plane in &planes {
                assert!(dot(xyz(*plane), *p) + plane[3] < 0.06);
            }
        }
    }

    #[test]
    fn hull_of_a_detailed_box_does_not_lose_its_way() {
        // A box with its sides covered by a fine grid of points: thousands
        // of points in six planes, as the flat sides of a detailed model.
        let mut points: Vec<Vec3> = Vec::new();
        let n = 24;
        for i in 0..=n {
            for j in 0..=n {
                let (u, v) = (i as f32 / n as f32, j as f32 / n as f32);
                for side in [0.0, 1.0] {
                    points.push([u * 2.0, v * 3.0, side * 4.0]);
                    points.push([u * 2.0, side * 3.0, v * 4.0]);
                    points.push([side * 2.0, u * 3.0, v * 4.0]);
                }
            }
        }
        assert!(points.len() > 3000);
        let started = std::time::Instant::now();
        let (vertices, planes) = convex_hull(&points).unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(20));
        assert_eq!(planes.len(), 6, "{planes:?}");
        // The eight corners are among the vertices.
        for corner in 0..8 {
            let c = [
                if corner & 1 == 0 { 0.0 } else { 2.0 },
                if corner & 2 == 0 { 0.0 } else { 3.0 },
                if corner & 4 == 0 { 0.0 } else { 4.0 },
            ];
            assert!(vertices.contains(&c), "{c:?}");
        }
    }

    #[test]
    fn hull_of_a_cube_has_six_planes() {
        let mut points = Vec::new();
        for i in 0..8 {
            points.push([(i & 1) as f32, ((i >> 1) & 1) as f32, ((i >> 2) & 1) as f32]);
        }
        points.push([0.5, 0.5, 0.5]);
        let (vertices, planes) = convex_hull(&points).unwrap();
        assert_eq!(vertices.len(), 8);
        assert_eq!(planes.len(), 6);
        for p in &planes {
            for v in &vertices {
                assert!(dot(xyz(*p), *v) + p[3] <= 1e-4);
            }
        }
        let faces = convex_faces(&vertices, &planes);
        assert_eq!(faces.triangles.len(), 12);
        assert!(convex_hull(&[[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]).is_none());
    }

    #[test]
    fn tessellated_shapes_have_the_right_size() {
        let capsule: Shape = ShapeKind::Capsule {
            radius: 0.5,
            a: [0.0, 0.0, 0.0, 0.5],
            b: [0.0, 2.0, 0.0, 0.5],
        }
        .into();
        let (lo, hi) = shape_mesh(&capsule).bounds().unwrap();
        assert!((lo[1] + 0.5).abs() < 1e-4 && (hi[1] - 2.5).abs() < 1e-4);
        assert!((hi[0] - 0.5).abs() < 1e-4);
        let b: Shape = ShapeKind::Box {
            radius: 0.0,
            half_extents: [1.0, 2.0, 3.0, 0.0],
        }
        .into();
        assert_eq!(shape_mesh(&b).edges().len(), 18);
        assert!((bounding_radius(&b) - 14f32.sqrt()).abs() < 1e-5);
    }
}

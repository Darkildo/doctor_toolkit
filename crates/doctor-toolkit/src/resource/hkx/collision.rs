//! Collision for new objects: Havok packfiles built from triangle meshes.
//!
//! Three kinds, all on one fixed (static) rigid body, laid out like the
//! game's own packfiles:
//!
//! - [`CollisionKind::Mesh`]: the triangles as an `hkpSimpleMeshShape` under
//!   an `hkpMoppBvTreeShape` with new MOPP code ([`super::mopp::build`]) — the
//!   form of 8 430 static meshes in the game;
//! - [`CollisionKind::Hull`]: the convex hull of the vertices as an
//!   `hkpConvexVerticesShape`;
//! - [`CollisionKind::Box`]: the axis-aligned bounding box as an
//!   `hkpBoxShape` (under an `hkpConvexTranslateShape` when it is not centred).
//!
//! The body gets one EA material (sound and effects of the surface): the
//! first material of a template packfile, usually the collision the new
//! object replaces, or [`DEFAULT_MATERIAL`].

use serde::Serialize;

use super::mopp::{self, Primitive};
use super::physics::{EaMaterial, Physics, Shape, ShapeKind, WELDING_NONE};
use crate::{Error, Result};

/// The most common EA material of the game (surface 1).
pub const DEFAULT_MATERIAL: EaMaterial = EaMaterial {
    words: [1, 0x01FF_0000, 0, 0, 0, 0, 0, 0],
};

/// Convex radius of new convex shapes (the game's usual 5 cm).
pub const CONVEX_RADIUS: f32 = 0.05;

/// Which shape to build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CollisionKind {
    /// Triangle mesh with MOPP code (static objects).
    #[default]
    Mesh,
    /// Convex hull of the vertices.
    Hull,
    /// Axis-aligned bounding box.
    Box,
}

impl std::str::FromStr for CollisionKind {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "mesh" => Ok(Self::Mesh),
            "hull" => Ok(Self::Hull),
            "box" => Ok(Self::Box),
            other => Err(Error::InvalidArgument(format!(
                "collision kind `{other}`: expected mesh, hull or box"
            ))),
        }
    }
}

/// Input geometry, in model space (metres, Y up, as the model's meshes).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CollisionMesh {
    pub positions: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
}

impl CollisionMesh {
    /// Adds a part with its own vertex numbering.
    pub fn append(&mut self, positions: &[[f32; 3]], triangles: &[[u32; 3]]) {
        let base = self.positions.len() as u32;
        self.positions.extend_from_slice(positions);
        self.triangles
            .extend(triangles.iter().map(|t| t.map(|i| i + base)));
    }

    fn check(&self) -> Result<()> {
        if self.positions.is_empty() {
            return Err(Error::InvalidArgument(
                "the collision mesh has no vertices".into(),
            ));
        }
        if self.positions.iter().flatten().any(|v| !v.is_finite()) {
            return Err(Error::InvalidArgument(
                "the collision mesh has a vertex that is not a finite number".into(),
            ));
        }
        let n = self.positions.len() as u32;
        if self.triangles.iter().flatten().any(|&i| i >= n) {
            return Err(Error::InvalidArgument(
                "a triangle uses a vertex the mesh does not have".into(),
            ));
        }
        Ok(())
    }

    fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for p in &self.positions {
            for a in 0..3 {
                lo[a] = lo[a].min(p[a]);
                hi[a] = hi[a].max(p[a]);
            }
        }
        (lo, hi)
    }
}

/// What [`build`] made.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CollisionSummary {
    pub kind: CollisionKind,
    pub vertices: usize,
    pub triangles: usize,
    /// Triangles dropped because two of their corners coincide.
    pub degenerate: usize,
    pub mopp_bytes: usize,
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
    pub material: [u32; 8],
    pub size: usize,
}

/// The mesh shape: an `hkpSimpleMeshShape` under MOPP code.
pub fn mesh_shape(mesh: &CollisionMesh) -> Result<(Shape, usize)> {
    mesh.check()?;
    let mut triangles = Vec::with_capacity(mesh.triangles.len());
    let mut degenerate = 0;
    for t in &mesh.triangles {
        if t[0] == t[1] || t[1] == t[2] || t[0] == t[2] {
            degenerate += 1;
            continue;
        }
        let [a, b, c] = t.map(|i| mesh.positions[i as usize]);
        if a == b || b == c || a == c {
            degenerate += 1;
            continue;
        }
        triangles.push(*t);
    }
    if triangles.is_empty() {
        return Err(Error::InvalidArgument(
            "the collision mesh has no triangles".into(),
        ));
    }
    let primitives: Vec<Primitive> = triangles
        .iter()
        .enumerate()
        .map(|(key, t)| {
            let corners = t.map(|i| mesh.positions[i as usize]);
            let mut min = corners[0];
            let mut max = corners[0];
            for c in &corners[1..] {
                for a in 0..3 {
                    min[a] = min[a].min(c[a]);
                    max[a] = max[a].max(c[a]);
                }
            }
            Primitive {
                key: key as u32,
                min,
                max,
            }
        })
        .collect();
    // A margin of 1 cm, like the tolerance Havok adds around triangles.
    let code = mopp::build(&primitives, 0.01);
    let shape = ShapeKind::Mopp {
        code,
        child: Box::new(
            ShapeKind::SimpleMesh {
                radius: 0.0,
                welding_type: WELDING_NONE,
                vertices: mesh
                    .positions
                    .iter()
                    .map(|p| [p[0], p[1], p[2], 0.0])
                    .collect(),
                triangles,
                welding: Vec::new(),
                material_indices: Vec::new(),
            }
            .into(),
        ),
    };
    Ok((shape.into(), degenerate))
}

/// The bounding box shape, translated when it is not centred on the origin.
pub fn box_shape(mesh: &CollisionMesh) -> Result<Shape> {
    mesh.check()?;
    let (lo, hi) = mesh.bounds();
    let half = [0, 1, 2].map(|a| ((hi[a] - lo[a]) * 0.5 - CONVEX_RADIUS).max(0.01));
    let centre = [0, 1, 2].map(|a| (hi[a] + lo[a]) * 0.5);
    let shape: Shape = ShapeKind::Box {
        radius: CONVEX_RADIUS,
        half_extents: [half[0], half[1], half[2], 0.0],
    }
    .into();
    if centre.iter().all(|&c| c.abs() < 1e-6) {
        return Ok(shape);
    }
    Ok(ShapeKind::ConvexTranslate {
        radius: CONVEX_RADIUS,
        translation: [centre[0], centre[1], centre[2], 0.0],
        child: Box::new(shape),
    }
    .into())
}

/// The convex hull shape.
pub fn hull_shape(mesh: &CollisionMesh) -> Result<Shape> {
    mesh.check()?;
    let (vertices, mut planes) =
        super::geometry::convex_hull(&mesh.positions).ok_or_else(|| {
            Error::InvalidArgument("the collision mesh is flat: it has no convex hull".into())
        })?;
    // As in the game's files, the planes enclose the convex radius.
    for p in &mut planes {
        p[3] -= CONVEX_RADIUS;
    }
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for v in &vertices {
        for a in 0..3 {
            lo[a] = lo[a].min(v[a]);
            hi[a] = hi[a].max(v[a]);
        }
    }
    let half = [0, 1, 2].map(|a| (hi[a] - lo[a]) * 0.5 + CONVEX_RADIUS);
    let centre = [0, 1, 2].map(|a| (hi[a] + lo[a]) * 0.5);
    Ok(ShapeKind::ConvexVertices {
        radius: CONVEX_RADIUS,
        aabb_half_extents: [half[0], half[1], half[2], 0.0],
        aabb_center: [centre[0], centre[1], centre[2], 0.0],
        vertices,
        planes,
        filler: Vec::new(),
    }
    .into())
}

/// Builds a packfile with collision for `mesh`. `material` is the EA material
/// of the body ([`template_material`] finds one in an existing packfile).
pub fn build(
    mesh: &CollisionMesh,
    kind: CollisionKind,
    material: EaMaterial,
    name: &str,
) -> Result<(Vec<u8>, CollisionSummary)> {
    let (shape, degenerate) = match kind {
        CollisionKind::Mesh => mesh_shape(mesh)?,
        CollisionKind::Hull => (hull_shape(mesh)?, 0),
        CollisionKind::Box => (box_shape(mesh)?, 0),
    };
    let mopp_bytes = match &shape.kind {
        ShapeKind::Mopp { code, .. } => code.data.len(),
        _ => 0,
    };
    let physics = Physics::fixed(vec![(Some(name.to_owned()), shape)], vec![material]);
    let bytes = physics.to_bytes()?;
    let (bounds_min, bounds_max) = mesh.bounds();
    let summary = CollisionSummary {
        kind,
        vertices: mesh.positions.len(),
        triangles: mesh.triangles.len() - degenerate,
        degenerate,
        mopp_bytes,
        bounds_min,
        bounds_max,
        material: material.words,
        size: bytes.len(),
    };
    Ok((bytes, summary))
}

/// The first EA material of a packfile, or [`DEFAULT_MATERIAL`].
pub fn template_material(packfile: &[u8]) -> EaMaterial {
    Physics::parse(packfile)
        .ok()
        .and_then(|p| p.materials.and_then(|m| m.into_iter().flatten().next()))
        .unwrap_or(DEFAULT_MATERIAL)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::hkx::Packfile;
    use crate::resource::hkx::geometry::physics_mesh;

    fn cube() -> CollisionMesh {
        let mut mesh = CollisionMesh::default();
        for i in 0..8 {
            mesh.positions.push([
                1.0 + (i & 1) as f32,
                ((i >> 1) & 1) as f32 * 2.0,
                ((i >> 2) & 1) as f32,
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
        mesh.triangles.push([0, 0, 1]);
        mesh
    }

    #[test]
    fn every_kind_builds_a_packfile_the_reader_takes_back() {
        for kind in [CollisionKind::Mesh, CollisionKind::Hull, CollisionKind::Box] {
            let (bytes, summary) = build(&cube(), kind, DEFAULT_MATERIAL, "doctor").unwrap();
            assert_eq!(summary.size, bytes.len());
            let packfile = Packfile::parse(&bytes).unwrap();
            assert_eq!(packfile.root_class, "hkRootLevelContainer");
            assert_eq!(packfile.contents_version, "Havok-5.5.0-r1");
            let physics = Physics::parse(&bytes).unwrap();
            assert_eq!(
                physics.materials.as_deref(),
                Some(&[Some(DEFAULT_MATERIAL)][..])
            );
            let body = physics.bodies().next().unwrap();
            assert_eq!(body.name.as_deref(), Some("doctor"));
            let (lo, hi) = physics_mesh(&physics).bounds().unwrap();
            let slack = if kind == CollisionKind::Box {
                CONVEX_RADIUS + 1e-4
            } else {
                1e-4
            };
            for a in 0..3 {
                assert!(
                    (lo[a] - cube().bounds().0[a]).abs() <= slack,
                    "{kind:?} {lo:?}"
                );
                assert!(
                    (hi[a] - cube().bounds().1[a]).abs() <= slack,
                    "{kind:?} {hi:?}"
                );
            }
            // Writing what was read gives the same bytes.
            let again = physics.to_bytes().unwrap();
            assert_eq!(again, bytes, "{kind:?}");
            if kind == CollisionKind::Mesh {
                assert_eq!(summary.degenerate, 1);
                let ShapeKind::Mopp { code, child } = &body.shape.kind else {
                    panic!("{:?}", body.shape)
                };
                let ShapeKind::SimpleMesh { triangles, .. } = &child.kind else {
                    panic!()
                };
                assert_eq!(
                    code.keys().unwrap(),
                    (0..triangles.len() as u32).collect::<Vec<_>>()
                );
            }
        }
        assert!(
            build(
                &CollisionMesh::default(),
                CollisionKind::Mesh,
                DEFAULT_MATERIAL,
                "x"
            )
            .is_err()
        );
        let mut bad = cube();
        bad.triangles.push([0, 1, 99]);
        assert!(build(&bad, CollisionKind::Box, DEFAULT_MATERIAL, "x").is_err());
    }

    #[test]
    fn huge_vertex_counts_are_rejected_without_allocating() {
        // Found by fuzzing: a vertex group count of billions allocated before
        // the array was checked.
        let (bytes, _) = build(&cube(), CollisionKind::Hull, DEFAULT_MATERIAL, "x").unwrap();
        let packfile = Packfile::parse(&bytes).unwrap();
        let shape = packfile
            .objects
            .iter()
            .find(|o| o.class == "hkpConvexVerticesShape")
            .unwrap();
        let at = (packfile.sections[shape.section].start + shape.offset + 0x44) as usize;
        let mut bad = bytes.clone();
        bad[at..at + 4].copy_from_slice(&0x7FFF_FFFFu32.to_le_bytes());
        assert!(Physics::parse(&bad).is_err());
    }

    #[test]
    fn objects_shorter_than_their_class_are_rejected() {
        // Found by fuzzing: a rigid body of 0xAC bytes was indexed at 0xD8.
        let (shape, _) = mesh_shape(&cube()).unwrap();
        let physics = Physics::fixed(vec![(None, shape)], vec![DEFAULT_MATERIAL]);
        let mut graph = physics.to_graph();
        let body = graph
            .nodes
            .iter()
            .position(|n| n.class.as_deref() == Some("hkpRigidBody"))
            .unwrap();
        graph.nodes[body].bytes.truncate(0xA0);
        let mut packfile = physics.to_packfile().unwrap();
        graph.store(&mut packfile).unwrap();
        assert!(Physics::parse(&packfile.to_bytes()).is_err());
    }
}

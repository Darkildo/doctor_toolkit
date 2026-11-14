//! Tree models, `treemodel` (`0xCDB9D42B`): the trees and palms of the
//! levels, referred to by `TreeModelPart.TreeModel` of a metamodel.
//!
//! A tree is a list of parts, each drawn with one material and one of three
//! vertex shaders of the game (`docs/formats/treemodel.md`):
//!
//! - [`PartKind::Trunk`] — `TreeTrunk`: indexed triangle strips (trunks,
//!   branches, some leaf meshes with `TreeNonRotatingLeaf` materials);
//! - [`PartKind::RotatingLeaves`] — `TreeRotatingLeaf`: leaf cards that turn
//!   to the camera. Every corner is the leaf centre with a size and a corner
//!   index into the 40 corners the game computes per frame from the camera
//!   (`LeafQuads`, see [`leaf_corner`]);
//! - [`PartKind::Leaves`] — `TreeNonRotatingLeaf`: fixed leaf cards and
//!   palm fronds as a quad list (or a triangle list).
//!
//! # Layout
//!
//! All values are little-endian.
//!
//! ```text
//! 0x00  u32  version, 2
//! 0x04  u32  magic 0x54726565 ("eerT" on disk)
//! 0x08  u32  mesh count (of all parts)
//! 0x0C  u32  part count
//! 0x10  u32  CRC-32 of the source path (the first string); the resource id
//!            is its SDBM hash
//! 0x14  2 × u32  0
//! 0x1C  u32  offset of the string table, u32 its size
//! 0x24  4 × f32  bounding sphere: centre, radius
//! 0x34  f32  height (`TreeHeight` of the wind)
//! 0x38  parts:
//!       u32 kind (0, 1, 2), u32 mesh count, u32 material (CRC-32 of the
//!       lower-case name), u32 name (offset in the string table);
//!       meshes: u32 vertex count, u32 index count, u32 quad list (kinds 1
//!       and 2: 1 quads, 0 triangles), vertices, kind 0: u16 strip indices
//!       and padding to 4 bytes (its bytes are not always zero)
//! strings: the source path (`.dae`), then the part names, NUL-terminated
//! ```
//!
//! Vertices of kinds 0 and 2 (24 bytes): f32×3 position, f32×2 UV, u8×4
//! normal (`b / 255 · 2 − 1`, w 255). Kind 1 (32 bytes): f32×3 leaf centre,
//! f32×2 UV, f32 size, f32 corner index, f32 a byte value / 255 the vertex
//! shaders do not read.

use crate::io::{self, Endian};
use crate::{Error, Result};

/// Type id of `treemodel`.
pub const TYPE_TREE: u32 = crate::hash::sdbm(b"treemodel");
/// "Tree" read as a little-endian u32.
pub const MAGIC: u32 = 0x5472_6565;
pub const VERSION: u32 = 2;
/// Corners the game computes for rotating leaves (`LeafQuads`): ten leaf
/// orientations of four corners.
pub const LEAF_CORNERS: usize = 40;

const HEADER_SIZE: usize = 0x38;
const WHAT: &str = "treemodel";

/// How a part is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
pub enum PartKind {
    /// Triangle strips (`TreeTrunk`, also some leaf meshes).
    Trunk = 0,
    /// Leaf cards facing the camera (`TreeRotatingLeaf`).
    RotatingLeaves = 1,
    /// Fixed leaf cards and fronds (`TreeNonRotatingLeaf`).
    Leaves = 2,
}

impl PartKind {
    fn from_u32(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Trunk),
            1 => Some(Self::RotatingLeaves),
            2 => Some(Self::Leaves),
            _ => None,
        }
    }

    /// Bytes of one vertex.
    pub fn stride(self) -> usize {
        match self {
            Self::RotatingLeaves => 32,
            _ => 24,
        }
    }
}

/// A vertex of kinds 0 and 2.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TreeVertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    /// x, y, z as `b / 255 · 2 − 1`, w (255).
    pub normal: [u8; 4],
}

impl TreeVertex {
    pub fn normal(&self) -> [f32; 3] {
        [0, 1, 2].map(|i| self.normal[i] as f32 / 255.0 * 2.0 - 1.0)
    }
}

/// A corner of a rotating leaf.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LeafVertex {
    /// The leaf centre.
    pub position: [f32; 3],
    pub uv: [f32; 2],
    /// Half the side of the leaf card.
    pub size: f32,
    /// Index into the game's 40 corners ([`leaf_corner`]).
    pub corner: f32,
    /// A byte value / 255 the vertex shaders do not read.
    pub shade: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Vertices {
    Mesh(Vec<TreeVertex>),
    Leaves(Vec<LeafVertex>),
}

impl Vertices {
    pub fn len(&self) -> usize {
        match self {
            Self::Mesh(v) => v.len(),
            Self::Leaves(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn position(&self, i: usize) -> [f32; 3] {
        match self {
            Self::Mesh(v) => v[i].position,
            Self::Leaves(v) => v[i].position,
        }
    }
}

/// One drawn mesh of a part; the game draws every mesh of every part.
#[derive(Debug, Clone, PartialEq)]
pub struct TreeMesh {
    /// Kinds 1 and 2: the vertices are a quad list (1) or a triangle list (0).
    pub quads: u32,
    pub vertices: Vertices,
    /// Strip indices (kind 0 only).
    pub indices: Vec<u16>,
    /// Bytes after the indices up to a multiple of 4.
    pub padding: Vec<u8>,
}

impl TreeMesh {
    /// Triangles of the mesh as a list: strips without their degenerate
    /// triangles (odd ones turned round), quads as two triangles.
    pub fn triangles(&self, kind: PartKind) -> Vec<[u32; 3]> {
        let n = self.vertices.len() as u32;
        let mut out = Vec::new();
        match kind {
            PartKind::Trunk => {
                for (k, w) in self.indices.windows(3).enumerate() {
                    let [a, b, c] = [w[0] as u32, w[1] as u32, w[2] as u32];
                    if a == b || b == c || a == c || a.max(b).max(c) >= n {
                        continue;
                    }
                    out.push(if k % 2 == 0 { [a, b, c] } else { [b, a, c] });
                }
            }
            _ if self.quads != 0 => {
                for q in (0..n / 4).map(|q| q * 4) {
                    out.push([q, q + 1, q + 2]);
                    out.push([q, q + 2, q + 3]);
                }
            }
            _ => {
                for t in (0..n / 3).map(|t| t * 3) {
                    out.push([t, t + 1, t + 2]);
                }
            }
        }
        out
    }
}

/// A part: meshes drawn with one material and shader.
#[derive(Debug, Clone, PartialEq)]
pub struct TreePart {
    pub kind: PartKind,
    /// CRC-32 of the lower-case material name.
    pub material_hash: u32,
    /// Offset of the name in the string table.
    pub name_offset: u32,
    pub meshes: Vec<TreeMesh>,
}

/// A parsed tree model.
#[derive(Debug, Clone, PartialEq)]
pub struct TreeModel {
    /// CRC-32 of the source path.
    pub path_hash: u32,
    /// The two words at 0x14, zero in the game.
    pub reserved: [u32; 2],
    /// Centre and radius.
    pub sphere: [f32; 4],
    /// Height the wind bends the tree over (`TreeHeight`).
    pub height: f32,
    pub parts: Vec<TreePart>,
    /// The string table as stored.
    pub strings: Vec<u8>,
    /// Bytes after the string table (zeros up to the resource's alignment).
    pub tail: Vec<u8>,
}

impl TreeModel {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let le = Endian::Little;
        let u32_at = |o: usize| io::read_u32(data, o, le);
        let f32_at = |o: usize| io::read_u32(data, o, le).map(f32::from_bits);
        let version = u32_at(0)?;
        let magic = u32_at(4)?;
        if magic != MAGIC {
            return Err(Error::BadMagic {
                what: WHAT,
                found: magic,
            });
        }
        if version != VERSION {
            return Err(Error::UnsupportedVersion {
                what: WHAT,
                version,
            });
        }
        let mesh_count = u32_at(8)? as usize;
        let part_count = u32_at(0xC)? as usize;
        let strings_at = u32_at(0x1C)? as usize;
        let strings_size = u32_at(0x20)? as usize;
        let strings = io::slice(data, strings_at, strings_size)?.to_vec();
        let mut at = HEADER_SIZE;
        let mut parts = Vec::with_capacity(part_count.min(64));
        for _ in 0..part_count {
            let kind = u32_at(at)?;
            let kind = PartKind::from_u32(kind)
                .ok_or_else(|| Error::malformed(WHAT, at, format!("part kind {kind}")))?;
            let meshes = u32_at(at + 4)? as usize;
            let material_hash = u32_at(at + 8)?;
            let name_offset = u32_at(at + 12)?;
            if name_offset as usize >= strings.len() {
                return Err(Error::malformed(WHAT, at + 12, "name outside the strings"));
            }
            at += 16;
            let mut part = TreePart {
                kind,
                material_hash,
                name_offset,
                meshes: Vec::with_capacity(meshes.min(8)),
            };
            for _ in 0..meshes {
                let vertex_count = u32_at(at)? as usize;
                let index_count = u32_at(at + 4)? as usize;
                let quads = u32_at(at + 8)?;
                if kind != PartKind::Trunk && index_count != 0 {
                    return Err(Error::malformed(WHAT, at + 4, "indices of leaves"));
                }
                at += 12;
                let stride = kind.stride();
                let bytes = io::slice(data, at, vertex_count.saturating_mul(stride))?;
                let vertices = match kind {
                    PartKind::RotatingLeaves => Vertices::Leaves(
                        bytes
                            .chunks_exact(stride)
                            .map(|v| {
                                let f = |i: usize| {
                                    f32::from_le_bytes(v[i * 4..i * 4 + 4].try_into().unwrap())
                                };
                                LeafVertex {
                                    position: [f(0), f(1), f(2)],
                                    uv: [f(3), f(4)],
                                    size: f(5),
                                    corner: f(6),
                                    shade: f(7),
                                }
                            })
                            .collect(),
                    ),
                    _ => Vertices::Mesh(
                        bytes
                            .chunks_exact(stride)
                            .map(|v| {
                                let f = |i: usize| {
                                    f32::from_le_bytes(v[i * 4..i * 4 + 4].try_into().unwrap())
                                };
                                TreeVertex {
                                    position: [f(0), f(1), f(2)],
                                    uv: [f(3), f(4)],
                                    normal: v[20..24].try_into().unwrap(),
                                }
                            })
                            .collect(),
                    ),
                };
                at += bytes.len();
                let indices: Vec<u16> = io::slice(data, at, index_count * 2)?
                    .chunks_exact(2)
                    .map(|b| u16::from_le_bytes([b[0], b[1]]))
                    .collect();
                at += index_count * 2;
                let end = (at + 3) & !3;
                let padding = io::slice(data, at, end - at)?.to_vec();
                at = end;
                part.meshes.push(TreeMesh {
                    quads,
                    vertices,
                    indices,
                    padding,
                });
            }
            parts.push(part);
        }
        if at != strings_at {
            return Err(Error::malformed(
                WHAT,
                at,
                format!("parts end before the strings at {strings_at:#x}"),
            ));
        }
        let model = Self {
            path_hash: u32_at(0x10)?,
            reserved: [u32_at(0x14)?, u32_at(0x18)?],
            sphere: [f32_at(0x24)?, f32_at(0x28)?, f32_at(0x2C)?, f32_at(0x30)?],
            height: f32_at(0x34)?,
            parts,
            strings,
            tail: data[strings_at + strings_size..].to_vec(),
        };
        if model.mesh_count() != mesh_count {
            return Err(Error::malformed(WHAT, 8, "mesh count"));
        }
        Ok(model)
    }

    /// The resource as stored.
    pub fn to_bytes(&self) -> Vec<u8> {
        let le = Endian::Little;
        let mut out = Vec::new();
        for v in [
            VERSION,
            MAGIC,
            self.mesh_count() as u32,
            self.parts.len() as u32,
        ] {
            io::put_u32(&mut out, v, le);
        }
        io::put_u32(&mut out, self.path_hash, le);
        io::put_u32(&mut out, self.reserved[0], le);
        io::put_u32(&mut out, self.reserved[1], le);
        let strings_at = out.len();
        io::put_u32(&mut out, 0, le);
        io::put_u32(&mut out, self.strings.len() as u32, le);
        for v in self.sphere.iter().chain([&self.height]) {
            io::put_u32(&mut out, v.to_bits(), le);
        }
        let f = |out: &mut Vec<u8>, v: f32| out.extend_from_slice(&v.to_le_bytes());
        for part in &self.parts {
            for v in [
                part.kind as u32,
                part.meshes.len() as u32,
                part.material_hash,
                part.name_offset,
            ] {
                io::put_u32(&mut out, v, le);
            }
            for mesh in &part.meshes {
                io::put_u32(&mut out, mesh.vertices.len() as u32, le);
                io::put_u32(&mut out, mesh.indices.len() as u32, le);
                io::put_u32(&mut out, mesh.quads, le);
                match &mesh.vertices {
                    Vertices::Mesh(vertices) => {
                        for v in vertices {
                            for x in v.position.iter().chain(&v.uv) {
                                f(&mut out, *x);
                            }
                            out.extend_from_slice(&v.normal);
                        }
                    }
                    Vertices::Leaves(vertices) => {
                        for v in vertices {
                            for x in v.position.iter().chain(&v.uv) {
                                f(&mut out, *x);
                            }
                            for x in [v.size, v.corner, v.shade] {
                                f(&mut out, x);
                            }
                        }
                    }
                }
                for i in &mesh.indices {
                    io::put_u16(&mut out, *i, le);
                }
                let end = (out.len() + 3) & !3;
                let mut padding = mesh.padding.clone();
                padding.resize(end - out.len(), 0);
                out.extend_from_slice(&padding);
            }
        }
        let at = out.len() as u32;
        out[strings_at..strings_at + 4].copy_from_slice(&at.to_le_bytes());
        out.extend_from_slice(&self.strings);
        out.extend_from_slice(&self.tail);
        out
    }

    /// Meshes of all parts.
    pub fn mesh_count(&self) -> usize {
        self.parts.iter().map(|p| p.meshes.len()).sum()
    }

    /// The string at `offset` of the string table.
    pub fn string(&self, offset: u32) -> &str {
        let rest = self.strings.get(offset as usize..).unwrap_or_default();
        let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
        std::str::from_utf8(&rest[..end]).unwrap_or("")
    }

    /// The path of the file the tree was exported from (`….dae`).
    pub fn source_path(&self) -> &str {
        self.string(0)
    }

    pub fn part_name(&self, part: &TreePart) -> &str {
        self.string(part.name_offset)
    }

    /// Box of every vertex, with leaf cards grown by their size; `None`
    /// for a tree without vertices.
    pub fn bounds(&self) -> Option<([f32; 3], [f32; 3])> {
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        let mut add = |p: [f32; 3], r: f32| {
            for i in 0..3 {
                min[i] = min[i].min(p[i] - r);
                max[i] = max[i].max(p[i] + r);
            }
        };
        for mesh in self.parts.iter().flat_map(|p| &p.meshes) {
            match &mesh.vertices {
                Vertices::Mesh(v) => v.iter().for_each(|v| add(v.position, 0.0)),
                Vertices::Leaves(v) => v
                    .iter()
                    .for_each(|v| add(v.position, v.size.abs() * std::f32::consts::SQRT_2)),
            }
        }
        (min[0] <= max[0]).then_some((min, max))
    }
}

/// A corner of a rotating leaf as the game places it with the wind at rest:
/// the offset from the leaf centre for a size of 1 and the leaf's normal,
/// both in camera space (x right, y up, z forward).
///
/// The game (`godfather2.exe` 0x9D0D20) fills `LeafQuads` every frame:
/// leaf orientation `v = 5i + j` (i < 2, j < 5) tilts the card by
/// `t = (i − ½) · 20°` about the camera's x axis and turns it by
/// `t + 0.075 · cos(π (j/4 − ½) + 5 · time)` in the view plane; the
/// corners are (−1, 1), (−1, −1), (1, −1), (1, 1), the normal (0, 1, 0).
pub fn leaf_corner(corner: f32) -> ([f32; 3], [f32; 3]) {
    const CORNERS: [[f32; 2]; 4] = [[-1.0, 1.0], [-1.0, -1.0], [1.0, -1.0], [1.0, 1.0]];
    let index = (corner.max(0.0) as usize).min(LEAF_CORNERS - 1);
    let (variant, c) = (index / 4, index % 4);
    let (i, j) = (variant / 5, variant % 5);
    let tilt = 20f32.to_radians() * (i as f32 - 0.5);
    let roll = tilt + 0.075 * (std::f32::consts::PI * (j as f32 * 0.25 - 0.5)).cos();
    let (sr, cr) = roll.sin_cos();
    let (st, ct) = tilt.sin_cos();
    let [cx, cy] = CORNERS[c];
    let (x, y) = (cx * cr - cy * sr, cx * sr + cy * cr);
    ([x, y * ct, y * st], [-sr, cr * ct, cr * st])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tree with a strip trunk, rotating leaves and a quad frond.
    fn sample() -> Vec<u8> {
        let mut d = Vec::new();
        let u = |d: &mut Vec<u8>, v: u32| d.extend_from_slice(&v.to_le_bytes());
        let f = |d: &mut Vec<u8>, v: f32| d.extend_from_slice(&v.to_le_bytes());
        for v in [2, MAGIC, 3, 3, 0x1234, 0, 0, 0, 0] {
            u(&mut d, v);
        }
        for v in [0.0, 1.0, 0.0, 2.0, 2.0] {
            f(&mut d, v);
        }
        // Trunk: 3 vertices, a strip of 5 indices (with padding 0xAB 0xCD).
        // Name 12: "trunk".
        for v in [0, 1, 0xAA, 12] {
            u(&mut d, v);
        }
        for v in [3, 5, 0] {
            u(&mut d, v);
        }
        for i in 0..3 {
            for v in [i as f32, 0.0, 0.0, 0.5, 0.5] {
                f(&mut d, v);
            }
            d.extend_from_slice(&[127, 255, 127, 255]);
        }
        for i in [0u16, 1, 2, 2, 1] {
            d.extend_from_slice(&i.to_le_bytes());
        }
        d.extend_from_slice(&[0xAB, 0xCD]);
        // Rotating leaves: one card.
        for v in [1, 1, 0xBB, 6] {
            u(&mut d, v);
        }
        for v in [4, 0, 1] {
            u(&mut d, v);
        }
        for c in 0..4 {
            for v in [0.0, 2.0, 0.0, 0.0, 0.0, 0.5, c as f32, 0.1] {
                f(&mut d, v);
            }
        }
        // Fixed leaves: a quad.
        for v in [2, 1, 0xCC, 6] {
            u(&mut d, v);
        }
        for v in [4, 0, 1] {
            u(&mut d, v);
        }
        for c in 0..4 {
            for v in [c as f32, 1.0, 0.0, 0.0, 0.0] {
                f(&mut d, v);
            }
            d.extend_from_slice(&[127, 127, 255, 255]);
        }
        let at = d.len() as u32;
        d[0x1C..0x20].copy_from_slice(&at.to_le_bytes());
        let strings = b"x:\\tree.dae\0trunk\0";
        d[0x20..0x24].copy_from_slice(&(strings.len() as u32).to_le_bytes());
        d.extend_from_slice(strings);
        d.extend_from_slice(&[0, 0]);
        d
    }

    #[test]
    fn parses_and_writes_back_byte_for_byte() {
        let data = sample();
        let tree = TreeModel::parse(&data).unwrap();
        assert_eq!(tree.parts.len(), 3);
        assert_eq!(tree.mesh_count(), 3);
        assert_eq!(tree.source_path(), "x:\\tree.dae");
        assert_eq!(tree.part_name(&tree.parts[0]), "trunk");
        assert_eq!(tree.parts[0].meshes[0].padding, [0xAB, 0xCD]);
        assert_eq!(tree.to_bytes(), data);
        // Strip 0 1 2 2 1: one triangle, the degenerate ones dropped.
        let trunk = &tree.parts[0];
        assert_eq!(trunk.meshes[0].triangles(trunk.kind), vec![[0, 1, 2]]);
        let frond = &tree.parts[2];
        assert_eq!(frond.meshes[0].triangles(frond.kind).len(), 2);
        let (min, max) = tree.bounds().unwrap();
        assert!(
            min[1] < 2.0 - 0.5 && max[1] > 2.0 + 0.5,
            "leaves grow the box"
        );
    }

    #[test]
    fn rejects_other_data() {
        let data = sample();
        let mut bad = data.clone();
        bad[4] = 0;
        assert!(TreeModel::parse(&bad).is_err());
        let mut bad = data.clone();
        bad[0x38] = 7;
        assert!(TreeModel::parse(&bad).is_err());
        assert!(TreeModel::parse(&data[..0x50]).is_err());
    }

    #[test]
    fn leaf_corners_are_the_card_corners_turned_a_little() {
        for index in 0..LEAF_CORNERS {
            let (offset, normal) = leaf_corner(index as f32);
            let len = offset.iter().map(|v| v * v).sum::<f32>().sqrt();
            assert!((len - std::f32::consts::SQRT_2).abs() < 1e-4);
            let n = normal.iter().map(|v| v * v).sum::<f32>().sqrt();
            assert!((n - 1.0).abs() < 1e-4 && normal[1] > 0.9);
        }
        // Corner 0 is the top left one.
        let (o, _) = leaf_corner(0.0);
        assert!(o[0] < -0.5 && o[1] > 0.5);
        let (o, _) = leaf_corner(2.0);
        assert!(o[0] > 0.5 && o[1] < -0.5);
    }
}

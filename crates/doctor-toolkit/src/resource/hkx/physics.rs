//! Rigid bodies, collision shapes and EA materials of a packfile as typed data.
//!
//! Layouts (32-bit, offsets in bytes; every object starts with a vtable
//! pointer and `u16 memSizeAndFlags, u16 referenceCount`, all zero in files;
//! `hkArray` is `{ptr, i32 size, u32 capacity | 0xC0000000}`):
//!
//! | Class | Size | Fields |
//! |---|---|---|
//! | `hkRootLevelContainer` | 0x10 | 0x00 array of `{char* name, char* class, ptr object, ptr hkClass}` |
//! | `hkpPhysicsData` | 0x20 | 0x08 world info (null), 0x0C array of systems |
//! | `hkpPhysicsSystem` | 0x50 | 0x08 rigid bodies, 0x14 constraints, 0x20 actions, 0x2C phantoms, 0x38 name, 0x40 active |
//! | `hkpRigidBody` | 0x200 | 0x10 shape, 0x58 allowed penetration, 0x70 name, 0x74 properties `{u32 key, u32, u64 value}`, 0x84 friction, 0x88 restitution, 0xD8 motion type, 0xE0 rotation (3 columns), 0x110 translation, 0x180 object radius |
//! | `hkpMoppBvTreeShape` | 0x40 | 0x10 code, 0x34 child |
//! | `hkpMoppCode` | 0x30 | 0x10 offset xyz and scale, 0x20 code bytes, 0x2C build type |
//! | `hkpSimpleMeshShape` | 0x50 | 0x18 vertices (`hkVector4`), 0x24 triangles `{a, b, c, welding}`, 0x30 material indices, 0x3C radius, 0x40 welding type |
//! | `hkpStorageExtendedMeshShape` | 0xE0 | 0x18 subpart index bits, 0x20 scaling, 0x30 AABB half extents, 0x40 centre, 0x50 triangle subparts (ptr, count), 0x58 shape subparts, 0x60 welding info, 0x6C welding type, 0x70 copy of the first triangle subpart, 0xC0 mesh storage, 0xCC shape storage |
//! | `…MeshSubpartStorage` | 0x50 | 0x08 vertices, 0x14 16-bit indices, 0x20 32-bit indices (4 per triangle), 0x2C, 0x38, 0x44 material arrays |
//! | `…ShapeSubpartStorage` | 0x40 | 0x08 shapes |
//! | `hkpBoxShape` | 0x30 | 0x10 radius, 0x20 half extents |
//! | `hkpSphereShape` | 0x20 | 0x10 radius |
//! | `hkpCapsuleShape` | 0x40 | 0x10 radius, 0x20 and 0x30 segment ends |
//! | `hkpCylinderShape` | 0x60 | 0x10 radius, 0x14 cylinder radius, 0x20 and 0x30 axis ends, 0x40 and 0x50 perpendiculars |
//! | `hkpConvexVerticesShape` | 0x60 | 0x10 radius, 0x20 AABB half extents, 0x30 centre, 0x40 vertices in groups of four (`x×4, y×4, z×4`), 0x4C vertex count, 0x50 plane equations |
//! | `hkpConvexTranslateShape` | 0x30 | 0x10 radius, 0x18 child, 0x20 translation |
//! | `hkpConvexTransformShape` | 0x60 | 0x10 radius, 0x18 child, 0x20 rotation columns, 0x50 translation |
//! | `hkpListShape` | 0x70 | 0x18 children `{ptr shape, u32 filter, …}`, 0x30 AABB half extents, 0x40 centre, 0x50 enabled-children bits |
//! | `EAMaterialArray` | 0x10 | 0x00 array of materials |
//! | `EAMaterial` | 0x20 | eight words, meaning unknown |
//!
//! The rigid body property `0x2F8D528A` is the body's material: a 1-based
//! index into the `EAMaterialArray` (0 in a few files).
//!
//! Objects keep the bytes they were read from: writing them back overlays the
//! typed fields on those bytes, so unchanged packfiles come back byte for byte
//! (fields not modelled here, such as runtime caches, are kept). New objects
//! start from zeros.

use serde::Serialize;

use super::graph::{Graph, Node};
use super::mopp::MoppCode;
use super::{END, Packfile, SectionBody, build_class_names};
use crate::{Error, Result};

/// Rigid body property holding the 1-based index of the body's EA material.
pub const PROPERTY_MATERIAL: u32 = 0x2F8D_528A;

/// Motion type of fixed (static) bodies.
pub const MOTION_FIXED: u8 = 7;

/// `hkpWeldingUtility::WELDING_TYPE_NONE`.
pub const WELDING_NONE: u8 = 6;

type Vec3 = [f32; 3];
type Vec4 = [f32; 4];

/// The physics content of a packfile.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Physics {
    pub systems: Vec<System>,
    /// The `EAMaterialArray`, `None` when the packfile has none. A few
    /// arrays end with an empty slot (`None`), used by no body.
    pub materials: Option<Vec<Option<EaMaterial>>>,
    /// Names of root-level variants other than physics data and materials
    /// (one packfile also carries an `hkxScene`). They are not written back.
    pub other_contents: Vec<String>,
    #[serde(skip)]
    raw: Raw,
}

#[derive(Debug, Clone, PartialEq, Default)]
struct Raw {
    root: Vec<u8>,
    data: Vec<u8>,
    materials: Vec<u8>,
    /// Names and class names of the two variants.
    names: [(String, String); 2],
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct System {
    pub name: Option<String>,
    pub bodies: Vec<RigidBody>,
    pub active: bool,
    #[serde(skip)]
    raw: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Property {
    pub key: u32,
    pub value: u64,
    /// The padding word as stored.
    pub padding: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RigidBody {
    pub name: Option<String>,
    pub shape: Shape,
    pub properties: Vec<Property>,
    /// 7 fixed, 4 box inertia, 2 sphere inertia, 6 keyframed.
    pub motion_type: u8,
    /// Rotation columns.
    pub rotation: [Vec3; 3],
    pub translation: Vec3,
    pub friction: f32,
    pub restitution: f32,
    /// Radius of the body around its centre of mass.
    pub object_radius: f32,
    #[serde(skip)]
    raw: Vec<u8>,
}

/// An `EAMaterial`: eight words whose meaning is unknown (the first is a
/// small surface number, 1–30).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct EaMaterial {
    pub words: [u32; 8],
}

/// A collision shape.
#[derive(Debug, Clone, Serialize)]
pub struct Shape {
    #[serde(flatten)]
    pub kind: ShapeKind,
    #[serde(skip)]
    raw: Vec<u8>,
    /// The object this shape was read from: a child shared by several
    /// parents is written once again.
    #[serde(skip)]
    origin: Option<usize>,
}

impl PartialEq for Shape {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
    }
}

impl From<ShapeKind> for Shape {
    fn from(kind: ShapeKind) -> Self {
        Self {
            kind,
            raw: Vec::new(),
            origin: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ShapeKind {
    Sphere {
        radius: f32,
    },
    Box {
        radius: f32,
        half_extents: Vec4,
    },
    Capsule {
        radius: f32,
        a: Vec4,
        b: Vec4,
    },
    Cylinder {
        radius: f32,
        cylinder_radius: f32,
        a: Vec4,
        b: Vec4,
    },
    ConvexVertices {
        radius: f32,
        aabb_half_extents: Vec4,
        aabb_center: Vec4,
        vertices: Vec<Vec3>,
        planes: Vec<Vec4>,
        /// Values that fill the last group of four vertices.
        #[serde(skip)]
        filler: Vec<Vec3>,
    },
    ConvexTranslate {
        radius: f32,
        translation: Vec4,
        child: Box<Shape>,
    },
    ConvexTransform {
        radius: f32,
        /// Rotation columns and translation, with their `w` as stored.
        transform: [Vec4; 4],
        child: Box<Shape>,
    },
    List {
        children: Vec<Shape>,
        aabb_half_extents: Vec4,
        aabb_center: Vec4,
        /// The 16 bytes of each child record after its shape pointer.
        #[serde(skip)]
        child_info: Vec<[u8; 12]>,
    },
    Mopp {
        code: MoppCode,
        child: Box<Shape>,
    },
    SimpleMesh {
        radius: f32,
        welding_type: u8,
        /// `xyz` and the `w` as stored (0 or 1).
        vertices: Vec<Vec4>,
        triangles: Vec<[u32; 3]>,
        /// Fourth word of each triangle (welding information).
        welding: Vec<u32>,
        material_indices: Vec<u8>,
    },
    ExtendedMesh(Box<ExtendedMesh>),
}

/// An `hkpStorageExtendedMeshShape`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExtendedMesh {
    pub subpart_index_bits: i32,
    pub scaling: Vec4,
    /// `w` is the triangle radius.
    pub aabb_half_extents: Vec4,
    pub aabb_center: Vec4,
    pub welding_type: u8,
    pub triangle_subparts: Vec<TriangleSubpart>,
    pub shape_subparts: Vec<ShapeSubpart>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TriangleSubpart {
    pub vertices: Vec<Vec4>,
    /// Four 32-bit indices per triangle; the fourth is 0.
    pub indices: Vec<[u32; 4]>,
    pub num_materials: u16,
    /// The 64-byte subpart record (counts and strides; pointers are null).
    #[serde(skip)]
    record: Vec<u8>,
    #[serde(skip)]
    storage: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ShapeSubpart {
    pub shapes: Vec<Shape>,
    pub rotation: Vec4,
    pub translation: Vec4,
    #[serde(skip)]
    record: Vec<u8>,
    #[serde(skip)]
    storage: Vec<u8>,
}

/// Class name, size in a file.
const SIZES: &[(&str, usize)] = &[
    ("hkRootLevelContainer", 0x10),
    ("hkpPhysicsData", 0x20),
    ("hkpPhysicsSystem", 0x50),
    ("hkpRigidBody", 0x200),
    ("hkpMoppBvTreeShape", 0x40),
    ("hkpMoppCode", 0x30),
    ("hkpSimpleMeshShape", 0x50),
    ("hkpStorageExtendedMeshShape", 0xE0),
    ("hkpStorageExtendedMeshShapeMeshSubpartStorage", 0x50),
    ("hkpStorageExtendedMeshShapeShapeSubpartStorage", 0x40),
    ("hkpBoxShape", 0x30),
    ("hkpSphereShape", 0x20),
    ("hkpCapsuleShape", 0x40),
    ("hkpCylinderShape", 0x60),
    ("hkpConvexVerticesShape", 0x60),
    ("hkpConvexTranslateShape", 0x30),
    ("hkpConvexTransformShape", 0x60),
    ("hkpListShape", 0x70),
    ("EAMaterialArray", 0x10),
    ("EAMaterial", 0x20),
];

fn class_size(class: &str) -> usize {
    SIZES
        .iter()
        .find(|(name, _)| *name == class)
        .map_or(0x10, |(_, size)| *size)
}

fn malformed(reason: impl Into<String>) -> Error {
    Error::malformed("hkx", 0, reason)
}

// ---------------------------------------------------------------------------
// Reading

struct Reader<'a> {
    graph: &'a Graph,
}

impl<'a> Reader<'a> {
    fn node(&self, index: usize) -> &'a Node {
        &self.graph.nodes[index]
    }

    fn class(&self, index: usize) -> &'a str {
        self.node(index).class.as_deref().unwrap_or("")
    }

    fn target(&self, node: usize, at: u32) -> Option<usize> {
        self.node(node).pointer(at).map(|p| p.target)
    }

    fn expect_object(&self, node: usize, at: u32, what: &str) -> Result<usize> {
        let target = self
            .target(node, at)
            .ok_or_else(|| malformed(format!("missing {what}")))?;
        if self.node(target).class.is_none() {
            return Err(malformed(format!("{what} is not an object")));
        }
        Ok(target)
    }

    fn vec4(&self, node: usize, at: u32) -> Vec4 {
        let n = self.node(node);
        [n.f32(at), n.f32(at + 4), n.f32(at + 8), n.f32(at + 12)]
    }

    fn vec3(&self, node: usize, at: u32) -> Vec3 {
        let v = self.vec4(node, at);
        [v[0], v[1], v[2]]
    }

    /// The data block of an array and its element count (count 0 gives no block).
    fn array(&self, node: usize, at: u32, element: usize) -> Result<Option<(usize, usize)>> {
        let count = self.node(node).u32(at + 4) as usize;
        self.counted(node, at, count, element)
    }

    fn counted(
        &self,
        node: usize,
        at: u32,
        count: usize,
        element: usize,
    ) -> Result<Option<(usize, usize)>> {
        let Some(pointer) = self.node(node).pointer(at) else {
            return if count == 0 {
                Ok(None)
            } else {
                Err(malformed("array without data"))
            };
        };
        let block = self.node(pointer.target);
        let need = count
            .checked_mul(element)
            .ok_or_else(|| malformed("array too long"))?;
        if pointer.offset != 0 || block.bytes.len() < need {
            return Err(malformed("array data shorter than its count"));
        }
        Ok(Some((pointer.target, count)))
    }

    fn string(&self, node: usize, at: u32) -> Option<String> {
        let target = self.node(node).pointer(at)?;
        let bytes = &self.node(target.target).bytes[target.offset as usize..];
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        Some(String::from_utf8_lossy(&bytes[..end]).into_owned())
    }

    /// Pointers stored in an array of pointers.
    fn object_array(&self, node: usize, at: u32, what: &str) -> Result<Vec<usize>> {
        let Some((block, count)) = self.array(node, at, 4)? else {
            return Ok(Vec::new());
        };
        (0..count)
            .map(|i| self.expect_object(block, 4 * i as u32, what))
            .collect()
    }

    fn vec4_array(&self, node: usize, at: u32) -> Result<Vec<Vec4>> {
        let Some((block, count)) = self.array(node, at, 16)? else {
            return Ok(Vec::new());
        };
        Ok((0..count)
            .map(|i| self.vec4(block, 16 * i as u32))
            .collect())
    }

    /// Checks that an object holds at least the bytes of its class.
    fn sized(&self, index: usize) -> Result<&'a Node> {
        let n = self.node(index);
        if n.bytes.len() < class_size(self.class(index)) {
            return Err(malformed(format!(
                "{} object shorter than its class",
                self.class(index)
            )));
        }
        Ok(n)
    }

    fn shape(&self, index: usize, depth: usize) -> Result<Shape> {
        if depth > 16 {
            return Err(malformed("shapes nested too deep"));
        }
        let n = self.sized(index)?;
        let radius = n.f32(0x10);
        let kind = match self.class(index) {
            "hkpSphereShape" => ShapeKind::Sphere { radius },
            "hkpBoxShape" => ShapeKind::Box {
                radius,
                half_extents: self.vec4(index, 0x20),
            },
            "hkpCapsuleShape" => ShapeKind::Capsule {
                radius,
                a: self.vec4(index, 0x20),
                b: self.vec4(index, 0x30),
            },
            "hkpCylinderShape" => ShapeKind::Cylinder {
                radius,
                cylinder_radius: n.f32(0x14),
                a: self.vec4(index, 0x20),
                b: self.vec4(index, 0x30),
            },
            "hkpConvexVerticesShape" => {
                let count = n.u32(0x4C) as usize;
                let groups = n.u32(0x44) as usize;
                if count > groups * 4 {
                    return Err(malformed("more vertices than vertex groups hold"));
                }
                let mut all = Vec::new();
                if let Some((block, groups)) = self.array(index, 0x40, 48)? {
                    let b = self.node(block);
                    for g in 0..groups as u32 {
                        for k in 0..4 {
                            let at = 48 * g + 4 * k;
                            all.push([b.f32(at), b.f32(at + 16), b.f32(at + 32)]);
                        }
                    }
                }
                let filler = all.split_off(count);
                ShapeKind::ConvexVertices {
                    radius,
                    aabb_half_extents: self.vec4(index, 0x20),
                    aabb_center: self.vec4(index, 0x30),
                    vertices: all,
                    planes: self.vec4_array(index, 0x50)?,
                    filler,
                }
            }
            "hkpConvexTranslateShape" => ShapeKind::ConvexTranslate {
                radius,
                translation: self.vec4(index, 0x20),
                child: Box::new(self.shape(self.expect_object(index, 0x18, "child")?, depth + 1)?),
            },
            "hkpConvexTransformShape" => ShapeKind::ConvexTransform {
                radius,
                transform: [
                    self.vec4(index, 0x20),
                    self.vec4(index, 0x30),
                    self.vec4(index, 0x40),
                    self.vec4(index, 0x50),
                ],
                child: Box::new(self.shape(self.expect_object(index, 0x18, "child")?, depth + 1)?),
            },
            "hkpListShape" => {
                let mut children = Vec::new();
                let mut child_info = Vec::new();
                if let Some((block, count)) = self.array(index, 0x18, 16)? {
                    for i in 0..count as u32 {
                        let child = self.expect_object(block, 16 * i, "list child")?;
                        children.push(self.shape(child, depth + 1)?);
                        let at = (16 * i + 4) as usize;
                        child_info.push(
                            self.node(block).bytes[at..at + 12]
                                .try_into()
                                .expect("12 bytes"),
                        );
                    }
                }
                ShapeKind::List {
                    children,
                    aabb_half_extents: self.vec4(index, 0x30),
                    aabb_center: self.vec4(index, 0x40),
                    child_info,
                }
            }
            "hkpMoppBvTreeShape" => {
                let code = self.expect_object(index, 0x10, "MOPP code")?;
                if self.class(code) != "hkpMoppCode" {
                    return Err(malformed("MOPP code of another class"));
                }
                let c = self.sized(code)?;
                let data = match self.array(code, 0x20, 1)? {
                    Some((block, count)) => self.node(block).bytes[..count].to_vec(),
                    None => Vec::new(),
                };
                ShapeKind::Mopp {
                    code: MoppCode {
                        offset: [c.f32(0x10), c.f32(0x14), c.f32(0x18)],
                        scale: c.f32(0x1C),
                        data,
                        build_type: c.u32(0x2C),
                        raw: c.bytes.clone(),
                    },
                    child: Box::new(
                        self.shape(self.expect_object(index, 0x34, "child")?, depth + 1)?,
                    ),
                }
            }
            "hkpSimpleMeshShape" => {
                let vertices = self.vec4_array(index, 0x18)?;
                let mut triangles = Vec::new();
                let mut welding = Vec::new();
                if let Some((block, count)) = self.array(index, 0x24, 16)? {
                    let b = self.node(block);
                    for i in 0..count as u32 {
                        triangles.push([b.u32(16 * i), b.u32(16 * i + 4), b.u32(16 * i + 8)]);
                        welding.push(b.u32(16 * i + 12));
                    }
                }
                let material_indices = match self.array(index, 0x30, 1)? {
                    Some((block, count)) => self.node(block).bytes[..count].to_vec(),
                    None => Vec::new(),
                };
                ShapeKind::SimpleMesh {
                    radius: n.f32(0x3C),
                    welding_type: n.bytes.get(0x40).copied().unwrap_or(0),
                    vertices,
                    triangles,
                    welding,
                    material_indices,
                }
            }
            "hkpStorageExtendedMeshShape" => {
                ShapeKind::ExtendedMesh(Box::new(self.extended(index, depth)?))
            }
            other => return Err(malformed(format!("unsupported shape class {other}"))),
        };
        Ok(Shape {
            kind,
            raw: n.bytes.clone(),
            origin: Some(index),
        })
    }

    fn extended(&self, index: usize, depth: usize) -> Result<ExtendedMesh> {
        let n = self.sized(index)?;
        let mesh_storage = self.object_array(index, 0xC0, "mesh storage")?;
        let shape_storage = self.object_array(index, 0xCC, "shape storage")?;
        let mut triangle_subparts = Vec::new();
        if let Some((block, count)) = self.counted(index, 0x50, n.u32(0x54) as usize, 64)? {
            if count != mesh_storage.len() {
                return Err(malformed("triangle subparts without storage"));
            }
            for (i, &storage) in mesh_storage.iter().enumerate() {
                let at = 64 * i;
                let record = self.node(block).bytes[at..at + 64].to_vec();
                let vertices = self.vec4_array(storage, 0x08)?;
                let mut indices = Vec::new();
                if let Some((ib, count)) = self.array(storage, 0x20, 4)? {
                    let b = self.node(ib);
                    for t in 0..(count / 4) as u32 {
                        let at = 16 * t;
                        indices.push([b.u32(at), b.u32(at + 4), b.u32(at + 8), b.u32(at + 12)]);
                    }
                }
                let s = self.node(storage);
                if [0x14, 0x2C, 0x38, 0x44].iter().any(|&a| s.u32(a + 4) != 0) {
                    return Err(malformed("mesh storage with 16-bit indices or materials"));
                }
                triangle_subparts.push(TriangleSubpart {
                    vertices,
                    indices,
                    num_materials: u16::from_le_bytes([record[10], record[11]]),
                    record,
                    storage: s.bytes.clone(),
                });
            }
        } else if !mesh_storage.is_empty() {
            return Err(malformed("mesh storage without triangle subparts"));
        }
        let mut shape_subparts = Vec::new();
        if let Some((block, count)) = self.counted(index, 0x58, n.u32(0x5C) as usize, 64)? {
            if count != shape_storage.len() {
                return Err(malformed("shape subparts without storage"));
            }
            for (i, &storage) in shape_storage.iter().enumerate() {
                let at = (64 * i) as u32;
                let shapes_count = self.node(block).u32(at + 0x14) as usize;
                let mut shapes = Vec::new();
                let mut targets = Vec::new();
                if let Some((sb, count)) = self.counted(block, at + 0x10, shapes_count, 4)? {
                    for k in 0..count as u32 {
                        let shape = self.expect_object(sb, 4 * k, "subpart shape")?;
                        targets.push(shape);
                        shapes.push(self.shape(shape, depth + 1)?);
                    }
                }
                if self.object_array(storage, 0x08, "stored shape")? != targets {
                    return Err(malformed("shape storage differs from its subpart"));
                }
                shape_subparts.push(ShapeSubpart {
                    shapes,
                    rotation: self.vec4(block, at + 0x20),
                    translation: self.vec4(block, at + 0x30),
                    record: self.node(block).bytes[at as usize..at as usize + 64].to_vec(),
                    storage: self.node(storage).bytes.clone(),
                });
            }
        } else if !shape_storage.is_empty() {
            return Err(malformed("shape storage without shape subparts"));
        }
        if n.u32(0x64) != 0 {
            return Err(malformed("extended mesh with welding information"));
        }
        Ok(ExtendedMesh {
            subpart_index_bits: n.u32(0x18) as i32,
            scaling: self.vec4(index, 0x20),
            aabb_half_extents: self.vec4(index, 0x30),
            aabb_center: self.vec4(index, 0x40),
            welding_type: n.bytes[0x6C],
            triangle_subparts,
            shape_subparts,
        })
    }

    fn body(&self, index: usize) -> Result<RigidBody> {
        let n = self.sized(index)?;
        let shape = self.shape(self.expect_object(index, 0x10, "body shape")?, 0)?;
        let mut properties = Vec::new();
        if let Some((block, count)) = self.array(index, 0x74, 16)? {
            let b = self.node(block);
            for i in 0..count as u32 {
                let at = 16 * i;
                properties.push(Property {
                    key: b.u32(at),
                    padding: b.u32(at + 4),
                    value: u64::from(b.u32(at + 8)) | (u64::from(b.u32(at + 12)) << 32),
                });
            }
        }
        Ok(RigidBody {
            name: self.string(index, 0x70),
            shape,
            properties,
            motion_type: n.bytes[0xD8],
            rotation: [
                self.vec3(index, 0xE0),
                self.vec3(index, 0xF0),
                self.vec3(index, 0x100),
            ],
            translation: self.vec3(index, 0x110),
            friction: n.f32(0x84),
            restitution: n.f32(0x88),
            object_radius: n.f32(0x180),
            raw: n.bytes.clone(),
        })
    }
}

impl Physics {
    /// Reads the physics content of a packfile.
    pub fn parse(data: &[u8]) -> Result<Self> {
        Self::from_packfile(&Packfile::parse(data)?)
    }

    pub fn from_packfile(packfile: &Packfile) -> Result<Self> {
        let graph = Graph::from_packfile(packfile)?;
        let r = Reader { graph: &graph };
        let root = graph.root;
        if r.class(root) != "hkRootLevelContainer" {
            return Err(malformed("root is not an hkRootLevelContainer"));
        }
        let mut physics = Self {
            systems: Vec::new(),
            materials: None,
            other_contents: Vec::new(),
            raw: Raw {
                root: r.node(root).bytes.clone(),
                ..Raw::default()
            },
        };
        let mut found_data = false;
        if let Some((block, count)) = r.array(root, 0, 16)? {
            for i in 0..count as u32 {
                let at = 16 * i;
                let name = r.string(block, at).unwrap_or_default();
                let class = r.string(block, at + 4).unwrap_or_default();
                let object = r.expect_object(block, at + 8, "root variant")?;
                match r.class(object) {
                    "hkpPhysicsData" if !found_data => {
                        found_data = true;
                        physics.raw.names[0] = (name, class);
                        physics.raw.data = r.node(object).bytes.clone();
                        for system in r.object_array(object, 0x0C, "physics system")? {
                            physics.systems.push(r.system(system)?);
                        }
                    }
                    "EAMaterialArray" if physics.materials.is_none() => {
                        physics.raw.names[1] = (name, class);
                        physics.raw.materials = r.node(object).bytes.clone();
                        let mut materials = Vec::new();
                        if let Some((block, count)) = r.array(object, 0, 4)? {
                            for i in 0..count as u32 {
                                let Some(m) = r.target(block, 4 * i) else {
                                    materials.push(None);
                                    continue;
                                };
                                let node = r.node(m);
                                let mut words = [0u32; 8];
                                for (k, w) in words.iter_mut().enumerate() {
                                    *w = node.u32(4 * k as u32);
                                }
                                materials.push(Some(EaMaterial { words }));
                            }
                        }
                        physics.materials = Some(materials);
                    }
                    other => physics.other_contents.push(other.to_owned()),
                }
            }
        }
        if !found_data {
            return Err(malformed("no hkpPhysicsData"));
        }
        Ok(physics)
    }

    /// All rigid bodies of all systems.
    pub fn bodies(&self) -> impl Iterator<Item = &RigidBody> {
        self.systems.iter().flat_map(|s| s.bodies.iter())
    }
}

impl Reader<'_> {
    fn system(&self, index: usize) -> Result<System> {
        let n = self.sized(index)?;
        for at in [0x14, 0x20, 0x2C] {
            if n.u32(at + 4) != 0 {
                return Err(malformed(
                    "physics system with constraints, actions or phantoms",
                ));
            }
        }
        let bodies = self
            .object_array(index, 0x08, "rigid body")?
            .into_iter()
            .map(|b| {
                if self.class(b) != "hkpRigidBody" {
                    return Err(malformed("system body is not an hkpRigidBody"));
                }
                self.body(b)
            })
            .collect::<Result<_>>()?;
        Ok(System {
            name: self.string(index, 0x38),
            bodies,
            active: n.bytes.get(0x40).is_some_and(|&b| b != 0),
            raw: n.bytes.clone(),
        })
    }
}

// ---------------------------------------------------------------------------
// Writing

/// Class names and signatures declared by packfiles the toolkit writes: every
/// class the game's packfiles with meshes, convex shapes and boxes declare,
/// then the other shape classes (signatures as in the game's files).
pub const CLASS_NAMES: &[(u32, &str)] = &[
    (0x38771F8E, "hkClass"),
    (0xA5240F57, "hkClassMember"),
    (0x8A3609CF, "hkClassEnum"),
    (0xCE6F8A6C, "hkClassEnumItem"),
    (0xAA0ADE1A, "hkpPhantom"),
    (0x4A948B16, "hkAabb"),
    (0xEB2364C1, "hkpEntitySmallArraySerializeOverrideType"),
    (0xC2A461E4, "hkpPhysicsData"),
    (0x0D13F6B4, "hkWorldMemoryAvailableWatchDog"),
    (0x1B58F0EF, "hkpPhysicsSystem"),
    (0x0B4E5770, "hkSweptTransform"),
    (0xE0708A00, "hkpShapeContainer"),
    (0x5879A2C3, "hkpCollidable"),
    (0x940569DC, "hkpBroadPhaseHandle"),
    (0xFA798537, "hkpEntity"),
    (0x9B1A3265, "hkpShapeCollection"),
    (0x0485A264, "hkpMaterial"),
    (0x9040D7BA, "hkpSimpleMeshShape"),
    (0x8B0A2DBF, "hkpKeyframedRigidMotion"),
    (0x4117D60E, "hkMoppBvTreeShapeBase"),
    (0x72EE59F8, "hkpMoppCode"),
    (0xAC084729, "hkAabbUint32"),
    (0xDBBA3C29, "hkpMoppBvTreeShape"),
    (0x3224DE76, "hkpRigidBody"),
    (0xE7ECA7EB, "hkpBvTreeShape"),
    (0xBB7A348B, "hkpModifierConstraintAtom"),
    (0x73AA1D38, "hkpSingleShapeContainer"),
    (0x63E5AE27, "EAMaterialArray"),
    (0x3B1C1113, "hkReferencedObject"),
    (0xD38738C1, "hkpSimpleMeshShapeTriangle"),
    (0x60960336, "hkpCollisionFilter"),
    (0x2033B565, "hkpConstraintInstance"),
    (0x9CE308E9, "hkpProperty"),
    (0xF598A34E, "hkRootLevelContainer"),
    (0x4E89E0F0, "hkpLinkedCollidable"),
    (0x1C3C8820, "hkMultiThreadCheck"),
    (0x3FCD7295, "hkpWorldCinfo"),
    (0xABE8F4F8, "hkpMaxSizeMotion"),
    (0xC75925AA, "hkpPropertyValue"),
    (0xE94D2688, "hkpCdBody"),
    (0xE0708A00, "hkBaseObject"),
    (0x666490A1, "hkpShape"),
    (0xC18F0D87, "hkpConstraintData"),
    (0x97B45527, "hkpCollidableBoundingVolumeData"),
    (0xA539881B, "hkpTypedBroadPhaseHandle"),
    (0x141ABDC9, "hkpMotion"),
    (0x50F6EE9F, "hkpWorldObject"),
    (0xB6966E59, "hkpAction"),
    (0xC6EE876A, "EAMaterial"),
    (0x81D074A4, "hkpConvexListFilter"),
    (0x853A899C, "hkRootLevelContainerNamedVariant"),
    (0x81147F05, "hkpEntitySpuCollisionCallback"),
    (0xD8FDBB08, "hkpMoppCodeCodeInfo"),
    (0x064E7CE4, "hkMotionState"),
    (0xED982D04, "hkpConstraintAtom"),
    (0x63D38E9C, "hkpConvexVerticesConnectivity"),
    (0xE7ECA7EB, "hkpSphereRepShape"),
    (0xF8F74F85, "hkpConvexShape"),
    (0x44FF20D2, "hkpConvexVerticesShape"),
    (0x3D80C5BF, "hkpConvexVerticesShapeFourVectors"),
    (0x3444D2D5, "hkpBoxShape"),
    (0x0795D9FA, "hkpSphereShape"),
    (0xDD0B1FD3, "hkpCapsuleShape"),
    (0x3E463C3A, "hkpCylinderShape"),
    (0x5B8BC234, "hkpConvexTranslateShape"),
    (0x99DA124B, "hkpConvexTransformShape"),
    (0x090CD72E, "hkpListShape"),
    (0x6D1DC26A, "hkpListShapeChildInfo"),
    (0x260C3908, "hkpExtendedMeshShape"),
    (0x7C9435C7, "hkpExtendedMeshShapeSubpart"),
    (0x3375309C, "hkpExtendedMeshShapeTrianglesSubpart"),
    (0xFCE016A3, "hkpExtendedMeshShapeShapesSubpart"),
    (0xC22CA89E, "hkpStorageExtendedMeshShape"),
    (0x3B2F0B51, "hkpStorageExtendedMeshShapeMeshSubpartStorage"),
    (0x502939ED, "hkpStorageExtendedMeshShapeShapeSubpartStorage"),
];

struct Writer {
    graph: Graph,
    /// Nodes of shapes written so far, by the object they were read from.
    shapes: std::collections::HashMap<usize, usize>,
}

fn base(raw: &[u8], class: &str) -> Vec<u8> {
    let size = class_size(class);
    if raw.len() >= size {
        raw.to_vec()
    } else {
        vec![0; size]
    }
}

fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_f32(bytes: &mut [u8], at: usize, value: f32) {
    put_u32(bytes, at, value.to_bits());
}

fn put_vec(bytes: &mut [u8], at: usize, values: &[f32]) {
    for (k, &v) in values.iter().enumerate() {
        put_f32(bytes, at + 4 * k, v);
    }
}

/// `hkArray` capacity word for `count` elements stored in the file.
fn capacity(count: usize) -> u32 {
    0xC000_0000 | count as u32
}

fn put_array_header(bytes: &mut [u8], at: usize, count: usize) {
    put_u32(bytes, at, 0);
    put_u32(bytes, at + 4, count as u32);
    put_u32(bytes, at + 8, capacity(count));
}

fn cstring(text: &str) -> Vec<u8> {
    let mut bytes = text.as_bytes().to_vec();
    bytes.push(0);
    bytes
}

impl Writer {
    fn object(&mut self, class: &str, bytes: Vec<u8>) -> usize {
        self.graph.add(Node::object(class, bytes))
    }

    fn data(&mut self, bytes: Vec<u8>) -> usize {
        self.graph.add(Node::data(bytes))
    }

    /// Adds `bytes` as an array block pointed at from `node`, unless empty.
    fn array(&mut self, node: usize, at: u32, bytes: Vec<u8>) -> Option<usize> {
        if bytes.is_empty() {
            return None;
        }
        let block = self.data(bytes);
        self.graph.link(node, at, block);
        Some(block)
    }

    fn string(&mut self, node: usize, at: u32, text: Option<&str>) {
        if let Some(text) = text {
            let block = self.data(cstring(text));
            self.graph.link(node, at, block);
        }
    }

    fn shape(&mut self, shape: &Shape) -> usize {
        if let Some(node) = shape.origin.and_then(|o| self.shapes.get(&o)) {
            return *node;
        }
        let node = self.new_shape(shape);
        if let Some(origin) = shape.origin {
            self.shapes.insert(origin, node);
        }
        node
    }

    fn new_shape(&mut self, shape: &Shape) -> usize {
        let raw = &shape.raw;
        match &shape.kind {
            ShapeKind::Sphere { radius } => {
                let mut b = base(raw, "hkpSphereShape");
                put_f32(&mut b, 0x10, *radius);
                self.object("hkpSphereShape", b)
            }
            ShapeKind::Box {
                radius,
                half_extents,
            } => {
                let mut b = base(raw, "hkpBoxShape");
                put_f32(&mut b, 0x10, *radius);
                put_vec(&mut b, 0x20, half_extents);
                self.object("hkpBoxShape", b)
            }
            ShapeKind::Capsule { radius, a, b: end } => {
                let mut b = base(raw, "hkpCapsuleShape");
                put_f32(&mut b, 0x10, *radius);
                put_vec(&mut b, 0x20, a);
                put_vec(&mut b, 0x30, end);
                self.object("hkpCapsuleShape", b)
            }
            ShapeKind::Cylinder {
                radius,
                cylinder_radius,
                a,
                b: end,
            } => {
                let mut b = base(raw, "hkpCylinderShape");
                if raw.is_empty() {
                    put_f32(&mut b, 0x18, 0.8);
                }
                put_f32(&mut b, 0x10, *radius);
                put_f32(&mut b, 0x14, *cylinder_radius);
                put_vec(&mut b, 0x20, a);
                put_vec(&mut b, 0x30, end);
                if raw.is_empty() {
                    let axis = sub3(&[end[0], end[1], end[2]], &[a[0], a[1], a[2]]);
                    let (p1, p2) = perpendiculars(axis);
                    put_vec(&mut b, 0x40, &[p1[0], p1[1], p1[2], 0.0]);
                    put_vec(&mut b, 0x50, &[p2[0], p2[1], p2[2], 0.0]);
                }
                self.object("hkpCylinderShape", b)
            }
            ShapeKind::ConvexVertices {
                radius,
                aabb_half_extents,
                aabb_center,
                vertices,
                planes,
                filler,
            } => {
                let mut b = base(raw, "hkpConvexVerticesShape");
                put_f32(&mut b, 0x10, *radius);
                put_vec(&mut b, 0x20, aabb_half_extents);
                put_vec(&mut b, 0x30, aabb_center);
                let groups = vertices.len().div_ceil(4);
                put_array_header(&mut b, 0x40, groups);
                put_u32(&mut b, 0x4C, vertices.len() as u32);
                put_array_header(&mut b, 0x50, planes.len());
                let node = self.object("hkpConvexVerticesShape", b);
                let mut all = vertices.clone();
                let last = vertices.last().copied().unwrap_or_default();
                for k in 0..groups * 4 - vertices.len() {
                    all.push(filler.get(k).copied().unwrap_or(last));
                }
                let mut data = Vec::with_capacity(48 * groups);
                for group in all.chunks(4) {
                    for axis in 0..3 {
                        for v in group {
                            data.extend_from_slice(&v[axis].to_le_bytes());
                        }
                    }
                }
                self.array(node, 0x40, data);
                self.array(
                    node,
                    0x50,
                    planes
                        .iter()
                        .flatten()
                        .flat_map(|v| v.to_le_bytes())
                        .collect(),
                );
                node
            }
            ShapeKind::ConvexTranslate {
                radius,
                translation,
                child,
            } => {
                let mut b = base(raw, "hkpConvexTranslateShape");
                put_f32(&mut b, 0x10, *radius);
                put_u32(&mut b, 0x18, 0);
                put_vec(&mut b, 0x20, translation);
                let node = self.object("hkpConvexTranslateShape", b);
                let child = self.shape(child);
                self.graph.link(node, 0x18, child);
                node
            }
            ShapeKind::ConvexTransform {
                radius,
                transform,
                child,
            } => {
                let mut b = base(raw, "hkpConvexTransformShape");
                put_f32(&mut b, 0x10, *radius);
                put_u32(&mut b, 0x18, 0);
                for (k, column) in transform.iter().enumerate() {
                    put_vec(&mut b, 0x20 + 16 * k, column);
                }
                let node = self.object("hkpConvexTransformShape", b);
                let child = self.shape(child);
                self.graph.link(node, 0x18, child);
                node
            }
            ShapeKind::List {
                children,
                aabb_half_extents,
                aabb_center,
                child_info,
            } => {
                let mut b = base(raw, "hkpListShape");
                if raw.is_empty() {
                    b[0x50..0x70].fill(0xFF);
                }
                put_array_header(&mut b, 0x18, children.len());
                put_vec(&mut b, 0x30, aabb_half_extents);
                put_vec(&mut b, 0x40, aabb_center);
                let node = self.object("hkpListShape", b);
                let mut data = vec![0u8; 16 * children.len()];
                for (i, info) in child_info.iter().take(children.len()).enumerate() {
                    data[16 * i + 4..16 * i + 16].copy_from_slice(info);
                }
                let block = self.array(node, 0x18, data);
                for (i, child) in children.iter().enumerate() {
                    let c = self.shape(child);
                    if let Some(block) = block {
                        self.graph.link(block, 16 * i as u32, c);
                    }
                }
                node
            }
            ShapeKind::Mopp { code, child } => {
                let mut b = base(raw, "hkpMoppBvTreeShape");
                put_u32(&mut b, 0x10, 0);
                put_u32(&mut b, 0x34, 0);
                let node = self.object("hkpMoppBvTreeShape", b);
                let mut c = base(&code.raw, "hkpMoppCode");
                put_vec(
                    &mut c,
                    0x10,
                    &[code.offset[0], code.offset[1], code.offset[2], code.scale],
                );
                put_array_header(&mut c, 0x20, code.data.len());
                put_u32(&mut c, 0x2C, code.build_type);
                let code_node = self.object("hkpMoppCode", c);
                self.array(code_node, 0x20, code.data.clone());
                self.graph.link(node, 0x10, code_node);
                let child = self.shape(child);
                self.graph.link(node, 0x34, child);
                node
            }
            ShapeKind::SimpleMesh {
                radius,
                welding_type,
                vertices,
                triangles,
                welding,
                material_indices,
            } => {
                let mut b = base(raw, "hkpSimpleMeshShape");
                if raw.is_empty() {
                    b[0x14] = 1; // welding disabled, as in every game file
                }
                put_array_header(&mut b, 0x18, vertices.len());
                put_array_header(&mut b, 0x24, triangles.len());
                put_array_header(&mut b, 0x30, material_indices.len());
                put_f32(&mut b, 0x3C, *radius);
                b[0x40] = *welding_type;
                let node = self.object("hkpSimpleMeshShape", b);
                self.array(
                    node,
                    0x18,
                    vertices
                        .iter()
                        .flatten()
                        .flat_map(|v| v.to_le_bytes())
                        .collect(),
                );
                let mut data = Vec::with_capacity(16 * triangles.len());
                for (i, t) in triangles.iter().enumerate() {
                    for v in t
                        .iter()
                        .chain(std::iter::once(welding.get(i).unwrap_or(&0)))
                    {
                        data.extend_from_slice(&v.to_le_bytes());
                    }
                }
                self.array(node, 0x24, data);
                self.array(node, 0x30, material_indices.clone());
                node
            }
            ShapeKind::ExtendedMesh(mesh) => self.extended(mesh, raw),
        }
    }

    fn extended(&mut self, mesh: &ExtendedMesh, raw: &[u8]) -> usize {
        let class = "hkpStorageExtendedMeshShape";
        let mut b = base(raw, class);
        put_u32(&mut b, 0x18, mesh.subpart_index_bits as u32);
        put_vec(&mut b, 0x20, &mesh.scaling);
        put_vec(&mut b, 0x30, &mesh.aabb_half_extents);
        put_vec(&mut b, 0x40, &mesh.aabb_center);
        put_u32(&mut b, 0x50, 0);
        put_u32(&mut b, 0x54, mesh.triangle_subparts.len() as u32);
        put_u32(&mut b, 0x58, 0);
        put_u32(&mut b, 0x5C, mesh.shape_subparts.len() as u32);
        put_array_header(&mut b, 0x60, 0);
        b[0x6C] = mesh.welding_type;
        put_array_header(&mut b, 0xC0, mesh.triangle_subparts.len());
        put_array_header(&mut b, 0xCC, mesh.shape_subparts.len());
        let node = self.object(class, b);
        // Triangle subpart records.
        let records: Vec<u8> = mesh
            .triangle_subparts
            .iter()
            .flat_map(triangle_record)
            .collect();
        self.array(node, 0x50, records);
        // Shape subpart records with their arrays of shapes.
        let mut shape_lists = Vec::new();
        if !mesh.shape_subparts.is_empty() {
            let records: Vec<u8> = mesh
                .shape_subparts
                .iter()
                .flat_map(|s| {
                    let mut r = if s.record.len() == 64 {
                        s.record.clone()
                    } else {
                        let mut r = vec![0u8; 64];
                        r[0] = 1;
                        r[1] = 1;
                        r[10..12].copy_from_slice(&1u16.to_le_bytes());
                        r
                    };
                    put_u32(&mut r, 0x10, 0);
                    put_u32(&mut r, 0x14, s.shapes.len() as u32);
                    put_vec(&mut r, 0x20, &s.rotation);
                    put_vec(&mut r, 0x30, &s.translation);
                    r
                })
                .collect();
            let block = self.data(records);
            self.graph.link(node, 0x58, block);
            for (i, s) in mesh.shape_subparts.iter().enumerate() {
                let list = self.data(vec![0u8; 4 * s.shapes.len()]);
                if !s.shapes.is_empty() {
                    self.graph.link(block, 64 * i as u32 + 0x10, list);
                }
                shape_lists.push(list);
            }
        }
        // Storage objects.
        let mut storages = Vec::new();
        for t in &mesh.triangle_subparts {
            let mut s = base(&t.storage, "hkpStorageExtendedMeshShapeMeshSubpartStorage");
            put_array_header(&mut s, 0x08, t.vertices.len());
            put_array_header(&mut s, 0x14, 0);
            put_array_header(&mut s, 0x20, 4 * t.indices.len());
            for at in [0x2C, 0x38, 0x44] {
                put_array_header(&mut s, at, 0);
            }
            let storage = self.object("hkpStorageExtendedMeshShapeMeshSubpartStorage", s);
            self.array(
                storage,
                0x08,
                t.vertices
                    .iter()
                    .flatten()
                    .flat_map(|v| v.to_le_bytes())
                    .collect(),
            );
            self.array(
                storage,
                0x20,
                t.indices
                    .iter()
                    .flatten()
                    .flat_map(|v| v.to_le_bytes())
                    .collect(),
            );
            storages.push(storage);
        }
        self.array(node, 0xC0, vec![0u8; 4 * storages.len()]);
        if let Some(p) = self.graph.nodes[node].pointer(0xC0).copied() {
            for (i, &s) in storages.iter().enumerate() {
                self.graph.link(p.target, 4 * i as u32, s);
            }
        }
        let mut shape_storages = Vec::new();
        for (i, s) in mesh.shape_subparts.iter().enumerate() {
            let shapes: Vec<usize> = s.shapes.iter().map(|shape| self.shape(shape)).collect();
            for (k, &shape) in shapes.iter().enumerate() {
                self.graph.link(shape_lists[i], 4 * k as u32, shape);
            }
            let mut st = base(&s.storage, "hkpStorageExtendedMeshShapeShapeSubpartStorage");
            put_array_header(&mut st, 0x08, shapes.len());
            for at in [0x14, 0x20, 0x2C] {
                put_array_header(&mut st, at, 0);
            }
            let storage = self.object("hkpStorageExtendedMeshShapeShapeSubpartStorage", st);
            if let Some(block) = self.array(storage, 0x08, vec![0u8; 4 * shapes.len()]) {
                for (k, &shape) in shapes.iter().enumerate() {
                    self.graph.link(block, 4 * k as u32, shape);
                }
            }
            shape_storages.push(storage);
        }
        if let Some(block) = self.array(node, 0xCC, vec![0u8; 4 * shape_storages.len()]) {
            for (i, &s) in shape_storages.iter().enumerate() {
                self.graph.link(block, 4 * i as u32, s);
            }
        }
        // The shape keeps a copy of its first triangle subpart.
        if let Some(first) = mesh.triangle_subparts.first() {
            let record = triangle_record(first);
            self.graph.nodes[node].bytes[0x70..0xB0].copy_from_slice(&record);
        } else if raw.is_empty() {
            self.graph.nodes[node].bytes[0x70..0xB0].fill(0);
        }
        node
    }

    fn body(&mut self, body: &RigidBody) -> usize {
        let mut b = if body.raw.len() >= 0x200 {
            body.raw.clone()
        } else {
            fixed_body_template()
        };
        put_u32(&mut b, 0x10, 0);
        put_array_header(&mut b, 0x74, body.properties.len());
        put_f32(&mut b, 0x84, body.friction);
        put_f32(&mut b, 0x88, body.restitution);
        b[0xD8] = body.motion_type;
        for (k, column) in body.rotation.iter().enumerate() {
            put_vec(&mut b, 0xE0 + 16 * k, column);
        }
        put_vec(&mut b, 0x110, &body.translation);
        put_f32(&mut b, 0x180, body.object_radius);
        let node = self.object("hkpRigidBody", b);
        // Arrays come in member order: the name, then the properties.
        self.string(node, 0x70, body.name.as_deref());
        let mut data = Vec::with_capacity(16 * body.properties.len());
        for p in &body.properties {
            for v in [p.key, p.padding, p.value as u32, (p.value >> 32) as u32] {
                data.extend_from_slice(&v.to_le_bytes());
            }
        }
        self.array(node, 0x74, data);
        let shape = self.shape(&body.shape);
        self.graph.link(node, 0x10, shape);
        node
    }
}

fn triangle_record(t: &TriangleSubpart) -> Vec<u8> {
    let mut r = if t.record.len() == 64 {
        t.record.clone()
    } else {
        let mut r = vec![0u8; 64];
        r[1] = 1; // material index striding type: 8-bit
        put_u32(&mut r, 0x18, 16); // vertex striding
        put_u32(&mut r, 0x34, 16); // index striding
        put_u32(&mut r, 0x38, 2); // 32-bit indices
        r
    };
    r[10..12].copy_from_slice(&t.num_materials.to_le_bytes());
    put_u32(&mut r, 0x10, t.indices.len() as u32);
    put_u32(&mut r, 0x1C, t.vertices.len() as u32);
    r
}

fn sub3(a: &Vec3, b: &Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn perpendiculars(axis: Vec3) -> (Vec3, Vec3) {
    let len = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
    let n = if len > 0.0 {
        [axis[0] / len, axis[1] / len, axis[2] / len]
    } else {
        [0.0, 1.0, 0.0]
    };
    let helper = if n[0].abs() < 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 0.0, 1.0]
    };
    let cross = |a: Vec3, b: Vec3| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let p1 = cross(n, helper);
    let l = (p1[0] * p1[0] + p1[1] * p1[1] + p1[2] * p1[2]).sqrt();
    let p1 = [p1[0] / l, p1[1] / l, p1[2] / l];
    (p1, cross(n, p1))
}

/// The bytes of a fixed rigid body as the game's packfiles store it, without
/// shape-dependent values.
fn fixed_body_template() -> Vec<u8> {
    let mut b = vec![0u8; 0x200];
    put_u32(&mut b, 0x14, END); // shape key
    b[0x28] = 1; // broad-phase handle type: entity
    b[0x2A] = 1; // object quality type
    put_u32(&mut b, 0x30, 1);
    put_u32(&mut b, 0x58, 0x7F7F_FFEE); // allowed penetration depth: computed by Havok
    put_u32(&mut b, 0x64, 0x8000_0000);
    put_u32(&mut b, 0x80, 1); // material response type
    put_f32(&mut b, 0x84, 0.5);
    put_f32(&mut b, 0x88, 0.4);
    put_u32(&mut b, 0x94, END);
    put_u32(&mut b, 0xA8, 0x8000_0000);
    put_u32(&mut b, 0xB4, 0x8000_0000);
    put_u32(&mut b, 0xC0, END);
    put_u32(&mut b, 0xD8, 0xC000_0F07); // fixed motion
    put_u32(&mut b, 0xDC, 0x0000_C000);
    for k in 0..3 {
        put_f32(&mut b, 0xE0 + 20 * k, 1.0); // identity rotation
    }
    put_f32(&mut b, 0x14C, 1.0); // rotation quaternions w
    put_f32(&mut b, 0x15C, 1.0);
    put_f32(&mut b, 0x188, 0.05); // angular damping
    put_u32(&mut b, 0x18C, 0x0001_7F7F);
    b
}

impl Physics {
    /// Physics for a new packfile with one fixed body per shape.
    pub fn fixed(bodies: Vec<(Option<String>, Shape)>, materials: Vec<EaMaterial>) -> Self {
        let has_materials = !materials.is_empty();
        let bodies = bodies
            .into_iter()
            .map(|(name, shape)| {
                let object_radius = super::geometry::bounding_radius(&shape);
                RigidBody {
                    name,
                    shape,
                    properties: if has_materials {
                        vec![Property {
                            key: PROPERTY_MATERIAL,
                            value: 1,
                            padding: 0,
                        }]
                    } else {
                        Vec::new()
                    },
                    motion_type: MOTION_FIXED,
                    rotation: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                    translation: [0.0; 3],
                    friction: 0.5,
                    restitution: 0.4,
                    object_radius,
                    raw: Vec::new(),
                }
            })
            .collect();
        Self {
            systems: vec![System {
                name: Some("Default Physics System".into()),
                bodies,
                active: true,
                raw: Vec::new(),
            }],
            materials: Some(materials.into_iter().map(Some).collect()),
            other_contents: Vec::new(),
            raw: Raw::default(),
        }
    }

    /// Lays the physics out as a data graph.
    pub fn to_graph(&self) -> Graph {
        let mut w = Writer {
            graph: Graph::default(),
            shapes: std::collections::HashMap::new(),
        };
        let mut root = base(&self.raw.root, "hkRootLevelContainer");
        let variants = 1 + usize::from(self.materials.is_some());
        put_array_header(&mut root, 0, variants);
        put_u32(&mut root, 8, 0);
        let root = w.object("hkRootLevelContainer", root);
        w.graph.root = root;
        let list = w.data(vec![0u8; 16 * variants]);
        w.graph.link(root, 0, list);
        let default_names = [
            ("Physics Data", "hkpPhysicsData"),
            ("EAMaterial Data", "EAMaterialArray"),
        ];
        let name = |i: usize| -> (String, String) {
            let (n, c) = &self.raw.names[i];
            if n.is_empty() {
                (default_names[i].0.into(), default_names[i].1.into())
            } else {
                (n.clone(), c.clone())
            }
        };
        let (n, c) = name(0);
        w.string(list, 0, Some(&n));
        w.string(list, 4, Some(&c));
        let mut data = base(&self.raw.data, "hkpPhysicsData");
        put_array_header(&mut data, 0x0C, self.systems.len());
        let data = w.object("hkpPhysicsData", data);
        w.graph.link(list, 8, data);
        let systems = w.data(vec![0u8; 4 * self.systems.len()]);
        if !self.systems.is_empty() {
            w.graph.link(data, 0x0C, systems);
        }
        for (i, system) in self.systems.iter().enumerate() {
            let mut s = base(&system.raw, "hkpPhysicsSystem");
            put_array_header(&mut s, 0x08, system.bodies.len());
            for at in [0x14, 0x20, 0x2C] {
                put_array_header(&mut s, at, 0);
            }
            put_u32(&mut s, 0x38, 0);
            s[0x40] = u8::from(system.active);
            let node = w.object("hkpPhysicsSystem", s);
            w.graph.link(systems, 4 * i as u32, node);
            let bodies = w.array(node, 0x08, vec![0u8; 4 * system.bodies.len()]);
            w.string(node, 0x38, system.name.as_deref());
            for (k, body) in system.bodies.iter().enumerate() {
                let b = w.body(body);
                if let Some(bodies) = bodies {
                    w.graph.link(bodies, 4 * k as u32, b);
                }
            }
        }
        if let Some(materials) = &self.materials {
            let (n, c) = name(1);
            w.string(list, 16, Some(&n));
            w.string(list, 20, Some(&c));
            let mut array = base(&self.raw.materials, "EAMaterialArray");
            put_array_header(&mut array, 0, materials.len());
            let array = w.object("EAMaterialArray", array);
            w.graph.link(list, 24, array);
            if let Some(block) = w.array(array, 0, vec![0u8; 4 * materials.len()]) {
                for (i, m) in materials.iter().enumerate() {
                    let Some(m) = m else { continue };
                    let bytes = m.words.iter().flat_map(|w| w.to_le_bytes()).collect();
                    let node = w.object("EAMaterial", bytes);
                    w.graph.link(block, 4 * i as u32, node);
                }
            }
        }
        w.graph
    }

    /// Writes the physics into `packfile`, replacing its data section.
    pub fn store(&self, packfile: &mut Packfile) -> Result<()> {
        if !self.other_contents.is_empty() {
            return Err(Error::InvalidArgument(format!(
                "cannot write packfiles with {}",
                self.other_contents.join(", ")
            )));
        }
        self.to_graph().store(packfile)
    }

    /// A new packfile holding this physics, laid out like the game's.
    pub fn to_packfile(&self) -> Result<Packfile> {
        let (names, offsets) = build_class_names(CLASS_NAMES);
        let root_name = CLASS_NAMES
            .iter()
            .position(|(_, n)| *n == "hkRootLevelContainer")
            .map(|i| offsets[i])
            .expect("root class declared");
        let mut class_section = SectionBody::named("__classnames__");
        class_section.data = names;
        let mut packfile = Packfile {
            user_tag: 0,
            file_version: 5,
            layout: [4, 1, 0, 1],
            contents_version: String::new(),
            root_class: String::new(),
            sections: Vec::new(),
            classes: Vec::new(),
            objects: Vec::new(),
            contents: (2, 0),
            contents_class_name: (0, root_name),
            version_field: *b"Havok-5.5.0-r1\0\xFF",
            flags: [END, END],
            bodies: vec![
                class_section,
                SectionBody::named("__types__"),
                SectionBody::named("__data__"),
            ],
        };
        self.store(&mut packfile)?;
        Ok(packfile)
    }

    /// Serialises a new packfile.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        Ok(self.to_packfile()?.to_bytes())
    }
}

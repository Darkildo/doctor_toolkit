//! `rcb` resources: rigs (skeletons) of characters, vehicles and animated props.
//!
//! An `rcb` is a relocatable memory image (see [`crate::resource::image`]),
//! versions 24 and 25. It holds one or more skeletons (characters have four:
//! `this_lod1`..`this_lod3` plus an unnamed one) and named bone sets.
//!
//! ```text
//! 0x00  u32  relocation table offset
//! 0x04  u32  version, 24 or 25
//! 0x08  u32  hash (unknown input); first word of a 128-bit name id, the
//!            other three are zero
//! 0x18  u32  skeleton count S
//! 0x1C  ptr  S × skeleton {u32 name hash, ptr name, u32 bone count N,
//!                          ptr hierarchy, ptr inverse bind matrices, ptr lookup}
//! 0x20  u32  bone set count B
//! 0x24  ptr  B × {u32 name hash, ptr {ptr bit words, u32 bit count}}
//!            (skeleton filters; the bit count is the bone count of skeleton 0)
//! 0x28..0x37 runtime pointers and a counter, zero in the files
//! 0x38  f32  base scale: 1.88 for humans (whose bind pose is 1.0 high), 1.0 otherwise
//! 0x3C  i32  0
//! 0x40  ptr  locators: one {u32 count, ptr} per skeleton; a locator is 0x30
//!            bytes {u32 id, 3 × u32 0, 4 × f32, 3 × f32, i32 bone or -1}
//! 0x44  u32  count and 0x48 ptr: u32 ids of referenced containers
//! 0x4C  u32  count and 0x50 ptr: pose adjustments {i32 id, u32 n, ptr n ×
//!            {quaternion, 3 × f32, i32 bone}}
//! 0x54  u32  count and 0x58 ptr: skeleton bindings, 0x20 bytes each
//!            {quaternion, 4 × f32 translation with w = 0}, 16-aligned
//! 0x5C  ptr  {u32 count, ptr}: blend-shape detail, always empty
//! 0x60  u32  count and 0x64 ptr: bone attributes, always empty
//! (names of 0x40…0x64 from the comparison with another EARS editor; every pointer
//! is in the relocation table exactly when it is not zero; not decoded here)
//!
//! hierarchy: N × {i32 parent (-1 for the root, always < own index),
//!                 i32 bind index (into the skeleton bindings, or -1), i32 -1,
//!                 u32 bind type (1 unbound, 2 bound)}
//! inverse bind matrices: N × 16 f32, row-major for row vectors (Direct3D):
//!     model space → bone space; the last column is (0, 0, 0, 1)
//! lookup: N × {u32 bone name hash, 3 × u32 0, u32 bone index}, sorted by hash
//! ```
//!
//! Bone names are SDBM hashes of names such as `l_wrist`; readable names come
//! from `data/gf2/bone_names.txt`. Meshes reference bones by these hashes (their
//! bone table maps each hash to the index in the skeleton).
//!
//! Because the matrices are row-major for row vectors, their 16 floats are
//! exactly the column-major layout of the column-vector matrix, which is what
//! glTF expects for inverse bind matrices.

use serde::Serialize;

use crate::Result;
use crate::hash::sdbm;
use crate::resource::image::Image;
use crate::resource::names::bone_names;

/// Type id of `rcb` resources.
pub const TYPE_RCB: u32 = sdbm(b"rcb");

const SKELETON_SIZE: usize = 24;
const HIERARCHY_SIZE: usize = 16;
const MATRIX_SIZE: usize = 64;
const LOOKUP_SIZE: usize = 20;

/// A 4×4 matrix in column-major order for column vectors (the glTF convention).
pub type Mat4 = [f32; 16];

/// One bone of a skeleton.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Bone {
    /// SDBM hash of the bone name.
    pub hash: u32,
    /// Readable name when the hash is known.
    pub name: Option<&'static str>,
    pub parent: Option<usize>,
    /// Index into the skeleton bindings of the rig (header 0x54/0x58), or −1
    /// when the bone is unbound. Kept as `link` in JSON.
    #[serde(rename = "link")]
    pub bind_index: i32,
    /// 2 when the bone is bound (`bind_index` ≥ 0), 1 otherwise. Kept as
    /// `kind` in JSON.
    #[serde(rename = "kind")]
    pub bind_type: u32,
    /// Model space → bone space, column-major for column vectors.
    pub inverse_bind: Mat4,
}

impl Bone {
    /// The name, or the hash in hexadecimal.
    pub fn display_name(&self) -> String {
        self.name
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{:08x}", self.hash))
    }
}

/// One skeleton of a rig.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Skeleton {
    pub name_hash: u32,
    pub name: String,
    pub bones: Vec<Bone>,
}

/// A named subset of the bones of the first skeleton.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoneSet {
    pub hash: u32,
    pub bones: Vec<usize>,
}

/// A parsed `rcb` resource.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Rig {
    pub version: u32,
    pub hash: u32,
    pub scale: f32,
    pub skeletons: Vec<Skeleton>,
    pub bone_sets: Vec<BoneSet>,
}

impl Rig {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let image = Image::parse(data, "rcb")?;
        let version = image.version();
        if !(24..=25).contains(&version) {
            return Err(crate::Error::UnsupportedVersion {
                what: "rcb",
                version,
            });
        }
        let skeleton_count = image.u32(0x18)? as usize;
        let skeletons_at = image.u32(0x1C)? as usize;
        let records = image.array(skeletons_at, skeleton_count, SKELETON_SIZE)?;
        let mut skeletons = Vec::with_capacity(skeleton_count);
        for i in 0..skeleton_count {
            let at = skeletons_at + SKELETON_SIZE * i;
            skeletons.push(parse_skeleton(&image, at)?);
        }
        debug_assert_eq!(records.len(), skeleton_count * SKELETON_SIZE);

        let set_count = image.u32(0x20)? as usize;
        let sets_at = image.u32(0x24)? as usize;
        image.array(sets_at, set_count, 8)?;
        let mut bone_sets = Vec::with_capacity(set_count);
        for i in 0..set_count {
            let hash = image.u32(sets_at + 8 * i)?;
            let set = image.u32(sets_at + 8 * i + 4)? as usize;
            let words_at = image.u32(set)? as usize;
            let bits = image.u32(set + 4)? as usize;
            let words = image.array(words_at, bits.div_ceil(32), 4)?;
            let bones = (0..bits)
                .filter(|&b| {
                    let word = u32::from_le_bytes(words[4 * (b / 32)..][..4].try_into().unwrap());
                    word >> (b % 32) & 1 != 0
                })
                .collect();
            bone_sets.push(BoneSet { hash, bones });
        }
        Ok(Self {
            version,
            hash: image.u32(8)?,
            scale: image.f32(0x38)?,
            skeletons,
            bone_sets,
        })
    }

    /// The first skeleton whose bones match a mesh bone table `(hash, index)`.
    pub fn skeleton_for(&self, bones: &[(u32, u32)]) -> Option<&Skeleton> {
        self.skeletons.iter().find(|s| s.matches(bones))
    }
}

fn parse_skeleton(image: &Image<'_>, at: usize) -> Result<Skeleton> {
    let name_hash = image.u32(at)?;
    let name_at = image.u32(at + 4)? as usize;
    let count = image.u32(at + 8)? as usize;
    let hierarchy_at = image.u32(at + 12)? as usize;
    let matrices_at = image.u32(at + 16)? as usize;
    let lookup_at = image.u32(at + 20)? as usize;
    let name = if name_at == 0 {
        String::new()
    } else {
        image.cstr(name_at)?
    };
    image.array(hierarchy_at, count, HIERARCHY_SIZE)?;
    image.array(matrices_at, count, MATRIX_SIZE)?;
    image.array(lookup_at, count, LOOKUP_SIZE)?;

    let mut hashes = vec![None; count];
    for i in 0..count {
        let entry = lookup_at + LOOKUP_SIZE * i;
        let index = image.u32(entry + 16)? as usize;
        let slot = hashes.get_mut(index).ok_or_else(|| {
            crate::Error::malformed("rcb", entry, format!("bone index {index} out of range"))
        })?;
        *slot = Some(image.u32(entry)?);
    }
    let mut bones = Vec::with_capacity(count);
    for (i, hash) in hashes.into_iter().enumerate() {
        let h = hierarchy_at + HIERARCHY_SIZE * i;
        let parent = image.i32(h)?;
        let parent = match usize::try_from(parent) {
            Ok(p) if p < i => Some(p),
            Err(_) if parent == -1 => None,
            _ => {
                return Err(crate::Error::malformed(
                    "rcb",
                    h,
                    format!("bone {i} has parent {parent}"),
                ));
            }
        };
        let hash = hash.ok_or_else(|| {
            crate::Error::malformed("rcb", lookup_at, format!("bone {i} missing from lookup"))
        })?;
        let mut inverse_bind = [0f32; 16];
        for (k, value) in inverse_bind.iter_mut().enumerate() {
            *value = image.f32(matrices_at + MATRIX_SIZE * i + 4 * k)?;
        }
        bones.push(Bone {
            hash,
            name: bone_names().get(hash),
            parent,
            bind_index: image.i32(h + 4)?,
            bind_type: image.u32(h + 12)?,
            inverse_bind,
        });
    }
    Ok(Skeleton {
        name_hash,
        name,
        bones,
    })
}

impl Skeleton {
    /// Whether every `(hash, index)` pair of a mesh bone table names the same bone here.
    pub fn matches(&self, bones: &[(u32, u32)]) -> bool {
        !bones.is_empty()
            && bones.iter().all(|&(hash, index)| {
                self.bones
                    .get(index as usize)
                    .is_some_and(|bone| bone.hash == hash)
            })
    }

    /// Bone → model space transforms in the bind pose (inverses of the inverse bind matrices).
    pub fn bind_globals(&self) -> Vec<Mat4> {
        self.bones
            .iter()
            .map(|b| invert_affine(&b.inverse_bind))
            .collect()
    }

    /// Bone → parent space transforms in the bind pose.
    pub fn bind_locals(&self) -> Vec<Mat4> {
        self.bones
            .iter()
            .map(|bone| {
                let global = invert_affine(&bone.inverse_bind);
                match bone.parent {
                    Some(p) => multiply(&self.bones[p].inverse_bind, &global),
                    None => global,
                }
            })
            .collect()
    }
}

/// `a × b` for column-major matrices.
pub fn multiply(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [0f32; 16];
    for col in 0..4 {
        for row in 0..4 {
            out[col * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[col * 4 + k]).sum();
        }
    }
    out
}

/// Inverse of an affine column-major matrix (last row 0, 0, 0, 1).
pub fn invert_affine(m: &Mat4) -> Mat4 {
    let a = |row: usize, col: usize| m[col * 4 + row];
    let det = a(0, 0) * (a(1, 1) * a(2, 2) - a(1, 2) * a(2, 1))
        - a(0, 1) * (a(1, 0) * a(2, 2) - a(1, 2) * a(2, 0))
        + a(0, 2) * (a(1, 0) * a(2, 1) - a(1, 1) * a(2, 0));
    let inv_det = if det.abs() > f32::EPSILON {
        1.0 / det
    } else {
        0.0
    };
    // Adjugate of the 3×3 part.
    let mut r = [[0f32; 3]; 3];
    r[0][0] = (a(1, 1) * a(2, 2) - a(1, 2) * a(2, 1)) * inv_det;
    r[0][1] = (a(0, 2) * a(2, 1) - a(0, 1) * a(2, 2)) * inv_det;
    r[0][2] = (a(0, 1) * a(1, 2) - a(0, 2) * a(1, 1)) * inv_det;
    r[1][0] = (a(1, 2) * a(2, 0) - a(1, 0) * a(2, 2)) * inv_det;
    r[1][1] = (a(0, 0) * a(2, 2) - a(0, 2) * a(2, 0)) * inv_det;
    r[1][2] = (a(0, 2) * a(1, 0) - a(0, 0) * a(1, 2)) * inv_det;
    r[2][0] = (a(1, 0) * a(2, 1) - a(1, 1) * a(2, 0)) * inv_det;
    r[2][1] = (a(0, 1) * a(2, 0) - a(0, 0) * a(2, 1)) * inv_det;
    r[2][2] = (a(0, 0) * a(1, 1) - a(0, 1) * a(1, 0)) * inv_det;
    let t = [a(0, 3), a(1, 3), a(2, 3)];
    let mut out = [0f32; 16];
    for row in 0..3 {
        for col in 0..3 {
            out[col * 4 + row] = r[row][col];
        }
        out[12 + row] = -(r[row][0] * t[0] + r[row][1] * t[1] + r[row][2] * t[2]);
    }
    out[15] = 1.0;
    out
}

/// Translation of a column-major matrix.
pub fn translation(m: &Mat4) -> [f32; 3] {
    [m[12], m[13], m[14]]
}

#[cfg(any(test, feature = "testing"))]
/// Builds an rcb with one skeleton of two bones: root at the origin and a
/// child at (0, 1, 0) rotated 90° about Z.
pub(crate) fn sample_rig() -> Vec<u8> {
    let mut d = vec![0u8; 0x200];
    let put = |d: &mut Vec<u8>, at: usize, v: u32| d[at..at + 4].copy_from_slice(&v.to_le_bytes());
    let putf = |d: &mut Vec<u8>, at: usize, v: f32| d[at..at + 4].copy_from_slice(&v.to_le_bytes());
    put(&mut d, 4, 25);
    put(&mut d, 8, 0x1234);
    put(&mut d, 0x18, 1);
    put(&mut d, 0x1C, 0x68);
    put(&mut d, 0x20, 1);
    put(&mut d, 0x24, 0x80);
    putf(&mut d, 0x38, 1.0);
    // skeleton record
    for (k, v) in [sdbm(b"this_lod1"), 0x88, 2, 0xA0, 0xC0, 0x140]
        .iter()
        .enumerate()
    {
        put(&mut d, 0x68 + 4 * k, *v);
    }
    // bone set: {hash, ptr 0x90} -> {ptr 0x98, 2 bits} -> 0b10
    put(&mut d, 0x80, 0xBEEF);
    put(&mut d, 0x84, 0x90);
    d[0x88..0x8E].copy_from_slice(b"skel\0\0");
    put(&mut d, 0x90, 0x98);
    put(&mut d, 0x94, 2);
    put(&mut d, 0x98, 0b10);
    // hierarchy
    for (k, v) in [u32::MAX, u32::MAX, u32::MAX, 1, 0, u32::MAX, u32::MAX, 1]
        .iter()
        .enumerate()
    {
        put(&mut d, 0xA0 + 4 * k, *v);
    }
    // inverse bind matrices (row-major, row vectors)
    let identity = [
        1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
    ];
    // child global: rotate +90° about Z then translate (0,1,0); inverse in row form:
    let child = [
        0., -1., 0., 0., 1., 0., 0., 0., 0., 0., 1., 0., -1., 0., 0., 1.,
    ];
    for (k, v) in identity.iter().chain(child.iter()).enumerate() {
        putf(&mut d, 0xC0 + 4 * k, *v);
    }
    // lookup sorted by hash
    let (root, hand) = (sdbm(b"m_root"), sdbm(b"l_wrist"));
    let mut entries = [(root, 0u32), (hand, 1u32)];
    entries.sort();
    for (k, (hash, index)) in entries.iter().enumerate() {
        put(&mut d, 0x140 + 20 * k, *hash);
        put(&mut d, 0x140 + 20 * k + 16, *index);
    }
    // relocation table
    let relocs = [0x90u32, 0x84, 0x7C, 0x78, 0x74, 0x6C, 0x24, 0x1C];
    put(&mut d, 0, 0x170);
    for (k, v) in relocs.iter().chain([0u32].iter()).enumerate() {
        put(&mut d, 0x170 + 4 * k, *v);
    }
    d.truncate(0x170 + 4 * (relocs.len() + 1));
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn sample() -> Vec<u8> {
        super::sample_rig()
    }

    #[test]
    fn parses_a_skeleton() {
        let rig = Rig::parse(&sample()).unwrap();
        assert_eq!(rig.version, 25);
        assert_eq!(rig.skeletons.len(), 1);
        let s = &rig.skeletons[0];
        assert_eq!(s.name, "skel");
        assert_eq!(s.bones.len(), 2);
        assert_eq!(s.bones[0].name, Some("m_root"));
        assert_eq!(s.bones[0].parent, None);
        assert_eq!(s.bones[1].name, Some("l_wrist"));
        assert_eq!(s.bones[1].parent, Some(0));
        assert_eq!(
            rig.bone_sets,
            vec![BoneSet {
                hash: 0xBEEF,
                bones: vec![1]
            }]
        );
        assert!(s.matches(&[(sdbm(b"l_wrist"), 1)]));
        assert!(!s.matches(&[(sdbm(b"l_wrist"), 0)]));
        assert!(!s.matches(&[]));
    }

    #[test]
    fn bind_pose_inverts_the_matrices() {
        let rig = Rig::parse(&sample()).unwrap();
        let s = &rig.skeletons[0];
        let globals = s.bind_globals();
        let t = translation(&globals[1]);
        assert!((t[0]).abs() < 1e-6 && (t[1] - 1.0).abs() < 1e-6, "{t:?}");
        // The local transform of a child of the identity root equals its global one.
        assert_eq!(s.bind_locals()[1], globals[1]);
        let product = multiply(&s.bones[1].inverse_bind, &globals[1]);
        for (i, v) in product.iter().enumerate() {
            let expected = if i % 5 == 0 { 1.0 } else { 0.0 };
            assert!((v - expected).abs() < 1e-6);
        }
    }

    #[test]
    fn rejects_bad_input() {
        assert!(Rig::parse(&[]).is_err());
        let mut d = sample();
        d[4] = 9;
        assert!(Rig::parse(&d).is_err());
        let mut d = sample();
        d[0x68 + 8] = 200; // more bones than the image holds
        assert!(Rig::parse(&d).is_err());
        let mut d = sample();
        d[0xB0] = 5; // parent after the bone
        d[0xB1..0xB4].fill(0);
        assert!(Rig::parse(&d).is_err());
    }
}

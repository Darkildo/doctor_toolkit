//! Builds new meshes from geometry: the inverse of [`crate::resource::mesh::Model`].
//!
//! Each [`PartGeometry`] (triangles with one material) becomes one or more
//! draw records of one of the three kinds the game uses:
//!
//! | Kind | Used for | Records | Vertex buffers |
//! |---|---|---|---|
//! | static | props, buildings | strip (2,2), triangle grid, 8-corner box | stream 0 positions in the mesh, stream 1 in the meshvolatile (GPU declaration) |
//! | rigid | weapons, heads, hair, vehicle parts | list (1,1), one bone, influences 0 | stream 0 position/normal/tangent in the mesh, stream 1 in the meshvolatile (CPU declaration) |
//! | skinned | bodies, clothes | list (1,1), palette ≤ 42 bones, CPU-skinned inline sources | CPU buffer without data, stream 1 in the meshvolatile |
//!
//! Records are split so that a record has at most [`MAX_PALETTE`] bones and a
//! vertex buffer at most 65 535 vertices (16-bit indices). Record fields the
//! geometry does not determine (render flags, the overlay layer at 0x28) are
//! copied from a template record of the same kind when the caller gives one.

use std::collections::HashMap;

use crate::resource::mesh::{
    self, ElementType, Primitive, RENDER_FLAG_LOW_LOD_ONLY, RENDER_FLAG_TRI_SORT, Usage,
};
use crate::resource::mesh_grid::Grid;
use crate::resource::mesh_image::{
    BufferImage, DataRef, FLAG_CPU, FLAG_VOLATILE, MeshImage, RECORD_SIZE, RecordImage,
};
use crate::{Error, Result};

/// Most bones in one record palette. The game's records use up to 42.
pub const MAX_PALETTE: usize = 42;

const MAX_VERTICES: usize = u16::MAX as usize;

/// Kind of draw record (see the module documentation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RecordKind {
    Static,
    /// All vertices follow one skeleton bone.
    Rigid,
    /// Per-vertex weights, skinned on the CPU.
    Skinned,
}

impl RecordKind {
    /// The kind of a record of an existing mesh.
    pub fn of(record: &RecordImage) -> Self {
        if record.u8_at(4) == 2 {
            Self::Static
        } else if record.u32_at(0x68) as i32 >= 0 {
            Self::Skinned
        } else {
            Self::Rigid
        }
    }
}

/// Geometry of one part: triangles with one material.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PartGeometry {
    pub name: String,
    /// CRC-32 name hash of the material.
    pub material_hash: u32,
    pub positions: Vec<[f32; 3]>,
    pub normals: Option<Vec<[f32; 3]>>,
    /// Tangent with the bitangent sign in w.
    pub tangents: Option<Vec<[f32; 4]>>,
    pub uv0: Option<Vec<[f32; 2]>>,
    pub uv1: Option<Vec<[f32; 2]>>,
    pub colors: Option<Vec<[u8; 4]>>,
    /// Skeleton bone indices and normalised weights, up to four per vertex.
    pub skin: Option<Vec<([u16; 4], [f32; 4])>>,
    pub triangles: Vec<[u32; 3]>,
}

/// What [`build`] needs besides the parts.
#[derive(Debug, Clone, Default)]
pub struct BuildOptions<'a> {
    /// Model name written into the mesh.
    pub name: String,
    /// Bone name hashes of the skeleton, by skeleton index; required for skinned parts.
    pub skeleton: Option<Vec<u32>>,
    /// The mesh being replaced or cloned: record kinds, formats, render
    /// flags and the bone of rigid records come from it.
    pub template: Option<&'a MeshImage>,
    /// Material a part's material was cloned from (new hash → template
    /// hash), to find the template record of the part.
    pub template_materials: HashMap<u32, u32>,
    /// Force a vertex format for every record instead of the template's.
    pub format: Option<u8>,
}

/// A record kind, format and template record chosen for a part.
#[derive(Debug, Clone, Copy)]
struct Plan<'a> {
    kind: RecordKind,
    format: u8,
    template: Option<&'a RecordImage>,
    /// Bone of rigid records.
    bone: u16,
}

/// Formats whose CPU declaration has a 20-byte stream 0 (rigid and skinned records).
const SKINNABLE: [u8; 7] = [0, 1, 2, 3, 4, 5, 10];

fn default_format(kind: RecordKind) -> u8 {
    match kind {
        RecordKind::Static => 1,
        RecordKind::Rigid | RecordKind::Skinned => 4,
    }
}

/// Half-float format of the characters' LOD2 (`PosTex1Norm_LowLOD`): skinned
/// records only, a 16-byte inline source with two influences.
const LOW_LOD: u8 = 11;

fn format_allowed(kind: RecordKind, format: u8) -> bool {
    match kind {
        RecordKind::Static => mesh::declarations(format).is_some(),
        RecordKind::Skinned if format == LOW_LOD => true,
        _ => SKINNABLE.contains(&format),
    }
}

/// Builds a mesh. Returns it with warnings about substituted data.
pub fn build(
    parts: &[PartGeometry],
    options: &BuildOptions<'_>,
) -> Result<(MeshImage, Vec<String>)> {
    let mut warnings = Vec::new();
    let template_records: Vec<&RecordImage> = options
        .template
        .map(|t| t.records.iter().collect())
        .unwrap_or_default();
    // A record of the same name first (flags differ between the records of
    // one mesh in 4 167 meshes of the game), then one with the material.
    let template_of = |kind: RecordKind, material: u32, name: &str| -> Option<&RecordImage> {
        if let Some(r) = template_records
            .iter()
            .copied()
            .find(|r| RecordKind::of(r) == kind && r.name == name)
        {
            return Some(r);
        }
        let material = options
            .template_materials
            .get(&material)
            .copied()
            .unwrap_or(material);
        template_records
            .iter()
            .copied()
            .filter(|r| RecordKind::of(r) == kind)
            .find(|r| r.u32_at(0x0C) == material)
            .or_else(|| {
                template_records
                    .iter()
                    .copied()
                    .find(|r| RecordKind::of(r) == kind)
            })
    };
    // Bone table: (hash, skeleton index). The template's table is kept as it
    // is and new bones are appended in first-use order. Every character mesh
    // of the game carries the same table of the whole skeleton in one order
    // (816 of 816 in `allcharacters`), and a clothing mesh with its own table
    // shows its parts on the wrong bones in the game (stage 24), so the table
    // index of a palette entry must stay the template's.
    let mut bone_table: Vec<(u32, u32)> = options
        .template
        .map(|t| t.bones.clone())
        .unwrap_or_default();
    let table_index = |bone: u16, hash: u32, table: &mut Vec<(u32, u32)>| -> Result<u8> {
        let pos = match table.iter().position(|&(_, b)| b == u32::from(bone)) {
            Some(p) => p,
            None => {
                table.push((hash, u32::from(bone)));
                table.len() - 1
            }
        };
        u8::try_from(pos).map_err(|_| Error::InvalidArgument("more than 256 bones used".into()))
    };
    let skeleton_hash = |bone: u16| -> Result<u32> {
        options
            .skeleton
            .as_ref()
            .and_then(|s| s.get(bone as usize).copied())
            .ok_or_else(|| Error::InvalidArgument(format!("bone {bone} is not in the skeleton")))
    };

    let mut plans = Vec::with_capacity(parts.len());
    for part in parts {
        validate(part)?;
        let single_bone = part.skin.as_ref().and_then(|skin| single_bone(skin));
        let (kind, mut bone) = match (&part.skin, single_bone) {
            (Some(_), Some(b)) => (RecordKind::Rigid, b),
            (Some(_), None) => (RecordKind::Skinned, 0),
            (None, _) => {
                // Unskinned geometry takes the template's kind: weapons and
                // props without a rig are rigid records bound to bone 0.
                let has = |k| template_records.iter().any(|r| RecordKind::of(r) == k);
                if template_records.is_empty() || has(RecordKind::Static) {
                    (RecordKind::Static, 0)
                } else {
                    if has(RecordKind::Skinned) && !has(RecordKind::Rigid) {
                        warnings.push(format!(
                            "part `{}` has no skin: it follows one bone of the model rigidly",
                            part.name
                        ));
                    }
                    (RecordKind::Rigid, 0)
                }
            }
        };
        if kind != RecordKind::Static && part.skin.is_some() && options.skeleton.is_none() {
            return Err(Error::InvalidArgument(format!(
                "part `{}` is skinned but no skeleton was given",
                part.name
            )));
        }
        let template = template_of(kind, part.material_hash, &part.name);
        if kind == RecordKind::Rigid
            && part.skin.is_none()
            && let Some(t) = template
        {
            bone = u16::from(t.u8_at(0x24));
        }
        let format = options
            .format
            .or_else(|| template.map(|t| t.u8_at(7)))
            .filter(|&f| format_allowed(kind, f))
            .unwrap_or_else(|| default_format(kind));
        plans.push(Plan {
            kind,
            format,
            template,
            bone,
        });
    }
    // Rigid records of unskinned parts: the template's bone table entry.
    let template_bone_hash = |bone: u16| -> Option<u32> {
        options
            .template?
            .bones
            .iter()
            .find(|&&(_, i)| i == u32::from(bone))
            .map(|&(h, _)| h)
    };

    let mut groups: HashMap<(RecordKind, u8), Group> = HashMap::new();
    let mut group_order: Vec<(RecordKind, u8)> = Vec::new();
    let mut records: Vec<PendingRecord> = Vec::new();
    for (part, plan) in parts.iter().zip(&plans) {
        let key = (plan.kind, plan.format);
        if let std::collections::hash_map::Entry::Vacant(e) = groups.entry(key) {
            group_order.push(key);
            e.insert(Group::new(plan.kind, plan.format)?);
        }
        let filled = fill_attributes(part, plan.format, &mut warnings);
        for chunk in split(part, plan.kind, &mut warnings) {
            let group = groups.get_mut(&key).expect("group inserted above");
            let palette: Vec<(u8, u8)> = match plan.kind {
                RecordKind::Static => Vec::new(),
                RecordKind::Rigid => {
                    let hash = match part.skin {
                        Some(_) => skeleton_hash(plan.bone)?,
                        None => template_bone_hash(plan.bone)
                            .or_else(|| skeleton_hash(plan.bone).ok())
                            .ok_or_else(|| {
                                Error::InvalidArgument(format!(
                                    "part `{}`: no bone to bind the rigid record to",
                                    part.name
                                ))
                            })?,
                    };
                    let t = table_index(plan.bone, hash, &mut bone_table)?;
                    vec![(bone_u8(plan.bone)?, t)]
                }
                RecordKind::Skinned => chunk
                    .bones
                    .iter()
                    .map(|&b| {
                        Ok((
                            bone_u8(b)?,
                            table_index(b, skeleton_hash(b)?, &mut bone_table)?,
                        ))
                    })
                    .collect::<Result<_>>()?,
            };
            let record = group.add(part, &filled, plan, &chunk, &palette)?;
            records.push(record);
        }
    }
    if records.is_empty() {
        return Err(Error::InvalidArgument("the model has no triangles".into()));
    }

    // Buffers: per group, stream 0 then stream 1; one index buffer per primitive.
    let mut vertex_buffers = Vec::new();
    let mut gpu_ordinal = 0i32;
    let mut cpu_ordinal = 0i32;
    // (group key, buffer number within group) → (stream0 list index, gpu0, cpu0, gpu1, list1)
    let mut buffer_slots: HashMap<((RecordKind, u8), usize), BufferSlots> = HashMap::new();
    for key in &group_order {
        let group = &groups[key];
        for (n, buffer) in group.buffers.iter().enumerate() {
            let mut slots = BufferSlots::default();
            let s0 = BufferImage {
                size: (buffer.count * group.stride0) as u32,
                count: buffer.count as u32,
                stride: group.stride0 as u16,
                format: group.format,
                flags: if group.kind == RecordKind::Skinned {
                    FLAG_CPU
                } else {
                    0
                },
                data: if group.kind == RecordKind::Skinned {
                    Vec::new()
                } else {
                    buffer.stream0.clone()
                },
            };
            slots.list0 = vertex_buffers.len();
            if s0.is_cpu() {
                slots.cpu0 = cpu_ordinal;
                cpu_ordinal += 1;
            } else {
                slots.gpu0 = gpu_ordinal;
                gpu_ordinal += 1;
            }
            vertex_buffers.push(s0);
            if group.stride1 > 0 {
                vertex_buffers.push(BufferImage {
                    size: buffer.stream1.len() as u32,
                    count: buffer.count as u32,
                    stride: group.stride1 as u16,
                    format: group.format,
                    flags: FLAG_VOLATILE,
                    data: buffer.stream1.clone(),
                });
                slots.gpu1 = gpu_ordinal;
                gpu_ordinal += 1;
            }
            buffer_slots.insert((*key, n), slots);
        }
    }
    let mut index_buffers: Vec<BufferImage> = Vec::new();
    let mut index_data: [Vec<u8>; 2] = [Vec::new(), Vec::new()];
    let mut out_records = Vec::with_capacity(records.len());
    let has_static = records.iter().any(|r| r.kind == RecordKind::Static);
    let has_lists = records.iter().any(|r| r.kind != RecordKind::Static);
    // Index buffer 0 holds strips, the next one lists.
    let strip_ib = has_static.then_some(0);
    let list_ib = has_lists.then_some(usize::from(has_static));
    let mut aabb = Aabb::default();
    for r in &records {
        let group = &groups[&(r.kind, r.format)];
        let slots = buffer_slots[&((r.kind, r.format), r.buffer)];
        let ib = if r.kind == RecordKind::Static {
            strip_ib
        } else {
            list_ib
        }
        .expect("index buffer of the primitive exists");
        let data = &mut index_data[usize::from(r.kind != RecordKind::Static)];
        let first = data.len() / 2;
        for &i in &r.indices {
            data.extend_from_slice(&i.to_le_bytes());
        }
        let mut raw = match &r.template {
            Some(t) => t.raw.clone(),
            None => default_raw(r.kind),
        };
        let mut rec = RecordImage {
            raw: std::mem::take(&mut raw),
            name: r.name.clone(),
            bounds: vec![0; 0x80],
            palette: r.palette.clone(),
            vertices: DataRef::None,
            indices: DataRef::Buffer {
                buffer: ib,
                offset: (2 * first) as u32,
            },
            grid: None,
            vertex_layer_bias: None,
            morph_targets: None,
            morph_deltas: None,
        };
        let static_ = r.kind == RecordKind::Static;
        rec.set_u8(4, if static_ { 2 } else { 1 });
        rec.set_u8(5, if static_ { 2 } else { 1 });
        rec.set_u8(
            6,
            match r.kind {
                RecordKind::Static => 0xFF,
                RecordKind::Rigid => 0,
                RecordKind::Skinned => r.influences,
            },
        );
        rec.set_u8(7, r.format);
        // `LowLODOnly` follows the vertex format (11, 12), never the
        // template: a format-11 template passing it on to a format-4 record
        // gave stretched triangles in the game (stage 24). `TriSort` needs a
        // sort bias per vertex and a sorted index buffer, which the builder
        // does not write, so it does not pass on either.
        let mut flags = rec.u32_at(8) & !RENDER_FLAG_TRI_SORT;
        if matches!(r.format, 11 | 12) {
            flags |= RENDER_FLAG_LOW_LOD_ONLY;
        } else {
            flags &= !RENDER_FLAG_LOW_LOD_ONLY;
        }
        rec.set_u32(8, flags);
        rec.set_u32(0x0C, r.material_hash);
        let sphere = bounding_sphere(&r.positions);
        for (k, v) in sphere.iter().enumerate() {
            rec.set_f32(0x10 + 4 * k, *v);
        }
        rec.set_u8(
            0x24,
            if r.kind == RecordKind::Rigid {
                bone_u8(r.bone)?
            } else {
                0xFF
            },
        );
        rec.set_u8(0x25, r.palette.len() as u8);
        rec.set_u8(0x26, group.stride0 as u8);
        rec.set_u8(0x27, group.stride1 as u8);
        // Visibility index (`VisIdx`): 0xFFFE in every record of the game.
        rec.set_u16(0x2A, 0xFFFE);
        // Morph count (u16) and custom opaque id (u16, always 0).
        rec.set_u32(0x2C, 0);
        rec.set_u32(0x30, r.indices.len() as u32);
        rec.set_u32(0x34, first as u32);
        rec.set_u16(0x38, r.vertex_count as u16);
        // Vertex offset of the record in the dynamic (CPU-skinned) vertex
        // data; equal to the first vertex (`0x40`) in all 157 229 records of
        // the game. Left from the template, CPU skinning wrote another
        // record's range and the record drew uninitialised vertices.
        rec.set_u16(0x3A, r.min as u16);
        rec.set_u16(0x3C, if static_ { 5 } else { 4 });
        let primitive = if static_ {
            Primitive::TriangleStrip
        } else {
            Primitive::TriangleList
        };
        rec.set_u16(0x3E, primitive_count(primitive, r.indices.len()));
        rec.set_u16(0x40, r.min as u16);
        rec.set_u16(0x42, (r.min + r.vertex_count - 1) as u16);
        for at in (0x44..0xA0).step_by(4) {
            rec.set_u32(at, 0);
        }
        rec.set_u32(0x60, ib as u32);
        rec.set_u32(
            0x64,
            if r.kind == RecordKind::Skinned {
                u32::MAX
            } else {
                slots.gpu0 as u32
            },
        );
        rec.set_u32(
            0x68,
            if r.kind == RecordKind::Skinned {
                slots.cpu0 as u32
            } else {
                u32::MAX
            },
        );
        rec.set_u32(
            0x6C,
            if group.stride1 > 0 {
                slots.gpu1 as u32
            } else {
                u32::MAX
            },
        );
        rec.set_u32(0x70, u32::MAX);
        // Tint colour (`TintRGBA`), white in every record of the game.
        for at in (0xA0..0xB0).step_by(4) {
            rec.set_f32(at, 1.0);
        }
        let mut b = Aabb::default();
        for p in &r.positions {
            b.add(*p);
            aabb.add(*p);
        }
        rec.bounds = record_bounds(r.kind, &b);
        rec.vertices = match r.kind {
            RecordKind::Skinned => DataRef::Inline(r.cpu_source.clone()),
            _ => DataRef::Buffer {
                buffer: slots.list0,
                offset: (r.min * group.stride0) as u32,
            },
        };
        if static_ {
            let strip: Vec<u16> = r.indices.clone();
            rec.grid = Some(Grid::build(&r.positions, &strip, r.min as u16, &r.runs).to_bytes());
        }
        out_records.push(rec);
    }
    for (k, prim) in [(0usize, strip_ib), (1, list_ib)] {
        if prim.is_some() {
            let data = std::mem::take(&mut index_data[k]);
            index_buffers.push(BufferImage {
                size: data.len() as u32,
                count: (data.len() / 2) as u32,
                stride: 2,
                format: if k == 0 { 2 } else { 1 },
                flags: 0,
                data,
            });
        }
    }
    let all_positions: Vec<[f32; 3]> = records.iter().flat_map(|r| r.positions.clone()).collect();
    let sphere = bounding_sphere(&all_positions);
    let image = MeshImage {
        name: options.name.clone(),
        platform: options.template.map_or(3, |t| t.platform),
        bounding_sphere: sphere,
        aabb_min: [aabb.min[0], aabb.min[1], aabb.min[2], 0.0],
        aabb_max: [aabb.max[0], aabb.max[1], aabb.max[2], 0.0],
        bones: bone_table,
        vertex_buffers,
        index_buffers,
        records: out_records,
        has_volatile: true,
    };
    Ok((image, warnings))
}

fn bone_u8(bone: u16) -> Result<u8> {
    u8::try_from(bone)
        .map_err(|_| Error::InvalidArgument(format!("skeleton bone {bone} above 255")))
}

#[derive(Debug, Clone, Copy, Default)]
struct BufferSlots {
    list0: usize,
    gpu0: i32,
    cpu0: i32,
    gpu1: i32,
}

fn validate(part: &PartGeometry) -> Result<()> {
    let n = part.positions.len();
    let check = |len: Option<usize>, what: &str| -> Result<()> {
        match len {
            Some(l) if l != n => Err(Error::InvalidArgument(format!(
                "part `{}`: {l} {what} for {n} positions",
                part.name
            ))),
            _ => Ok(()),
        }
    };
    check(part.normals.as_ref().map(Vec::len), "normals")?;
    check(part.tangents.as_ref().map(Vec::len), "tangents")?;
    check(part.uv0.as_ref().map(Vec::len), "UVs")?;
    check(part.uv1.as_ref().map(Vec::len), "second UVs")?;
    check(part.colors.as_ref().map(Vec::len), "colours")?;
    check(part.skin.as_ref().map(Vec::len), "skin weights")?;
    if part.triangles.iter().flatten().any(|&i| i as usize >= n) {
        return Err(Error::InvalidArgument(format!(
            "part `{}`: a triangle uses a vertex that does not exist",
            part.name
        )));
    }
    if part.positions.iter().flatten().any(|v| !v.is_finite()) {
        return Err(Error::InvalidArgument(format!(
            "part `{}`: a position is not a finite number",
            part.name
        )));
    }
    Ok(())
}

/// The bone every vertex follows with full weight, if there is one.
fn single_bone(skin: &[([u16; 4], [f32; 4])]) -> Option<u16> {
    let mut bone = None;
    for (j, w) in skin {
        let mut main = None;
        for k in 0..4 {
            if w[k] > 1e-4 {
                if main.is_some_and(|m| m != j[k]) {
                    return None;
                }
                main = Some(j[k]);
            }
        }
        let m = main?;
        if bone.is_some_and(|b| b != m) {
            return None;
        }
        bone = Some(m);
    }
    bone
}

/// Attributes of a part completed for a format: missing normals are
/// computed, missing tangents derived from the UVs, missing UVs and
/// colours filled with defaults.
struct Filled {
    normals: Vec<[f32; 3]>,
    tangents: Vec<[f32; 4]>,
    uv0: Vec<[f32; 2]>,
    uv1: Vec<[f32; 2]>,
    colors: Vec<[u8; 4]>,
}

fn fill_attributes(part: &PartGeometry, format: u8, warnings: &mut Vec<String>) -> Filled {
    let (cpu, gpu) = mesh::declarations(format).expect("format checked");
    let uses = |u: Usage| gpu.iter().chain(cpu.unwrap_or(&[])).any(|e| e.3 == u);
    let n = part.positions.len();
    let normals = match &part.normals {
        Some(n) => n.iter().map(|v| normalize(*v)).collect(),
        None => {
            if uses(Usage::Normal) {
                warnings.push(format!("part `{}`: normals computed", part.name));
            }
            smooth_normals(&part.positions, &part.triangles)
        }
    };
    let uv0 = match &part.uv0 {
        Some(uv) => uv.clone(),
        None => {
            if uses(Usage::Uv0) {
                warnings.push(format!("part `{}`: no UVs, zeros written", part.name));
            }
            vec![[0.0; 2]; n]
        }
    };
    let tangents = match &part.tangents {
        Some(t) => t.clone(),
        None => tangents_from_uv(&part.positions, &normals, &uv0, &part.triangles),
    };
    let uv1 = part.uv1.clone().unwrap_or_else(|| uv0.clone());
    let colors = part
        .colors
        .clone()
        .unwrap_or_else(|| vec![[255, 255, 255, 255]; n]);
    Filled {
        normals,
        tangents,
        uv0,
        uv1,
        colors,
    }
}

/// A piece of a part that fits one record.
struct Chunk {
    triangles: Vec<[u32; 3]>,
    /// Vertices of the part used by the chunk, in local order.
    vertices: Vec<u32>,
    /// Skeleton bones of the palette (skinned records).
    bones: Vec<u16>,
}

/// Splits a part so that every chunk has at most 65 535 vertices and, for
/// skinned parts, at most [`MAX_PALETTE`] palette bones.
fn split(part: &PartGeometry, kind: RecordKind, warnings: &mut Vec<String>) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut current = Chunk {
        triangles: Vec::new(),
        vertices: Vec::new(),
        bones: Vec::new(),
    };
    let mut local: HashMap<u32, u32> = HashMap::new();
    let bones_of = |v: u32| -> Vec<u16> {
        match (&part.skin, kind) {
            (Some(skin), RecordKind::Skinned) => {
                let (j, w) = skin[v as usize];
                (0..4).filter(|&k| w[k] > 0.0).map(|k| j[k]).collect()
            }
            _ => Vec::new(),
        }
    };
    for t in &part.triangles {
        let new_vertices = t.iter().filter(|v| !local.contains_key(v)).count();
        let mut new_bones: Vec<u16> = t
            .iter()
            .flat_map(|&v| bones_of(v))
            .filter(|b| !current.bones.contains(b))
            .collect();
        new_bones.sort_unstable();
        new_bones.dedup();
        if !current.triangles.is_empty()
            && (current.vertices.len() + new_vertices > MAX_VERTICES - 2
                || current.bones.len() + new_bones.len() > MAX_PALETTE)
        {
            chunks.push(std::mem::replace(
                &mut current,
                Chunk {
                    triangles: Vec::new(),
                    vertices: Vec::new(),
                    bones: Vec::new(),
                },
            ));
            local.clear();
            new_bones = t.iter().flat_map(|&v| bones_of(v)).collect();
            new_bones.sort_unstable();
            new_bones.dedup();
        }
        current.bones.extend(new_bones);
        let mut lt = [0u32; 3];
        for (k, &v) in t.iter().enumerate() {
            lt[k] = *local.entry(v).or_insert_with(|| {
                current.vertices.push(v);
                (current.vertices.len() - 1) as u32
            });
        }
        current.triangles.push(lt);
    }
    if !current.triangles.is_empty() {
        chunks.push(current);
    }
    if chunks.len() > 1 {
        warnings.push(format!(
            "part `{}` split into {} records ({})",
            part.name,
            chunks.len(),
            if kind == RecordKind::Skinned {
                "palette or vertex limit"
            } else {
                "vertex limit"
            }
        ));
    }
    chunks
}

/// A record before the buffers are numbered.
struct PendingRecord {
    kind: RecordKind,
    format: u8,
    template: Option<RecordImage>,
    name: String,
    material_hash: u32,
    bone: u16,
    influences: u8,
    palette: Vec<(u8, u8)>,
    buffer: usize,
    min: usize,
    vertex_count: usize,
    positions: Vec<[f32; 3]>,
    /// Absolute indices: a strip (static) or a list.
    indices: Vec<u16>,
    /// Strip positions where runs start (static records).
    runs: Vec<usize>,
    cpu_source: Vec<u8>,
}

struct GroupBuffer {
    count: usize,
    stream0: Vec<u8>,
    stream1: Vec<u8>,
}

/// Records of one kind and format share vertex buffers.
struct Group {
    kind: RecordKind,
    format: u8,
    /// Declaration of the buffer streams.
    declaration: &'static [mesh::Element],
    stride0: usize,
    stride1: usize,
    buffers: Vec<GroupBuffer>,
}

impl Group {
    fn new(kind: RecordKind, format: u8) -> Result<Self> {
        let (cpu, gpu) = mesh::declarations(format)
            .ok_or_else(|| Error::InvalidArgument(format!("unknown vertex format {format}")))?;
        let declaration = match kind {
            RecordKind::Static => gpu,
            _ => cpu.ok_or_else(|| {
                Error::InvalidArgument(format!("format {format} has no CPU declaration"))
            })?,
        };
        // Rigid and skinned records have a 20-byte stream 0 (position,
        // normal, tangent), also for formats without a tangent; the low-LOD
        // format has its own 12 bytes (half-float position, normal).
        let stride0 = match kind {
            RecordKind::Static => mesh::extent(declaration, 0),
            RecordKind::Skinned if format == LOW_LOD => mesh::extent(declaration, 0),
            _ => 20,
        };
        Ok(Self {
            kind,
            format,
            declaration,
            stride0,
            stride1: mesh::extent(declaration, 1),
            buffers: Vec::new(),
        })
    }

    fn add(
        &mut self,
        part: &PartGeometry,
        filled: &Filled,
        plan: &Plan<'_>,
        chunk: &Chunk,
        palette: &[(u8, u8)],
    ) -> Result<PendingRecord> {
        let n = chunk.vertices.len();
        if self
            .buffers
            .last()
            .is_none_or(|b| b.count + n > MAX_VERTICES)
        {
            self.buffers.push(GroupBuffer {
                count: 0,
                stream0: Vec::new(),
                stream1: Vec::new(),
            });
        }
        let buffer_index = self.buffers.len() - 1;
        let buffer = self.buffers.last_mut().expect("pushed above");
        let min = buffer.count;
        let mut positions = Vec::with_capacity(n);
        let mut cpu_source = Vec::new();
        let mut influences = 1u8;
        for &v in &chunk.vertices {
            let v = v as usize;
            let p = part.positions[v];
            positions.push(p);
            let mut s0 = vec![0u8; self.stride0];
            let mut s1 = vec![0u8; self.stride1];
            for &(stream, offset, ty, usage) in self.declaration {
                let target = if stream == 0 { &mut s0 } else { &mut s1 };
                let bytes = &mut target[offset..offset + ty.size()];
                match usage {
                    Usage::Position => put_floats(bytes, ty, &p),
                    Usage::Normal => put_unorm(bytes, &extend(filled.normals[v], 0.0)),
                    Usage::Tangent => put_unorm(bytes, &filled.tangents[v]),
                    Usage::Color => bytes.copy_from_slice(&filled.colors[v]),
                    Usage::Uv0 => put_floats(bytes, ty, &filled.uv0[v]),
                    Usage::Uv1 => put_floats(bytes, ty, &filled.uv1[v]),
                }
            }
            if self.kind != RecordKind::Static
                && self.stride0 >= 20
                && !self
                    .declaration
                    .iter()
                    .any(|e| e.0 == 0 && e.3 == Usage::Tangent)
            {
                put_unorm(&mut s0[16..20], &filled.tangents[v]);
            }
            if self.kind == RecordKind::Skinned {
                let (joints, weights) = part.skin.as_ref().expect("skinned part")[v];
                let source = if self.format == LOW_LOD {
                    skinned_source_low_lod(
                        p,
                        filled.normals[v],
                        joints,
                        weights,
                        &chunk.bones,
                        &mut influences,
                    )
                    .to_vec()
                } else {
                    skinned_source(
                        p,
                        filled.normals[v],
                        filled.tangents[v],
                        joints,
                        weights,
                        &chunk.bones,
                        &mut influences,
                    )
                    .to_vec()
                };
                cpu_source.extend_from_slice(&source);
            }
            buffer.stream0.extend_from_slice(&s0);
            buffer.stream1.extend_from_slice(&s1);
        }
        buffer.count += n;
        let base = min as u32;
        let (indices, runs) = match self.kind {
            RecordKind::Static => strip(&chunk.triangles, base),
            _ => (
                chunk
                    .triangles
                    .iter()
                    .flatten()
                    .map(|&i| (i + base) as u16)
                    .collect(),
                Vec::new(),
            ),
        };
        Ok(PendingRecord {
            kind: self.kind,
            format: self.format,
            template: plan.template.cloned(),
            name: part.name.clone(),
            material_hash: part.material_hash,
            bone: plan.bone,
            influences,
            palette: palette.to_vec(),
            buffer: buffer_index,
            min,
            vertex_count: n,
            positions,
            indices,
            runs,
            cpu_source,
        })
    }
}

/// Influences sorted by weight, heaviest first, at most `limit`, quantised
/// to u16 weights summing to 65535.
fn quantised_influences(joints: [u16; 4], weights: [f32; 4], limit: usize) -> Vec<(u16, u16)> {
    let mut pairs: Vec<(u16, f32)> = (0..4)
        .filter(|&k| weights[k] > 0.0)
        .map(|k| (joints[k], weights[k]))
        .collect();
    if pairs.is_empty() {
        // A vertex without influences would collapse to the origin.
        return vec![(joints[0], 65535)];
    }
    pairs.sort_by(|a, b| b.1.total_cmp(&a.1));
    pairs.truncate(limit);
    let sum: f32 = pairs.iter().map(|p| p.1).sum();
    let mut q: Vec<u16> = pairs
        .iter()
        .map(|p| (p.1 / sum * 65535.0).round() as u16)
        .collect();
    let total: i32 = q.iter().map(|&x| x as i32).sum();
    if let Some(first) = q.first_mut() {
        *first = (*first as i32 + 65535 - total).clamp(0, 65535) as u16;
    }
    pairs.iter().map(|p| p.0).zip(q).collect()
}

/// The 16-byte inline source of a low-LOD (format 11) CPU-skinned vertex:
/// f16 × 3 position, two palette indices in the fourth half, DEC3N normal,
/// two u16 weights summing to 65535. Lighter influences are dropped.
fn skinned_source_low_lod(
    p: [f32; 3],
    normal: [f32; 3],
    joints: [u16; 4],
    weights: [f32; 4],
    palette: &[u16],
    influences: &mut u8,
) -> [u8; 16] {
    let mut out = [0u8; 16];
    for k in 0..3 {
        out[2 * k..2 * k + 2].copy_from_slice(&f32_to_f16(p[k]).to_le_bytes());
    }
    out[8..12].copy_from_slice(&dec3n(normal, 0).to_le_bytes());
    let pairs = quantised_influences(joints, weights, 2);
    *influences = (*influences).max(pairs.len() as u8);
    for (k, (bone, w)) in pairs.iter().enumerate() {
        let index = palette.iter().position(|b| b == bone).unwrap_or(0);
        out[6 + k] = index as u8;
        out[12 + 2 * k..14 + 2 * k].copy_from_slice(&w.to_le_bytes());
    }
    out
}

/// The 32-byte inline source of a CPU-skinned vertex: f32 × 3 position,
/// DEC3N normal and tangent, palette indices, u16 weights summing to 65535.
fn skinned_source(
    p: [f32; 3],
    normal: [f32; 3],
    tangent: [f32; 4],
    joints: [u16; 4],
    weights: [f32; 4],
    palette: &[u16],
    influences: &mut u8,
) -> [u8; 32] {
    let mut out = [0u8; 32];
    for k in 0..3 {
        out[4 * k..4 * k + 4].copy_from_slice(&p[k].to_le_bytes());
    }
    out[12..16].copy_from_slice(&dec3n(normal, 0).to_le_bytes());
    let sign = if tangent[3] < 0.0 { 2 } else { 0 };
    out[16..20].copy_from_slice(&dec3n([tangent[0], tangent[1], tangent[2]], sign).to_le_bytes());
    let pairs = quantised_influences(joints, weights, 4);
    *influences = (*influences).max(pairs.len() as u8);
    for (k, (bone, w)) in pairs.iter().enumerate() {
        let index = palette.iter().position(|b| b == bone).unwrap_or(0);
        out[20 + k] = index as u8;
        out[24 + 2 * k..26 + 2 * k].copy_from_slice(&w.to_le_bytes());
    }
    out
}

/// Direct3D `DEC3N`: three signed 10-bit fields (× 511) and two bits of w.
fn dec3n(v: [f32; 3], w: u32) -> u32 {
    let field = |x: f32| ((x.clamp(-1.0, 1.0) * 511.0).round() as i32 as u32) & 0x3FF;
    field(v[0]) | field(v[1]) << 10 | field(v[2]) << 20 | (w & 3) << 30
}

fn put_floats(bytes: &mut [u8], ty: ElementType, values: &[f32]) {
    match ty {
        ElementType::F16x2 | ElementType::F16x4 => {
            for (k, chunk) in bytes.chunks_exact_mut(2).enumerate() {
                let v = values
                    .get(k)
                    .copied()
                    .unwrap_or(if k == 3 { 1.0 } else { 0.0 });
                chunk.copy_from_slice(&f32_to_f16(v).to_le_bytes());
            }
        }
        _ => {
            for (k, chunk) in bytes.chunks_exact_mut(4).enumerate() {
                chunk.copy_from_slice(&values.get(k).copied().unwrap_or(0.0).to_le_bytes());
            }
        }
    }
}

/// UBYTE4N as the game decodes it: `b / 255 · 2 − 1`.
fn put_unorm(bytes: &mut [u8], v: &[f32; 4]) {
    for (b, x) in bytes.iter_mut().zip(v) {
        *b = ((x.clamp(-1.0, 1.0) + 1.0) * 0.5 * 255.0).round() as u8;
    }
}

fn extend(v: [f32; 3], w: f32) -> [f32; 4] {
    [v[0], v[1], v[2], w]
}

/// IEEE 754 single to half precision, rounded to nearest.
pub fn f32_to_f16(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xFF) as i32;
    let mantissa = bits & 0x7F_FFFF;
    if exp == 0xFF {
        return sign | 0x7C00 | if mantissa != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 0x1F {
        return sign | 0x7C00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = (mantissa | 0x80_0000) >> (1 - e);
        return sign | ((m + 0x1000) >> 13) as u16;
    }
    let half = sign | ((e as u32) << 10) as u16 | (mantissa >> 13) as u16;
    // Round to nearest; a carry into the exponent is still correct.
    if mantissa & 0x1000 != 0 {
        half + 1
    } else {
        half
    }
}

/// The primitive count of a draw record (u16 at 0x3E): the number of
/// indices of a triangle strip, of triangles of a list, modulo 65 536 as the
/// game stores it (one strip of `miami/local` is longer than 65 535 indices).
pub fn primitive_count(primitive: Primitive, indices: usize) -> u16 {
    let count = match primitive {
        Primitive::TriangleStrip => indices,
        Primitive::TriangleList => indices / 3,
    };
    count as u16
}

/// A triangle strip of `triangles` (local indices + `base`) and the strip
/// positions where runs start. Consecutive triangles that continue the
/// strip are appended with one index; otherwise the strip restarts with
/// degenerate triangles so that every run starts at an even position (as
/// in the game files, which keeps the winding).
pub fn strip(triangles: &[[u32; 3]], base: u32) -> (Vec<u16>, Vec<usize>) {
    const MAX_RUN: usize = 64;
    let mut out: Vec<u16> = Vec::with_capacity(triangles.len() * 4);
    let mut runs = Vec::new();
    let mut run_length = 0;
    for t in triangles {
        let t = t.map(|i| (i + base) as u16);
        if t[0] == t[1] || t[1] == t[2] || t[0] == t[2] {
            continue;
        }
        let len = out.len();
        if len >= 3 && run_length < MAX_RUN {
            // The next strip triangle is (s[len-2], s[len-1], x), reversed at odd positions.
            let (a, b) = (out[len - 2], out[len - 1]);
            let q = len - 2;
            let expected = |x: u16| {
                if q.is_multiple_of(2) {
                    [a, b, x]
                } else {
                    [b, a, x]
                }
            };
            for r in 0..3 {
                let rotated = [t[r], t[(r + 1) % 3], t[(r + 2) % 3]];
                let x = rotated[2];
                if expected(x) == rotated {
                    out.push(x);
                    run_length += 1;
                    break;
                }
            }
            if out.len() > len {
                continue;
            }
        }
        if len == 0 {
            runs.push(0);
            out.extend_from_slice(&t);
        } else {
            // c c d d [d] e f: degenerate joins, the triangle at an even position.
            let last = out[len - 1];
            out.extend_from_slice(&[last, t[0], t[0]]);
            if (out.len() - 1) % 2 == 1 {
                out.push(t[0]);
            }
            runs.push(out.len() - 1);
            out.extend_from_slice(&t[1..]);
        }
        run_length = 1;
    }
    (out, runs)
}

#[derive(Debug, Clone, Copy)]
struct Aabb {
    min: [f32; 3],
    max: [f32; 3],
}

impl Default for Aabb {
    fn default() -> Self {
        Self {
            min: [f32::INFINITY; 3],
            max: [f32::NEG_INFINITY; 3],
        }
    }
}

impl Aabb {
    fn add(&mut self, p: [f32; 3]) {
        for ((lo, hi), v) in self.min.iter_mut().zip(&mut self.max).zip(p) {
            *lo = lo.min(v);
            *hi = hi.max(v);
        }
    }
}

/// 8 corners with w = 1, x fastest (static) or the AABB with w = 0 (others).
fn record_bounds(kind: RecordKind, b: &Aabb) -> Vec<u8> {
    let mut out = Vec::with_capacity(0x80);
    let mut put = |v: [f32; 4]| {
        for x in v {
            out.extend_from_slice(&x.to_le_bytes());
        }
    };
    if kind == RecordKind::Static {
        for z in [b.min[2], b.max[2]] {
            for y in [b.min[1], b.max[1]] {
                for x in [b.min[0], b.max[0]] {
                    put([x, y, z, 1.0]);
                }
            }
        }
    } else {
        put([b.min[0], b.min[1], b.min[2], 0.0]);
        put([b.max[0], b.max[1], b.max[2], 0.0]);
    }
    out.resize(0x80, 0);
    out
}

/// A bounding sphere: the box centre and the farthest point.
fn bounding_sphere(points: &[[f32; 3]]) -> [f32; 4] {
    let mut b = Aabb::default();
    for p in points {
        b.add(*p);
    }
    if points.is_empty() {
        return [0.0; 4];
    }
    let c = [0, 1, 2].map(|a| (b.min[a] + b.max[a]) * 0.5);
    let r = points
        .iter()
        .map(|p| ((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2) + (p[2] - c[2]).powi(2)).sqrt())
        .fold(0.0f32, f32::max);
    [c[0], c[1], c[2], r]
}

/// Raw bytes of a record when no template record of its kind exists.
fn default_raw(_kind: RecordKind) -> Vec<u8> {
    vec![0; RECORD_SIZE]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 1e-12 {
        v.map(|x| x / len)
    } else {
        [0.0, 1.0, 0.0]
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Area-weighted vertex normals.
pub fn smooth_normals(positions: &[[f32; 3]], triangles: &[[u32; 3]]) -> Vec<[f32; 3]> {
    let mut acc = vec![[0f32; 3]; positions.len()];
    for t in triangles {
        let [a, b, c] = t.map(|i| positions[i as usize]);
        let n = cross(sub(b, a), sub(c, a));
        for &i in t {
            for k in 0..3 {
                acc[i as usize][k] += n[k];
            }
        }
    }
    acc.into_iter().map(normalize).collect()
}

/// Per-vertex tangents from UV gradients (Lengyel), orthogonalised against the normal.
pub fn tangents_from_uv(
    positions: &[[f32; 3]],
    normals: &[[f32; 3]],
    uv: &[[f32; 2]],
    triangles: &[[u32; 3]],
) -> Vec<[f32; 4]> {
    let n = positions.len();
    let mut tan = vec![[0f32; 3]; n];
    let mut bit = vec![[0f32; 3]; n];
    for t in triangles {
        let [i0, i1, i2] = t.map(|i| i as usize);
        let (e1, e2) = (
            sub(positions[i1], positions[i0]),
            sub(positions[i2], positions[i0]),
        );
        let (du1, dv1) = (uv[i1][0] - uv[i0][0], uv[i1][1] - uv[i0][1]);
        let (du2, dv2) = (uv[i2][0] - uv[i0][0], uv[i2][1] - uv[i0][1]);
        let det = du1 * dv2 - du2 * dv1;
        if det.abs() < 1e-12 {
            continue;
        }
        let r = 1.0 / det;
        let s = [0, 1, 2].map(|k| (e1[k] * dv2 - e2[k] * dv1) * r);
        let b = [0, 1, 2].map(|k| (e2[k] * du1 - e1[k] * du2) * r);
        for i in [i0, i1, i2] {
            for k in 0..3 {
                tan[i][k] += s[k];
                bit[i][k] += b[k];
            }
        }
    }
    (0..n)
        .map(|i| {
            let nrm = normals[i];
            let t = tan[i];
            let d = dot(nrm, t);
            let mut o = normalize([t[0] - nrm[0] * d, t[1] - nrm[1] * d, t[2] - nrm[2] * d]);
            if dot(t, t) < 1e-20 {
                // No UV gradient: any vector perpendicular to the normal.
                let axis = if nrm[0].abs() < 0.9 {
                    [1.0, 0.0, 0.0]
                } else {
                    [0.0, 1.0, 0.0]
                };
                o = normalize(cross(cross(nrm, axis), nrm));
            }
            let w = if dot(cross(nrm, o), bit[i]) < 0.0 {
                -1.0
            } else {
                1.0
            };
            [o[0], o[1], o[2], w]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::mesh::Model;

    fn quad(name: &str, x: f32) -> PartGeometry {
        PartGeometry {
            name: name.into(),
            material_hash: 0x1234,
            positions: vec![
                [x, 0.0, 0.0],
                [x + 1.0, 0.0, 0.0],
                [x, 1.0, 0.0],
                [x + 1.0, 1.0, 0.0],
            ],
            uv0: Some(vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]]),
            triangles: vec![[0, 1, 2], [2, 1, 3]],
            ..Default::default()
        }
    }

    /// A triangle rotated so that its smallest index comes first.
    fn canon(t: [u32; 3]) -> [u32; 3] {
        let r = (0..3).min_by_key(|&r| t[r]).unwrap();
        [t[r], t[(r + 1) % 3], t[(r + 2) % 3]]
    }

    fn decoded(image: &MeshImage) -> Model {
        let (mesh, volatile) = image.encode().unwrap();
        Model::parse(&mesh, &volatile).unwrap()
    }

    /// The bitangent sign goes into the w bits of the tangent as the game
    /// stores it: 0 for +1, 2 for −1 (never 1 or 3).
    #[test]
    fn skinned_tangent_sign_is_0_or_2() {
        let w = |sign: f32| {
            let mut influences = 0;
            let v = skinned_source(
                [0.0; 3],
                [0.0, 0.0, 1.0],
                [1.0, 0.0, 0.0, sign],
                [0; 4],
                [1.0, 0.0, 0.0, 0.0],
                &[0],
                &mut influences,
            );
            u32::from_le_bytes(v[16..20].try_into().unwrap()) >> 30
        };
        assert_eq!((w(1.0), w(-1.0)), (0, 2));
    }

    #[test]
    fn primitive_counts_wrap_like_the_game() {
        assert_eq!(primitive_count(Primitive::TriangleStrip, 10), 10);
        assert_eq!(primitive_count(Primitive::TriangleList, 30), 10);
        assert_eq!(primitive_count(Primitive::TriangleStrip, 65_535), 65_535);
        // Stored modulo 65 536, not clamped to 65 535.
        assert_eq!(primitive_count(Primitive::TriangleStrip, 65_536), 0);
        assert_eq!(primitive_count(Primitive::TriangleStrip, 70_000), 4_464);
        assert_eq!(primitive_count(Primitive::TriangleList, 3 * 65_537), 1);
    }

    #[test]
    fn strips_keep_triangles_and_winding() {
        let tris = [[0, 1, 2], [2, 1, 3], [4, 5, 6], [5, 7, 6], [8, 9, 10]];
        let (s, runs) = strip(&tris, 10);
        assert!(runs.iter().all(|r| r % 2 == 0));
        assert_eq!(runs[0], 0);
        let mut got = Vec::new();
        for (q, w) in s.windows(3).enumerate() {
            if w[0] == w[1] || w[1] == w[2] || w[0] == w[2] {
                continue;
            }
            let t = if q % 2 == 0 {
                [w[0], w[1], w[2]]
            } else {
                [w[0], w[2], w[1]]
            };
            got.push(t.map(|i| i as u32 - 10));
        }
        // Same triangles with the same orientation (up to rotation).
        let want: Vec<_> = tris.iter().map(|&t| canon(t)).collect();
        let got: Vec<_> = got.into_iter().map(canon).collect();
        assert_eq!(got, want);
    }

    #[test]
    fn static_parts_round_trip_through_the_reader() {
        let parts = [quad("a", 0.0), quad("b", 5.0)];
        let (image, warnings) = build(
            &parts,
            &BuildOptions {
                name: "test".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            warnings.iter().all(|w| w.contains("normals computed")),
            "{warnings:?}"
        );
        let model = decoded(&image);
        assert_eq!(model.records.len(), 2);
        assert_eq!(model.name, "test");
        for (r, part) in model.records.iter().zip(&parts) {
            assert_eq!(r.material_hash, 0x1234);
            assert_eq!(r.format, 1);
            let v = model.vertices(r).unwrap();
            assert_eq!(v.positions, part.positions);
            let uv = v.uv0.unwrap();
            assert_eq!(&uv, part.uv0.as_ref().unwrap());
            for n in v.normals.unwrap() {
                assert!((n[2] - 1.0).abs() < 0.01, "{n:?}");
            }
            let tris: Vec<_> = model.triangles(r).unwrap().into_iter().map(canon).collect();
            let want: Vec<_> = part.triangles.iter().map(|&t| canon(t)).collect();
            assert_eq!(tris, want);
        }
        assert_eq!(model.aabb_max, [6.0, 1.0, 0.0]);
        // The encoded image parses back to the same structure.
        let (mesh, vol) = image.encode().unwrap();
        assert_eq!(MeshImage::parse(&mesh, &vol).unwrap(), image);
    }

    #[test]
    fn skinned_parts_split_by_palette() {
        // 60 triangles, each on its own bone: two records of 42 and 18 bones.
        let mut part = PartGeometry {
            name: "body".into(),
            material_hash: 7,
            ..Default::default()
        };
        let mut skin = Vec::new();
        for b in 0..60u16 {
            let base = part.positions.len() as u32;
            for k in 0..3 {
                part.positions.push([b as f32, k as f32, 0.0]);
                skin.push(([b, (b + 1) % 60, 0, 0], [0.75, 0.25, 0.0, 0.0]));
            }
            part.triangles.push([base, base + 1, base + 2]);
        }
        part.skin = Some(skin);
        let skeleton: Vec<u32> = (0..60).map(|i| 0x100 + i).collect();
        let (image, warnings) = build(
            &[part.clone()],
            &BuildOptions {
                name: "skinned".into(),
                skeleton: Some(skeleton.clone()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            warnings.iter().filter(|w| w.contains("split")).count(),
            1,
            "{warnings:?}"
        );
        let model = decoded(&image);
        assert!(model.records.len() >= 2);
        let mut seen = 0;
        for r in &model.records {
            assert!(r.palette.len() <= MAX_PALETTE);
            assert_eq!(r.influences, 2);
            let v = model.vertices(r).unwrap();
            let skin = crate::export::gltf::vertex_skin(&r.palette, r.rigid_bone, &v, 60);
            for (k, (j, w)) in skin.iter().enumerate() {
                let p = v.positions[k];
                let b = p[0] as u16;
                assert_eq!(j[0], b);
                assert_eq!(j[1], (b + 1) % 60);
                assert!((w[0] - 0.75).abs() < 1e-3);
            }
            for &(bone, table) in &r.palette {
                assert_eq!(
                    model.bones[table as usize],
                    (skeleton[bone as usize], bone as u32)
                );
            }
            seen += model.triangles(r).unwrap().len();
        }
        assert_eq!(seen, 60);
    }

    #[test]
    fn single_bone_parts_become_rigid_records() {
        let mut part = quad("hat", 0.0);
        part.skin = Some(vec![([5, 0, 0, 0], [1.0, 0.0, 0.0, 0.0]); 4]);
        let (image, _) = build(
            &[part],
            &BuildOptions {
                name: "hat".into(),
                skeleton: Some((0..10).collect()),
                ..Default::default()
            },
        )
        .unwrap();
        let model = decoded(&image);
        let r = &model.records[0];
        assert_eq!(r.rigid_bone, Some(5));
        assert_eq!(r.palette, vec![(5, 0)]);
        assert_eq!(model.bones, vec![(5, 5)]);
        assert_eq!(model.vertices(r).unwrap().positions.len(), 4);
    }

    #[test]
    fn skinned_parts_keep_the_bone_table_of_the_template() {
        // The template's table holds the whole skeleton in its own order, as
        // the character meshes of the game do.
        let skeleton: Vec<u32> = (0..10).map(|i| 0x100 + i).collect();
        let mut template_part = quad("old", 0.0);
        template_part.skin = Some(vec![([1, 2, 0, 0], [0.5, 0.5, 0.0, 0.0]); 4]);
        let (mut template, _) = build(
            &[template_part],
            &BuildOptions {
                name: "old".into(),
                skeleton: Some(skeleton.clone()),
                ..Default::default()
            },
        )
        .unwrap();
        let order = [0u32, 1, 2, 5, 9, 3, 4, 8, 6, 7];
        template.bones = order.iter().map(|&b| (skeleton[b as usize], b)).collect();
        let mut part = quad("new", 0.0);
        part.skin = Some(vec![([7, 3, 0, 0], [0.75, 0.25, 0.0, 0.0]); 4]);
        let (image, _) = build(
            &[part],
            &BuildOptions {
                name: "new".into(),
                skeleton: Some(skeleton.clone()),
                template: Some(&template),
                ..Default::default()
            },
        )
        .unwrap();
        let model = decoded(&image);
        assert_eq!(model.bones, template.bones);
        let r = &model.records[0];
        assert_eq!(r.palette.len(), 2);
        for &(bone, table) in &r.palette {
            assert_eq!(order[table as usize], u32::from(bone));
        }
    }

    #[test]
    fn records_in_other_formats_drop_the_low_lod_flag_of_the_template() {
        let skeleton: Vec<u32> = (0..10).map(|i| 0x100 + i).collect();
        let mut part = quad("lod2", 0.0);
        part.skin = Some(vec![([1, 2, 0, 0], [0.5, 0.5, 0.0, 0.0]); 4]);
        let options = BuildOptions {
            name: "lod2".into(),
            skeleton: Some(skeleton),
            ..Default::default()
        };
        let (mut template, _) = build(&[part.clone()], &options).unwrap();
        // A LOD2 record of the game: format 11, flags 0x4001.
        template.records[0].set_u8(7, 11);
        template.records[0].set_u32(8, 0x4001);
        let (image, _) = build(
            &[part.clone()],
            &BuildOptions {
                template: Some(&template),
                format: Some(4),
                ..options.clone()
            },
        )
        .unwrap();
        let r = &image.records[0];
        assert_eq!(r.u8_at(7), 4);
        assert_eq!(r.u32_at(8), 0x0001);
        // In the template's own format the flags stay.
        let (image, _) = build(
            &[part],
            &BuildOptions {
                template: Some(&template),
                ..options
            },
        )
        .unwrap();
        let r = &image.records[0];
        assert_eq!(r.u8_at(7), 11);
        assert_eq!(r.u32_at(8), 0x4001);
    }

    #[test]
    fn records_take_their_vertex_offset_and_template_by_name() {
        let skeleton: Vec<u32> = (0..10).map(|i| 0x100 + i).collect();
        let skinned = |name: &str, x: f32| {
            let mut part = quad(name, x);
            part.skin = Some(vec![([1, 2, 0, 0], [0.5, 0.5, 0.0, 0.0]); 4]);
            part
        };
        let parts = [skinned("a", 0.0), skinned("b", 1.0), skinned("c", 2.0)];
        let options = BuildOptions {
            name: "jacket".into(),
            skeleton: Some(skeleton),
            ..Default::default()
        };
        let (mut template, _) = build(&parts, &options).unwrap();
        // Template records of one material with their own flags and stale
        // vertex offsets, as an edited mesh of the game would have.
        for (i, r) in template.records.iter_mut().enumerate() {
            r.set_u16(0x3A, 0);
            r.set_u32(8, [0x1, 0x10, 0x0][i]);
        }
        let reordered = [parts[2].clone(), parts[0].clone(), parts[1].clone()];
        let (image, _) = build(
            &reordered,
            &BuildOptions {
                template: Some(&template),
                ..options
            },
        )
        .unwrap();
        let mut firsts = Vec::new();
        for r in &image.records {
            assert_eq!(r.u16_at(0x3A), r.u16_at(0x40), "record {}", r.name);
            firsts.push(r.u16_at(0x40));
            let flags = match r.name.as_str() {
                "a" => 0x1,
                "b" => 0x0, // TriSort is not passed on
                _ => 0x0,
            };
            assert_eq!(r.u32_at(8), flags, "record {}", r.name);
        }
        assert_eq!(firsts, vec![0, 4, 8]);
    }

    #[test]
    fn a_vertex_without_influences_keeps_one() {
        assert_eq!(
            quantised_influences([7, 0, 0, 0], [0.0; 4], 4),
            vec![(7, 65535)]
        );
    }

    #[test]
    fn low_lod_skinned_records_round_trip_with_two_influences() {
        let skeleton: Vec<u32> = (0..10).map(|i| 0x100 + i).collect();
        let mut part = quad("lod2", 0.5);
        // Three influences: the lightest is dropped, the rest renormalised.
        part.skin = Some(vec![([3, 6, 8, 0], [0.6, 0.3, 0.1, 0.0]); 4]);
        let (image, _) = build(
            &[part.clone()],
            &BuildOptions {
                name: "lod2".into(),
                skeleton: Some(skeleton),
                format: Some(11),
                ..Default::default()
            },
        )
        .unwrap();
        let model = decoded(&image);
        let r = &model.records[0];
        assert_eq!(r.format, 11);
        assert_eq!((r.stride0, r.stride1), (12, 4));
        assert_eq!(r.influences, 2);
        let v = model.vertices(r).unwrap();
        for (k, p) in v.positions.iter().enumerate() {
            for c in 0..3 {
                assert!((p[c] - part.positions[k][c]).abs() < 1e-3, "{p:?}");
            }
        }
        let uv = v.uv0.as_ref().unwrap();
        assert_eq!(uv, part.uv0.as_ref().unwrap());
        let skin = crate::export::gltf::vertex_skin(&r.palette, r.rigid_bone, &v, 10);
        for (j, w) in skin {
            assert_eq!(&j[..2], &[3, 6]);
            assert!((w[0] - 2.0 / 3.0).abs() < 1e-3, "{w:?}");
            assert!((w[1] - 1.0 / 3.0).abs() < 1e-3, "{w:?}");
            assert_eq!(w[2], 0.0);
        }
    }

    #[test]
    fn half_floats_round_trip() {
        for v in [0.0f32, 1.0, -2.5, 0.333, 1000.0, 1e-5] {
            let back = mesh::f16_to_f32(f32_to_f16(v));
            assert!((back - v).abs() <= v.abs() * 1e-3 + 1e-4, "{v} -> {back}");
        }
    }
}

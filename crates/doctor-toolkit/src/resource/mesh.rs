//! `mesh` resources ("MGAE", version 42) and their `meshvolatile` companions.
//!
//! A `mesh` is a 32-bit memory image with a relocation table. Every pointer in
//! it is an offset into the concatenation `mesh ‖ meshvolatile`; the volatile
//! resource (the next resource in the stream, with the bitwise inverse id)
//! holds the second vertex stream of most buffers and has no header.
//! The layout is documented in `docs/formats/geometry.md`.

use crate::io::{self, Endian};
use crate::{Error, Result};

const MAGIC: u32 = 0x4541_474D;
const VERSION: u32 = 42;
const RECORD_SIZE: usize = 0xB0;
const BUFFER_SIZE: usize = 0x10;

/// Vertex and index buffer descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Buffer {
    pub size: u32,
    pub count: u32,
    pub stride: u16,
    /// Vertex format id for vertex buffers; 1 or 2 for index buffers.
    pub format: u8,
    /// Bit 0: CPU buffer without data; 2: data in the meshvolatile resource.
    pub flags: u8,
    pub ptr: u32,
}

impl Buffer {
    fn is_cpu(&self) -> bool {
        self.flags & 1 != 0
    }
}

/// Primitive topology of a draw record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Primitive {
    TriangleList,
    TriangleStrip,
}

/// Render flag `AlphaSort` (record 0x08): transparent glass sorted by depth
/// (2 154 records: `glass*_sort`, `window_sort`, shader `WindowGlass`).
pub const RENDER_FLAG_ALPHA_SORT: u32 = 0x0008;
/// Render flag `TriSort`: sorted triangles with a sort bias per vertex
/// (record 0x48) and a sorted CPU index buffer (0x70); hair and lashes, 61 records.
pub const RENDER_FLAG_TRI_SORT: u32 = 0x0010;
/// Render flag `DrawInSky`: the sky dome and clouds (32 records).
pub const RENDER_FLAG_DRAW_IN_SKY: u32 = 0x0020;
/// Render flag `DoNotCastShadow` (5 249 records, names with `_dncs_`).
pub const RENDER_FLAG_NO_SHADOW: u32 = 0x0800;
/// Render flag `DoNotRenderDepth`: overlays (decals), only records with an
/// overlay layer (`overlay_id` > 0), 20 829 records.
pub const RENDER_FLAG_NO_DEPTH: u32 = 0x1000;
/// Render flag `LowLODOnly`: every record of vertex formats 11 and 12 and no
/// other (28 805 records); the runtime picks the 16-byte skinning source by it.
pub const RENDER_FLAG_LOW_LOD_ONLY: u32 = 0x4000;
/// Render flag `CustomOpaque_0`: panorama buildings and skies (shader
/// `gf_pano`), 77 records.
pub const RENDER_FLAG_CUSTOM_OPAQUE_0: u32 = 0x8000;

/// Names of the render flags, from the comparison with another EARS editor; each
/// checked against the records that carry it. Bit 0x1 (1 381 records, by
/// their name "two UV sets") is left out: almost none of them has a second UV set.
pub const RENDER_FLAG_NAMES: [(u32, &str); 7] = [
    (RENDER_FLAG_ALPHA_SORT, "AlphaSort"),
    (RENDER_FLAG_TRI_SORT, "TriSort"),
    (RENDER_FLAG_DRAW_IN_SKY, "DrawInSky"),
    (RENDER_FLAG_NO_SHADOW, "DoNotCastShadow"),
    (RENDER_FLAG_NO_DEPTH, "DoNotRenderDepth"),
    (RENDER_FLAG_LOW_LOD_ONLY, "LowLODOnly"),
    (RENDER_FLAG_CUSTOM_OPAQUE_0, "CustomOpaque_0"),
];

/// The render flags as text: known names, then the other bits in hexadecimal,
/// e.g. `AlphaSort | LowLODOnly | 0x1`; `-` for none.
pub fn render_flag_names(flags: u32) -> String {
    let mut parts: Vec<String> = RENDER_FLAG_NAMES
        .iter()
        .filter(|(bit, _)| flags & bit != 0)
        .map(|(_, name)| (*name).to_owned())
        .collect();
    let known = RENDER_FLAG_NAMES.iter().fold(0, |all, (bit, _)| all | bit);
    if flags & !known != 0 {
        parts.push(format!("{:#x}", flags & !known));
    }
    if parts.is_empty() {
        "-".to_owned()
    } else {
        parts.join(" | ")
    }
}

/// One draw call: a range of indices with one material.
#[derive(Debug, Clone, PartialEq)]
pub struct DrawRecord {
    pub name: String,
    pub format: u8,
    /// Skin type (record 0x06): 0xFF static geometry, 0 every vertex on one
    /// rigid bone (`rigid_bone`), 1–4 most bones weighting one vertex (CPU
    /// skinning; equals the real maximum in all 1 932 such records).
    pub influences: u8,
    /// See [`RENDER_FLAG_NAMES`] and [`render_flag_names`].
    pub render_flags: u32,
    /// Overlay (decal) layer, record 0x28: 0 for ordinary geometry, 1–16 for
    /// layers drawn over it (`*_ov_N` in the record name).
    pub overlay_id: u8,
    /// CRC-32 of the lower-case material name.
    pub material_hash: u32,
    pub bounding_sphere: [f32; 4],
    /// Skeleton bone every vertex is bound to, for records without per-vertex weights.
    pub rigid_bone: Option<u8>,
    /// Bone palette: `(skeleton bone, bone table index)`. Per-vertex joint
    /// indices of skinned records index this list.
    pub palette: Vec<(u8, u8)>,
    pub stride0: u8,
    pub stride1: u8,
    pub morph_count: u32,
    pub index_count: u32,
    pub vertex_count: u16,
    pub min_index: u16,
    pub max_index: u16,
    pub primitive: Primitive,
    vb_static: i32,
    vb_cpu: i32,
    vb_volatile: i32,
    vertices_ptr: u32,
    indices_ptr: u32,
    morph_targets_ptr: u32,
    morph_deltas_ptr: u32,
}

/// A morph target of a draw record (body shapes `fat`, `skinny`; facial shapes).
#[derive(Debug, Clone, PartialEq)]
pub struct Morph {
    /// SDBM of the target name (`sdbm("fat")`); partsdb blenders name the weights.
    pub hash: u32,
    /// `(vertex of the record, position offset)`.
    pub deltas: Vec<(u16, [f32; 3])>,
}

/// Decoded vertices of one draw record, from `min_index` to `max_index`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Vertices {
    pub positions: Vec<[f32; 3]>,
    pub normals: Option<Vec<[f32; 3]>>,
    /// Tangent with the bitangent sign in w (±1).
    pub tangents: Option<Vec<[f32; 4]>>,
    pub uv0: Option<Vec<[f32; 2]>>,
    pub uv1: Option<Vec<[f32; 2]>>,
    pub colors: Option<Vec<[u8; 4]>>,
    /// Bone palette indices and weights of CPU-skinned records.
    pub joints: Option<Vec<[u8; 4]>>,
    pub weights: Option<Vec<[f32; 4]>>,
}

/// A parsed model.
#[derive(Debug, Clone)]
pub struct Model {
    image: Vec<u8>,
    pub name: String,
    pub bounding_sphere: [f32; 4],
    pub aabb_min: [f32; 3],
    pub aabb_max: [f32; 3],
    /// Bone table: `(bone name hash, skeleton bone index)`.
    pub bones: Vec<(u32, u32)>,
    pub vertex_buffers: Vec<Buffer>,
    pub index_buffers: Vec<Buffer>,
    pub records: Vec<DrawRecord>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ElementType {
    F32x3,
    F32x2,
    U8x4Norm,
    F16x2,
    F16x4,
}

impl ElementType {
    pub(crate) fn size(self) -> usize {
        match self {
            Self::F32x3 => 12,
            Self::F32x2 | Self::F16x4 => 8,
            Self::U8x4Norm | Self::F16x2 => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Usage {
    Position,
    Normal,
    Tangent,
    Color,
    Uv0,
    Uv1,
}

/// `(stream, offset, type, usage)`, from the Direct3D declarations in `godfather2.exe`.
pub(crate) type Element = (u8, usize, ElementType, Usage);

mod decl {
    use super::Element;
    use super::ElementType::*;
    use super::Usage::*;

    pub const CPU_PCTT_NT: &[Element] = &[
        (0, 0, F32x3, Position),
        (0, 12, U8x4Norm, Normal),
        (0, 16, U8x4Norm, Tangent),
        (1, 0, F32x2, Uv0),
        (1, 8, F32x2, Uv1),
        (1, 16, U8x4Norm, Color),
    ];
    pub const GPU_PCTT_NT: &[Element] = &[
        (0, 0, F32x3, Position),
        (1, 0, U8x4Norm, Color),
        (1, 4, F32x2, Uv0),
        (1, 12, F32x2, Uv1),
        (1, 20, U8x4Norm, Normal),
        (1, 24, U8x4Norm, Tangent),
    ];
    pub const CPU_PCT_NT: &[Element] = &[
        (0, 0, F32x3, Position),
        (0, 12, U8x4Norm, Normal),
        (0, 16, U8x4Norm, Tangent),
        (1, 0, F32x2, Uv0),
        (1, 8, U8x4Norm, Color),
    ];
    pub const GPU_PCT_NT: &[Element] = &[
        (0, 0, F32x3, Position),
        (1, 0, U8x4Norm, Color),
        (1, 4, F32x2, Uv0),
        (1, 12, U8x4Norm, Normal),
        (1, 16, U8x4Norm, Tangent),
    ];
    pub const CPU_PTT_NT: &[Element] = &[
        (0, 0, F32x3, Position),
        (0, 12, U8x4Norm, Normal),
        (0, 16, U8x4Norm, Tangent),
        (1, 0, F32x2, Uv0),
        (1, 8, F32x2, Uv1),
    ];
    pub const GPU_PTT_NT: &[Element] = &[
        (0, 0, F32x3, Position),
        (1, 0, F32x2, Uv0),
        (1, 8, F32x2, Uv1),
        (1, 16, U8x4Norm, Normal),
        (1, 20, U8x4Norm, Tangent),
    ];
    pub const CPU_PT_NT: &[Element] = &[
        (0, 0, F32x3, Position),
        (0, 12, U8x4Norm, Normal),
        (0, 16, U8x4Norm, Tangent),
        (1, 0, F32x2, Uv0),
    ];
    pub const GPU_PT_NT: &[Element] = &[
        (0, 0, F32x3, Position),
        (1, 0, F32x2, Uv0),
        (1, 8, U8x4Norm, Normal),
        (1, 12, U8x4Norm, Tangent),
    ];
    pub const CPU_PT_N: &[Element] = &[
        (0, 0, F32x3, Position),
        (0, 12, U8x4Norm, Normal),
        (1, 0, F32x2, Uv0),
    ];
    pub const GPU_PT_N: &[Element] = &[
        (0, 0, F32x3, Position),
        (1, 0, F32x2, Uv0),
        (1, 8, U8x4Norm, Normal),
    ];
    pub const PCT: &[Element] = &[
        (0, 0, F32x3, Position),
        (1, 0, U8x4Norm, Color),
        (1, 4, F32x2, Uv0),
    ];
    pub const PC: &[Element] = &[(0, 0, F32x3, Position), (1, 0, U8x4Norm, Color)];
    pub const PTT: &[Element] = &[
        (0, 0, F32x3, Position),
        (1, 0, F32x2, Uv0),
        (1, 8, F32x2, Uv1),
    ];
    pub const PT: &[Element] = &[(0, 0, F32x3, Position), (1, 0, F32x2, Uv0)];
    pub const CPU_P: &[Element] = &[
        (0, 0, F32x3, Position),
        (0, 12, U8x4Norm, Normal),
        (0, 16, U8x4Norm, Tangent),
    ];
    pub const P: &[Element] = &[(0, 0, F32x3, Position)];
    pub const CPU_PCT_N: &[Element] = &[
        (0, 0, F32x3, Position),
        (0, 12, U8x4Norm, Normal),
        (1, 0, F32x2, Uv0),
        (1, 8, U8x4Norm, Color),
    ];
    pub const GPU_PCT_N: &[Element] = &[
        (0, 0, F32x3, Position),
        (1, 0, U8x4Norm, Color),
        (1, 4, F32x2, Uv0),
        (1, 12, U8x4Norm, Normal),
    ];
    pub const CPU_LOWLOD: &[Element] = &[
        (0, 0, F16x4, Position),
        (0, 8, U8x4Norm, Normal),
        (1, 0, F16x2, Uv0),
    ];
    pub const GPU_LOWLOD: &[Element] = &[
        (0, 0, F32x3, Position),
        (1, 0, F16x2, Uv0),
        (1, 4, U8x4Norm, Normal),
    ];
    pub const GPU_LOWLOD_AO: &[Element] = &[
        (0, 0, F32x3, Position),
        (1, 0, U8x4Norm, Normal),
        (1, 4, F16x2, Uv0),
    ];
}

/// Vertex format id → (name, CPU declaration, GPU declaration).
pub const FORMAT_NAMES: [&str; 13] = [
    "PosColorTex1Tex2NormTan",
    "PosColorTex1NormTan",
    "PosColorTex1Norm",
    "PosTex1Tex2NormTan",
    "PosTex1NormTan",
    "PosTex1Norm",
    "PosColorTex1",
    "PosColor",
    "PosTex1Tex2",
    "PosTex1",
    "Pos",
    "PosTex1Norm_LowLOD",
    "PosTex1NormAO_LowLOD",
];

pub(crate) fn declarations(format: u8) -> Option<(Option<&'static [Element]>, &'static [Element])> {
    use decl::*;
    Some(match format {
        0 => (Some(CPU_PCTT_NT), GPU_PCTT_NT),
        1 => (Some(CPU_PCT_NT), GPU_PCT_NT),
        2 => (Some(CPU_PCT_N), GPU_PCT_N),
        3 => (Some(CPU_PTT_NT), GPU_PTT_NT),
        4 => (Some(CPU_PT_NT), GPU_PT_NT),
        5 => (Some(CPU_PT_N), GPU_PT_N),
        6 => (None, PCT),
        7 => (None, PC),
        8 => (None, PTT),
        9 => (Some(PT), PT),
        10 => (Some(CPU_P), P),
        11 => (Some(CPU_LOWLOD), GPU_LOWLOD),
        12 => (None, GPU_LOWLOD_AO),
        _ => return None,
    })
}

pub(crate) fn extent(declaration: &[Element], stream: u8) -> usize {
    declaration
        .iter()
        .filter(|e| e.0 == stream)
        .map(|e| e.1 + e.2.size())
        .max()
        .unwrap_or(0)
}

/// Picks the CPU or GPU variant of a format from the actual stream strides.
fn pick_declaration(format: u8, stride0: usize, stride1: usize) -> Result<&'static [Element]> {
    let (cpu, gpu) = declarations(format)
        .ok_or_else(|| Error::malformed("mesh", 0, format!("unknown vertex format {format}")))?;
    if extent(gpu, 1) == stride1 && extent(gpu, 0) == stride0 {
        return Ok(gpu);
    }
    if let Some(cpu) = cpu
        && extent(cpu, 1) == stride1
        && extent(cpu, 0) <= stride0
    {
        return Ok(cpu);
    }
    Err(Error::malformed(
        "mesh",
        0,
        format!("no declaration of format {format} matches strides {stride0}/{stride1}"),
    ))
}

/// IEEE 754 half precision to single precision.
pub fn f16_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = (bits >> 10) & 0x1F;
    let mantissa = (bits & 0x3FF) as f32;
    sign * match exponent {
        0 => mantissa * 2f32.powi(-24),
        31 => {
            if mantissa == 0.0 {
                f32::INFINITY
            } else {
                f32::NAN
            }
        }
        e => (1.0 + mantissa / 1024.0) * 2f32.powi(e as i32 - 15),
    }
}

/// Direct3D `DEC3N`: three signed 10-bit fields scaled by 1/511, and two bits of w.
fn dec3n(value: u32) -> ([f32; 3], u32) {
    let field = |shift: u32| {
        let v = ((value >> shift) & 0x3FF) as i32;
        (if v >= 512 { v - 1024 } else { v }) as f32 / 511.0
    };
    ([field(0), field(10), field(20)], value >> 30)
}

impl Model {
    /// Parses a mesh and its meshvolatile companion (empty when absent).
    pub fn parse(mesh: &[u8], volatile: &[u8]) -> Result<Self> {
        let le = Endian::Little;
        let mut image = Vec::with_capacity(mesh.len() + volatile.len());
        image.extend_from_slice(mesh);
        image.extend_from_slice(volatile);
        let d = &image[..];
        let u32_at = |o: usize| io::read_u32(d, o, le);
        let f32_at = |o: usize| u32_at(o).map(f32::from_bits);
        let vec = |o: usize, n: usize| -> Result<Vec<f32>> {
            (0..n).map(|i| f32_at(o + 4 * i)).collect()
        };
        let cstr = |o: u32| -> Result<String> {
            Ok(String::from_utf8_lossy(io::cstr(d, o as usize)?).into_owned())
        };

        if u32_at(0)? != MAGIC {
            return Err(Error::BadMagic {
                what: "mesh",
                found: u32_at(0)?,
            });
        }
        if u32_at(4)? != VERSION {
            return Err(Error::UnsupportedVersion {
                what: "mesh",
                version: u32_at(4)?,
            });
        }
        let sphere = vec(0x10, 4)?;
        let name = cstr(u32_at(0x20)?)?;
        let record_count = u32_at(0x34)? as usize;
        let bone_count = u32_at(0x38)? as usize;
        let vb_count = io::read_u16(d, 0x3C, le)? as usize;
        let ib_count = io::read_u16(d, 0x3E, le)? as usize;
        let records_at = u32_at(0x50)? as usize;
        let bones_at = u32_at(0x60)? as usize;
        let vbs_at = u32_at(0x68)? as usize;
        let ibs_at = u32_at(0x6C)? as usize;
        let aabb_min = vec(0xA0, 3)?;
        let aabb_max = vec(0xB0, 3)?;
        io::slice(d, records_at, record_count.saturating_mul(RECORD_SIZE))?;
        io::slice(d, bones_at, bone_count.saturating_mul(8))?;

        let buffers = |at: usize, n: usize| -> Result<Vec<Buffer>> {
            (0..n)
                .map(|i| {
                    let b = at + BUFFER_SIZE * i;
                    Ok(Buffer {
                        size: u32_at(b)?,
                        count: u32_at(b + 4)?,
                        stride: io::read_u16(d, b + 8, le)?,
                        format: io::slice(d, b + 10, 1)?[0],
                        flags: io::slice(d, b + 11, 1)?[0],
                        ptr: u32_at(b + 12)?,
                    })
                })
                .collect()
        };
        let vertex_buffers = buffers(vbs_at, vb_count)?;
        let index_buffers = buffers(ibs_at, ib_count)?;
        let bones = (0..bone_count)
            .map(|i| Ok((u32_at(bones_at + 8 * i)?, u32_at(bones_at + 8 * i + 4)?)))
            .collect::<Result<_>>()?;

        let mut records = Vec::with_capacity(record_count);
        for i in 0..record_count {
            let b = records_at + RECORD_SIZE * i;
            let byte = |o: usize| -> Result<u8> { Ok(io::slice(d, b + o, 1)?[0]) };
            let half = |o: usize| io::read_u16(d, b + o, le);
            let int = |o: usize| u32_at(b + o).map(|v| v as i32);
            let palette_size = byte(0x25)? as usize;
            let palette_at = u32_at(b + 0x44)? as usize;
            let palette = if palette_at == 0 {
                Vec::new()
            } else {
                let bytes = io::slice(d, palette_at, 2 * palette_size)?;
                bytes.chunks_exact(2).map(|p| (p[0], p[1])).collect()
            };
            let primitive = match half(0x3C)? {
                4 => Primitive::TriangleList,
                5 => Primitive::TriangleStrip,
                other => {
                    return Err(Error::malformed(
                        "mesh record",
                        b,
                        format!("primitive type {other}"),
                    ));
                }
            };
            let s = vec(b + 0x10, 4)?;
            records.push(DrawRecord {
                name: cstr(u32_at(b)?)?,
                influences: byte(6)?,
                format: byte(7)?,
                render_flags: u32_at(b + 8)?,
                overlay_id: byte(0x28)?,
                material_hash: u32_at(b + 0x0C)?,
                bounding_sphere: [s[0], s[1], s[2], s[3]],
                rigid_bone: Some(byte(0x24)?).filter(|&b| b != 0xFF),
                palette,
                stride0: byte(0x26)?,
                stride1: byte(0x27)?,
                morph_count: u32_at(b + 0x2C)?,
                index_count: u32_at(b + 0x30)?,
                vertex_count: half(0x38)?,
                min_index: half(0x40)?,
                max_index: half(0x42)?,
                primitive,
                vb_static: int(0x64)?,
                vb_cpu: int(0x68)?,
                vb_volatile: int(0x6C)?,
                vertices_ptr: u32_at(b + 0x84)?,
                indices_ptr: u32_at(b + 0x88)?,
                morph_targets_ptr: u32_at(b + 0x90)?,
                morph_deltas_ptr: u32_at(b + 0x94)?,
            });
        }

        Ok(Self {
            image,
            name,
            bounding_sphere: [sphere[0], sphere[1], sphere[2], sphere[3]],
            aabb_min: [aabb_min[0], aabb_min[1], aabb_min[2]],
            aabb_max: [aabb_max[0], aabb_max[1], aabb_max[2]],
            bones,
            vertex_buffers,
            index_buffers,
            records,
        })
    }

    fn gpu_vertex_buffer(&self, index: i32) -> Result<&Buffer> {
        usize::try_from(index)
            .ok()
            .and_then(|i| self.vertex_buffers.iter().filter(|b| !b.is_cpu()).nth(i))
            .ok_or_else(|| {
                Error::malformed("mesh record", 0, format!("vertex buffer {index} missing"))
            })
    }

    fn cpu_vertex_buffer(&self, index: i32) -> Result<&Buffer> {
        usize::try_from(index)
            .ok()
            .and_then(|i| self.vertex_buffers.iter().filter(|b| b.is_cpu()).nth(i))
            .ok_or_else(|| {
                Error::malformed(
                    "mesh record",
                    0,
                    format!("CPU vertex buffer {index} missing"),
                )
            })
    }

    /// Morph targets of a record: table `{u32 hash, 1, count, first}` at
    /// 0x90, deltas `{f16 × 3, u16 vertex}` at 0x94; vertices count from the
    /// record's first vertex.
    pub fn morphs(&self, record: &DrawRecord) -> Result<Vec<Morph>> {
        if record.morph_count == 0 || record.morph_targets_ptr == 0 {
            return Ok(Vec::new());
        }
        let d = &self.image;
        let le = Endian::Little;
        let table = record.morph_targets_ptr as usize;
        io::slice(
            d,
            table,
            16usize.saturating_mul(record.morph_count as usize),
        )?;
        (0..record.morph_count as usize)
            .map(|k| {
                let at = table + 16 * k;
                let hash = io::read_u32(d, at, le)?;
                let count = io::read_u32(d, at + 8, le)? as usize;
                let first = io::read_u32(d, at + 12, le)? as usize;
                let start = (record.morph_deltas_ptr as usize).saturating_add(8 * first);
                let bytes = io::slice(d, start, 8usize.saturating_mul(count))?;
                let deltas = bytes
                    .chunks_exact(8)
                    .map(|c| {
                        let h = |i: usize| f16_to_f32(u16::from_le_bytes([c[i], c[i + 1]]));
                        (u16::from_le_bytes([c[6], c[7]]), [h(0), h(2), h(4)])
                    })
                    .collect();
                Ok(Morph { hash, deltas })
            })
            .collect()
    }

    /// Triangles of a record as indices into its [`Vertices`]; degenerate triangles are dropped.
    pub fn triangles(&self, record: &DrawRecord) -> Result<Vec<[u32; 3]>> {
        let n = record.index_count as usize;
        let bytes = io::slice(&self.image, record.indices_ptr as usize, 2 * n)?;
        let indices: Vec<u32> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]) as u32)
            .collect();
        let base = record.min_index as u32;
        let count = record.vertex_count as u32;
        let mut triangles = Vec::with_capacity(n / 3 + 2);
        let mut push = |a: u32, b: u32, c: u32| -> Result<()> {
            if a == b || b == c || a == c {
                return Ok(());
            }
            let tri = [
                a.wrapping_sub(base),
                b.wrapping_sub(base),
                c.wrapping_sub(base),
            ];
            if tri.iter().any(|&i| i >= count) {
                return Err(Error::malformed(
                    "mesh record",
                    0,
                    "index outside the record's vertex range",
                ));
            }
            triangles.push(tri);
            Ok(())
        };
        match record.primitive {
            Primitive::TriangleList => {
                for t in indices.chunks_exact(3) {
                    push(t[0], t[1], t[2])?;
                }
            }
            Primitive::TriangleStrip => {
                for (i, w) in indices.windows(3).enumerate() {
                    if i % 2 == 0 {
                        push(w[0], w[1], w[2])?
                    } else {
                        push(w[0], w[2], w[1])?
                    }
                }
            }
        }
        Ok(triangles)
    }

    /// Decodes the vertices of a record.
    pub fn vertices(&self, record: &DrawRecord) -> Result<Vertices> {
        let n = record.vertex_count as usize;
        let declaration = pick_declaration(
            record.format,
            record.stride0 as usize,
            record.stride1 as usize,
        )?;
        let mut out = Vertices::default();

        let stream0 = if record.vb_cpu >= 0 {
            let gpu_stride = self.cpu_vertex_buffer(record.vb_cpu)?.stride;
            self.cpu_source(record, gpu_stride, &mut out)?;
            None
        } else {
            let vb = self.gpu_vertex_buffer(record.vb_static)?;
            Some((record.vertices_ptr as usize, vb.stride as usize))
        };
        let stream1 = if record.vb_volatile >= 0 {
            let vb = self.gpu_vertex_buffer(record.vb_volatile)?;
            Some((
                vb.ptr as usize + record.min_index as usize * vb.stride as usize,
                vb.stride as usize,
            ))
        } else {
            None
        };

        for &(stream, offset, ty, usage) in declaration {
            let (base, stride) = match (stream, stream0, stream1) {
                (0, Some(s), _) => s,
                (0, None, _) => continue, // CPU-skinned: stream 0 came from the inline source
                (_, _, Some(s)) => s,
                _ => {
                    return Err(Error::malformed(
                        "mesh record",
                        0,
                        "second vertex stream missing",
                    ));
                }
            };
            let element = |v: usize| io::slice(&self.image, base + v * stride + offset, ty.size());
            match usage {
                Usage::Position => {
                    out.positions = (0..n)
                        .map(|v| {
                            let e = element(v)?;
                            Ok(match ty {
                                ElementType::F16x4 => [0, 2, 4]
                                    .map(|i| f16_to_f32(u16::from_le_bytes([e[i], e[i + 1]]))),
                                _ => [0, 4, 8].map(|i| {
                                    f32::from_le_bytes(e[i..i + 4].try_into().expect("4 bytes"))
                                }),
                            })
                        })
                        .collect::<Result<_>>()?;
                }
                Usage::Normal | Usage::Tangent => {
                    let values: Vec<[f32; 4]> = (0..n)
                        .map(|v| {
                            Ok(element(v)?
                                .try_into()
                                .map(|b: [u8; 4]| b.map(|x| x as f32 / 255.0 * 2.0 - 1.0))
                                .expect("4 bytes"))
                        })
                        .collect::<Result<_>>()?;
                    if usage == Usage::Normal {
                        out.normals = Some(
                            values
                                .iter()
                                .map(|v| normalize([v[0], v[1], v[2]]))
                                .collect(),
                        );
                    } else {
                        out.tangents = Some(
                            values
                                .iter()
                                .map(|v| {
                                    let t = normalize([v[0], v[1], v[2]]);
                                    [t[0], t[1], t[2], if v[3] < 0.0 { -1.0 } else { 1.0 }]
                                })
                                .collect(),
                        );
                    }
                }
                Usage::Color => {
                    out.colors = Some(
                        (0..n)
                            .map(|v| Ok(element(v)?.try_into().expect("4 bytes")))
                            .collect::<Result<_>>()?,
                    );
                }
                Usage::Uv0 | Usage::Uv1 => {
                    let uv: Vec<[f32; 2]> = (0..n)
                        .map(|v| {
                            let e = element(v)?;
                            Ok(match ty {
                                ElementType::F16x2 => {
                                    [0, 2].map(|i| f16_to_f32(u16::from_le_bytes([e[i], e[i + 1]])))
                                }
                                _ => [0, 4].map(|i| {
                                    f32::from_le_bytes(e[i..i + 4].try_into().expect("4 bytes"))
                                }),
                            })
                        })
                        .collect::<Result<_>>()?;
                    if usage == Usage::Uv0 {
                        out.uv0 = Some(uv)
                    } else {
                        out.uv1 = Some(uv)
                    }
                }
            }
        }
        if out.positions.len() != n {
            return Err(Error::malformed(
                "mesh record",
                0,
                "record has no positions",
            ));
        }
        Ok(out)
    }

    /// The two w bits of every DEC3N tangent of a CPU-skinned record with
    /// tangents (32-byte source vertices): 0 or 2 in the game, 2 for a
    /// negative bitangent sign. Empty for other records.
    pub fn skinned_tangent_w(&self, record: &DrawRecord) -> Result<Vec<u8>> {
        if record.vb_cpu < 0 || self.cpu_vertex_buffer(record.vb_cpu)?.stride != 20 {
            return Ok(Vec::new());
        }
        let at = record.vertices_ptr as usize;
        (0..record.vertex_count as usize)
            .map(|v| {
                let e = io::slice(&self.image, at + 32 * v + 16, 4)?;
                Ok((u32::from_le_bytes(e.try_into().expect("4 bytes")) >> 30) as u8)
            })
            .collect()
    }

    /// Inline source vertices of CPU-skinned records.
    fn cpu_source(&self, record: &DrawRecord, gpu_stride: u16, out: &mut Vertices) -> Result<()> {
        let n = record.vertex_count as usize;
        let at = record.vertices_ptr as usize;
        let le = |b: &[u8]| u32::from_le_bytes(b.try_into().expect("4 bytes"));
        let half = |b: &[u8]| f16_to_f32(u16::from_le_bytes([b[0], b[1]]));
        let (mut positions, mut normals, mut joints, mut weights) = (
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
        );
        match gpu_stride {
            20 => {
                let mut tangents = Vec::with_capacity(n);
                for v in 0..n {
                    let e = io::slice(&self.image, at + 32 * v, 32)?;
                    positions.push(
                        [0, 4, 8]
                            .map(|i| f32::from_le_bytes(e[i..i + 4].try_into().expect("4 bytes"))),
                    );
                    normals.push(normalize(dec3n(le(&e[12..16])).0));
                    let (t, w) = dec3n(le(&e[16..20]));
                    let t = normalize(t);
                    tangents.push([t[0], t[1], t[2], if w >= 2 { -1.0 } else { 1.0 }]);
                    joints.push([e[20], e[21], e[22], e[23]]);
                    weights.push(
                        [24, 26, 28, 30]
                            .map(|i| u16::from_le_bytes([e[i], e[i + 1]]) as f32 / 65535.0),
                    );
                }
                out.tangents = Some(tangents);
            }
            12 => {
                for v in 0..n {
                    let e = io::slice(&self.image, at + 16 * v, 16)?;
                    positions.push([half(&e[0..2]), half(&e[2..4]), half(&e[4..6])]);
                    normals.push(normalize(dec3n(le(&e[8..12])).0));
                    joints.push([e[6], e[7], 0, 0]);
                    let w = [12, 14].map(|i| u16::from_le_bytes([e[i], e[i + 1]]) as f32 / 65535.0);
                    weights.push([w[0], w[1], 0.0, 0.0]);
                }
            }
            other => {
                return Err(Error::malformed(
                    "mesh record",
                    0,
                    format!("CPU vertex stride {other}"),
                ));
            }
        }
        out.positions = positions;
        out.normals = Some(normals);
        out.joints = Some(joints);
        out.weights = Some(weights);
        Ok(())
    }
}

/// Type id of `mesh` resources.
pub const TYPE_MESH: u32 = crate::hash::sdbm(b"mesh");
/// Type id of `meshvolatile` resources.
pub const TYPE_MESH_VOLATILE: u32 = crate::hash::sdbm(b"meshvolatile");

/// Index of the `meshvolatile` that belongs to mesh resource `index`: the
/// next resource of that type whose id is the bitwise inverse of the mesh id.
pub fn volatile_for(
    layout: &crate::container::ustream::StreamLayout,
    index: usize,
) -> Option<usize> {
    let id = layout.resources.get(index)?.header.ids[3];
    layout
        .resources
        .iter()
        .enumerate()
        .skip(index + 1)
        .chain(layout.resources.iter().enumerate().take(index))
        .find(|(_, r)| r.header.type_id == TYPE_MESH_VOLATILE && r.header.ids[3] == !id)
        .map(|(i, _)| i)
}

/// Loads mesh resource `index` of a stream together with its volatile data.
pub fn load(stream: &crate::game::StreamFile, index: usize) -> Result<Model> {
    let mesh = stream.resource_data(index)?;
    let volatile = match volatile_for(stream.layout(), index) {
        Some(v) => stream.resource_data(v)?,
        None => Vec::new(),
    };
    Model::parse(&mesh, &volatile)
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 1e-6 {
        v.map(|x| x / len)
    } else {
        [0.0, 1.0, 0.0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_floats() {
        assert_eq!(f16_to_f32(0x3C00), 1.0);
        assert_eq!(f16_to_f32(0xC000), -2.0);
        assert_eq!(f16_to_f32(0x3555), 0.333_251_95);
        assert_eq!(f16_to_f32(0x0001), 2f32.powi(-24));
        assert!(f16_to_f32(0x7E00).is_nan());
    }

    #[test]
    fn render_flags_are_named() {
        assert_eq!(render_flag_names(0), "-");
        assert_eq!(
            render_flag_names(0x4808),
            "AlphaSort | DoNotCastShadow | LowLODOnly"
        );
        assert_eq!(
            render_flag_names(0x1021),
            "DrawInSky | DoNotRenderDepth | 0x1"
        );
    }

    #[test]
    fn dec3n_fields() {
        let (v, w) = dec3n(511 | (0x200 << 10) | (2 << 30));
        assert_eq!(v, [1.0, -512.0 / 511.0, 0.0]);
        assert_eq!(w, 2);
    }

    #[test]
    fn declarations_cover_all_formats() {
        for format in 0..13u8 {
            let (cpu, gpu) = declarations(format).unwrap();
            assert!(gpu.iter().any(|e| e.3 == Usage::Position));
            assert_eq!(
                pick_declaration(format, extent(gpu, 0), extent(gpu, 1)).unwrap(),
                gpu
            );
            if let Some(cpu) = cpu
                && extent(cpu, 1) != extent(gpu, 1)
            {
                assert_eq!(
                    pick_declaration(format, 20, extent(cpu, 1)).unwrap(),
                    cpu,
                    "format {format}"
                );
            }
        }
        assert!(declarations(13).is_none());
    }

    #[test]
    fn rejects_bad_input() {
        assert!(Model::parse(&[], &[]).is_err());
        let mut bad = vec![0u8; 0x100];
        bad[..4].copy_from_slice(&MAGIC.to_le_bytes());
        bad[4] = 41;
        assert!(matches!(
            Model::parse(&bad, &[]),
            Err(Error::UnsupportedVersion { .. })
        ));
    }
}

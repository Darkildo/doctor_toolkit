//! Writer for `mesh` resources: a complete structured form of the `MGAE`
//! memory image ([`MeshImage`]) and its encoder.
//!
//! [`MeshImage::parse`] keeps everything the image holds, including fields
//! whose meaning is unknown (raw record bytes, bounds, triangle grids, morph
//! data); [`MeshImage::encode`] lays the image out again the way the game's
//! exporter does and rebuilds every pointer and the relocation table. The
//! corpus test checks `encode(parse(x)) == x` byte for byte on every mesh.
//!
//! Layout of the image (all offsets relative to the start of the mesh;
//! `A(n)` means "aligned to n"):
//!
//! ```text
//! header                                         0x100
//! runtime area A: 0x40 × vertex buffers, zeros   A80
//! runtime area B: 0x40 × index buffers, zeros    A80
//! draw records, 0xB0 each                        A80
//! record bounds, 0x80 each                       A80
//! morph target tables                            first A80, then A10
//! vertex buffer descriptors                      A80
//! index buffer descriptors                       A10
//! vertex data, in buffer order:                  A80 per buffer
//!   GPU buffer kept in the mesh: its data
//!   CPU buffer: inline sources of its records    A10 between records
//! index data, in buffer order:                   A80 per buffer
//!   GPU buffer: its data
//!   CPU buffer: inline index lists of its records  A2 between records
//! per-vertex bytes (record 0x48)                 A80 each
//! triangle grids of static records               A10 each
//! morph deltas                                   first A80, then A4
//! bone palettes                                  first A80, then A2
//! bone table                                     A10
//! strings: model name, record names (shared)     A10
//! relocation table {u32 slot, u32 value}         A10, sorted by slot
//! padding                                        A80 = len(mesh)
//! meshvolatile: data of the flag-2 buffers       A4 each, padded to A80
//! ```
//!
//! The alignments are those of all 17 622 meshes of the game, which the
//! corpus test re-encodes byte for byte (`docs/formats/geometry.md` §7).

use std::collections::HashMap;

use crate::io::{self, Endian};
use crate::{Error, Result};

const MAGIC: u32 = 0x4541_474D;
const VERSION: u32 = 42;
const HEADER_SIZE: usize = 0x100;
pub(crate) const RECORD_SIZE: usize = 0xB0;
const BOUNDS_SIZE: usize = 0x80;
const BUFFER_SIZE: usize = 0x10;

/// Buffer flag: CPU buffer without data.
pub const FLAG_CPU: u8 = 1;
/// Buffer flag: data lives in the meshvolatile resource.
pub const FLAG_VOLATILE: u8 = 2;

/// Record pointer slots.
mod slot {
    pub const NAME: usize = 0x00;
    pub const VERTEX_LAYER_BIAS: usize = 0x48;
    pub const BOUNDS: usize = 0x20;
    pub const PALETTE: usize = 0x44;
    pub const VERTICES: usize = 0x84;
    pub const INDICES: usize = 0x88;
    pub const GRID: usize = 0x8C;
    pub const MORPH_TARGETS: usize = 0x90;
    pub const MORPH_DELTAS: usize = 0x94;
    pub const ALL: [usize; 9] = [
        NAME,
        VERTEX_LAYER_BIAS,
        BOUNDS,
        PALETTE,
        VERTICES,
        INDICES,
        GRID,
        MORPH_TARGETS,
        MORPH_DELTAS,
    ];
}

/// A vertex or index buffer with its data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferImage {
    /// `size` field: count × stride, also for CPU buffers without data.
    pub size: u32,
    pub count: u32,
    pub stride: u16,
    /// Vertex format id; 1 or 2 for index buffers.
    pub format: u8,
    /// Bit 0: CPU buffer without data; 2: data in the meshvolatile.
    pub flags: u8,
    /// The data; empty for CPU buffers.
    pub data: Vec<u8>,
}

impl BufferImage {
    pub fn is_cpu(&self) -> bool {
        self.flags & FLAG_CPU != 0
    }
    pub fn is_volatile(&self) -> bool {
        self.flags & FLAG_VOLATILE != 0
    }
}

/// Where the vertices (`0x84`) or indices (`0x88`) of a record are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataRef {
    None,
    /// Inside a buffer: index into the vertex or index buffer list, byte offset.
    Buffer {
        buffer: usize,
        offset: u32,
    },
    /// Data of the record itself (CPU-skinned sources, CPU index lists).
    Inline(Vec<u8>),
}

/// One draw record with everything its pointers lead to.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordImage {
    /// The 0xB0 record bytes; pointer slots are rebuilt by the encoder.
    pub raw: Vec<u8>,
    pub name: String,
    /// 8 corners (static) or the AABB (skinned), 0x80 bytes.
    pub bounds: Vec<u8>,
    /// `(skeleton bone, bone table index)`; empty without palette pointer.
    pub palette: Vec<(u8, u8)>,
    pub vertices: DataRef,
    pub indices: DataRef,
    /// Triangle grid of static records (see [`crate::resource::mesh_grid`]).
    pub grid: Option<Vec<u8>>,
    /// Sort bias per vertex (record field `0x48`, `VertexLayerBias`): only in
    /// the 61 records with the TriSort render flag (hair and lashes of
    /// characters), which also have a sorted CPU index buffer (`0x70`).
    pub vertex_layer_bias: Option<Vec<u8>>,
    /// Morph target table `{u32 hash, 1, count, first}` × morph count.
    pub morph_targets: Option<Vec<u8>>,
    /// Morph deltas `{f16 × 3, u16 vertex}`.
    pub morph_deltas: Option<Vec<u8>>,
}

impl RecordImage {
    pub fn u8_at(&self, at: usize) -> u8 {
        self.raw[at]
    }
    pub fn u16_at(&self, at: usize) -> u16 {
        u16::from_le_bytes([self.raw[at], self.raw[at + 1]])
    }
    pub fn u32_at(&self, at: usize) -> u32 {
        u32::from_le_bytes(self.raw[at..at + 4].try_into().expect("4 bytes"))
    }
    pub fn set_u8(&mut self, at: usize, v: u8) {
        self.raw[at] = v;
    }
    pub fn set_u16(&mut self, at: usize, v: u16) {
        self.raw[at..at + 2].copy_from_slice(&v.to_le_bytes());
    }
    pub fn set_u32(&mut self, at: usize, v: u32) {
        self.raw[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
    pub fn set_f32(&mut self, at: usize, v: f32) {
        self.set_u32(at, v.to_bits());
    }
}

/// A complete, encodable mesh.
#[derive(Debug, Clone, PartialEq)]
pub struct MeshImage {
    pub name: String,
    /// Header field 0x08, the platform: 3 (PC) in every mesh (1 Xbox 360,
    /// 2 PS3, 4 Wii in the other builds of the engine).
    pub platform: u32,
    pub bounding_sphere: [f32; 4],
    /// AABB minimum and maximum with their w components.
    pub aabb_min: [f32; 4],
    pub aabb_max: [f32; 4],
    /// `(bone name hash, skeleton bone index)`.
    pub bones: Vec<(u32, u32)>,
    pub vertex_buffers: Vec<BufferImage>,
    pub index_buffers: Vec<BufferImage>,
    pub records: Vec<RecordImage>,
    /// Whether the mesh has a meshvolatile companion: header flags 0x24 =
    /// 0x10 ("has extra vertex buffer data"), else 0.
    pub has_volatile: bool,
}

fn align(value: usize, to: usize) -> usize {
    value.div_ceil(to) * to
}

fn malformed(at: usize, message: impl Into<String>) -> Error {
    Error::malformed("mesh", at, message.into())
}

impl MeshImage {
    /// Parses a mesh and its meshvolatile (empty when absent).
    pub fn parse(mesh: &[u8], volatile: &[u8]) -> Result<Self> {
        let le = Endian::Little;
        let mut image = Vec::with_capacity(mesh.len() + volatile.len());
        image.extend_from_slice(mesh);
        image.extend_from_slice(volatile);
        let d = &image[..];
        let u32_at = |o: usize| io::read_u32(d, o, le);
        let u16_at = |o: usize| io::read_u16(d, o, le);
        let f32x4 = |o: usize| -> Result<[f32; 4]> {
            Ok([
                f32::from_bits(u32_at(o)?),
                f32::from_bits(u32_at(o + 4)?),
                f32::from_bits(u32_at(o + 8)?),
                f32::from_bits(u32_at(o + 12)?),
            ])
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
        let record_count = u32_at(0x34)? as usize;
        let bone_count = u32_at(0x38)? as usize;
        let vb_count = u16_at(0x3C)? as usize;
        let ib_count = u16_at(0x3E)? as usize;
        let records_at = u32_at(0x50)? as usize;
        let bounds_at = u32_at(0x54)? as usize;
        let bones_at = u32_at(0x60)? as usize;
        io::slice(d, records_at, record_count.saturating_mul(RECORD_SIZE))?;
        io::slice(d, bones_at, bone_count.saturating_mul(8))?;

        let buffers = |at: usize, n: usize| -> Result<Vec<(BufferImage, u32)>> {
            io::slice(d, at, n.saturating_mul(BUFFER_SIZE))?;
            (0..n)
                .map(|i| {
                    let b = at + BUFFER_SIZE * i;
                    let size = u32_at(b)?;
                    let flags = d[b + 11];
                    let ptr = u32_at(b + 12)?;
                    let data = if ptr == 0 {
                        Vec::new()
                    } else {
                        io::slice(d, ptr as usize, size as usize)?.to_vec()
                    };
                    Ok((
                        BufferImage {
                            size,
                            count: u32_at(b + 4)?,
                            stride: u16_at(b + 8)?,
                            format: d[b + 10],
                            flags,
                            data,
                        },
                        ptr,
                    ))
                })
                .collect()
        };
        let vbs = buffers(u32_at(0x68)? as usize, vb_count)?;
        let ibs = buffers(u32_at(0x6C)? as usize, ib_count)?;
        let find = |list: &[(BufferImage, u32)], ptr: u32| -> Option<DataRef> {
            list.iter().enumerate().find_map(|(i, (b, at))| {
                (*at != 0 && ptr >= *at && ptr < at + b.size.max(1)).then(|| DataRef::Buffer {
                    buffer: i,
                    offset: ptr - at,
                })
            })
        };

        let mut records = Vec::with_capacity(record_count);
        for i in 0..record_count {
            let b = records_at + RECORD_SIZE * i;
            let raw = d[b..b + RECORD_SIZE].to_vec();
            let r32 = |o: usize| u32::from_le_bytes(raw[o..o + 4].try_into().expect("4 bytes"));
            let r16 = |o: usize| u16::from_le_bytes([raw[o], raw[o + 1]]);
            let bounds_ptr = r32(slot::BOUNDS) as usize;
            if bounds_ptr != bounds_at + BOUNDS_SIZE * i {
                return Err(malformed(b, "record bounds out of order"));
            }
            let palette_ptr = r32(slot::PALETTE) as usize;
            let palette = if palette_ptr == 0 {
                Vec::new()
            } else {
                io::slice(d, palette_ptr, 2 * raw[0x25] as usize)?
                    .chunks_exact(2)
                    .map(|p| (p[0], p[1]))
                    .collect()
            };
            let vertex_count = r16(0x38) as usize;
            let index_count = r32(0x30) as usize;
            let vb_cpu = r32(0x68) as i32;
            let vertices_ptr = r32(slot::VERTICES);
            let vertices = if vertices_ptr == 0 {
                DataRef::None
            } else if vb_cpu >= 0 {
                let gpu_stride = vbs
                    .iter()
                    .filter(|(v, _)| v.is_cpu())
                    .nth(vb_cpu as usize)
                    .map(|(v, _)| v.stride)
                    .ok_or_else(|| malformed(b, "CPU vertex buffer missing"))?;
                let size = cpu_source_size(gpu_stride, vertex_count)
                    .ok_or_else(|| malformed(b, format!("CPU vertex stride {gpu_stride}")))?;
                DataRef::Inline(io::slice(d, vertices_ptr as usize, size)?.to_vec())
            } else {
                find(&vbs, vertices_ptr)
                    .ok_or_else(|| malformed(b, "vertices outside every buffer"))?
            };
            let indices_ptr = r32(slot::INDICES);
            let indices = if indices_ptr == 0 {
                DataRef::None
            } else {
                match find(&ibs, indices_ptr) {
                    Some(r) => r,
                    None => DataRef::Inline(
                        io::slice(d, indices_ptr as usize, 2 * index_count)?.to_vec(),
                    ),
                }
            };
            let bytes_ptr = r32(slot::VERTEX_LAYER_BIAS) as usize;
            let vertex_layer_bias = if bytes_ptr == 0 {
                None
            } else {
                Some(io::slice(d, bytes_ptr, vertex_count)?.to_vec())
            };
            let grid_ptr = r32(slot::GRID) as usize;
            let grid = if grid_ptr == 0 {
                None
            } else {
                let size = crate::resource::mesh_grid::size_at(d, grid_ptr)?;
                Some(io::slice(d, grid_ptr, size)?.to_vec())
            };
            let morphs = r32(0x2C) as usize;
            let targets_ptr = r32(slot::MORPH_TARGETS) as usize;
            let (morph_targets, deltas_len) = if targets_ptr == 0 {
                (None, 0)
            } else {
                let table = io::slice(d, targets_ptr, 16 * morphs)?;
                let total = table
                    .chunks_exact(16)
                    .map(|t| u32::from_le_bytes(t[8..12].try_into().expect("4 bytes")) as usize)
                    .sum::<usize>();
                (Some(table.to_vec()), total)
            };
            let deltas_ptr = r32(slot::MORPH_DELTAS) as usize;
            let morph_deltas = if deltas_ptr == 0 {
                None
            } else {
                Some(io::slice(d, deltas_ptr, 8 * deltas_len)?.to_vec())
            };
            let name = cstr(r32(slot::NAME))?;
            let mut raw = raw;
            for at in slot::ALL {
                raw[at..at + 4].fill(0);
            }
            records.push(RecordImage {
                name,
                bounds: io::slice(d, bounds_ptr, BOUNDS_SIZE)?.to_vec(),
                raw,
                palette,
                vertices,
                indices,
                grid,
                vertex_layer_bias,
                morph_targets,
                morph_deltas,
            });
        }
        let bones = (0..bone_count)
            .map(|i| Ok((u32_at(bones_at + 8 * i)?, u32_at(bones_at + 8 * i + 4)?)))
            .collect::<Result<_>>()?;
        Ok(Self {
            name: cstr(u32_at(0x20)?)?,
            platform: u32_at(8)?,
            bounding_sphere: f32x4(0x10)?,
            aabb_min: f32x4(0xA0)?,
            aabb_max: f32x4(0xB0)?,
            bones,
            vertex_buffers: vbs.into_iter().map(|(b, _)| b).collect(),
            index_buffers: ibs.into_iter().map(|(b, _)| b).collect(),
            records,
            has_volatile: u32_at(0x24)? != 0,
        })
    }

    /// Encodes the mesh and its meshvolatile (empty when the mesh has none).
    pub fn encode(&self) -> Result<(Vec<u8>, Vec<u8>)> {
        Writer::new(self).write()
    }
}

/// Size of the inline source of a CPU-skinned record: 32 bytes per vertex
/// when the GPU stream 0 has stride 20, 16 bytes when it has stride 12.
pub fn cpu_source_size(gpu_stride: u16, vertices: usize) -> Option<usize> {
    match gpu_stride {
        20 => Some(32 * vertices),
        12 => Some(16 * vertices),
        _ => None,
    }
}

struct Writer<'a> {
    mesh: &'a MeshImage,
    out: Vec<u8>,
    /// `(slot, value)` of every pointer.
    relocations: Vec<(u32, u32)>,
}

impl<'a> Writer<'a> {
    fn new(mesh: &'a MeshImage) -> Self {
        Self {
            mesh,
            out: Vec::new(),
            relocations: Vec::new(),
        }
    }

    fn pad(&mut self, to: usize) {
        let len = align(self.out.len(), to);
        self.out.resize(len, 0);
    }

    fn put(&mut self, bytes: &[u8]) -> u32 {
        let at = self.out.len() as u32;
        self.out.extend_from_slice(bytes);
        at
    }

    fn ptr(&mut self, slot: usize, value: u32) {
        if value != 0 {
            self.out[slot..slot + 4].copy_from_slice(&value.to_le_bytes());
            self.relocations.push((slot as u32, value));
        }
    }

    fn set_u32(&mut self, at: usize, value: u32) {
        self.out[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn set_u16(&mut self, at: usize, value: u16) {
        self.out[at..at + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn write(mut self) -> Result<(Vec<u8>, Vec<u8>)> {
        let m = self.mesh;
        let nvb = m.vertex_buffers.len();
        let nib = m.index_buffers.len();
        let nrec = m.records.len();
        if nvb > u16::MAX as usize || nib > u16::MAX as usize {
            return Err(Error::InvalidArgument("too many buffers in a mesh".into()));
        }
        for r in &m.records {
            if r.raw.len() != RECORD_SIZE || r.bounds.len() != BOUNDS_SIZE {
                return Err(Error::InvalidArgument(format!(
                    "record `{}`: raw record or bounds of the wrong size",
                    r.name
                )));
            }
            if r.palette.len() > u8::MAX as usize {
                return Err(Error::InvalidArgument(format!(
                    "record `{}`: palette of {} bones",
                    r.name,
                    r.palette.len()
                )));
            }
        }

        self.out.resize(HEADER_SIZE, 0);
        // Runtime areas.
        let area_a = self.out.len();
        self.out.resize(area_a + align(0x40 * nvb, 0x80), 0);
        let area_b = self.out.len();
        self.out.resize(area_b + align(0x40 * nib, 0x80), 0);
        // Records and bounds.
        self.pad(0x80);
        let records_at = self.out.len();
        for r in &m.records {
            self.out.extend_from_slice(&r.raw);
        }
        self.pad(0x80);
        let bounds_at = self.out.len();
        for (i, r) in m.records.iter().enumerate() {
            let at = self.put(&r.bounds);
            self.ptr(records_at + RECORD_SIZE * i + slot::BOUNDS, at);
        }
        // Morph target tables.
        let mut first = true;
        for (i, r) in m.records.iter().enumerate() {
            if let Some(table) = &r.morph_targets {
                self.pad(if first { 0x80 } else { 0x10 });
                first = false;
                let at = self.put(table);
                self.ptr(records_at + RECORD_SIZE * i + slot::MORPH_TARGETS, at);
            }
        }
        // Buffer descriptors.
        self.pad(0x80);
        let vbs_at = self.out.len();
        self.out.resize(vbs_at + BUFFER_SIZE * nvb, 0);
        self.pad(0x10);
        let ibs_at = self.out.len();
        self.out.resize(ibs_at + BUFFER_SIZE * nib, 0);
        for (at, list) in [(vbs_at, &m.vertex_buffers), (ibs_at, &m.index_buffers)] {
            for (i, b) in list.iter().enumerate() {
                let o = at + BUFFER_SIZE * i;
                self.set_u32(o, b.size);
                self.set_u32(o + 4, b.count);
                self.set_u16(o + 8, b.stride);
                self.out[o + 10] = b.format;
                self.out[o + 11] = b.flags;
            }
        }
        // Vertex data kept in the mesh, in buffer order. A CPU buffer has
        // no data of its own; the inline sources of the records that use it
        // (record field 0x68) take its place.
        let mut vb_at = vec![0u32; nvb];
        let mut placed = vec![false; nrec];
        let mut cpu_ordinal = 0;
        for (i, b) in m.vertex_buffers.iter().enumerate() {
            if b.is_cpu() {
                let mut first = true;
                for (k, r) in m.records.iter().enumerate() {
                    if let DataRef::Inline(data) = &r.vertices
                        && r.u32_at(0x68) as i32 == cpu_ordinal
                    {
                        self.pad(if first { 0x80 } else { 0x10 });
                        first = false;
                        let at = self.put(data);
                        self.ptr(records_at + RECORD_SIZE * k + slot::VERTICES, at);
                        placed[k] = true;
                    }
                }
                cpu_ordinal += 1;
            } else if !b.is_volatile() && !b.data.is_empty() {
                self.pad(0x80);
                vb_at[i] = self.put(&b.data);
            }
        }
        for (k, r) in m.records.iter().enumerate() {
            if let DataRef::Inline(data) = &r.vertices
                && !placed[k]
            {
                self.pad(0x80);
                let at = self.put(data);
                self.ptr(records_at + RECORD_SIZE * k + slot::VERTICES, at);
            }
        }
        // Index data, in buffer order; inline CPU index lists (record field
        // 0x70) take the place of their CPU buffer.
        let mut ib_at = vec![0u32; nib];
        let mut placed = vec![false; nrec];
        let mut cpu_ordinal = 0;
        for (i, b) in m.index_buffers.iter().enumerate() {
            if b.is_cpu() {
                let mut first = true;
                for (k, r) in m.records.iter().enumerate() {
                    if let DataRef::Inline(data) = &r.indices
                        && r.u32_at(0x70) as i32 == cpu_ordinal
                    {
                        self.pad(if first { 0x80 } else { 2 });
                        first = false;
                        let at = self.put(data);
                        self.ptr(records_at + RECORD_SIZE * k + slot::INDICES, at);
                        placed[k] = true;
                    }
                }
                cpu_ordinal += 1;
            } else if !b.data.is_empty() {
                self.pad(0x80);
                ib_at[i] = self.put(&b.data);
            }
        }
        for (k, r) in m.records.iter().enumerate() {
            if let DataRef::Inline(data) = &r.indices
                && !placed[k]
            {
                self.pad(0x80);
                let at = self.put(data);
                self.ptr(records_at + RECORD_SIZE * k + slot::INDICES, at);
            }
        }
        // Per-vertex bytes.
        for (i, r) in m.records.iter().enumerate() {
            if let Some(bytes) = &r.vertex_layer_bias {
                self.pad(0x80);
                let at = self.put(bytes);
                self.ptr(records_at + RECORD_SIZE * i + slot::VERTEX_LAYER_BIAS, at);
            }
        }
        // Grids.
        for (i, r) in m.records.iter().enumerate() {
            if let Some(grid) = &r.grid {
                self.pad(0x10);
                let at = self.put(grid);
                self.ptr(records_at + RECORD_SIZE * i + slot::GRID, at);
            }
        }
        self.pad(0x80);
        // Morph deltas.
        let mut first = true;
        for (i, r) in m.records.iter().enumerate() {
            if let Some(deltas) = &r.morph_deltas {
                self.pad(if first { 0x80 } else { 0x4 });
                first = false;
                let at = self.put(deltas);
                self.ptr(records_at + RECORD_SIZE * i + slot::MORPH_DELTAS, at);
            }
        }
        self.pad(0x80);
        // Palettes.
        let mut first = true;
        for (i, r) in m.records.iter().enumerate() {
            if !r.palette.is_empty() {
                self.pad(if first { 0x10 } else { 0x2 });
                first = false;
                let bytes: Vec<u8> = r.palette.iter().flat_map(|&(a, b)| [a, b]).collect();
                let at = self.put(&bytes);
                self.ptr(records_at + RECORD_SIZE * i + slot::PALETTE, at);
            }
        }
        // Bone table.
        let mut bones_at = 0;
        if !m.bones.is_empty() {
            self.pad(0x10);
            let bytes: Vec<u8> = m
                .bones
                .iter()
                .flat_map(|&(h, i)| h.to_le_bytes().into_iter().chain(i.to_le_bytes()))
                .collect();
            bones_at = self.put(&bytes);
        }
        // Strings.
        self.pad(0x10);
        let mut strings: HashMap<&str, u32> = HashMap::new();
        let mut string = |w: &mut Self, s: &'a str| -> u32 {
            *strings.entry(s).or_insert_with(|| {
                let at = w.put(s.as_bytes());
                w.out.push(0);
                at
            })
        };
        let name_at = string(&mut self, &m.name);
        for (i, r) in m.records.iter().enumerate() {
            let at = string(&mut self, &r.name);
            self.ptr(records_at + RECORD_SIZE * i + slot::NAME, at);
        }

        // Header fields that are not pointers into the volatile data.
        self.set_u32(0, MAGIC);
        self.set_u32(4, VERSION);
        self.set_u32(8, m.platform);
        for (k, v) in m.bounding_sphere.iter().enumerate() {
            self.set_u32(0x10 + 4 * k, v.to_bits());
        }
        self.ptr(0x20, name_at);
        self.set_u32(0x24, if m.has_volatile { 16 } else { 0 });
        self.set_u32(0x34, nrec as u32);
        self.set_u32(0x38, m.bones.len() as u32);
        self.set_u16(0x3C, nvb as u16);
        self.set_u16(0x3E, nib as u16);
        let gpu = |l: &[BufferImage]| l.iter().filter(|b| !b.is_cpu()).count() as u16;
        let cpu = |l: &[BufferImage]| l.iter().filter(|b| b.is_cpu()).count() as u16;
        self.set_u16(0x40, gpu(&m.vertex_buffers));
        self.set_u16(0x42, gpu(&m.index_buffers));
        self.set_u16(0x44, cpu(&m.vertex_buffers));
        self.set_u16(0x46, cpu(&m.index_buffers));
        self.ptr(0x50, records_at as u32);
        self.ptr(0x54, bounds_at as u32);
        self.ptr(0x60, bones_at);
        self.ptr(0x68, vbs_at as u32);
        self.ptr(0x6C, ibs_at as u32);
        self.ptr(0x78, area_a as u32);
        self.ptr(0x7C, area_b as u32);
        for (k, v) in m.aabb_min.iter().chain(&m.aabb_max).enumerate() {
            self.set_u32(0xA0 + 4 * k, v.to_bits());
        }

        // The relocation table has one entry per pointer. Pointers into the
        // buffers are only known once the volatile data is placed after the
        // table, so count them first.
        let has_data = |b: &BufferImage| !b.is_cpu() && !b.data.is_empty();
        let buffer_pointers = m.vertex_buffers.iter().filter(|b| has_data(b)).count()
            + m.index_buffers.iter().filter(|b| has_data(b)).count();
        let record_pointers = m
            .records
            .iter()
            .map(|r| {
                usize::from(matches!(r.vertices, DataRef::Buffer { .. }))
                    + usize::from(matches!(r.indices, DataRef::Buffer { .. }))
            })
            .sum::<usize>();
        // + 1: the pointer to the table itself (0x4C).
        let count = self.relocations.len() + 1 + buffer_pointers + record_pointers;
        self.pad(0x10);
        let relocs_at = self.out.len();
        self.out.resize(relocs_at + 8 * count, 0);
        self.pad(0x80);
        let mesh_len = self.out.len();
        self.ptr(0x4C, relocs_at as u32);
        self.set_u32(0x48, count as u32);

        // Volatile data.
        let mut volatile = Vec::new();
        for (i, b) in m.vertex_buffers.iter().enumerate() {
            if has_data(b) && b.is_volatile() {
                volatile.resize(align(volatile.len(), 4), 0);
                vb_at[i] = (mesh_len + volatile.len()) as u32;
                volatile.extend_from_slice(&b.data);
            }
        }
        if !volatile.is_empty() && !m.has_volatile {
            return Err(Error::InvalidArgument(
                "volatile vertex data in a mesh without meshvolatile".into(),
            ));
        }
        volatile.resize(align(volatile.len(), 0x80), 0);

        for (i, &at) in vb_at.iter().enumerate() {
            self.ptr(vbs_at + BUFFER_SIZE * i + 12, at);
        }
        for (i, &at) in ib_at.iter().enumerate() {
            self.ptr(ibs_at + BUFFER_SIZE * i + 12, at);
        }
        for (i, r) in m.records.iter().enumerate() {
            let base = records_at + RECORD_SIZE * i;
            if let DataRef::Buffer { buffer, offset } = r.vertices {
                let at = vb_at.get(buffer).copied().unwrap_or(0);
                if at == 0 {
                    return Err(Error::InvalidArgument(format!(
                        "record `{}` uses vertex buffer {buffer} without data",
                        r.name
                    )));
                }
                self.ptr(base + slot::VERTICES, at + offset);
            }
            if let DataRef::Buffer { buffer, offset } = r.indices {
                let at = ib_at.get(buffer).copied().unwrap_or(0);
                if at == 0 {
                    return Err(Error::InvalidArgument(format!(
                        "record `{}` uses index buffer {buffer} without data",
                        r.name
                    )));
                }
                self.ptr(base + slot::INDICES, at + offset);
            }
        }
        debug_assert_eq!(self.relocations.len(), count);
        self.relocations.sort_unstable();
        let table: Vec<u8> = self
            .relocations
            .iter()
            .flat_map(|&(s, v)| s.to_le_bytes().into_iter().chain(v.to_le_bytes()))
            .collect();
        self.out[relocs_at..relocs_at + table.len()].copy_from_slice(&table);
        let total = mesh_len + volatile.len();
        self.set_u32(0x0C, total as u32);
        self.set_u32(0x9C, if m.has_volatile { mesh_len as u32 } else { 0 });
        Ok((self.out, volatile))
    }
}

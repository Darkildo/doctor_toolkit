//! `simgroup` resources: placed game objects and their attributes (magic `SimG`).
//!
//! # Layout
//!
//! All values are little-endian. Every pointer inside the resource is
//! self-relative: the target is the position of the pointer plus its value.
//!
//! ```text
//! 0x00  8 zero bytes
//! 0x08  u32  magic 0x53696D47 ("GmiS" on disk)
//! 0x0C  u32  version, 1
//! 0x10  u32  group hash (input unknown)
//! 0x14  u32  flags, 1
//! 0x18  u32  object count N
//! 0x1C  u32  0
//! 0x20  u32  aux region size
//! 0x24  u32  string region size
//! 0x28  u32  object table offset, 0x30
//! 0x2C  u32  0
//! 0x30  N × u32  object offsets (absolute, ascending)
//!       aux region: 16-byte GUIDs, 12-byte vectors, 64-byte matrices
//!       string region: a NUL, then NUL-terminated strings, padded to 4 bytes
//!       attribute packets, back to back
//!       objects, back to back up to the end of the resource
//! ```
//!
//! An attribute packet holds the attributes one class of the hierarchy adds:
//!
//! ```text
//! u32  class hash: SDBM of the lower-case class name
//! u16  attribute count A
//! ⌈A/8⌉ bytes  default mask: bit i set = attribute i is zero and not stored
//!       zero padding to a multiple of 4 bytes from the packet start
//! one u32 per stored attribute: a scalar, or a self-relative pointer into the
//!       string region (strings, event names) or the aux region (GUIDs,
//!       vectors, matrices)
//! ```
//!
//! An object:
//!
//! ```text
//! 0x00  28 zero bytes (runtime state)
//! 0x1C  u8   flags: bit 0 = serialised form (always set), bit 3 = has components
//! 0x1D  u8   second flag byte, 0 or 1
//! 0x1E  u8   link count L
//! 0x1F  u8   packet count P
//! 0x20  i32  pointer to the component packet, 0 without components
//! 0x24  i32  pointer to the object GUID in the aux region
//! 0x28  u32  class hash
//! 0x2C  L × i32  pointers to GUIDs in the aux region
//!       P × i32  pointers to packets, base class first
//! ```
//!
//! Packets are shared between objects with equal attributes. The component
//! packet lists the class hashes of the components attached to the object.
//!
//! Attribute values carry no type. The kind of every attribute was inferred
//! from the game files (`research/simgroup_schema.py`) and is kept in
//! `data/gf2/simgroup_schema.txt`; attributes missing there are classified by
//! the region their value points into.

use std::collections::HashMap;
use std::fmt;
use std::sync::OnceLock;

use serde::{Serialize, Serializer};

use crate::io::{self, Endian};
use crate::resource::names::sim_classes;
use crate::{Error, Result};

/// The magic as read as a little-endian `u32`.
pub const MAGIC: u32 = 0x5369_6D47;
const HEADER_SIZE: usize = 0x30;
const OBJECT_HEADER_SIZE: usize = 0x2C;
const GUID_SIZE: usize = 16;
const MAX_UNKNOWN_BLOB: usize = 64;

static GF2_SCHEMA: &str = include_str!("../../../../data/gf2/simgroup_schema.txt");

/// A 128-bit identifier.
///
/// Object GUIDs are NeoGuids: the name of the machine that created the
/// object, 10 ASCII letters and digits starting with an implicit `X`
/// (`XCHRIST427`), and two 28-bit keys. As little-endian words `w0..w3`:
/// `w0 = name[9] << 24 | key1 >> 4`, `w1 = (key1 & 0xF) << 28 | key2`, and
/// bytes 8..16 are `name[1..9]`. The text form is `<name>_<key1>_<key2>_v3`
/// with the keys as 7 hexadecimal digits, as in message names such as
/// `XCHAVEZ197_C8A0137_6EDD352_v3_social_started`. Checked on all 111 518
/// object GUIDs of the game (127 machines). A reference to a resource (an
/// asset) is the 32-bit resource id repeated four times, see
/// [`Guid::resource_id`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Guid(pub [u8; 16]);

impl Guid {
    /// The resource id when the GUID is a resource reference: four equal
    /// non-zero little-endian words. The id is `ids[3]` of the resource
    /// header (the last id shown by `list`).
    pub fn resource_id(&self) -> Option<u32> {
        let w =
            |k: usize| u32::from_le_bytes(self.0[4 * k..4 * k + 4].try_into().expect("4 bytes"));
        let id = w(0);
        (id != 0 && (1..4).all(|k| w(k) == id)).then_some(id)
    }

    /// The GUID that references the resource with id `id`.
    pub fn from_resource_id(id: u32) -> Self {
        let mut bytes = [0; 16];
        for chunk in bytes.chunks_exact_mut(4) {
            chunk.copy_from_slice(&id.to_le_bytes());
        }
        Self(bytes)
    }

    pub fn is_nil(&self) -> bool {
        self.0 == [0; 16]
    }
}

impl fmt::Display for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

impl Serialize for Guid {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl std::str::FromStr for Guid {
    type Err = Error;

    /// 32 hexadecimal digits, the form [`Display`](fmt::Display) writes.
    fn from_str(s: &str) -> Result<Self> {
        let s = s.trim();
        let bad = || Error::InvalidArgument(format!("GUID `{s}`: 32 hexadecimal digits expected"));
        if s.len() != 32 || !s.is_ascii() {
            return Err(bad());
        }
        let mut bytes = [0u8; 16];
        for (k, b) in bytes.iter_mut().enumerate() {
            *b = u8::from_str_radix(&s[2 * k..2 * k + 2], 16).map_err(|_| bad())?;
        }
        Ok(Self(bytes))
    }
}

impl<'de> serde::Deserialize<'de> for Guid {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// A 32-bit hash, written as eight hexadecimal digits in JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Hash32(pub u32);

impl fmt::Display for Hash32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:08x}", self.0)
    }
}

impl Serialize for Hash32 {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for Hash32 {
    /// Eight hexadecimal digits (an optional `0x` prefix is accepted).
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        u32::from_str_radix(text.trim_start_matches("0x"), 16)
            .map(Hash32)
            .map_err(|_| serde::de::Error::custom(format!("`{text}` is not a hexadecimal hash")))
    }
}

/// Kind of an attribute value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Int,
    Float,
    Hash,
    String,
    Guid,
    Vec3,
    Matrix,
}

impl Kind {
    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "int" => Self::Int,
            "float" => Self::Float,
            "hash" => Self::Hash,
            "string" => Self::String,
            "guid" => Self::Guid,
            "vec3" => Self::Vec3,
            "matrix" => Self::Matrix,
            _ => return None,
        })
    }

    /// Size of the aux blob a value of this kind points to.
    fn aux_size(self) -> Option<usize> {
        match self {
            Self::Guid => Some(GUID_SIZE),
            Self::Vec3 => Some(12),
            Self::Matrix => Some(64),
            _ => None,
        }
    }
}

/// What is known about one attribute of a class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttrInfo {
    pub kind: Kind,
    /// Name, when a metamodel of the game names the attribute.
    pub name: Option<&'static str>,
}

/// Attribute kinds and names keyed by `(class hash, attribute index)`.
#[derive(Debug, Default)]
pub struct Schema {
    attrs: HashMap<(u32, u16), AttrInfo>,
}

impl Schema {
    /// Parses lines `<class hash> <index> <kind> <count> [<name>]`; `#` starts a comment.
    pub fn from_text(text: &'static str) -> Self {
        let mut attrs = HashMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut cols = line.split_whitespace();
            let (Some(class), Some(index), Some(kind)) = (cols.next(), cols.next(), cols.next())
            else {
                continue;
            };
            let name = cols.nth(1);
            if let (Ok(class), Ok(index), Some(kind)) = (
                u32::from_str_radix(class, 16),
                index.parse(),
                Kind::parse(kind),
            ) {
                attrs.insert((class, index), AttrInfo { kind, name });
            }
        }
        Self { attrs }
    }

    /// The schema of The Godfather II (PC), with the attributes of the
    /// classes of native mods named by `DOCTOR_TOOLKIT_CLASSES`
    /// (`game_types::custom`).
    pub fn gf2() -> &'static Self {
        static SCHEMA: OnceLock<Schema> = OnceLock::new();
        SCHEMA.get_or_init(|| {
            let mut schema = Self::from_text(GF2_SCHEMA);
            for class in crate::game_types::custom::installed() {
                for a in &class.attrs {
                    let name: &'static str = Box::leak(a.name.clone().into_boxed_str());
                    schema.attrs.insert(
                        (class.hash, a.index),
                        AttrInfo {
                            kind: a.kind,
                            name: Some(name),
                        },
                    );
                }
            }
            schema
        })
    }

    pub fn get(&self, class: u32, index: u16) -> Option<AttrInfo> {
        self.attrs.get(&(class, index)).copied()
    }

    pub fn len(&self) -> usize {
        self.attrs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.attrs.is_empty()
    }
}

/// A decoded attribute value.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "lowercase")]
pub enum Value {
    Int(i32),
    Float(f32),
    Hash(Hash32),
    String(String),
    Guid(Guid),
    Vec3([f32; 3]),
    /// Row-major 4×4 matrix; the last row is the translation.
    Matrix([[f32; 4]; 4]),
    /// Aux data of an attribute whose kind is not known; the size is a guess.
    Bytes(#[serde(serialize_with = "hex_bytes")] Vec<u8>),
}

fn hex_bytes<S: Serializer>(bytes: &[u8], s: S) -> std::result::Result<S::Ok, S::Error> {
    s.collect_str(&HexBytes(bytes))
}

struct HexBytes<'a>(&'a [u8]);

impl fmt::Display for HexBytes<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int(v) => write!(f, "{v}"),
            Self::Float(v) => write!(f, "{v}"),
            Self::Hash(v) => write!(f, "{v}"),
            Self::String(v) => write!(f, "{v:?}"),
            Self::Guid(v) => write!(f, "{v}"),
            Self::Vec3([x, y, z]) => write!(f, "({x}, {y}, {z})"),
            Self::Matrix(m) => {
                let t = m[3];
                write!(f, "matrix, translation ({}, {}, {})", t[0], t[1], t[2])
            }
            Self::Bytes(b) => write!(f, "{}", HexBytes(b)),
        }
    }
}

impl Value {
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Int(_) => "int",
            Self::Float(_) => "float",
            Self::Hash(_) => "hash",
            Self::String(_) => "string",
            Self::Guid(_) => "guid",
            Self::Vec3(_) => "vec3",
            Self::Matrix(_) => "matrix",
            Self::Bytes(_) => "bytes",
        }
    }
}

/// One stored (non-zero) attribute.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Attr {
    pub index: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<&'static str>,
    /// The stored `u32`: the scalar itself, or the self-relative pointer as
    /// read. For pointers [`SimGroup::encode`] writes [`Attr::target`] instead.
    #[serde(skip)]
    pub raw: u32,
    #[serde(flatten)]
    pub value: Value,
    /// Id of the resource a GUID value references (see [`Guid::resource_id`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<Hash32>,
    /// Where a pointer value points; `None` for scalars.
    #[serde(skip)]
    pub target: Option<Target>,
}

/// The data a pointer attribute points to: an offset into one region.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    /// Offset into [`SimGroup::aux`].
    Aux(usize),
    /// Offset into [`SimGroup::strings`].
    String(usize),
}

/// The attributes one class adds to an object.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Packet {
    /// Offset in the resource as read; 0 for packets made by an edit.
    #[serde(skip)]
    pub offset: usize,
    pub class: Hash32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class_name: Option<&'static str>,
    /// Number of attributes of the class, stored or not.
    pub attr_count: u16,
    /// Stored attributes; the others are zero.
    pub attrs: Vec<Attr>,
}

/// A placed object.
#[derive(Debug, Clone, PartialEq)]
pub struct Object {
    pub offset: usize,
    pub flags: u8,
    pub flags2: u8,
    pub class: u32,
    pub guid: Guid,
    /// Resources the object needs, as resource GUIDs (see [`Guid::resource_id`]):
    /// meshes of guns and items, `hkx` of cars, `wgi` of graph partitions.
    pub links: Vec<Guid>,
    /// Offset of [`Object::guid`] in the aux region.
    pub guid_ref: usize,
    /// Offsets of [`Object::links`] in the aux region.
    pub link_refs: Vec<usize>,
    /// Index into [`SimGroup::packets`] of the component list.
    pub components: Option<usize>,
    /// Indices into [`SimGroup::packets`], base class first.
    pub packets: Vec<usize>,
}

impl Object {
    pub fn class_name(&self) -> Option<&'static str> {
        sim_classes().get(self.class)
    }
}

/// A parsed SimGroup.
#[derive(Debug, Clone, PartialEq)]
pub struct SimGroup {
    pub version: u32,
    pub group_hash: u32,
    pub flags: u32,
    pub objects: Vec<Object>,
    /// Attribute packets in file order, shared by the objects.
    pub packets: Vec<Packet>,
    /// The aux region as stored: GUIDs, vectors and matrices. Edits append
    /// new blocks; [`SimGroup::encode`] drops the blocks nothing refers to.
    pub aux: Vec<u8>,
    /// The string region as stored. Edits append new strings;
    /// [`SimGroup::encode`] lays the region out again.
    pub strings: Vec<u8>,
    /// Set by edits that add data to the regions: [`SimGroup::encode`] then
    /// rebuilds them instead of writing them as stored.
    pub(crate) relayout: bool,
}

/// Class hash of `CSystemCommands`, the class that holds the transform.
pub const CLASS_SYSTEM_COMMANDS: u32 = 0xB390_B11A;
/// Class hash of `Entity`, the class that holds the model.
pub const CLASS_ENTITY: u32 = 0x3852_3FC3;
/// Attribute index of `CMD_LoadMatrix` in `CSystemCommands`.
pub const ATTR_LOAD_MATRIX: u16 = 1;
/// Attribute index of `CMD_SetModel` in `Entity`.
pub const ATTR_SET_MODEL: u16 = 1;

/// Regions of the resource, used to classify pointers.
struct Regions {
    aux: usize,
    strings: usize,
    packets: usize,
}

impl SimGroup {
    /// Parses a resource with the attribute kinds of The Godfather II.
    pub fn parse(data: &[u8]) -> Result<Self> {
        Self::parse_with(data, Schema::gf2())
    }

    pub fn parse_with(data: &[u8], schema: &Schema) -> Result<Self> {
        let le = Endian::Little;
        let u32_at = |o: usize| io::read_u32(data, o, le);
        let magic = u32_at(8)?;
        if magic != MAGIC {
            return Err(Error::BadMagic {
                what: "SimGroup",
                found: magic,
            });
        }
        let version = u32_at(0x0C)?;
        if version != 1 {
            return Err(Error::UnsupportedVersion {
                what: "SimGroup",
                version,
            });
        }
        let group_hash = u32_at(0x10)?;
        let flags = u32_at(0x14)?;
        let count = u32_at(0x18)? as usize;
        let aux_size = u32_at(0x20)? as usize;
        let string_size = u32_at(0x24)? as usize;
        let table = u32_at(0x28)? as usize;
        // The table follows the header in every game file, and `encode`
        // writes it there; a table elsewhere would not survive a rewrite.
        if table != HEADER_SIZE {
            return Err(Error::malformed(
                "SimGroup",
                0x28,
                format!("object table at {table:#x}, not after the header"),
            ));
        }
        // Reject counts and sizes the file cannot hold before allocating.
        io::slice(data, table, count.saturating_mul(4))?;
        let aux = table + 4 * count;
        let strings = aux
            .checked_add(aux_size)
            .ok_or_else(|| Error::malformed("SimGroup", 0x20, "aux size overflows"))?;
        let packets_at = strings
            .checked_add(string_size)
            .ok_or_else(|| Error::malformed("SimGroup", 0x24, "string size overflows"))?;
        io::slice(data, aux, packets_at - aux)?;
        let offsets = (0..count)
            .map(|i| u32_at(table + 4 * i).map(|o| o as usize))
            .collect::<Result<Vec<_>>>()?;
        let first_object = offsets.first().copied().unwrap_or(data.len());
        if first_object < packets_at || first_object > data.len() {
            return Err(Error::malformed(
                "SimGroup",
                table,
                "object before the packets",
            ));
        }
        let regions = Regions {
            aux,
            strings,
            packets: packets_at,
        };

        // Packets: raw values first; decoding needs every aux pointer.
        let mut raw_packets = Vec::new();
        let mut at = packets_at;
        while at < first_object {
            let raw = RawPacket::read(data, at, first_object)?;
            at = raw.end;
            raw_packets.push(raw);
        }
        let by_offset: HashMap<usize, usize> = raw_packets
            .iter()
            .enumerate()
            .map(|(i, p)| (p.offset, i))
            .collect();

        let mut aux_targets = Vec::new();
        let mut objects = Vec::with_capacity(count);
        let mut expected = first_object;
        for (i, &o) in offsets.iter().enumerate() {
            if o != expected {
                return Err(Error::malformed(
                    "SimGroup",
                    table + 4 * i,
                    "objects not contiguous",
                ));
            }
            let head = io::slice(data, o, OBJECT_HEADER_SIZE)?;
            let (obj_flags, flags2, links, packet_count) =
                (head[0x1C], head[0x1D], head[0x1E], head[0x1F]);
            if obj_flags & 1 == 0 {
                return Err(Error::malformed(
                    "SimGroup",
                    o + 0x1C,
                    "object not in serialised form",
                ));
            }
            let packet_at = |field: usize| -> Result<usize> {
                let target = rel(data, field)?;
                by_offset.get(&target).copied().ok_or_else(|| {
                    Error::malformed("SimGroup", field, "pointer to a packet expected")
                })
            };
            let components = if io::read_u32(data, o + 0x20, le)? != 0 {
                Some(packet_at(o + 0x20)?)
            } else {
                None
            };
            let guid_at = rel(data, o + 0x24)?;
            aux_targets.push(guid_at);
            let guid = read_guid(data, guid_at, &regions, o + 0x24)?;
            let class = u32_at(o + 0x28)?;
            let mut link_guids = Vec::with_capacity(links as usize);
            let mut link_refs = Vec::with_capacity(links as usize);
            for k in 0..links as usize {
                let field = o + OBJECT_HEADER_SIZE + 4 * k;
                let target = rel(data, field)?;
                aux_targets.push(target);
                link_guids.push(read_guid(data, target, &regions, field)?);
                link_refs.push(target - aux);
            }
            let packets = (0..packet_count as usize)
                .map(|k| packet_at(o + OBJECT_HEADER_SIZE + 4 * (links as usize + k)))
                .collect::<Result<Vec<_>>>()?;
            expected = o + OBJECT_HEADER_SIZE + 4 * (links as usize + packet_count as usize);
            objects.push(Object {
                offset: o,
                flags: obj_flags,
                flags2,
                class,
                guid,
                links: link_guids,
                guid_ref: guid_at - aux,
                link_refs,
                components,
                packets,
            });
        }
        if expected != data.len() {
            return Err(Error::malformed(
                "SimGroup",
                expected,
                "data after the last object",
            ));
        }

        for p in &raw_packets {
            for &(_, slot, value) in &p.values {
                let target = slot.wrapping_add(value as i32 as isize as usize);
                if (regions.aux..regions.strings).contains(&target) {
                    aux_targets.push(target);
                }
            }
        }
        aux_targets.sort_unstable();
        aux_targets.dedup();

        let packets = raw_packets
            .into_iter()
            .map(|raw| raw.decode(data, &regions, &aux_targets, schema))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            version,
            group_hash,
            flags,
            objects,
            packets,
            aux: data[aux..strings].to_vec(),
            strings: data[strings..packets_at].to_vec(),
            relayout: false,
        })
    }

    /// Writes the group back in the layout [`SimGroup::parse`] reads.
    ///
    /// Pointers are written from [`Attr::target`], [`Object::guid_ref`] and
    /// [`Object::link_refs`], so packets may change size, order and sharing.
    /// After an edit that added data (see the `set_*` methods), the aux
    /// region loses the blocks nothing refers to any more and the string
    /// region is laid out again: a NUL, then every referenced string once,
    /// in the order of the first reference, padded to 4 bytes (the layout of
    /// every SimGroup of the game). Unchanged groups are reproduced byte for byte.
    pub fn encode(&self) -> Vec<u8> {
        let le = Endian::Little;
        let count = self.objects.len();
        let aux = HEADER_SIZE + 4 * count;
        let aux_layout = AuxLayout::new(self);
        let string_layout = StringLayout::new(self);
        let aux_region = aux_layout.region(&self.aux);
        let string_region = string_layout.region(&self.strings);
        let strings = aux + aux_region.len();
        let mut out = vec![0u8; 8];
        for v in [
            MAGIC,
            self.version,
            self.group_hash,
            self.flags,
            count as u32,
            0,
            aux_region.len() as u32,
            string_region.len() as u32,
            HEADER_SIZE as u32,
            0,
        ] {
            io::put_u32(&mut out, v, le);
        }
        out.resize(aux, 0);
        out.extend_from_slice(&aux_region);
        out.extend_from_slice(string_region);
        let rel_to = |field: usize, target: usize| (target as i64 - field as i64) as i32 as u32;
        let mut packet_at = Vec::with_capacity(self.packets.len());
        for p in &self.packets {
            let start = out.len();
            packet_at.push(start);
            io::put_u32(&mut out, p.class.0, le);
            io::put_u16(&mut out, p.attr_count, le);
            let mut mask = vec![0xFFu8; (p.attr_count as usize).div_ceil(8)];
            for i in p.attr_count..(mask.len() * 8) as u16 {
                mask[i as usize / 8] &= !(1 << (i % 8));
            }
            for a in &p.attrs {
                mask[a.index as usize / 8] &= !(1 << (a.index % 8));
            }
            out.extend_from_slice(&mask);
            // The values start at a file offset aligned to 4, as the reader
            // takes them (the same as aligned to the packet in the game's
            // files, whose packets all start aligned).
            out.resize(io::align_up(out.len(), 4), 0);
            for a in &p.attrs {
                let field = out.len();
                let value = match a.target {
                    Some(Target::Aux(o)) => rel_to(field, aux + aux_layout.map(o)),
                    Some(Target::String(o)) => rel_to(field, strings + string_layout.map(o)),
                    None => a.raw,
                };
                io::put_u32(&mut out, value, le);
            }
        }
        for (i, o) in self.objects.iter().enumerate() {
            let at = out.len();
            out[HEADER_SIZE + 4 * i..HEADER_SIZE + 4 * i + 4]
                .copy_from_slice(&(at as u32).to_le_bytes());
            out.extend_from_slice(&[0; 0x1C]);
            out.extend_from_slice(&[
                o.flags,
                o.flags2,
                o.links.len() as u8,
                o.packets.len() as u8,
            ]);
            let comp = o.components.map_or(0, |c| rel_to(at + 0x20, packet_at[c]));
            io::put_u32(&mut out, comp, le);
            let guid = aux + aux_layout.map(o.guid_ref);
            io::put_u32(&mut out, rel_to(at + 0x24, guid), le);
            io::put_u32(&mut out, o.class, le);
            for &r in &o.link_refs {
                let field = out.len();
                io::put_u32(&mut out, rel_to(field, aux + aux_layout.map(r)), le);
            }
            for &p in &o.packets {
                let field = out.len();
                io::put_u32(&mut out, rel_to(field, packet_at[p]), le);
            }
        }
        out
    }

    /// Value of attribute `index` of class `class` in `object`, if stored.
    pub fn attr<'a>(&'a self, object: &'a Object, class: u32, index: u16) -> Option<&'a Value> {
        self.object_packets(object)
            .filter(|p| p.class.0 == class)
            .flat_map(|p| &p.attrs)
            .find(|a| a.index == index)
            .map(|a| &a.value)
    }

    /// World transform of `object` (`CSystemCommands.CMD_LoadMatrix`): a
    /// row-major 4×4 matrix with rows right, up, at, position; Y is up.
    pub fn transform(&self, object: &Object) -> Option<[[f32; 4]; 4]> {
        match self.attr(object, CLASS_SYSTEM_COMMANDS, ATTR_LOAD_MATRIX) {
            Some(Value::Matrix(m)) => Some(*m),
            _ => None,
        }
    }

    /// Id of the `metamodel` resource an `Entity` object shows (`CMD_SetModel`).
    pub fn model(&self, object: &Object) -> Option<u32> {
        match self.attr(object, CLASS_ENTITY, ATTR_SET_MODEL) {
            Some(Value::Guid(g)) => g.resource_id(),
            _ => None,
        }
    }

    /// Packets of `object`, base class first.
    pub fn object_packets<'a>(&'a self, object: &'a Object) -> impl Iterator<Item = &'a Packet> {
        object.packets.iter().map(|&i| &self.packets[i])
    }

    /// Value of the first attribute named `name` of `object`.
    pub fn find_attr<'a>(&'a self, object: &'a Object, name: &str) -> Option<&'a Value> {
        self.object_packets(object)
            .flat_map(|p| &p.attrs)
            .find(|a| a.name == Some(name))
            .map(|a| &a.value)
    }

    /// A display name for `object`: its `CMD_SetName` string when it has one.
    pub fn object_name<'a>(&'a self, object: &'a Object) -> Option<&'a str> {
        match self.find_attr(object, "CMD_SetName") {
            Some(Value::String(s)) => Some(s),
            _ => None,
        }
    }
}

/// Where the aux blocks go on write: as stored, or without the blocks
/// nothing refers to.
struct AuxLayout {
    /// Kept runs of the stored region: `(start, end, new start)`, ascending.
    /// Empty when the region is written as stored.
    runs: Vec<(usize, usize, usize)>,
}

impl AuxLayout {
    fn new(group: &SimGroup) -> Self {
        let identity = Self { runs: Vec::new() };
        if !group.relayout {
            return identity;
        }
        let mut used = vec![false; group.aux.len()];
        let mut mark = |at: usize, size: usize| {
            let end = (at + size).min(used.len());
            used[at.min(end)..end].fill(true);
        };
        for o in &group.objects {
            mark(o.guid_ref, GUID_SIZE);
            for &r in &o.link_refs {
                mark(r, GUID_SIZE);
            }
        }
        for a in group.packets.iter().flat_map(|p| &p.attrs) {
            let size = match &a.value {
                Value::Guid(_) => GUID_SIZE,
                Value::Vec3(_) => 12,
                Value::Matrix(_) => 64,
                // The true size of an untyped blob is a guess: keep everything.
                Value::Bytes(_) => return identity,
                _ => continue,
            };
            if let Some(Target::Aux(at)) = a.target {
                mark(at, size);
            }
        }
        let mut runs = Vec::new();
        let mut kept = 0;
        let mut at = 0;
        while at < used.len() {
            let len = used[at..].iter().take_while(|&&u| u == used[at]).count();
            if used[at] {
                runs.push((at, at + len, kept));
                kept += len;
            }
            at += len;
        }
        Self { runs }
    }

    /// New offset of a referenced stored offset.
    fn map(&self, at: usize) -> usize {
        if self.runs.is_empty() {
            return at;
        }
        let i = self.runs.partition_point(|&(start, _, _)| start <= at);
        match i.checked_sub(1).map(|i| self.runs[i]) {
            Some((start, end, new)) if at < end => new + (at - start),
            _ => at,
        }
    }

    fn region<'a>(&self, aux: &'a [u8]) -> std::borrow::Cow<'a, [u8]> {
        if self.runs.is_empty() {
            return aux.into();
        }
        let mut out = Vec::with_capacity(aux.len());
        for &(start, end, _) in &self.runs {
            out.extend_from_slice(&aux[start..end]);
        }
        out.into()
    }
}

/// Where the strings go on write: as stored, or laid out again.
struct StringLayout {
    /// The new region and the new offset of every referenced stored offset;
    /// `None` when the region is written as stored.
    rebuilt: Option<(Vec<u8>, HashMap<usize, usize>)>,
}

impl StringLayout {
    fn new(group: &SimGroup) -> Self {
        if !group.relayout {
            return Self { rebuilt: None };
        }
        let mut region = vec![0u8];
        let mut map = HashMap::new();
        let mut by_text: HashMap<&[u8], usize> = HashMap::new();
        for a in group.packets.iter().flat_map(|p| &p.attrs) {
            let Some(Target::String(at)) = a.target else {
                continue;
            };
            if map.contains_key(&at) {
                continue;
            }
            let rest = group.strings.get(at..).unwrap_or_default();
            let text = &rest[..rest.iter().position(|&b| b == 0).unwrap_or(rest.len())];
            let new = *by_text.entry(text).or_insert_with(|| {
                let new = region.len();
                region.extend_from_slice(text);
                region.push(0);
                new
            });
            map.insert(at, new);
        }
        region.resize(io::align_up(region.len(), 4), 0);
        Self {
            rebuilt: Some((region, map)),
        }
    }

    fn map(&self, at: usize) -> usize {
        match &self.rebuilt {
            Some((_, map)) => map.get(&at).copied().unwrap_or(0),
            None => at,
        }
    }

    fn region<'a>(&'a self, strings: &'a [u8]) -> &'a [u8] {
        match &self.rebuilt {
            Some((region, _)) => region,
            None => strings,
        }
    }
}

fn rel(data: &[u8], field: usize) -> Result<usize> {
    let value = io::read_u32(data, field, Endian::Little)? as i32;
    field
        .checked_add_signed(value as isize)
        .ok_or_else(|| Error::malformed("SimGroup", field, "pointer out of range"))
}

fn read_guid(data: &[u8], at: usize, regions: &Regions, field: usize) -> Result<Guid> {
    if at < regions.aux || at + GUID_SIZE > regions.strings {
        return Err(Error::malformed(
            "SimGroup",
            field,
            "GUID outside the aux region",
        ));
    }
    Ok(Guid(
        io::slice(data, at, GUID_SIZE)?
            .try_into()
            .expect("16 bytes"),
    ))
}

struct RawPacket {
    offset: usize,
    end: usize,
    class: u32,
    count: u16,
    /// `(attribute index, slot offset, stored value)`.
    values: Vec<(u16, usize, u32)>,
}

impl RawPacket {
    fn read(data: &[u8], at: usize, limit: usize) -> Result<Self> {
        let le = Endian::Little;
        let class = io::read_u32(data, at, le)?;
        let count = io::read_u16(data, at + 4, le)?;
        let mask = io::slice(data, at + 6, (count as usize).div_ceil(8))?;
        let mut slot = io::align_up(at + 6 + mask.len(), 4);
        if slot > limit {
            return Err(Error::malformed(
                "SimGroup",
                at,
                "packet runs into the objects",
            ));
        }
        let mut values = Vec::new();
        for index in 0..count {
            if mask[index as usize / 8] >> (index % 8) & 1 != 0 {
                continue;
            }
            if slot + 4 > limit {
                return Err(Error::malformed(
                    "SimGroup",
                    at,
                    "packet runs into the objects",
                ));
            }
            values.push((index, slot, io::read_u32(data, slot, le)?));
            slot += 4;
        }
        Ok(Self {
            offset: at,
            end: slot,
            class,
            count,
            values,
        })
    }

    fn decode(
        self,
        data: &[u8],
        regions: &Regions,
        aux_targets: &[usize],
        schema: &Schema,
    ) -> Result<Packet> {
        let attrs = self
            .values
            .iter()
            .map(|&(index, slot, raw)| {
                let info = schema.get(self.class, index);
                let target = slot.wrapping_add(raw as i32 as isize as usize);
                let value = decode_value(
                    data,
                    regions,
                    aux_targets,
                    info.map(|i| i.kind),
                    raw,
                    target,
                )?;
                let resource = match &value {
                    Value::Guid(g) => g.resource_id().map(Hash32),
                    _ => None,
                };
                let target = match &value {
                    Value::String(_) => Some(Target::String(target - regions.strings)),
                    Value::Guid(_) | Value::Vec3(_) | Value::Matrix(_) | Value::Bytes(_) => {
                        Some(Target::Aux(target - regions.aux))
                    }
                    _ => None,
                };
                Ok(Attr {
                    index,
                    name: info.and_then(|i| i.name),
                    raw,
                    value,
                    resource,
                    target,
                })
            })
            .collect::<Result<_>>()?;
        Ok(Packet {
            offset: self.offset,
            class: Hash32(self.class),
            class_name: sim_classes().get(self.class),
            attr_count: self.count,
            attrs,
        })
    }
}

fn decode_value(
    data: &[u8],
    regions: &Regions,
    aux_targets: &[usize],
    kind: Option<Kind>,
    raw: u32,
    target: usize,
) -> Result<Value> {
    let in_strings = (regions.strings..regions.packets).contains(&target);
    let in_aux = (regions.aux..regions.strings).contains(&target);
    let floats = |at: usize, n: usize| -> Result<Vec<f32>> {
        (0..n)
            .map(|k| io::read_u32(data, at + 4 * k, Endian::Little).map(f32::from_bits))
            .collect()
    };
    let string = || -> Result<Value> {
        let bytes = io::cstr(&data[..regions.packets], target)?;
        Ok(Value::String(String::from_utf8_lossy(bytes).into_owned()))
    };
    Ok(match kind {
        Some(Kind::Int) => Value::Int(raw as i32),
        Some(Kind::Float) => Value::Float(f32::from_bits(raw)),
        Some(Kind::Hash) => Value::Hash(Hash32(raw)),
        Some(Kind::String) if in_strings => string()?,
        Some(k @ (Kind::Guid | Kind::Vec3 | Kind::Matrix))
            if in_aux && target + k.aux_size().unwrap_or(0) <= regions.strings =>
        {
            match k {
                Kind::Guid => Value::Guid(Guid(
                    io::slice(data, target, GUID_SIZE)?
                        .try_into()
                        .expect("16 bytes"),
                )),
                Kind::Vec3 => Value::Vec3(floats(target, 3)?.try_into().expect("3 floats")),
                _ => {
                    let m = floats(target, 16)?;
                    Value::Matrix(std::array::from_fn(|r| {
                        std::array::from_fn(|c| m[4 * r + c])
                    }))
                }
            }
        }
        // Unknown attribute, or a value that does not fit its kind.
        _ if in_strings => string()?,
        _ if in_aux => {
            let next = aux_targets
                .iter()
                .find(|&&t| t > target)
                .copied()
                .unwrap_or(regions.strings);
            let end = next.min(regions.strings).min(target + MAX_UNKNOWN_BLOB);
            Value::Bytes(io::slice(data, target, end - target)?.to_vec())
        }
        _ => Value::Hash(Hash32(raw)),
    })
}

/// JSON form: objects with their packets resolved.
impl Serialize for SimGroup {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Doc<'a> {
            version: u32,
            group_hash: Hash32,
            flags: u32,
            objects: Vec<ObjectJson<'a>>,
        }
        #[derive(Serialize)]
        struct ObjectJson<'a> {
            class: Hash32,
            #[serde(skip_serializing_if = "Option::is_none")]
            class_name: Option<&'static str>,
            guid: Guid,
            #[serde(skip_serializing_if = "Option::is_none")]
            name: Option<&'a str>,
            /// Translation row of the transform.
            #[serde(skip_serializing_if = "Option::is_none")]
            position: Option<[f32; 3]>,
            /// Resource id of the metamodel of an `Entity`.
            #[serde(skip_serializing_if = "Option::is_none")]
            model: Option<Hash32>,
            flags: u8,
            flags2: u8,
            /// Ids of the resources the object needs.
            #[serde(skip_serializing_if = "Vec::is_empty")]
            links: Vec<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            components: Option<Vec<ComponentJson>>,
            packets: Vec<&'a Packet>,
        }
        #[derive(Serialize)]
        struct ComponentJson {
            class: Hash32,
            #[serde(skip_serializing_if = "Option::is_none")]
            class_name: Option<&'static str>,
        }
        let objects = self
            .objects
            .iter()
            .map(|o| ObjectJson {
                class: Hash32(o.class),
                class_name: o.class_name(),
                guid: o.guid,
                name: self.object_name(o),
                position: self.transform(o).map(|m| [m[3][0], m[3][1], m[3][2]]),
                model: self.model(o).map(Hash32),
                flags: o.flags,
                flags2: o.flags2,
                links: o
                    .links
                    .iter()
                    .map(|g| match g.resource_id() {
                        Some(id) => Hash32(id).to_string(),
                        None => g.to_string(),
                    })
                    .collect(),
                components: o.components.map(|i| {
                    self.packets[i]
                        .attrs
                        .iter()
                        .map(|a| ComponentJson {
                            class: Hash32(a.raw),
                            class_name: sim_classes().get(a.raw),
                        })
                        .collect()
                }),
                packets: self.object_packets(o).collect(),
            })
            .collect();
        Doc {
            version: self.version,
            group_hash: Hash32(self.group_hash),
            flags: self.flags,
            objects,
        }
        .serialize(s)
    }
}

#[cfg(any(test, feature = "testing"))]
/// Builds a SimGroup with one `Entity`-like object of class 0x38523FC3:
/// packet A (class 0x11111111, 3 attributes: string, default, int) and
/// packet B (class 0x38523FC3, 2 attributes: guid, matrix).
pub(crate) fn sample_group() -> Vec<u8> {
    let mut aux = Vec::new();
    let guid: [u8; 16] = *b"\x01\x02\x03\x04\x05\x06\x07\x08CHRIST42";
    aux.extend_from_slice(&guid); // object guid at aux+0
    aux.extend_from_slice(b"\xAA\xBB\xCC\xDD\0\0\0\0MACHINE1"); // attribute guid at aux+16
    for k in 0..16 {
        aux.extend_from_slice(&(k as f32).to_le_bytes()); // matrix at aux+32
    }
    let strings = b"\0hello\0\0".to_vec();
    let count = 1usize;
    let aux_at = HEADER_SIZE + 4 * count;
    let str_at = aux_at + aux.len();
    let pk_at = str_at + strings.len();
    let mut out = vec![0u8; 8];
    for v in [
        MAGIC,
        1,
        0xDEAD_BEEF,
        1,
        count as u32,
        0,
        aux.len() as u32,
        strings.len() as u32,
        HEADER_SIZE as u32,
        0,
    ] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    // Object offset is patched below.
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&aux);
    out.extend_from_slice(&strings);
    let put_rel = |out: &mut Vec<u8>, target: usize| {
        let at = out.len();
        out.extend_from_slice(&((target as i64 - at as i64) as i32).to_le_bytes());
    };
    // Packet A.
    let packet_a = out.len();
    out.extend_from_slice(&0x1111_1111u32.to_le_bytes());
    out.extend_from_slice(&3u16.to_le_bytes());
    out.push(0b010); // attribute 1 is default
    out.push(0);
    put_rel(&mut out, str_at + 1); // "hello"
    out.extend_from_slice(&(-7i32).to_le_bytes());
    // Packet B.
    let packet_b = out.len();
    out.extend_from_slice(&0x3852_3FC3u32.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.push(0);
    out.push(0);
    put_rel(&mut out, aux_at + 16);
    put_rel(&mut out, aux_at + 32);
    assert_eq!(pk_at, packet_a);
    // Object.
    let object = out.len();
    out[HEADER_SIZE..HEADER_SIZE + 4].copy_from_slice(&(object as u32).to_le_bytes());
    out.extend_from_slice(&[0u8; 0x1C]);
    out.extend_from_slice(&[3, 1, 0, 2]);
    out.extend_from_slice(&0u32.to_le_bytes());
    put_rel(&mut out, aux_at);
    out.extend_from_slice(&0x3852_3FC3u32.to_le_bytes());
    put_rel(&mut out, packet_a);
    put_rel(&mut out, packet_b);
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The sample SimGroup of [`super::sample_group`].
    pub(crate) fn sample() -> Vec<u8> {
        sample_group()
    }

    pub(crate) fn schema() -> Schema {
        Schema::from_text(
            "# test\n11111111 0 string 1 CMD_SetName\n11111111 2 int 1\n\
             38523fc3 0 guid 1\n38523fc3 1 matrix 1 CMD_LoadMatrix\n",
        )
    }

    /// Found by fuzzing: an aux region of one byte puts the first packet at
    /// an odd offset; the writer pads its values as the reader takes them.
    #[test]
    fn a_packet_at_an_unaligned_offset_round_trips() {
        let data = [
            0xF7, 0x53, 0xFF, 0xFF, 0x0A, 0xFF, 0xFF, 0xFF, 0x47, 0x6D, 0x69, 0x53, 0x01, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x25, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x30, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x7E, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0E,
        ];
        let group = SimGroup::parse(&data).unwrap();
        let bytes = group.encode();
        let again = SimGroup::parse(&bytes).unwrap();
        assert_eq!(again.encode(), bytes);
    }

    #[test]
    fn parses_objects_and_attributes() {
        let group = SimGroup::parse_with(&sample(), &schema()).unwrap();
        assert_eq!(group.group_hash, 0xDEAD_BEEF);
        assert_eq!(group.objects.len(), 1);
        assert_eq!(group.packets.len(), 2);
        let object = &group.objects[0];
        assert_eq!(object.class_name(), Some("Entity"));
        assert_eq!(&object.guid.0[8..], b"CHRIST42");
        let a = &group.packets[object.packets[0]];
        assert_eq!(a.attr_count, 3);
        assert_eq!(a.attrs.len(), 2);
        assert_eq!(a.attrs[0].value, Value::String("hello".into()));
        assert_eq!(a.attrs[1].index, 2);
        assert_eq!(a.attrs[1].value, Value::Int(-7));
        let b = &group.packets[object.packets[1]];
        assert!(matches!(b.attrs[0].value, Value::Guid(g) if g.0[0] == 0xAA));
        let Value::Matrix(m) = b.attrs[1].value else {
            panic!("matrix expected");
        };
        assert_eq!(m[3], [12.0, 13.0, 14.0, 15.0]);
        assert_eq!(group.object_name(object), Some("hello"));
        let json = serde_json::to_value(&group).unwrap();
        assert_eq!(json["objects"][0]["class_name"], "Entity");
        assert_eq!(
            json["objects"][0]["packets"][0]["attrs"][0]["value"],
            "hello"
        );
        assert_eq!(
            json["objects"][0]["packets"][1]["attrs"][1]["type"],
            "matrix"
        );
    }

    #[test]
    fn encodes_back_to_the_same_bytes() {
        let data = sample();
        let group = SimGroup::parse_with(&data, &schema()).unwrap();
        assert_eq!(group.encode(), data);
    }

    #[test]
    fn finds_transforms_and_resource_references() {
        let mut data = sample();
        let first = SimGroup::parse_with(&data, &schema()).unwrap();
        let at = first.packets[1].offset;
        data[at..at + 4].copy_from_slice(&CLASS_SYSTEM_COMMANDS.to_le_bytes());
        let schema = Schema::from_text("b390b11a 0 guid 1\nb390b11a 1 matrix 1 CMD_LoadMatrix\n");
        let group = SimGroup::parse_with(&data, &schema).unwrap();
        let m = group.transform(&group.objects[0]).unwrap();
        assert_eq!(m[3], [12.0, 13.0, 14.0, 15.0]);
        assert_eq!(group.model(&group.objects[0]), None);
        assert_eq!(group.encode(), data);

        let r = Guid::from_resource_id(0x1234_5678);
        assert_eq!(r.resource_id(), Some(0x1234_5678));
        assert_eq!(Guid([7; 16]).resource_id(), Some(0x0707_0707));
        assert_eq!(Guid::default().resource_id(), None);
        assert_eq!(group.objects[0].guid.resource_id(), None);
    }

    #[test]
    fn unknown_attributes_are_classified_by_region() {
        let group = SimGroup::parse_with(&sample(), &Schema::default()).unwrap();
        let a = &group.packets[0];
        assert_eq!(a.attrs[0].value, Value::String("hello".into()));
        assert_eq!(a.attrs[1].value, Value::Hash(Hash32(-7i32 as u32)));
        let b = &group.packets[1];
        // The guid blob ends where the matrix starts.
        assert!(matches!(&b.attrs[0].value, Value::Bytes(v) if v.len() == 16));
        assert!(matches!(&b.attrs[1].value, Value::Bytes(v) if v.len() == 64));
    }

    #[test]
    fn rejects_bad_input() {
        let data = sample();
        for len in 0..data.len() {
            assert!(SimGroup::parse(&data[..len]).is_err());
        }
        let mut bad = data.clone();
        bad[8] ^= 1;
        assert!(matches!(SimGroup::parse(&bad), Err(Error::BadMagic { .. })));
        let mut huge = data.clone();
        huge[0x18..0x1C].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(SimGroup::parse(&huge).is_err());
        let mut far = data.clone();
        far[0x20..0x24].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
        assert!(SimGroup::parse(&far).is_err());
        // Found by fuzzing: a table further down parsed, but `encode` moved it
        // after the header and the packet pointers no longer fit.
        let mut moved = data.clone();
        moved[0x28..0x2C].copy_from_slice(&(HEADER_SIZE as u32 + 4).to_le_bytes());
        assert!(SimGroup::parse(&moved).is_err());
        // Corrupt every byte in turn: parsing must not panic.
        for i in 0..data.len() {
            let mut d = data.clone();
            d[i] ^= 0x80;
            let _ = SimGroup::parse(&d);
        }
    }
}

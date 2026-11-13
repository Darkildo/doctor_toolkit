//! `graph` resources: navigation graphs for AI (pedestrians, cars, patrols,
//! ground pathing, parking, audio occlusion).
//!
//! A graph is a memory image of the engine's graph object: the game checks
//! that the buffer is 16-byte aligned and that the word at +8 is 16, then
//! turns 17 offsets of the header into pointers (`godfather2.exe`
//! `0x005FA870`, fixup `0x00601410`). The GUID at +0x10 is overwritten with
//! the ids of the resource header when the graph is loaded.
//!
//! Every field and section below is checked on all 2 178 graphs of the game
//! (`docs/formats/navigation-graph.md`); [`Graph::parse`] followed by
//! [`Graph::to_bytes`] gives back the same bytes for all of them.
//!
//! # Header (0x94 bytes, little-endian, offsets from the start)
//!
//! ```text
//! 0x00  8 bytes  zero (runtime object header)
//! 0x08  u32  version: 16 (GF2); 14 and 15 differ in record sizes only
//! 0x0C  u16  nodes N, u16 links C
//! 0x10  16 bytes  GUID of the graph
//! 0x20  *nodes      0x24 *links       0x28 *neighbour GUIDs
//! 0x2C  u16 neighbour GUIDs, u16 neighbour links
//! 0x30  *neighbour links  0x34 *neighbour GUID indices  0x38 *runtime neighbour pointers
//! 0x3C  u16 neighbour slots, u16 R-tree nodes
//! 0x40  *precomputed paths  0x44 *R-tree node indices  0x48 *R-tree nodes
//! 0x4C  u16 property instance of the graph (0xFFFF none), u16 graph type flags
//! 0x50  *properties, u32 count   0x58 *property sets, u32 count
//! 0x60  *property instances, u32 count   0x68 *property data, u32 bytes
//! 0x70  *visibility bits, u32 bytes   0x78 u32 run-length flag
//! 0x7C  *lights  0x80 *light sets  0x84 u16 lights, u16 light sets
//! 0x88  u32 hash identity  0x8C *external hashes  0x90 u16 external count, u16 0
//! ```

mod derive;
mod edit;

use serde::{Deserialize, Serialize};

use crate::io::{self, Endian};
use crate::resource::simgroup::{Guid, Hash32};
use crate::{Error, Result};

pub use derive::{
    GraphInfo, LEAF_SIZE, RTreeNode, build_rtree, compute_paths, distance, paths_valid,
    visibility_bit,
};
pub use edit::Neighbourhood;

/// Type id of `graph`.
pub const TYPE_GRAPH: u32 = crate::hash::sdbm(b"graph");
/// The version GF2 loads.
pub const VERSION: u32 = 16;
/// Size of the header.
pub const HEADER_SIZE: usize = 0x94;
/// "No property instance".
pub const NO_PROPERTIES: u16 = 0xFFFF;
/// Most links one node can have (the count is a byte).
pub const MAX_LINKS: usize = 255;

/// Graph type flags (header 0x4E). Names as in the game's debug display
/// filter; 61 graphs of the game combine two bits (0x03, 0x09).
pub mod graph_flags {
    pub const GROUND_PATHING: u16 = 0x0001;
    pub const PATROL: u16 = 0x0002;
    pub const VEHICULAR: u16 = 0x0004;
    pub const PED: u16 = 0x0008;
    pub const VEHICULAR_PARKING: u16 = 0x0010;
    pub const AUDIO_OCCLUSION: u16 = 0x0020;

    /// Names of the bits, lowest first.
    pub const NAMES: [(u16, &str); 6] = [
        (GROUND_PATHING, "ground"),
        (PATROL, "patrol"),
        (VEHICULAR, "vehicle"),
        (PED, "ped"),
        (VEHICULAR_PARKING, "parking"),
        (AUDIO_OCCLUSION, "audio"),
    ];
}

/// Link flags (authored bits; the high bits are set by the game at run time).
pub mod link_flags {
    pub const MUST_VAULT: u16 = 0x0002;
    pub const MUST_LADDER_CLIMB: u16 = 0x0004;
    pub const PATHING_ONLY: u16 = 0x0008;
    pub const CROSSWALK: u16 = 0x0010;
    pub const FALL: u16 = 0x0020;

    /// Names of the authored bits.
    pub const NAMES: [(u16, &str); 5] = [
        (MUST_VAULT, "vault"),
        (MUST_LADDER_CLIMB, "ladder"),
        (PATHING_ONLY, "pathing-only"),
        (CROSSWALK, "crosswalk"),
        (FALL, "fall"),
    ];
}

/// Node flags: looking for the link nearest to a point, the game skips
/// links into nodes with `DISABLED` (`0x4000`, `godfather2.exe`
/// `0x005FFEB0`); no node of the game files has a flag.
pub mod node_flags {
    pub const DISABLED: u16 = 0x4000;
    pub const BLOCKED: u16 = 0x8000;
}

/// Names of the set bits, joined by `|`, or `-`.
pub fn flag_names(flags: u16, names: &[(u16, &str)]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut rest = flags;
    for &(bit, name) in names {
        if flags & bit != 0 {
            out.push(name.to_owned());
            rest &= !bit;
        }
    }
    if rest != 0 {
        out.push(format!("{rest:#06x}"));
    }
    if out.is_empty() {
        "-".into()
    } else {
        out.join("|")
    }
}

/// A navigation graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Graph {
    /// 16 in GF2.
    pub version: u32,
    /// The GUID of the graph; SimGroup objects and other graphs name the
    /// graph by it. The game takes it from the resource header.
    pub guid: Guid,
    /// Graph type bits ([`graph_flags`]).
    pub flags: u16,
    /// Hash by which other graphs may name this one (`sdbm("graph")` in
    /// most graphs of the game).
    pub identity: Hash32,
    /// Property instance of the graph itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<u16>,
    pub nodes: Vec<Node>,
    /// Graphs that neighbour links lead into, by GUID (slot `k` is
    /// `neighbours[k]`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub neighbours: Vec<Guid>,
    /// Graphs named by hash identity; their slots follow the GUID slots.
    /// Unused in GF2.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_neighbours: Vec<Hash32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub property_sets: Vec<PropertySet>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub property_instances: Vec<PropertyInstance>,
    /// Which pairs of nodes see each other.
    pub visibility: Visibility,
    /// For every pair of nodes, the first link of a shortest path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub precomputed_paths: Option<PathTable>,
    /// Spatial index of the nodes.
    pub rtree: RTree,
    /// Unused in GF2.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lights: Vec<Guid>,
    /// Unused in GF2.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub light_sets: Vec<LightSet>,
}

/// A node with the links that leave it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    /// World position.
    pub position: [f32; 3],
    pub radius: f32,
    /// [`node_flags`]; 0 in the game files.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub flags: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<u16>,
    /// Index into [`Graph::light_sets`]; 0 in the game files.
    #[serde(default, skip_serializing_if = "is_zero_i16")]
    pub light_set: i16,
    /// Links inside the graph, in file order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<Link>,
    /// Links into neighbouring graphs; GUID slots before hash slots.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub neighbour_links: Vec<NeighbourLink>,
}

/// A one-way link to another node of the graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Link {
    /// Index of the target node.
    pub to: u16,
    /// Distance between the nodes, the cost of the link.
    pub length: f32,
    /// [`link_flags`].
    #[serde(default, skip_serializing_if = "is_zero")]
    pub flags: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<u16>,
}

/// A one-way link into a node of another graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NeighbourLink {
    /// Slot of the other graph: [`Graph::neighbours`], then
    /// [`Graph::external_neighbours`].
    pub neighbour: u16,
    /// Node index in the other graph.
    pub node: u16,
    pub length: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub flags: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<u16>,
}

/// Fields of a property set: name, type and place in an instance's data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PropertySet {
    pub fields: Vec<PropertyField>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PropertyField {
    /// SDBM hash of the property name.
    pub name: Hash32,
    pub kind: PropertyKind,
    /// Byte offset of the value inside an instance.
    pub offset: u16,
}

/// Type of a property value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PropertyKind {
    Int,
    UInt,
    Float,
    Guid,
    Hash,
}

impl PropertyKind {
    fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            1 => Self::Int,
            2 => Self::UInt,
            3 => Self::Float,
            4 => Self::Guid,
            5 => Self::Hash,
            _ => return None,
        })
    }

    fn code(self) -> u8 {
        match self {
            Self::Int => 1,
            Self::UInt => 2,
            Self::Float => 3,
            Self::Guid => 4,
            Self::Hash => 5,
        }
    }

    /// Size of a value.
    pub fn size(self) -> usize {
        match self {
            Self::Guid => 16,
            _ => 4,
        }
    }
}

/// Values of a property set for one node, link or graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PropertyInstance {
    /// Index into [`Graph::property_sets`].
    pub set: u16,
    /// One value per field of the set, in field order.
    pub values: Vec<PropertyValue>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PropertyValue {
    Int(i32),
    UInt(u32),
    Float(f32),
    Guid(Guid),
    Hash(Hash32),
}

impl PropertyValue {
    pub fn kind(&self) -> PropertyKind {
        match self {
            Self::Int(_) => PropertyKind::Int,
            Self::UInt(_) => PropertyKind::UInt,
            Self::Float(_) => PropertyKind::Float,
            Self::Guid(_) => PropertyKind::Guid,
            Self::Hash(_) => PropertyKind::Hash,
        }
    }

    fn read(kind: PropertyKind, bytes: &[u8]) -> Self {
        let w = || u32::from_le_bytes(bytes[..4].try_into().expect("4 bytes"));
        match kind {
            PropertyKind::Int => Self::Int(w() as i32),
            PropertyKind::UInt => Self::UInt(w()),
            PropertyKind::Float => Self::Float(f32::from_bits(w())),
            PropertyKind::Hash => Self::Hash(Hash32(w())),
            PropertyKind::Guid => Self::Guid(Guid(bytes[..16].try_into().expect("16 bytes"))),
        }
    }

    fn write(&self, out: &mut [u8]) {
        match *self {
            Self::Int(v) => out[..4].copy_from_slice(&v.to_le_bytes()),
            Self::UInt(v) | Self::Hash(Hash32(v)) => out[..4].copy_from_slice(&v.to_le_bytes()),
            Self::Float(v) => out[..4].copy_from_slice(&v.to_le_bytes()),
            Self::Guid(g) => out[..16].copy_from_slice(&g.0),
        }
    }
}

/// A run of [`Graph::lights`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LightSet {
    pub first: u16,
    pub count: u8,
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub pad: u8,
}

/// Node-to-node visibility: one bit per pair `a < b`, row by row
/// (`bit = a·(2N − a − 1)/2 + b − a − 1`, `godfather2.exe` `0x005FC8C0`).
/// The game uses it to decide whether a car on node `a` may be moved onto
/// node `b`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    /// Plain bits, all pairs visible except `hidden`; unused bits set. All
    /// graphs of the game have this form with nothing hidden.
    Pairs { hidden: Vec<[u16; 2]> },
    /// Any other content, kept as is.
    Raw {
        rle: u32,
        #[serde(with = "hex")]
        bytes: Vec<u8>,
    },
}

/// First links of shortest paths: a nibble per pair `(from, to)` at index
/// `from·N + to`, low nibble first; `k + 1` is the `k`-th link of `from`,
/// 0 means `from == to` or no path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PathTable(#[serde(with = "hex")] pub Vec<u8>);

impl PathTable {
    /// The stored value for `(from, to)` in a graph of `n` nodes.
    pub fn get(&self, n: usize, from: usize, to: usize) -> u8 {
        let k = from * n + to;
        self.0.get(k / 2).map_or(0, |b| (b >> (4 * (k % 2))) & 0xF)
    }
}

/// The R-tree over node positions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RTree {
    /// No nodes.
    Empty,
    /// The tree as nested lists: a leaf is a list of node indices, an inner
    /// node a list of subtrees. Boxes and links between tree nodes follow
    /// from it and the node positions; all graphs of the game have this form.
    Shape(RShape),
    /// Any other content, kept as is.
    Raw {
        nodes: Vec<RTreeNode>,
        indices: Vec<u16>,
    },
}

/// A subtree of [`RTree::Shape`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RShape {
    Leaf(Vec<u16>),
    Inner(Vec<RShape>),
}

fn is_zero(v: &u16) -> bool {
    *v == 0
}

fn is_zero_u8(v: &u8) -> bool {
    *v == 0
}

fn is_zero_i16(v: &i16) -> bool {
    *v == 0
}

mod hex {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        let mut text = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            text.push_str(&format!("{b:02x}"));
        }
        s.serialize_str(&text)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        let bad = || serde::de::Error::custom("hexadecimal digits expected");
        if !text.len().is_multiple_of(2) || !text.is_ascii() {
            return Err(bad());
        }
        (0..text.len() / 2)
            .map(|k| u8::from_str_radix(&text[2 * k..2 * k + 2], 16).map_err(|_| bad()))
            .collect()
    }
}

/// Record sizes of a version: node, link.
fn strides(version: u32) -> Option<(usize, usize)> {
    match version {
        14 => Some((32, 12)),
        15 => Some((36, 12)),
        16 => Some((36, 16)),
        _ => None,
    }
}

const NEIGHBOUR_LINK_SIZE: usize = 16;
const RTREE_NODE_SIZE: usize = 36;

fn align(x: usize, a: usize) -> usize {
    x.div_ceil(a) * a
}

/// Bytes of visibility bits for `n` nodes.
pub fn visibility_len(n: usize) -> usize {
    (n * n.saturating_sub(1) / 2).div_ceil(8)
}

/// Bytes of the path table for `n` nodes.
pub fn path_table_len(n: usize) -> usize {
    (n * n).div_ceil(2)
}

struct Reader<'a> {
    data: &'a [u8],
}

impl Reader<'_> {
    fn u32(&self, at: usize) -> Result<u32> {
        io::read_u32(self.data, at, Endian::Little)
    }
    fn u16(&self, at: usize) -> Result<u16> {
        io::read_u16(self.data, at, Endian::Little)
    }
    fn u8(&self, at: usize) -> Result<u8> {
        Ok(io::slice(self.data, at, 1)?[0])
    }
    fn f32(&self, at: usize) -> Result<f32> {
        self.u32(at).map(f32::from_bits)
    }
    fn guid(&self, at: usize) -> Result<Guid> {
        Ok(Guid(
            io::slice(self.data, at, 16)?.try_into().expect("16 bytes"),
        ))
    }
    /// A section of `count` records of `size` bytes at the pointer stored
    /// at `field`; checks that it lies in the data.
    fn section(&self, field: usize, count: usize, size: usize) -> Result<usize> {
        let at = self.u32(field)? as usize;
        let len = count
            .checked_mul(size)
            .ok_or_else(|| Error::malformed("graph", field, "section too large"))?;
        if len > 0 {
            if at == 0 {
                return Err(Error::malformed("graph", field, "missing section"));
            }
            io::slice(self.data, at, len)?;
        }
        Ok(at)
    }
}

fn opt(index: u16) -> Option<u16> {
    (index != NO_PROPERTIES).then_some(index)
}

impl Graph {
    /// An empty graph of the GF2 version.
    pub fn new(guid: Guid, flags: u16) -> Self {
        Self {
            version: VERSION,
            guid,
            flags,
            identity: Hash32(TYPE_GRAPH),
            properties: None,
            nodes: Vec::new(),
            neighbours: Vec::new(),
            external_neighbours: Vec::new(),
            property_sets: Vec::new(),
            property_instances: Vec::new(),
            visibility: Visibility::Pairs { hidden: Vec::new() },
            precomputed_paths: Some(PathTable(Vec::new())),
            rtree: RTree::Empty,
            lights: Vec::new(),
            light_sets: Vec::new(),
        }
    }

    pub fn parse(data: &[u8]) -> Result<Self> {
        let r = Reader { data };
        io::slice(data, 0, HEADER_SIZE)?;
        let version = r.u32(8)?;
        let (node_size, link_size) = strides(version).ok_or_else(|| {
            Error::malformed("graph", 8, format!("unsupported version {version}"))
        })?;
        let n = r.u16(0x0C)? as usize;
        let c = r.u16(0x0E)? as usize;
        let guid = r.guid(0x10)?;
        let guid_count = r.u16(0x2C)? as usize;
        let nl_count = r.u16(0x2E)? as usize;
        let slot_count = r.u16(0x3C)? as usize;
        let rt_count = r.u16(0x3E)? as usize;
        let ext_count = r.u16(0x90)? as usize;
        if ext_count > slot_count {
            return Err(Error::malformed(
                "graph",
                0x90,
                "more external neighbours than slots",
            ));
        }
        let nodes_at = r.section(0x20, n, node_size)?;
        let links_at = r.section(0x24, c, link_size)?;
        let guids_at = r.section(0x28, guid_count, 16)?;
        let nl_at = r.section(0x30, nl_count, NEIGHBOUR_LINK_SIZE)?;
        let gidx_at = r.section(0x34, slot_count - ext_count, 2)?;
        let ext_at = r.section(0x8C, ext_count, 4)?;
        let paths_at = r.u32(0x40)? as usize;
        let rt_idx_at = if rt_count > 0 {
            r.section(0x44, n, 2)?
        } else {
            0
        };
        let rt_at = r.section(0x48, rt_count, RTREE_NODE_SIZE)?;
        let prop_count = r.u32(0x54)? as usize;
        let set_count = r.u32(0x5C)? as usize;
        let inst_count = r.u32(0x64)? as usize;
        let data_len = r.u32(0x6C)? as usize;
        let props_at = r.section(0x50, prop_count, 8)?;
        let sets_at = r.section(0x58, set_count, 8)?;
        let inst_at = r.section(0x60, inst_count, 8)?;
        let pdata_at = r.section(0x68, data_len, 1)?;
        let vis_len = r.u32(0x74)? as usize;
        let vis_at = r.section(0x70, vis_len, 1)?;
        let rle = r.u32(0x78)?;
        let light_count = r.u16(0x84)? as usize;
        let lset_count = r.u16(0x86)? as usize;
        let lights_at = r.section(0x7C, light_count, 16)?;
        let lsets_at = r.section(0x80, lset_count, 4)?;

        // Neighbour slots.
        let guids: Vec<Guid> = (0..guid_count)
            .map(|k| r.guid(guids_at + 16 * k))
            .collect::<Result<_>>()?;
        let neighbours = (0..slot_count - ext_count)
            .map(|k| {
                let at = gidx_at + 2 * k;
                let i = r.u16(at)? as usize;
                guids
                    .get(i)
                    .copied()
                    .ok_or_else(|| Error::malformed("graph", at, "neighbour GUID out of range"))
            })
            .collect::<Result<_>>()?;
        let external_neighbours = (0..ext_count)
            .map(|k| r.u32(ext_at + 4 * k).map(Hash32))
            .collect::<Result<_>>()?;

        // Properties.
        let mut fields = Vec::with_capacity(prop_count);
        for k in 0..prop_count {
            let at = props_at + 8 * k;
            let code = r.u8(at + 6)?;
            let kind = PropertyKind::from_code(code).ok_or_else(|| {
                Error::malformed("graph", at + 6, format!("unknown property type {code}"))
            })?;
            fields.push(PropertyField {
                name: Hash32(r.u32(at)?),
                kind,
                offset: r.u16(at + 4)?,
            });
        }
        let mut property_sets = Vec::with_capacity(set_count);
        let mut next = 0usize;
        for k in 0..set_count {
            let at = sets_at + 8 * k;
            let first = r.u32(at)? as usize;
            let count = r.u32(at + 4)? as usize;
            if first != next || count > prop_count - first {
                return Err(Error::malformed(
                    "graph",
                    at,
                    "property sets are not consecutive runs",
                ));
            }
            next += count;
            let set = PropertySet {
                fields: fields[first..first + count].to_vec(),
            };
            if set.fields.iter().any(|f| {
                f.offset as usize + f.kind.size()
                    > set.fields.iter().map(|g| g.kind.size()).sum::<usize>()
            }) {
                return Err(Error::malformed(
                    "graph",
                    at,
                    "property offsets overlap or leave gaps",
                ));
            }
            property_sets.push(set);
        }
        if next != prop_count {
            return Err(Error::malformed(
                "graph",
                0x54,
                "properties outside every set",
            ));
        }
        let pdata = io::slice(data, pdata_at, data_len)?;
        let mut property_instances = Vec::with_capacity(inst_count);
        let mut next = 0usize;
        for k in 0..inst_count {
            let at = inst_at + 8 * k;
            let offset = r.u32(at)? as usize;
            let set = r.u16(at + 4)?;
            let fields = &property_sets
                .get(set as usize)
                .ok_or_else(|| Error::malformed("graph", at + 4, "property set out of range"))?
                .fields;
            let size = fields.iter().map(|f| f.kind.size()).sum::<usize>();
            if offset != next || offset + size > pdata.len() {
                return Err(Error::malformed(
                    "graph",
                    at,
                    "property values are not consecutive",
                ));
            }
            next += size;
            let values = fields
                .iter()
                .map(|f| {
                    let o = offset + f.offset as usize;
                    PropertyValue::read(f.kind, &pdata[o..o + f.kind.size()])
                })
                .collect();
            property_instances.push(PropertyInstance { set, values });
        }
        if next != data_len {
            return Err(Error::malformed(
                "graph",
                0x6C,
                "property data outside every instance",
            ));
        }
        let check_props = |index: u16, at: usize| -> Result<Option<u16>> {
            if index != NO_PROPERTIES && index as usize >= inst_count {
                return Err(Error::malformed(
                    "graph",
                    at,
                    "property instance out of range",
                ));
            }
            Ok(opt(index))
        };

        // Nodes with their links.
        let mut nodes = Vec::with_capacity(n);
        let (mut next_link, mut next_nl) = (0usize, 0usize);
        for k in 0..n {
            let at = nodes_at + node_size * k;
            let first = r.u16(at + 0x10)? as usize;
            let count = r.u8(at + 0x14)? as usize;
            let nl_guid = r.u8(at + 0x15)? as usize;
            let nl_first = r.u16(at + 0x16)? as usize;
            let nl_ext = r.u8(at + 0x1C)? as usize;
            if first != next_link || first + count > c {
                return Err(Error::malformed(
                    "graph",
                    at + 0x10,
                    "links are not grouped by node",
                ));
            }
            if nl_first != next_nl || nl_first + nl_guid + nl_ext > nl_count {
                return Err(Error::malformed(
                    "graph",
                    at + 0x16,
                    "neighbour links are not grouped by node",
                ));
            }
            next_link += count;
            next_nl += nl_guid + nl_ext;
            let mut links = Vec::with_capacity(count);
            for e in first..first + count {
                let la = links_at + link_size * e;
                let to = r.u16(la + 4)?;
                if r.u16(la + 6)? as usize != k {
                    return Err(Error::malformed(
                        "graph",
                        la + 6,
                        "link source is not its node",
                    ));
                }
                if to as usize >= n {
                    return Err(Error::malformed("graph", la + 4, "link to a missing node"));
                }
                links.push(Link {
                    to,
                    length: r.f32(la)?,
                    flags: r.u16(la + 10)?,
                    properties: check_props(r.u16(la + 8)?, la + 8)?,
                });
            }
            let mut neighbour_links = Vec::with_capacity(nl_guid + nl_ext);
            for (j, e) in (nl_first..nl_first + nl_guid + nl_ext).enumerate() {
                let la = nl_at + NEIGHBOUR_LINK_SIZE * e;
                let slot = r.u16(la + 12)?;
                let is_guid = (slot as usize) < slot_count - ext_count;
                if slot as usize >= slot_count || is_guid != (j < nl_guid) {
                    return Err(Error::malformed(
                        "graph",
                        la + 12,
                        "neighbour slot out of range",
                    ));
                }
                if r.u16(la + 6)? as usize != k {
                    return Err(Error::malformed(
                        "graph",
                        la + 6,
                        "neighbour link source is not its node",
                    ));
                }
                neighbour_links.push(NeighbourLink {
                    neighbour: slot,
                    node: r.u16(la + 4)?,
                    length: r.f32(la)?,
                    flags: r.u16(la + 10)?,
                    properties: check_props(r.u16(la + 8)?, la + 8)?,
                });
            }
            nodes.push(Node {
                position: [r.f32(at)?, r.f32(at + 4)?, r.f32(at + 8)?],
                radius: r.f32(at + 0x0C)?,
                flags: r.u16(at + 0x18)?,
                properties: check_props(r.u16(at + 0x12)?, at + 0x12)?,
                light_set: r.u16(at + 0x1A)? as i16,
                links,
                neighbour_links,
            });
        }
        if next_link != c || next_nl != nl_count {
            return Err(Error::malformed(
                "graph",
                0x0E,
                "link counts differ from the nodes",
            ));
        }

        let vis = io::slice(data, vis_at, vis_len)?;
        let visibility = derive::visibility_from_bits(n, rle, vis);
        let precomputed_paths = if paths_at != 0 {
            Some(PathTable(
                io::slice(data, paths_at, path_table_len(n))?.to_vec(),
            ))
        } else {
            None
        };
        let rtree = if rt_count == 0 {
            RTree::Empty
        } else {
            let raw_nodes = (0..rt_count)
                .map(|k| RTreeNode::read(&r, rt_at + RTREE_NODE_SIZE * k))
                .collect::<Result<Vec<_>>>()?;
            let indices = (0..n)
                .map(|k| r.u16(rt_idx_at + 2 * k))
                .collect::<Result<Vec<_>>>()?;
            derive::rtree_from_raw(&nodes, raw_nodes, indices)
        };
        let lights = (0..light_count)
            .map(|k| r.guid(lights_at + 16 * k))
            .collect::<Result<_>>()?;
        let light_sets = (0..lset_count)
            .map(|k| {
                let at = lsets_at + 4 * k;
                Ok(LightSet {
                    first: r.u16(at)?,
                    count: r.u8(at + 2)?,
                    pad: r.u8(at + 3)?,
                })
            })
            .collect::<Result<_>>()?;
        Ok(Self {
            version,
            guid,
            flags: r.u16(0x4E)?,
            identity: Hash32(r.u32(0x88)?),
            properties: check_props(r.u16(0x4C)?, 0x4C)?,
            nodes,
            neighbours,
            external_neighbours,
            property_sets,
            property_instances,
            visibility,
            precomputed_paths,
            rtree,
            lights,
            light_sets,
        })
    }

    /// Number of links inside the graph.
    pub fn link_count(&self) -> usize {
        self.nodes.iter().map(|n| n.links.len()).sum()
    }

    /// Number of links into other graphs.
    pub fn neighbour_link_count(&self) -> usize {
        self.nodes.iter().map(|n| n.neighbour_links.len()).sum()
    }

    /// Every link as `(from, link)`.
    pub fn links(&self) -> impl Iterator<Item = (usize, &Link)> {
        self.nodes
            .iter()
            .enumerate()
            .flat_map(|(k, n)| n.links.iter().map(move |l| (k, l)))
    }

    /// Checks what the file cannot hold or the game cannot use; empty when
    /// the graph can be written.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let n = self.nodes.len();
        if strides(self.version).is_none() {
            out.push(format!("unsupported version {}", self.version));
        }
        if n > u16::MAX as usize {
            out.push(format!("{n} nodes, at most 65535"));
        }
        if self.link_count() > u16::MAX as usize {
            out.push(format!("{} links, at most 65535", self.link_count()));
        }
        let slots = self.neighbours.len() + self.external_neighbours.len();
        let instance_ok =
            |p: Option<u16>| p.is_none_or(|i| (i as usize) < self.property_instances.len());
        if !instance_ok(self.properties) {
            out.push("graph: property instance out of range".into());
        }
        for (k, node) in self.nodes.iter().enumerate() {
            if node.links.len() > MAX_LINKS {
                out.push(format!("node {k}: {} links, at most 255", node.links.len()));
            }
            if node.neighbour_links.len() > MAX_LINKS {
                out.push(format!("node {k}: too many neighbour links"));
            }
            if !instance_ok(node.properties) {
                out.push(format!("node {k}: property instance out of range"));
            }
            if !node.position.iter().all(|v| v.is_finite()) || !node.radius.is_finite() {
                out.push(format!("node {k}: position or radius is not a number"));
            }
            for l in &node.links {
                if l.to as usize >= n {
                    out.push(format!("node {k}: link to missing node {}", l.to));
                }
                if !instance_ok(l.properties) {
                    out.push(format!("node {k}: link property instance out of range"));
                }
            }
            let mut seen_external = false;
            for l in &node.neighbour_links {
                if l.neighbour as usize >= slots {
                    out.push(format!(
                        "node {k}: neighbour slot {} out of range",
                        l.neighbour
                    ));
                }
                let external = l.neighbour as usize >= self.neighbours.len();
                if seen_external && !external {
                    out.push(format!(
                        "node {k}: neighbour links by GUID must come before links by hash"
                    ));
                }
                seen_external |= external;
                if !instance_ok(l.properties) {
                    out.push(format!(
                        "node {k}: neighbour link property instance out of range"
                    ));
                }
            }
        }
        for (k, inst) in self.property_instances.iter().enumerate() {
            match self.property_sets.get(inst.set as usize) {
                None => out.push(format!("property instance {k}: set out of range")),
                Some(set) => {
                    if set.fields.len() != inst.values.len()
                        || set
                            .fields
                            .iter()
                            .zip(&inst.values)
                            .any(|(f, v)| f.kind != v.kind())
                    {
                        out.push(format!(
                            "property instance {k}: values do not match set {}",
                            inst.set
                        ));
                    }
                }
            }
        }
        for (k, set) in self.property_sets.iter().enumerate() {
            let size = set.size();
            let mut used = vec![false; size];
            for f in &set.fields {
                for b in used.iter_mut().skip(f.offset as usize).take(f.kind.size()) {
                    if *b {
                        out.push(format!("property set {k}: fields overlap"));
                    }
                    *b = true;
                }
            }
            if used.iter().any(|b| !b) {
                out.push(format!("property set {k}: gaps between fields"));
            }
        }
        if let RTree::Shape(shape) = &self.rtree {
            let mut seen = vec![false; n];
            if !derive::shape_covers(shape, &mut seen) || seen.iter().any(|s| !s) {
                out.push("R-tree: every node must be in exactly one leaf".into());
            } else if derive::rtree_records(&self.nodes, &self.rtree)
                .is_some_and(|(r, _)| r.len() > u16::MAX as usize)
            {
                out.push("R-tree: more than 65535 tree nodes".into());
            }
        }
        if let RTree::Raw { nodes, indices } = &self.rtree
            && (nodes.is_empty() || indices.len() != n)
        {
            out.push("R-tree: raw records need one index per graph node".into());
        }
        if self.nodes.is_empty() && !matches!(self.rtree, RTree::Empty) {
            out.push("R-tree: a graph without nodes has no tree".into());
        }
        if let Some(t) = &self.precomputed_paths
            && t.0.len() != path_table_len(n)
        {
            out.push("precomputed paths: wrong size".into());
        }
        if let Visibility::Pairs { hidden } = &self.visibility
            && hidden
                .iter()
                .any(|&[a, b]| a == b || a as usize >= n || b as usize >= n)
        {
            out.push("visibility: hidden pair out of range".into());
        }
        out
    }

    /// The resource data. Fails when [`Self::problems`] finds anything.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let problems = self.problems();
        if let Some(first) = problems.first() {
            return Err(Error::InvalidArgument(format!(
                "graph cannot be written: {first}{}",
                if problems.len() > 1 {
                    format!(" (and {} more)", problems.len() - 1)
                } else {
                    String::new()
                }
            )));
        }
        Ok(self.write())
    }

    fn write(&self) -> Vec<u8> {
        let (node_size, link_size) = strides(self.version).expect("checked");
        let n = self.nodes.len();
        let c = self.link_count();
        let nl = self.neighbour_link_count();
        let slots = self.neighbours.len() + self.external_neighbours.len();
        let prop_count: usize = self.property_sets.iter().map(|s| s.fields.len()).sum();
        let data_len: usize = self
            .property_instances
            .iter()
            .map(|i| self.property_sets[i.set as usize].size())
            .sum();
        let vis = derive::visibility_bytes(n, &self.visibility);
        let rle = match &self.visibility {
            Visibility::Raw { rle, .. } => *rle,
            Visibility::Pairs { .. } => 0,
        };
        let rtree = derive::rtree_records(&self.nodes, &self.rtree);

        // Layout, in the order and with the padding of the game files.
        let mut cur = HEADER_SIZE;
        let place = |len: usize, present: bool, cur: &mut usize| -> usize {
            if !present {
                return 0;
            }
            let at = *cur;
            *cur += len;
            at
        };
        let guids_at = place(
            16 * self.neighbours.len(),
            !self.neighbours.is_empty(),
            &mut cur,
        );
        let nodes_at = place(node_size * n, n > 0, &mut cur);
        let ptrs_at = place(8 * slots, slots > 0, &mut cur);
        let links_at = place(link_size * c, c > 0, &mut cur);
        let nl_at = place(NEIGHBOUR_LINK_SIZE * nl, nl > 0, &mut cur);
        let has_props = !self.property_sets.is_empty() || !self.property_instances.is_empty();
        let props_at = place(8 * prop_count, has_props, &mut cur);
        let sets_at = place(8 * self.property_sets.len(), has_props, &mut cur);
        let inst_at = place(8 * self.property_instances.len(), has_props, &mut cur);
        let pdata_at = place(data_len, true, &mut cur);
        cur = align(cur, 4) + 4;
        let vis_at = place(vis.len(), true, &mut cur);
        cur = align(cur + 3, 4);
        let ext_n = self.external_neighbours.len();
        let ext_at = place(4 * ext_n, ext_n > 0, &mut cur);
        let gidx_at = place(
            2 * self.neighbours.len(),
            !self.neighbours.is_empty(),
            &mut cur,
        );
        let paths_at = match &self.precomputed_paths {
            Some(t) => place(t.0.len(), true, &mut cur),
            None => 0,
        };
        let light_n = self.lights.len();
        if light_n > 0 {
            cur = align(cur, 16);
        }
        let lights_at = place(16 * light_n, light_n > 0, &mut cur);
        let lsets_at = place(
            4 * self.light_sets.len(),
            !self.light_sets.is_empty(),
            &mut cur,
        );
        let (rt_at, rt_idx_at) = match &rtree {
            Some((records, indices)) => {
                cur = align(cur, 16);
                let rt = place(RTREE_NODE_SIZE * records.len(), true, &mut cur);
                (rt, place(2 * indices.len(), true, &mut cur))
            }
            None => (0, 0),
        };
        let mut out = vec![0u8; cur];

        let put32 = |out: &mut Vec<u8>, at: usize, v: u32| {
            out[at..at + 4].copy_from_slice(&v.to_le_bytes());
        };
        let put16 = |out: &mut Vec<u8>, at: usize, v: u16| {
            out[at..at + 2].copy_from_slice(&v.to_le_bytes());
        };
        let putf = |out: &mut Vec<u8>, at: usize, v: f32| {
            out[at..at + 4].copy_from_slice(&v.to_le_bytes());
        };
        put32(&mut out, 0x08, self.version);
        put16(&mut out, 0x0C, n as u16);
        put16(&mut out, 0x0E, c as u16);
        out[0x10..0x20].copy_from_slice(&self.guid.0);
        put32(&mut out, 0x20, nodes_at as u32);
        put32(&mut out, 0x24, links_at as u32);
        put32(&mut out, 0x28, guids_at as u32);
        put16(&mut out, 0x2C, self.neighbours.len() as u16);
        put16(&mut out, 0x2E, nl as u16);
        put32(&mut out, 0x30, nl_at as u32);
        put32(&mut out, 0x34, gidx_at as u32);
        put32(&mut out, 0x38, ptrs_at as u32);
        put16(&mut out, 0x3C, slots as u16);
        put16(
            &mut out,
            0x3E,
            rtree.as_ref().map_or(0, |(r, _)| r.len()) as u16,
        );
        put32(&mut out, 0x40, paths_at as u32);
        put32(&mut out, 0x44, rt_idx_at as u32);
        put32(&mut out, 0x48, rt_at as u32);
        put16(&mut out, 0x4C, self.properties.unwrap_or(NO_PROPERTIES));
        put16(&mut out, 0x4E, self.flags);
        put32(&mut out, 0x50, props_at as u32);
        put32(&mut out, 0x54, prop_count as u32);
        put32(&mut out, 0x58, sets_at as u32);
        put32(&mut out, 0x5C, self.property_sets.len() as u32);
        put32(&mut out, 0x60, inst_at as u32);
        put32(&mut out, 0x64, self.property_instances.len() as u32);
        put32(&mut out, 0x68, pdata_at as u32);
        put32(&mut out, 0x6C, data_len as u32);
        put32(&mut out, 0x70, vis_at as u32);
        put32(&mut out, 0x74, vis.len() as u32);
        put32(&mut out, 0x78, rle);
        put32(&mut out, 0x7C, lights_at as u32);
        put32(&mut out, 0x80, lsets_at as u32);
        put16(&mut out, 0x84, light_n as u16);
        put16(&mut out, 0x86, self.light_sets.len() as u16);
        put32(&mut out, 0x88, self.identity.0);
        put32(&mut out, 0x8C, ext_at as u32);
        put16(&mut out, 0x90, ext_n as u16);

        for (k, g) in self.neighbours.iter().enumerate() {
            out[guids_at + 16 * k..guids_at + 16 * k + 16].copy_from_slice(&g.0);
            put16(&mut out, gidx_at + 2 * k, k as u16);
        }
        for (k, h) in self.external_neighbours.iter().enumerate() {
            put32(&mut out, ext_at + 4 * k, h.0);
        }
        let (mut e, mut ne) = (0usize, 0usize);
        for (k, node) in self.nodes.iter().enumerate() {
            let at = nodes_at + node_size * k;
            for (i, v) in node.position.iter().enumerate() {
                putf(&mut out, at + 4 * i, *v);
            }
            putf(&mut out, at + 0x0C, node.radius);
            put16(&mut out, at + 0x10, e as u16);
            put16(
                &mut out,
                at + 0x12,
                node.properties.unwrap_or(NO_PROPERTIES),
            );
            out[at + 0x14] = node.links.len() as u8;
            let by_guid = node
                .neighbour_links
                .iter()
                .filter(|l| (l.neighbour as usize) < self.neighbours.len())
                .count();
            out[at + 0x15] = by_guid as u8;
            put16(&mut out, at + 0x16, ne as u16);
            put16(&mut out, at + 0x18, node.flags);
            put16(&mut out, at + 0x1A, node.light_set as u16);
            out[at + 0x1C] = (node.neighbour_links.len() - by_guid) as u8;
            for l in &node.links {
                let la = links_at + link_size * e;
                putf(&mut out, la, l.length);
                put16(&mut out, la + 4, l.to);
                put16(&mut out, la + 6, k as u16);
                put16(&mut out, la + 8, l.properties.unwrap_or(NO_PROPERTIES));
                put16(&mut out, la + 10, l.flags);
                e += 1;
            }
            for l in &node.neighbour_links {
                let la = nl_at + NEIGHBOUR_LINK_SIZE * ne;
                putf(&mut out, la, l.length);
                put16(&mut out, la + 4, l.node);
                put16(&mut out, la + 6, k as u16);
                put16(&mut out, la + 8, l.properties.unwrap_or(NO_PROPERTIES));
                put16(&mut out, la + 10, l.flags);
                put16(&mut out, la + 12, l.neighbour);
                ne += 1;
            }
        }
        let mut first = 0usize;
        for (k, set) in self.property_sets.iter().enumerate() {
            put32(&mut out, sets_at + 8 * k, first as u32);
            put32(&mut out, sets_at + 8 * k + 4, set.fields.len() as u32);
            for f in &set.fields {
                let at = props_at + 8 * first;
                put32(&mut out, at, f.name.0);
                put16(&mut out, at + 4, f.offset);
                out[at + 6] = f.kind.code();
                first += 1;
            }
        }
        let mut offset = 0usize;
        for (k, inst) in self.property_instances.iter().enumerate() {
            let set = &self.property_sets[inst.set as usize];
            put32(&mut out, inst_at + 8 * k, offset as u32);
            put16(&mut out, inst_at + 8 * k + 4, inst.set);
            for (f, v) in set.fields.iter().zip(&inst.values) {
                let at = pdata_at + offset + f.offset as usize;
                v.write(&mut out[at..at + f.kind.size()]);
            }
            offset += set.size();
        }
        out[vis_at..vis_at + vis.len()].copy_from_slice(&vis);
        if let Some(t) = &self.precomputed_paths {
            out[paths_at..paths_at + t.0.len()].copy_from_slice(&t.0);
        }
        for (k, g) in self.lights.iter().enumerate() {
            out[lights_at + 16 * k..lights_at + 16 * k + 16].copy_from_slice(&g.0);
        }
        for (k, s) in self.light_sets.iter().enumerate() {
            let at = lsets_at + 4 * k;
            put16(&mut out, at, s.first);
            out[at + 2] = s.count;
            out[at + 3] = s.pad;
        }
        if let Some((records, indices)) = &rtree {
            for (k, rec) in records.iter().enumerate() {
                rec.write(&mut out[rt_at + RTREE_NODE_SIZE * k..]);
            }
            for (k, &i) in indices.iter().enumerate() {
                put16(&mut out, rt_idx_at + 2 * k, i);
            }
        }
        out
    }
}

impl PropertySet {
    /// Bytes of an instance's values.
    pub fn size(&self) -> usize {
        self.fields
            .iter()
            .map(|f| f.offset as usize + f.kind.size())
            .max()
            .unwrap_or(0)
            .max(self.fields.iter().map(|f| f.kind.size()).sum())
    }
}

#[cfg(test)]
mod tests;

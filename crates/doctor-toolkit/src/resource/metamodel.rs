//! `metamodel` resources: state machines of interactive objects (magic `MMdl`, version 14).
//!
//! A metamodel describes an object made of parts (models, collision, lights,
//! effects) whose attributes are set per state, with variables and conditions
//! that switch states (`DestructibleState_0` → `DestructibleState_1` when
//! `health` drops).
//!
//! # Layout
//!
//! The resource is a memory image: every pointer is an absolute offset from
//! the start of the resource; 0 is a null pointer.
//!
//! ```text
//! 0x00  u32  magic 0x4D4D646C ("ldMM" on disk)
//! 0x04  u32  version, 14
//! 0x08  u32  size of the resource
//! 0x0C  u32  flags of the runtime, 0
//! 0x10  4 × u32  GUID: the stream id of this resource, four times
//! 0x20  u32  name hash (often sdbm("metamodel"))
//! 0x24  u32  path hash: the resource id
//! 0x28  u32  original path: the id of the unfiltered metamodel this one was
//!            split from (its own id when the filter mask is 0xFFFFFFFF)
//! 0x2C  u32  source path hash
//! 0x30  u32  filter mask: one of FILTER_HIGH 1, FILTER_MEDIUM 2, FILTER_LOW 4,
//!            FILTER_MAP_VIEW 8, FILTER_INTERIOR 0x10, FILTER_TRANSITION 0x20,
//!            or 0xFFFFFFFF (all); matches the stream folder (`high/`, ...)
//! 0x34  9 × u32  table pointers T0..T6, attribute data (T7), user data (always 0)
//! 0x58  8 × u16  table counts T0..T6, the last one always 0
//! 0x68  tables, pointer lists, strings and the data blob (T7), with zero padding
//! ```
//!
//! | Table | Stride | Entry |
//! |---|---|---|
//! | T0 resources | 0x20 | `u32 name hash, u32 path hash (id), u32 type id, 4 × u32 GUID, u32 user data 0` |
//! | T1 states | 0x30 | `u32 type hash, *type, *name, 16-byte GUID, *properties, u16 count, u16 flags, *parts, *conditions, u16 part count, u16 condition count` |
//! | T2 variables | 0x0C | `*name, u32 hash, u32 kind` (1 bool, 2 int, 4 float) |
//! | T3 properties | 0x18 | `u32 value, *name, *class, u32 attribute index, u32 name hash, u32 type` |
//! | T4 conditions | 0x0C | `u32 value, *variable, u32 operator` ([`OP_GE`] … [`OP_AND_OR_EQ`]) |
//! | T5 parts | 0x24 | `u32 type hash, *type, *name, 16-byte GUID, *properties, u16 count, u16 object type` |
//! | T6 sub-objects (transforms) | 0x24 | as T5 |
//!
//! The "flags" of T1, T5 and T6 are an object type: 1 state, 2 simple part
//! (`ModelPart`, `VFXPart`, ...), 4 metamodel part, 8 entity part
//! (`Destructible`, `Door`, `EARSLight`, ...), 0x10 transform. The GUIDs of
//! states and parts are NeoGuids (see [`crate::resource::simgroup::Guid`]).
//! Field names from the comparison with another EARS editor, checked on all 12 315
//! metamodels (`docs/formats/metamodel.md`).
//!
//! Pointer lists are arrays of `u32` pointers to table entries. Property
//! types: 1 bool, 2 int, 4 float, 8 string pointer, 0x10 pointer to a T0
//! resource, 0x20 pointer to a 4×4 matrix, 0x40 colour (ARGB), 0x80 pointer
//! to 3 floats, 0x100 pointer to an array of u32 (`u32 count`, then the
//! values; all 713 in the game are `ComponentList` = `9f3fa445, 1, dc51ebb1`),
//! 0x200 pointer to a T6 sub-object.
//! Hashes are SDBM of the lower-case name. A property is a SimGroup attribute:
//! `class` and `attribute index` address the same value as a SimGroup packet.

use serde::{Serialize, Serializer};

use crate::io::{self, Endian};
use crate::resource::names::type_tags;
use crate::resource::simgroup::{Guid, Hash32};
use crate::{Error, Result};

pub const MAGIC: u32 = 0x4D4D_646C;

/// Filter mask (header 0x30): the high level of detail.
pub const FILTER_HIGH: u32 = 0x01;
/// Filter mask: medium level of detail.
pub const FILTER_MEDIUM: u32 = 0x02;
/// Filter mask: low level of detail.
pub const FILTER_LOW: u32 = 0x04;
/// Filter mask: map view (only in `*/global` streams).
pub const FILTER_MAP_VIEW: u32 = 0x08;
/// Filter mask: interiors.
pub const FILTER_INTERIOR: u32 = 0x10;
/// Filter mask: transitions.
pub const FILTER_TRANSITION: u32 = 0x20;
/// Filter mask of a metamodel that is not split by level of detail.
pub const FILTER_ALL: u32 = 0xFFFF_FFFF;

/// Condition operator (T4): variable > value (not used in the game).
pub const OP_GT: u32 = 0;
/// Condition operator: variable < value (not used in the game).
pub const OP_LT: u32 = 1;
/// Condition operator: variable ≥ value (9 677 conditions; the near end of a
/// LOD distance range).
pub const OP_GE: u32 = 2;
/// Condition operator: variable ≤ value (9 714 conditions; the far end).
pub const OP_LE: u32 = 3;
/// Condition operator: variable = value (1 412, on bools).
pub const OP_EQ: u32 = 4;
/// Condition operator: variable ≠ value (141, on bools).
pub const OP_NEQ: u32 = 5;
/// Condition operator: a bit test of an int (6 060, `stateEntering`/`stateLeaving`).
pub const OP_AND_OR_EQ: u32 = 6;
const HEADER_SIZE: usize = 0x68;
const STRIDES: [usize; 7] = [0x20, 0x30, 0x0C, 0x18, 0x0C, 0x24, 0x24];

/// A reference to another resource (T0).
///
/// The game finds the resource by the GUID ([`Self::ids`], the four ids of
/// the target's header), not by the path hash [`Self::id`]: the two differ in
/// 2 260 references of the game, all to nested metamodels, where `id` names
/// the unsplit original (1 084) or nothing (1 176) and the GUID names the
/// split metamodel of the same filter mask. Resolve with [`Self::guid`] or
/// [`Self::target_id`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResourceRef {
    pub hash: Hash32,
    /// Path hash of the resource; see [`Self::target_id`] for the id to look up.
    pub id: Hash32,
    pub type_id: Hash32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_name: Option<&'static str>,
    /// GUID of the target: its header ids.
    pub ids: [Hash32; 4],
}

impl ResourceRef {
    /// The GUID the reference resolves by: equal to the `ids` of the
    /// target's resource header (what `check-refs` matches).
    pub fn guid(&self) -> [u32; 4] {
        self.ids.map(|h| h.0)
    }

    /// The resource id (`ids[3]` of the target's header, the key of
    /// lookups by id) the reference resolves to: the last word of
    /// [`Self::guid`]. All four words are equal in every reference of the
    /// game.
    pub fn target_id(&self) -> u32 {
        self.ids[3].0
    }
}

/// Reads table T0 (the resource references) of a metamodel without parsing
/// the rest; for reference checks.
pub fn resource_refs(data: &[u8]) -> Result<Vec<ResourceRef>> {
    let magic = io::read_u32(data, 0, Endian::Little)?;
    if magic != MAGIC {
        return Err(Error::BadMagic {
            what: "metamodel",
            found: magic,
        });
    }
    io::slice(data, 0, HEADER_SIZE)?;
    let start = io::read_u32(data, 0x34, Endian::Little)? as usize;
    let count = io::read_u16(data, 0x58, Endian::Little)? as usize;
    if count > 0 && start < HEADER_SIZE {
        return Err(Error::malformed("metamodel", 0x34, "table in the header"));
    }
    io::slice(data, start, count * STRIDES[0])?;
    (0..count)
        .map(|k| resource_ref(data, start + STRIDES[0] * k))
        .collect()
}

fn resource_ref(data: &[u8], at: usize) -> Result<ResourceRef> {
    let word = |o: usize| io::read_u32(data, at + o, Endian::Little).map(Hash32);
    let type_id = word(8)?;
    Ok(ResourceRef {
        hash: word(0)?,
        id: word(4)?,
        type_id,
        type_name: type_tags().get(type_id.0),
        ids: [word(0x0C)?, word(0x10)?, word(0x14)?, word(0x18)?],
    })
}

/// A state (T1).
#[derive(Debug, Clone, PartialEq)]
pub struct State {
    pub type_hash: u32,
    pub type_name: String,
    pub name: String,
    pub guid: Guid,
    pub flags: u16,
    /// Indices into [`MetaModel::properties`].
    pub properties: Vec<usize>,
    /// Indices into [`MetaModel::parts`].
    pub parts: Vec<usize>,
    /// Indices into [`MetaModel::conditions`].
    pub conditions: Vec<usize>,
}

/// A state variable (T2).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Variable {
    pub name: String,
    pub hash: Hash32,
    /// 1 bool, 2 int, 4 float.
    pub kind: u32,
}

/// Value of a property (T3).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "lowercase")]
pub enum PropValue {
    Bool(bool),
    Int(i32),
    Float(f32),
    String(String),
    /// Index into [`MetaModel::resources`].
    Resource(usize),
    Matrix([[f32; 4]; 4]),
    Color(Hash32),
    Vec3([f32; 3]),
    /// Array of u32 (type 0x100): component class hashes in the game
    /// (`ComponentList`).
    Array(Vec<Hash32>),
    /// Index into [`MetaModel::sub_objects`].
    Object(usize),
    /// A null pointer of a pointer type.
    Null,
    /// A type not seen in the game files.
    Unknown(Hash32),
}

/// A property: one SimGroup attribute value (T3).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Property {
    pub name: String,
    pub hash: Hash32,
    /// SimGroup class the attribute belongs to.
    pub class: String,
    pub index: u32,
    /// Type flag as stored (1, 2, 4, ... 0x200).
    #[serde(skip)]
    pub type_flag: u32,
    #[serde(flatten)]
    pub value: PropValue,
}

/// A condition on a variable (T4).
#[derive(Debug, Clone, PartialEq)]
pub struct Condition {
    pub value: u32,
    /// Index into [`MetaModel::variables`].
    pub variable: usize,
    /// Comparison operator, [`OP_GE`] … [`OP_AND_OR_EQ`] in the game.
    pub operator: u32,
}

/// A part or sub-object (T5, T6).
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub type_hash: u32,
    pub type_name: String,
    pub name: String,
    pub guid: Guid,
    pub flags: u16,
    /// Indices into [`MetaModel::properties`].
    pub properties: Vec<usize>,
}

/// A parsed metamodel.
#[derive(Debug, Clone, PartialEq)]
pub struct MetaModel {
    pub version: u32,
    pub ids: [u32; 4],
    /// Header words at 0x20..0x34: name hash, path hash (resource id),
    /// original path (id of the unfiltered metamodel), source path hash,
    /// filter mask ([`FILTER_HIGH`] ...).
    pub header: [u32; 5],
    pub resources: Vec<ResourceRef>,
    pub states: Vec<State>,
    pub variables: Vec<Variable>,
    pub properties: Vec<Property>,
    pub conditions: Vec<Condition>,
    pub parts: Vec<Part>,
    pub sub_objects: Vec<Part>,
}

struct Reader<'a> {
    data: &'a [u8],
    tables: [(usize, usize); 7],
}

impl<'a> Reader<'a> {
    fn u32(&self, at: usize) -> Result<u32> {
        io::read_u32(self.data, at, Endian::Little)
    }

    fn u16(&self, at: usize) -> Result<u16> {
        io::read_u16(self.data, at, Endian::Little)
    }

    fn string(&self, field: usize) -> Result<String> {
        let at = self.u32(field)? as usize;
        if at < HEADER_SIZE {
            return Err(Error::malformed(
                "metamodel",
                field,
                "string pointer into the header",
            ));
        }
        Ok(String::from_utf8_lossy(io::cstr(self.data, at)?).into_owned())
    }

    fn guid(&self, at: usize) -> Result<Guid> {
        Ok(Guid(
            io::slice(self.data, at, 16)?.try_into().expect("16 bytes"),
        ))
    }

    fn floats<const N: usize>(&self, at: usize) -> Result<[f32; N]> {
        let mut out = [0.0; N];
        for (k, v) in out.iter_mut().enumerate() {
            *v = f32::from_bits(self.u32(at + 4 * k)?);
        }
        Ok(out)
    }

    /// An array of u32 at `at`: the count, then the values.
    fn array(&self, at: usize) -> Result<Vec<Hash32>> {
        let count = self.u32(at)? as usize;
        // Bounds the allocation by the size of the resource.
        io::slice(self.data, at + 4, count.saturating_mul(4))?;
        (0..count)
            .map(|k| self.u32(at + 4 + 4 * k).map(Hash32))
            .collect()
    }

    /// Index of the entry of `table` that `pointer` (read at `field`) points to.
    fn entry(&self, table: usize, pointer: u32, field: usize) -> Result<usize> {
        let (start, count) = self.tables[table];
        let offset = (pointer as usize)
            .checked_sub(start)
            .filter(|o| o % STRIDES[table] == 0 && o / STRIDES[table] < count)
            .ok_or_else(|| {
                Error::malformed(
                    "metamodel",
                    field,
                    format!("pointer is not a T{table} entry"),
                )
            })?;
        Ok(offset / STRIDES[table])
    }

    /// Reads a list of `count` pointers at `*field` to entries of `table`.
    fn list(&self, field: usize, count: u16, table: usize) -> Result<Vec<usize>> {
        let at = self.u32(field)? as usize;
        if count == 0 {
            return Ok(Vec::new());
        }
        io::slice(self.data, at, 4 * count as usize)?;
        (0..count as usize)
            .map(|k| self.entry(table, self.u32(at + 4 * k)?, at + 4 * k))
            .collect()
    }

    fn part(&self, at: usize) -> Result<Part> {
        Ok(Part {
            type_hash: self.u32(at)?,
            type_name: self.string(at + 4)?,
            name: self.string(at + 8)?,
            guid: self.guid(at + 0x0C)?,
            flags: self.u16(at + 0x22)?,
            properties: self.list(at + 0x1C, self.u16(at + 0x20)?, 3)?,
        })
    }

    fn property(&self, at: usize) -> Result<Property> {
        let raw = self.u32(at)?;
        let type_flag = self.u32(at + 0x14)?;
        let pointer = |size: usize| -> Result<Option<usize>> {
            if raw == 0 {
                return Ok(None);
            }
            let p = raw as usize;
            if p < HEADER_SIZE {
                return Err(Error::malformed(
                    "metamodel",
                    at,
                    "value pointer into the header",
                ));
            }
            io::slice(self.data, p, size)?;
            Ok(Some(p))
        };
        let value = match type_flag {
            0x01 => PropValue::Bool(raw != 0),
            0x02 => PropValue::Int(raw as i32),
            0x04 => PropValue::Float(f32::from_bits(raw)),
            0x08 => match pointer(1)? {
                Some(_) => PropValue::String(self.string(at)?),
                None => PropValue::Null,
            },
            0x10 => match raw {
                0 => PropValue::Null,
                _ => PropValue::Resource(self.entry(0, raw, at)?),
            },
            0x20 => match pointer(64)? {
                Some(p) => {
                    let m = self.floats::<16>(p)?;
                    PropValue::Matrix(std::array::from_fn(|r| {
                        std::array::from_fn(|c| m[4 * r + c])
                    }))
                }
                None => PropValue::Null,
            },
            0x40 => PropValue::Color(Hash32(raw)),
            0x80 => match pointer(12)? {
                Some(p) => PropValue::Vec3(self.floats(p)?),
                None => PropValue::Null,
            },
            0x100 => match pointer(4)? {
                Some(p) => PropValue::Array(self.array(p)?),
                None => PropValue::Null,
            },
            0x200 => match raw {
                0 => PropValue::Null,
                _ => PropValue::Object(self.entry(6, raw, at)?),
            },
            _ => PropValue::Unknown(Hash32(raw)),
        };
        Ok(Property {
            name: self.string(at + 4)?,
            hash: Hash32(self.u32(at + 0x10)?),
            class: self.string(at + 8)?,
            index: self.u32(at + 0x0C)?,
            type_flag,
            value,
        })
    }
}

impl MetaModel {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let le = Endian::Little;
        let u32_at = |o: usize| io::read_u32(data, o, le);
        let magic = u32_at(0)?;
        if magic != MAGIC {
            return Err(Error::BadMagic {
                what: "metamodel",
                found: magic,
            });
        }
        let version = u32_at(4)?;
        if version != 14 {
            return Err(Error::UnsupportedVersion {
                what: "metamodel",
                version,
            });
        }
        io::slice(data, 0, HEADER_SIZE)?;
        let mut tables = [(0usize, 0usize); 7];
        for (i, table) in tables.iter_mut().enumerate() {
            let start = u32_at(0x34 + 4 * i)? as usize;
            let count = io::read_u16(data, 0x58 + 2 * i, le)? as usize;
            if count > 0 {
                if start < HEADER_SIZE {
                    return Err(Error::malformed(
                        "metamodel",
                        0x34 + 4 * i,
                        "table in the header",
                    ));
                }
                // Bounds the allocations below by the size of the resource.
                io::slice(data, start, count * STRIDES[i])?;
            }
            *table = (start, count);
        }
        let r = Reader { data, tables };
        let entries = |i: usize| (0..tables[i].1).map(move |k| tables[i].0 + STRIDES[i] * k);

        let resources = entries(0)
            .map(|at| resource_ref(data, at))
            .collect::<Result<_>>()?;
        let states = entries(1)
            .map(|at| {
                Ok(State {
                    type_hash: r.u32(at)?,
                    type_name: r.string(at + 4)?,
                    name: r.string(at + 8)?,
                    guid: r.guid(at + 0x0C)?,
                    flags: r.u16(at + 0x22)?,
                    properties: r.list(at + 0x1C, r.u16(at + 0x20)?, 3)?,
                    parts: r.list(at + 0x24, r.u16(at + 0x2C)?, 5)?,
                    conditions: r.list(at + 0x28, r.u16(at + 0x2E)?, 4)?,
                })
            })
            .collect::<Result<_>>()?;
        let variables = entries(2)
            .map(|at| {
                Ok(Variable {
                    name: r.string(at)?,
                    hash: Hash32(r.u32(at + 4)?),
                    kind: r.u32(at + 8)?,
                })
            })
            .collect::<Result<_>>()?;
        let properties = entries(3).map(|at| r.property(at)).collect::<Result<_>>()?;
        let conditions = entries(4)
            .map(|at| {
                Ok(Condition {
                    value: r.u32(at)?,
                    variable: r.entry(2, r.u32(at + 4)?, at + 4)?,
                    operator: r.u32(at + 8)?,
                })
            })
            .collect::<Result<_>>()?;
        let parts = entries(5).map(|at| r.part(at)).collect::<Result<_>>()?;
        let sub_objects = entries(6).map(|at| r.part(at)).collect::<Result<_>>()?;
        Ok(Self {
            version,
            ids: std::array::from_fn(|i| u32_at(0x10 + 4 * i).unwrap_or(0)),
            header: std::array::from_fn(|i| u32_at(0x20 + 4 * i).unwrap_or(0)),
            resources,
            states,
            variables,
            properties,
            conditions,
            parts,
            sub_objects,
        })
    }

    /// The value of a condition, read according to the kind of its variable.
    pub fn condition_value(&self, condition: &Condition) -> serde_json::Value {
        let raw = condition.value;
        match self.variables.get(condition.variable).map(|v| v.kind) {
            Some(1) => serde_json::Value::Bool(raw != 0),
            Some(4) => serde_json::json!(f32::from_bits(raw)),
            _ => serde_json::json!(raw as i32),
        }
    }
}

/// JSON form: states and parts with their properties and conditions resolved.
impl Serialize for MetaModel {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Doc<'a> {
            version: u32,
            ids: [Hash32; 4],
            header: [Hash32; 5],
            resources: &'a [ResourceRef],
            variables: &'a [Variable],
            states: Vec<StateJson<'a>>,
            parts: Vec<PartJson<'a>>,
            #[serde(skip_serializing_if = "Vec::is_empty")]
            sub_objects: Vec<PartJson<'a>>,
        }
        #[derive(Serialize)]
        struct StateJson<'a> {
            name: &'a str,
            #[serde(rename = "type")]
            type_name: &'a str,
            guid: Guid,
            flags: u16,
            properties: Vec<&'a Property>,
            /// Indices into `parts`.
            parts: &'a [usize],
            conditions: Vec<ConditionJson<'a>>,
        }
        #[derive(Serialize)]
        struct ConditionJson<'a> {
            variable: &'a str,
            operator: u32,
            value: serde_json::Value,
        }
        #[derive(Serialize)]
        struct PartJson<'a> {
            name: &'a str,
            #[serde(rename = "type")]
            type_name: &'a str,
            guid: Guid,
            flags: u16,
            properties: Vec<&'a Property>,
        }
        let props = |list: &[usize]| list.iter().map(|&i| &self.properties[i]).collect();
        fn part<'a>(properties: &'a [Property], p: &'a Part) -> PartJson<'a> {
            PartJson {
                name: &p.name,
                type_name: &p.type_name,
                guid: p.guid,
                flags: p.flags,
                properties: p.properties.iter().map(|&i| &properties[i]).collect(),
            }
        }
        Doc {
            version: self.version,
            ids: self.ids.map(Hash32),
            header: self.header.map(Hash32),
            resources: &self.resources,
            variables: &self.variables,
            states: self
                .states
                .iter()
                .map(|st| StateJson {
                    name: &st.name,
                    type_name: &st.type_name,
                    guid: st.guid,
                    flags: st.flags,
                    properties: props(&st.properties),
                    parts: &st.parts,
                    conditions: st
                        .conditions
                        .iter()
                        .map(|&i| {
                            let c = &self.conditions[i];
                            ConditionJson {
                                variable: &self.variables[c.variable].name,
                                operator: c.operator,
                                value: self.condition_value(c),
                            }
                        })
                        .collect(),
                })
                .collect(),
            parts: self
                .parts
                .iter()
                .map(|p| part(&self.properties, p))
                .collect(),
            sub_objects: self
                .sub_objects
                .iter()
                .map(|p| part(&self.properties, p))
                .collect(),
        }
        .serialize(s)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Writes a metamodel with one resource, one state with a matrix property
    /// and a condition, one part with a resource property, and one variable.
    pub(crate) fn sample() -> Vec<u8> {
        let mut d = vec![0u8; 0x200];
        let put =
            |d: &mut Vec<u8>, at: usize, v: u32| d[at..at + 4].copy_from_slice(&v.to_le_bytes());
        let put16 =
            |d: &mut Vec<u8>, at: usize, v: u16| d[at..at + 2].copy_from_slice(&v.to_le_bytes());
        put(&mut d, 0, MAGIC);
        put(&mut d, 4, 14);
        put(&mut d, 8, 0x200);
        // Tables: T0 0x68, T1 0x88, T2 0xB8, T3 0xC4 (2 entries), T4 0xF4, T5 0x100.
        for (i, (at, n)) in [
            (0x68, 1),
            (0x88, 1),
            (0xB8, 1),
            (0xC4, 2),
            (0xF4, 1),
            (0x100, 1),
        ]
        .into_iter()
        .enumerate()
        {
            put(&mut d, 0x34 + 4 * i, at);
            put16(&mut d, 0x58 + 2 * i, n);
        }
        // Strings at 0x150.
        let strings =
            b"\0State\0S0\0health\0Model\0ModelPart\0CMD_LoadMatrix\0CSystemCommands\0P0\0";
        d[0x150..0x150 + strings.len()].copy_from_slice(strings);
        let s = |name: &str| {
            let pos = strings
                .windows(name.len() + 2)
                .position(|w| {
                    w[0] == 0 && &w[1..=name.len()] == name.as_bytes() && w[name.len() + 1] == 0
                })
                .unwrap();
            0x151 + pos as u32
        };
        // T0: mesh reference, GUID = the id four times, as in the game.
        put(&mut d, 0x68, 0x1234);
        put(&mut d, 0x6C, 0xABCD);
        put(&mut d, 0x70, 0x0196_1BED);
        for k in 0..4 {
            put(&mut d, 0x74 + 4 * k, 0xABCD);
        }
        // T1: state with property list at 0x130, part list at 0x134, condition list at 0x138.
        put(&mut d, 0x88, 0xFE60_4109);
        put(&mut d, 0x8C, s("State"));
        put(&mut d, 0x90, s("S0"));
        d[0x94..0xA4].copy_from_slice(b"\x01\x02\x03\x04\x05\x06\x07\x08MMOPTIMI");
        put(&mut d, 0xA4, 0x130);
        put16(&mut d, 0xA8, 1);
        put16(&mut d, 0xAA, 1);
        put(&mut d, 0xAC, 0x134);
        put(&mut d, 0xB0, 0x138);
        put16(&mut d, 0xB4, 1);
        put16(&mut d, 0xB6, 1);
        // T2: variable.
        put(&mut d, 0xB8, s("health"));
        put(&mut d, 0xC0, 4);
        // T3: matrix property (blob at 0x1C0), resource property.
        put(&mut d, 0xC4, 0x1C0);
        put(&mut d, 0xC8, s("CMD_LoadMatrix"));
        put(&mut d, 0xCC, s("CSystemCommands"));
        put(&mut d, 0xD0, 1);
        put(&mut d, 0xD8, 0x20);
        put(&mut d, 0xDC, 0x68);
        put(&mut d, 0xE0, s("Model"));
        put(&mut d, 0xE4, s("ModelPart"));
        put(&mut d, 0xE8, 1);
        put(&mut d, 0xF0, 0x10);
        // T4: health < 50.0.
        put(&mut d, 0xF4, 50f32.to_bits());
        put(&mut d, 0xF8, 0xB8);
        put(&mut d, 0xFC, 3);
        // T5: part with property list at 0x13C.
        put(&mut d, 0x104, s("ModelPart"));
        put(&mut d, 0x108, s("P0"));
        put(&mut d, 0x11C, 0x13C);
        put16(&mut d, 0x120, 1);
        put16(&mut d, 0x122, 2);
        // Lists.
        put(&mut d, 0x130, 0xC4);
        put(&mut d, 0x134, 0x100);
        put(&mut d, 0x138, 0xF4);
        put(&mut d, 0x13C, 0xDC);
        // Matrix: identity with a translation.
        for (k, v) in [
            1.0f32, 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 5., 6., 7., 1.,
        ]
        .into_iter()
        .enumerate()
        {
            put(&mut d, 0x1C0 + 4 * k, v.to_bits());
        }
        d
    }

    #[test]
    fn parses_states_parts_and_properties() {
        let mm = MetaModel::parse(&sample()).unwrap();
        assert_eq!(mm.resources[0].type_name, Some("mesh"));
        assert_eq!(mm.states[0].name, "S0");
        assert_eq!(&mm.states[0].guid.0[8..], b"MMOPTIMI");
        assert_eq!(mm.states[0].properties, vec![0]);
        assert_eq!(mm.states[0].parts, vec![0]);
        let PropValue::Matrix(m) = mm.properties[0].value else {
            panic!("matrix expected");
        };
        assert_eq!(m[3], [5.0, 6.0, 7.0, 1.0]);
        assert_eq!(mm.properties[1].value, PropValue::Resource(0));
        assert_eq!(mm.properties[1].class, "ModelPart");
        assert_eq!(mm.parts[0].properties, vec![1]);
        assert_eq!(mm.parts[0].flags, 2);
        assert_eq!(mm.conditions[0].variable, 0);
        let json = serde_json::to_value(&mm).unwrap();
        assert_eq!(json["states"][0]["conditions"][0]["variable"], "health");
        assert_eq!(json["states"][0]["conditions"][0]["value"], 50.0);
        assert_eq!(json["parts"][0]["properties"][0]["type"], "resource");
    }

    #[test]
    fn rejects_bad_input() {
        let d = sample();
        assert!(MetaModel::parse(&d[..0x40]).is_err());
        let mut bad = d.clone();
        bad[0] ^= 1;
        assert!(matches!(
            MetaModel::parse(&bad),
            Err(Error::BadMagic { .. })
        ));
        let mut huge = d.clone();
        huge[0x5E..0x60].copy_from_slice(&u16::MAX.to_le_bytes());
        assert!(MetaModel::parse(&huge).is_err());
        let mut dangling = d.clone();
        dangling[0xF8..0xFC].copy_from_slice(&0xBCu32.to_le_bytes());
        assert!(MetaModel::parse(&dangling).is_err());
        for i in 0..d.len() {
            let mut c = d.clone();
            c[i] ^= 0x41;
            let _ = MetaModel::parse(&c);
        }
    }
}

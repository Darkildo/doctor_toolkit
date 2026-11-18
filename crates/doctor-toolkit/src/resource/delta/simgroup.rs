//! Deltas of SimGroups: objects added and removed and attributes set, the
//! objects addressed by GUID.
//!
//! ```json
//! {"format":1,"kind":"simgroup","ops":[
//!   {"op":"remove","object":"<GUID>"},
//!   {"op":"set","object":"<GUID>","attribute":"38523fc3.4","old":{"int":1},"new":{"int":2}},
//!   {"op":"transform","object":"<GUID>","old":[[…],[…],[…],[…]],"matrix":[[…],[…],[…],[…]]},
//!   {"op":"model","object":"<GUID>","old":"2f4fcbd4","model":"70010001"},
//!   {"op":"add","object":"<GUID>","class":"38523fc3","flags":3,"flags2":1,"packets":[…]}
//! ],"layout":{"base":"<SHA-256>","packets":[[0,7],[9,1],[7,2]]}}
//! ```
//!
//! # Operations and layout
//!
//! The operations say what the group holds; they apply to any version of
//! the group. They do not say where the writer puts the packets, the aux
//! blocks and the strings, and that depends on the order the author made the
//! edits in: an object moved, another moved, the first moved again leaves
//! the two matrices in the other order than moving each once. So that the
//! delta still gives the author's bytes, [`diff`] adds a `layout` when the
//! operations alone do not: the order of the packets, which of two equal
//! packets an object uses, and the contents of the two regions, written as
//! runs of what the operations gave. On the files of the game the
//! operations alone give the author's bytes for one move, one attribute or
//! one removal, for half of the duplicated objects and for one in eight of
//! twelve mixed edits; with the layout, for all of them
//! (`docs/formats/delta.md` §9).
//!
//! The layout moves data and never changes a value: a pointer follows its
//! block, and [`apply`] refuses a layout that would give a pointer other
//! bytes or an object a packet with other values. It holds only on the base
//! it was made for: [`apply`] uses it when the SHA-256 of the base is the
//! one it names and ignores it on any other base.

use std::collections::HashMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{Applied, FORMAT, Float, Hex, Kind, Report, check_head};
use crate::resource::names::sim_classes;
use crate::resource::simgroup::{
    ATTR_LOAD_MATRIX, ATTR_SET_MODEL, Attr, CLASS_ENTITY, CLASS_SYSTEM_COMMANDS, Guid, Hash32,
    Object, Packet, Schema, SimGroup, Target, Value,
};
use crate::resource::simgroup_edit::{compare_by_guid, describe};
use crate::workspace::store::sha256_hex;
use crate::{Error, Result};

/// A delta of a SimGroup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimGroupDelta {
    pub format: u32,
    pub kind: Kind,
    /// The operations, in the order they are applied.
    pub ops: Vec<Op>,
    /// Where the data goes on the base the delta was made for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<Layout>,
}

/// A row-major 4×4 matrix: rows right, up, at and position.
pub type Matrix = [[Float; 4]; 4];

/// One operation on a group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Op {
    /// Removes the object and the packets only it used.
    Remove { object: Guid },
    /// Sets an attribute of a class of the object; no value is the default.
    Set {
        object: Guid,
        attribute: AttrRef,
        /// The value the author changed, so a change of the same attribute
        /// by something else can be told.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        old: Option<DeltaValue>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        new: Option<DeltaValue>,
    },
    /// Sets the world matrix (`CSystemCommands.CMD_LoadMatrix`).
    Transform {
        object: Guid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        old: Option<Matrix>,
        matrix: Matrix,
    },
    /// Sets the `metamodel` the object shows (`Entity.CMD_SetModel`), by
    /// resource id; no id clears the model.
    Model {
        object: Guid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        old: Option<Hash32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<Hash32>,
    },
    /// Appends an object with its packets. A packet equal to one of the
    /// group is shared with it, as [`SimGroup::copy_object_from`] does.
    Add {
        object: Guid,
        class: Hash32,
        flags: u8,
        flags2: u8,
        /// Resources the object needs, as resource GUIDs.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        links: Vec<Guid>,
        /// The packet that lists the classes of the components.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        components: Option<PacketDoc>,
        /// The attribute packets, base class first.
        packets: Vec<PacketDoc>,
    },
}

impl Op {
    /// GUID of the object the operation addresses.
    pub fn object(&self) -> Guid {
        match self {
            Self::Remove { object }
            | Self::Set { object, .. }
            | Self::Transform { object, .. }
            | Self::Model { object, .. }
            | Self::Add { object, .. } => *object,
        }
    }

    /// What the operation addresses, as the notes of [`apply`] name it: the
    /// object, and the attribute for an operation that sets one.
    pub fn target(&self) -> String {
        let attribute = match self {
            Self::Remove { .. } | Self::Add { .. } => None,
            Self::Set { attribute, .. } => Some(*attribute),
            Self::Transform { .. } => Some(AttrRef {
                class: CLASS_SYSTEM_COMMANDS,
                index: ATTR_LOAD_MATRIX,
            }),
            Self::Model { .. } => Some(AttrRef {
                class: CLASS_ENTITY,
                index: ATTR_SET_MODEL,
            }),
        };
        target_of(&self.object(), attribute)
    }
}

/// The address of an object, or of one of its attributes, for people.
fn target_of(object: &Guid, attribute: Option<AttrRef>) -> String {
    match attribute {
        Some(attribute) => format!("object {object}, {}", attribute.describe()),
        None => format!("object {object}"),
    }
}

/// An attribute of a class: `<class hash>.<index>`, as `b390b11a.1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AttrRef {
    pub class: u32,
    pub index: u16,
}

impl std::fmt::Display for AttrRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:08x}.{}", self.class, self.index)
    }
}

impl AttrRef {
    /// The attribute with the names the library knows, for people:
    /// `CSystemCommands.CMD_LoadMatrix`, `Entity.4`.
    pub fn describe(&self) -> String {
        let class = sim_classes()
            .get(self.class)
            .map_or_else(|| format!("{:08x}", self.class), str::to_owned);
        match Schema::gf2()
            .get(self.class, self.index)
            .and_then(|i| i.name)
        {
            Some(name) => format!("{class}.{name}"),
            None => format!("{class}.{}", self.index),
        }
    }
}

impl Serialize for AttrRef {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for AttrRef {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        let parsed = text.split_once('.').and_then(|(class, index)| {
            let class = (class.len() == 8)
                .then(|| u32::from_str_radix(class, 16).ok())
                .flatten()?;
            Some(Self {
                class,
                index: index.parse().ok()?,
            })
        });
        parsed.ok_or_else(|| {
            serde::de::Error::custom(format!(
                "`{text}`: an attribute is `<class hash>.<index>`, as `b390b11a.1`"
            ))
        })
    }
}

/// An attribute value in a document: `{"int":-7}`, `{"float":0.5}`,
/// `{"hash":"0000abcd"}`, `{"string":"door_01"}`, `{"guid":"<32 digits>"}`,
/// `{"vec3":[x,y,z]}`, `{"matrix":[[…],[…],[…],[…]]}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeltaValue {
    Int(i32),
    Float(Float),
    Hash(Hash32),
    String(String),
    Guid(Guid),
    Vec3([Float; 3]),
    Matrix(Matrix),
    /// Aux data of an attribute whose kind is not known.
    Bytes(Hex),
}

impl DeltaValue {
    pub fn from_value(value: &Value) -> Self {
        match value {
            Value::Int(v) => Self::Int(*v),
            Value::Float(v) => Self::Float(Float(*v)),
            Value::Hash(v) => Self::Hash(*v),
            Value::String(v) => Self::String(v.clone()),
            Value::Guid(v) => Self::Guid(*v),
            Value::Vec3(v) => Self::Vec3(v.map(Float)),
            Value::Matrix(m) => Self::Matrix(matrix_doc(m)),
            Value::Bytes(b) => Self::Bytes(Hex(b.clone())),
        }
    }

    pub fn to_value(&self) -> Value {
        match self {
            Self::Int(v) => Value::Int(*v),
            Self::Float(v) => Value::Float(v.0),
            Self::Hash(v) => Value::Hash(*v),
            Self::String(v) => Value::String(v.clone()),
            Self::Guid(v) => Value::Guid(*v),
            Self::Vec3(v) => Value::Vec3(v.map(|f| f.0)),
            Self::Matrix(m) => Value::Matrix(matrix_value(m)),
            Self::Bytes(b) => Value::Bytes(b.0.clone()),
        }
    }

    /// The stored word of a scalar. Ints, floats and hashes are one word in
    /// the file, and which of the three an attribute is read as comes from
    /// the schema, so two scalars are the same value when their words are.
    fn word(&self) -> Option<u32> {
        match self {
            Self::Int(v) => Some(*v as u32),
            Self::Float(v) => Some(v.0.to_bits()),
            Self::Hash(v) => Some(v.0),
            _ => None,
        }
    }

    /// Whether the value is the default, which a packet does not store.
    fn is_default(&self) -> bool {
        match self {
            Self::String(s) => s.is_empty(),
            v => v.word() == Some(0),
        }
    }

    /// Whether two values are stored as the same bytes.
    fn same(a: Option<&Self>, b: Option<&Self>) -> bool {
        match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => match (a.word(), b.word()) {
                (Some(x), Some(y)) => x == y,
                (None, None) => a == b,
                _ => false,
            },
            _ => false,
        }
    }
}

fn matrix_doc(m: &[[f32; 4]; 4]) -> Matrix {
    m.map(|row| row.map(Float))
}

fn matrix_value(m: &Matrix) -> [[f32; 4]; 4] {
    m.map(|row| row.map(|f| f.0))
}

/// A packet of a new object: the attributes one class adds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PacketDoc {
    pub class: Hash32,
    /// Number of attributes of the class, stored or not.
    pub count: u16,
    /// The stored attributes, by ascending index; the others are defaults.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attrs: Vec<AttrDoc>,
}

/// One stored attribute of a [`PacketDoc`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttrDoc {
    pub index: u16,
    pub value: DeltaValue,
}

/// Where the writer puts the data on the base the delta was made for, as
/// runs of what the operations alone give. An empty list leaves its part as
/// the operations gave it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layout {
    /// SHA-256 of the base; the layout is ignored on any other base.
    pub base: String,
    /// The order of the packets: runs `[first, count]` of packet numbers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub packets: Vec<[u32; 2]>,
    /// Objects that use another packet with the same values: `[object,
    /// slot, packet]`, the packet by its number in the new order and the
    /// component list as the slot after the last packet. Equal packets are
    /// not always shared in the files of the game, and which of two an
    /// object got depends on how it was made.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub users: Vec<[u32; 3]>,
    /// The aux region: runs `[offset, size]` of the region, and bytes no
    /// value refers to as hexadecimal text.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aux: Vec<Piece>,
    /// Pointers into the aux region that do not follow their run: `[number
    /// of the pointer, offset in the new region]`. The region may hold the
    /// same bytes twice, and a pointer may use either place.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aux_pointers: Vec<[u32; 2]>,
    /// The string region: runs `[first, count]` of string numbers, and
    /// strings no value refers to as text.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub strings: Vec<Piece>,
    /// Pointers into the string region that do not point to the first
    /// place of their string: `[number of the pointer, offset]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub string_pointers: Vec<[u32; 2]>,
}

/// A part of a region in a [`Layout`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Piece {
    /// `[first, count]`: data the operations gave, moved here.
    Run([u32; 2]),
    /// Data nothing refers to: left by an edit that was undone later.
    Literal(String),
}

impl SimGroupDelta {
    /// Checks what the structure of the document cannot: the new objects.
    pub fn validate(&self) -> Result<()> {
        check_head(self.format, self.kind, Kind::SimGroup)?;
        for (i, op) in self.ops.iter().enumerate() {
            let Op::Add {
                object,
                links,
                components,
                packets,
                ..
            } = op
            else {
                continue;
            };
            let bad = |why: String| {
                Error::InvalidArgument(format!("operation {} (new object {object}): {why}", i + 1))
            };
            if object.is_nil() || object.resource_id().is_some() {
                return Err(bad("this is not an object GUID".into()));
            }
            // The file keeps both counts in one byte each.
            if links.len() > 255 || packets.len() > 255 {
                return Err(bad("more than 255 links or packets".into()));
            }
            for packet in packets.iter().chain(components) {
                let mut next = 0u32;
                for attr in &packet.attrs {
                    if u32::from(attr.index) < next || attr.index >= packet.count {
                        return Err(bad(format!(
                            "attributes of packet {} are not in ascending order below {}",
                            packet.class, packet.count
                        )));
                    }
                    next = u32::from(attr.index) + 1;
                    if attr.value.is_default() {
                        return Err(bad(format!(
                            "attribute {} of packet {} is a default: it is not stored",
                            attr.index, packet.class
                        )));
                    }
                    if matches!(&attr.value, DeltaValue::String(s) if s.contains('\0')) {
                        return Err(bad("a string contains a NUL byte".into()));
                    }
                    if matches!(&attr.value, DeltaValue::Bytes(b) if b.0.is_empty()) {
                        return Err(bad("aux data of no bytes".into()));
                    }
                }
            }
        }
        Ok(())
    }
}

// ----------------------------------------------------------------- diff

/// The delta that turns `base` into `mine`, or `None` when the change is
/// not one of the operations (the header of the group, the class, the
/// links, the flags or the components of an object, the order of the
/// objects, an object in the middle) or the delta does not give `mine` back
/// byte for byte.
pub fn diff(base: &[u8], mine: &[u8]) -> Result<Option<SimGroupDelta>> {
    let Some(ops) = operations(base, mine)? else {
        return Ok(None);
    };
    let mut delta = SimGroupDelta {
        format: FORMAT,
        kind: Kind::SimGroup,
        ops,
        layout: None,
    };
    let Some(plain) = read_back_and_apply(&delta, base) else {
        return Ok(None);
    };
    if plain == mine {
        return Ok(Some(delta));
    }
    // The operations give the right group in another layout: name the
    // author's.
    let Some(layout) = layout_between(&plain, mine, base) else {
        return Ok(None);
    };
    delta.layout = Some(layout);
    Ok(match read_back_and_apply(&delta, base) {
        Some(data) if data == mine => Some(delta),
        _ => None,
    })
}

/// The operations that turn `base` into `mine`, without the check that they
/// give the same bytes: what changed in the group, whatever its layout.
/// `None` when the change is not one of the operations.
pub fn operations(base: &[u8], mine: &[u8]) -> Result<Option<Vec<Op>>> {
    let old = SimGroup::parse(base)?;
    let new = SimGroup::parse(mine)?;
    Ok(ops_between(&old, &new))
}

/// The data the document gives on `base` when it is read from its bytes, as
/// a mod will read it.
fn read_back_and_apply(delta: &SimGroupDelta, base: &[u8]) -> Option<Vec<u8>> {
    let bytes = serde_json::to_vec(delta).ok()?;
    let read: SimGroupDelta = serde_json::from_slice(&bytes).ok()?;
    read.validate().ok()?;
    let applied = apply(&read, base).ok()?;
    applied.skipped.is_empty().then_some(applied.data)
}

fn packet_doc(packet: &Packet) -> PacketDoc {
    PacketDoc {
        class: packet.class,
        count: packet.attr_count,
        attrs: packet
            .attrs
            .iter()
            .map(|a| AttrDoc {
                index: a.index,
                value: DeltaValue::from_value(&a.value),
            })
            .collect(),
    }
}

fn ops_between(old: &SimGroup, new: &SimGroup) -> Option<Vec<Op>> {
    if (old.version, old.group_hash, old.flags) != (new.version, new.group_hash, new.flags) {
        return None;
    }
    let compared = compare_by_guid(old, new);
    if !compared.other.is_empty() {
        return None;
    }
    // Objects are removed anywhere and added at the end: the others keep
    // their order.
    let kept: Vec<usize> = compared.matched.iter().map(|&(_, b)| b).collect();
    if !kept.windows(2).all(|w| w[0] < w[1]) {
        return None;
    }
    if let (Some(&last), Some(&first)) = (kept.last(), compared.added.first())
        && first < last
    {
        return None;
    }
    // What `compare_by_guid` does not look at.
    for &(a, b) in &compared.matched {
        let (x, y) = (&old.objects[a], &new.objects[b]);
        let shape = |g: &SimGroup, o: &Object| -> Vec<(u32, u16)> {
            g.object_packets(o)
                .chain(o.components.map(|c| &g.packets[c]))
                .map(|p| (p.class.0, p.attr_count))
                .collect()
        };
        if (x.flags, x.flags2) != (y.flags, y.flags2) || shape(old, x) != shape(new, y) {
            return None;
        }
    }
    // An operation finds its object by GUID: the GUID has to name one object.
    let mut count: HashMap<Guid, usize> = HashMap::new();
    for o in &old.objects {
        *count.entry(o.guid).or_default() += 1;
    }
    let unique = |guid: &Guid| count.get(guid).copied().unwrap_or(0) == 1;

    let mut ops = Vec::new();
    for &a in &compared.removed {
        let guid = old.objects[a].guid;
        if !unique(&guid) {
            return None;
        }
        ops.push(Op::Remove { object: guid });
    }
    // The writer appends the packet of an edited object, so the edits go in
    // the order of the packets they left.
    let mut sets: Vec<_> = compared
        .attrs
        .iter()
        .map(|d| (new.objects[d.object].packets[d.slot], d.object, d.index, d))
        .collect();
    sets.sort_by_key(|&(packet, object, index, _)| (packet, object, index));
    for (_, object, index, d) in sets {
        let guid = new.objects[object].guid;
        if !unique(&guid) {
            return None;
        }
        let old_value = d.old.as_ref().map(DeltaValue::from_value);
        let new_value = d.new.as_ref().map(DeltaValue::from_value);
        if DeltaValue::same(old_value.as_ref(), new_value.as_ref()) {
            // The same bytes that do not compare equal: a NaN.
            continue;
        }
        let model = |v: &Option<Value>| match v {
            None => Some(None),
            Some(Value::Guid(g)) => g.resource_id().map(|id| Some(Hash32(id))),
            Some(_) => None,
        };
        ops.push(match (&d.old, &d.new) {
            (was, Some(Value::Matrix(m)))
                if (d.class, index) == (CLASS_SYSTEM_COMMANDS, ATTR_LOAD_MATRIX)
                    && matches!(was, None | Some(Value::Matrix(_))) =>
            {
                Op::Transform {
                    object: guid,
                    old: match was {
                        Some(Value::Matrix(m)) => Some(matrix_doc(m)),
                        _ => None,
                    },
                    matrix: matrix_doc(m),
                }
            }
            (a, b)
                if (d.class, index) == (CLASS_ENTITY, ATTR_SET_MODEL)
                    && model(a).is_some()
                    && model(b).is_some() =>
            {
                Op::Model {
                    object: guid,
                    old: model(a).flatten(),
                    model: model(b).flatten(),
                }
            }
            _ => Op::Set {
                object: guid,
                attribute: AttrRef {
                    class: d.class,
                    index,
                },
                old: old_value,
                new: new_value,
            },
        });
    }
    for &b in &compared.added {
        let o = &new.objects[b];
        ops.push(Op::Add {
            object: o.guid,
            class: Hash32(o.class),
            flags: o.flags,
            flags2: o.flags2,
            links: o.links.clone(),
            components: o.components.map(|c| packet_doc(&new.packets[c])),
            packets: new.object_packets(o).map(packet_doc).collect(),
        });
    }
    Some(ops)
}

// ---------------------------------------------------------------- apply

fn find(group: &SimGroup, guid: &Guid) -> Option<usize> {
    group.objects.iter().position(|o| o.guid == *guid)
}

/// Applies `delta` to the group `base`.
///
/// An operation on an object that is not in the group, or on a class the
/// object does not have, is skipped and listed. A new object whose GUID is
/// taken is an error. An attribute that held neither the value the author
/// changed nor the new one is set all the same and listed as a conflict.
pub fn apply(delta: &SimGroupDelta, base: &[u8]) -> Result<Applied> {
    delta.validate()?;
    let mut group = SimGroup::parse(base)?;
    let mut report = Report::default();
    for (i, op) in delta.ops.iter().enumerate() {
        let target = target_of(&op.object(), None);
        match op {
            Op::Remove { object } => match find(&group, object) {
                Some(at) => {
                    group.remove_object(at)?;
                    report.applied();
                }
                None => report.skip(i, target, "the object is not in the group"),
            },
            Op::Set {
                object,
                attribute,
                old,
                new,
            } => set(
                &mut group,
                &mut report,
                (i, object),
                *attribute,
                old.as_ref(),
                new.as_ref(),
            ),
            Op::Transform {
                object,
                old,
                matrix,
            } => set(
                &mut group,
                &mut report,
                (i, object),
                AttrRef {
                    class: CLASS_SYSTEM_COMMANDS,
                    index: ATTR_LOAD_MATRIX,
                },
                old.map(DeltaValue::Matrix).as_ref(),
                Some(&DeltaValue::Matrix(*matrix)),
            ),
            Op::Model { object, old, model } => {
                let value = |id: &Option<Hash32>| {
                    id.map(|id| DeltaValue::Guid(Guid::from_resource_id(id.0)))
                };
                set(
                    &mut group,
                    &mut report,
                    (i, object),
                    AttrRef {
                        class: CLASS_ENTITY,
                        index: ATTR_SET_MODEL,
                    },
                    value(old).as_ref(),
                    value(model).as_ref(),
                );
            }
            Op::Add { object, .. } => {
                if find(&group, object).is_some() {
                    return Err(Error::InvalidArgument(format!(
                        "operation {}: an object with GUID {object} is already in the group",
                        i + 1
                    )));
                }
                group.copy_object_from(&carrier(op), 0, *object)?;
                report.applied();
            }
        }
    }
    let mut data = if report.any_applied() {
        group.encode()
    } else {
        base.to_vec()
    };
    if let Some(layout) = &delta.layout
        && layout.base == sha256_hex(base)
    {
        let mut laid = SimGroup::parse(&data)?;
        apply_layout(&mut laid, layout)?;
        data = laid.encode();
    }
    Ok(report.finish(data))
}

/// Sets one attribute for [`apply`], with its notes.
fn set(
    group: &mut SimGroup,
    report: &mut Report,
    (op, object): (usize, &Guid),
    attribute: AttrRef,
    old: Option<&DeltaValue>,
    new: Option<&DeltaValue>,
) {
    let target = target_of(object, Some(attribute));
    let Some(at) = find(group, object) else {
        return report.skip(op, target, "the object is not in the group");
    };
    let Ok(slot) = group.slot_of(at, attribute.class) else {
        return report.skip(op, target, "the object has no packet of this class");
    };
    let packet = &group.packets[group.objects[at].packets[slot]];
    let current = packet
        .attrs
        .iter()
        .find(|a| a.index == attribute.index)
        .map(|a| a.value.clone());
    let current_doc = current.as_ref().map(DeltaValue::from_value);
    let differs = !DeltaValue::same(current_doc.as_ref(), old)
        && !DeltaValue::same(current_doc.as_ref(), new);
    match group.set_attr(at, slot, attribute.index, new.map(DeltaValue::to_value)) {
        Ok(_) => {
            report.applied();
            if differs {
                let was = old.map(DeltaValue::to_value);
                report.conflict(
                    op,
                    target,
                    format!(
                        "the value was {}, not {} as when the delta was made",
                        describe(current.as_ref()),
                        describe(was.as_ref())
                    ),
                );
            }
        }
        // The class of this group has fewer attributes, or keeps another
        // kind of value there.
        Err(error) => report.skip(op, target, error.to_string()),
    }
}

/// A group that holds the one object of an [`Op::Add`], for
/// [`SimGroup::copy_object_from`]. Its pointers point nowhere: the copy
/// takes the values, not the places.
fn carrier(op: &Op) -> SimGroup {
    let Op::Add {
        object,
        class,
        flags,
        flags2,
        links,
        components,
        packets,
    } = op
    else {
        unreachable!("only a new object has a carrier");
    };
    let packet = |doc: &PacketDoc| Packet {
        offset: 0,
        class: doc.class,
        class_name: sim_classes().get(doc.class.0),
        attr_count: doc.count,
        attrs: doc
            .attrs
            .iter()
            .map(|a| {
                let value = a.value.to_value();
                Attr {
                    index: a.index,
                    name: Schema::gf2()
                        .get(doc.class.0, a.index)
                        .and_then(|info| info.name),
                    raw: a.value.word().unwrap_or(0),
                    resource: match &value {
                        Value::Guid(g) => g.resource_id().map(Hash32),
                        _ => None,
                    },
                    target: match &value {
                        Value::Int(_) | Value::Float(_) | Value::Hash(_) => None,
                        Value::String(_) => Some(Target::String(0)),
                        _ => Some(Target::Aux(0)),
                    },
                    value,
                }
            })
            .collect(),
    };
    let mut group = SimGroup::empty();
    group.packets = packets.iter().chain(components).map(packet).collect();
    group.objects.push(Object {
        offset: 0,
        flags: *flags,
        flags2: *flags2,
        class: class.0,
        guid: *object,
        links: links.clone(),
        guid_ref: 0,
        link_refs: vec![0; links.len()],
        components: components.as_ref().map(|_| packets.len()),
        packets: (0..packets.len()).collect(),
    });
    group
}

// --------------------------------------------------------------- layout

fn layout_error(why: &str) -> Error {
    Error::InvalidArgument(format!("the layout of the delta does not fit: {why}"))
}

/// Size of the aux block a value points to.
fn aux_size(value: &Value) -> usize {
    match value {
        Value::Guid(_) => 16,
        Value::Vec3(_) => 12,
        Value::Matrix(_) => 64,
        Value::Bytes(b) => b.len(),
        _ => 0,
    }
}

/// Every pointer into the aux region with the size of its block, in a fixed
/// order: the GUID and the links of each object, then the values of each
/// packet. The place of a pointer in this list is its number in a layout.
fn aux_refs(group: &mut SimGroup) -> Vec<(&mut usize, usize)> {
    let mut refs = Vec::new();
    for o in &mut group.objects {
        refs.push((&mut o.guid_ref, 16));
        refs.extend(o.link_refs.iter_mut().map(|r| (r, 16)));
    }
    for a in group.packets.iter_mut().flat_map(|p| &mut p.attrs) {
        let size = aux_size(&a.value);
        if let Some(Target::Aux(at)) = &mut a.target {
            refs.push((at, size));
        }
    }
    refs
}

/// Every pointer into the string region, in the order of the packets.
fn string_refs(group: &mut SimGroup) -> Vec<&mut usize> {
    group
        .packets
        .iter_mut()
        .flat_map(|p| &mut p.attrs)
        .filter_map(|a| match &mut a.target {
            Some(Target::String(at)) => Some(at),
            _ => None,
        })
        .collect()
}

/// The strings of a string region with their offsets: after the first NUL,
/// up to the padding.
fn string_list(region: &[u8]) -> Option<Vec<(usize, &[u8])>> {
    if region.first() != Some(&0) {
        return None;
    }
    let mut list = Vec::new();
    let mut at = 1;
    while at < region.len() && region[at..].iter().any(|&b| b != 0) {
        let end = at + region[at..].iter().position(|&b| b == 0)?;
        list.push((at, &region[at..end]));
        at = end + 1;
    }
    Some(list)
}

/// The block of `size` bytes at `at` of an aux region.
fn block(region: &[u8], at: usize, size: usize) -> Option<&[u8]> {
    region.get(at..at.checked_add(size)?)
}

/// The string at `at` of a string region, up to its NUL.
fn string_at(region: &[u8], at: usize) -> Option<&[u8]> {
    let rest = region.get(at..)?;
    Some(&rest[..rest.iter().position(|&b| b == 0)?])
}

/// The aux region a layout describes, made of the region `old` the
/// operations gave.
struct AuxPlan {
    region: Vec<u8>,
    /// `(old offset, size, new offset)` of the runs, by old offset; `None`
    /// when the layout leaves the region as it is.
    runs: Option<Vec<(usize, usize, usize)>>,
}

impl AuxPlan {
    fn new(old: &[u8], pieces: &[Piece]) -> Result<Self> {
        if pieces.is_empty() {
            return Ok(Self {
                region: old.to_vec(),
                runs: None,
            });
        }
        let mut region = Vec::new();
        let mut runs = Vec::new();
        for piece in pieces {
            match piece {
                Piece::Run([first, size]) => {
                    let (first, size) = (*first as usize, *size as usize);
                    let bytes = first
                        .checked_add(size)
                        .and_then(|end| old.get(first..end))
                        .ok_or_else(|| layout_error("an aux run is outside the region"))?;
                    runs.push((first, size, region.len()));
                    region.extend_from_slice(bytes);
                }
                Piece::Literal(text) => region.extend_from_slice(
                    &Hex::bytes(text)
                        .ok_or_else(|| layout_error("aux bytes are not hexadecimal"))?,
                ),
            }
        }
        runs.sort_unstable();
        Ok(Self {
            region,
            runs: Some(runs),
        })
    }

    /// Where the block of `size` bytes at `at` of the old region goes: into
    /// the run that starts last at or before it, or any other that holds it.
    fn target(&self, at: usize, size: usize) -> Option<usize> {
        let Some(runs) = &self.runs else {
            return Some(at);
        };
        let inside =
            |&&(first, len, _): &&(usize, usize, usize)| first <= at && at + size <= first + len;
        let near = runs.partition_point(|&(first, _, _)| first <= at);
        runs[..near]
            .last()
            .filter(inside)
            .or_else(|| runs.iter().find(inside))
            .map(|&(first, _, new)| new + (at - first))
    }
}

/// The string region a layout describes, made of the region `old` the
/// operations gave.
struct StringPlan {
    region: Vec<u8>,
    /// Old offsets of the strings and their new offsets; `None` when the
    /// layout leaves the region as it is.
    moved: Option<(Vec<usize>, Vec<Option<usize>>)>,
}

impl StringPlan {
    fn new(old: &[u8], pieces: &[Piece]) -> Result<Self> {
        if pieces.is_empty() {
            return Ok(Self {
                region: old.to_vec(),
                moved: None,
            });
        }
        let list = string_list(old)
            .ok_or_else(|| layout_error("the string region does not start with a NUL"))?;
        let mut region = vec![0u8];
        let mut moved: Vec<Option<usize>> = vec![None; list.len()];
        for piece in pieces {
            match piece {
                Piece::Run([first, count]) => {
                    let first = *first as usize;
                    let run = first
                        .checked_add(*count as usize)
                        .and_then(|end| list.get(first..end))
                        .ok_or_else(|| layout_error("a string run is outside the region"))?;
                    for (k, (_, text)) in run.iter().enumerate() {
                        moved[first + k].get_or_insert(region.len());
                        region.extend_from_slice(text);
                        region.push(0);
                    }
                }
                Piece::Literal(text) => {
                    if text.contains('\0') {
                        return Err(layout_error("a string contains a NUL byte"));
                    }
                    region.extend_from_slice(text.as_bytes());
                    region.push(0);
                }
            }
        }
        region.resize(region.len().next_multiple_of(4), 0);
        Ok(Self {
            region,
            moved: Some((list.iter().map(|&(at, _)| at).collect(), moved)),
        })
    }

    /// Where the string at `at` of the old region goes: to its first place
    /// in the new one. Offset 0 is the empty string of every region.
    fn target(&self, at: usize) -> Option<usize> {
        let Some((offsets, moved)) = &self.moved else {
            return Some(at);
        };
        if at == 0 {
            return Some(0);
        }
        offsets.binary_search(&at).ok().and_then(|n| moved[n])
    }
}

/// The pointers of a layout that do not follow the rule, by number.
fn pointer_exceptions(list: &[[u32; 2]]) -> HashMap<usize, usize> {
    list.iter()
        .map(|&[number, at]| (number as usize, at as usize))
        .collect()
}

/// The packet each packet number stands for: the first object and slot
/// that use it (the component list is the slot after the last packet).
fn first_users(group: &SimGroup) -> Option<Vec<(usize, usize)>> {
    let mut users = vec![None; group.packets.len()];
    for (i, o) in group.objects.iter().enumerate() {
        for (slot, &p) in o.packets.iter().chain(&o.components).enumerate() {
            users[p].get_or_insert((i, slot));
        }
    }
    // A packet no object uses has no name to find it by.
    users.into_iter().collect()
}

/// Slot `slot` of an object: one of its packets, or the component list as
/// the slot after the last packet.
fn slot_mut(object: &mut Object, slot: usize) -> Option<&mut usize> {
    if slot == object.packets.len() {
        object.components.as_mut()
    } else {
        object.packets.get_mut(slot)
    }
}

/// Whether two packets hold the same class and the same values, so an
/// object may use either.
fn same_packet(a: &Packet, b: &Packet) -> bool {
    (a.class, a.attr_count, a.attrs.len()) == (b.class, b.attr_count, b.attrs.len())
        && a.attrs.iter().zip(&b.attrs).all(|(x, y)| {
            x.index == y.index
                && DeltaValue::same(
                    Some(&DeltaValue::from_value(&x.value)),
                    Some(&DeltaValue::from_value(&y.value)),
                )
        })
}

/// Puts the packets in the order `order` (new number → old number).
fn reorder_packets(group: &mut SimGroup, order: &[usize]) {
    let mut new_number = vec![0; order.len()];
    for (new, &old) in order.iter().enumerate() {
        new_number[old] = new;
    }
    let mut old: Vec<Option<Packet>> = std::mem::take(&mut group.packets)
        .into_iter()
        .map(Some)
        .collect();
    group.packets = order
        .iter()
        .map(|&p| old[p].take().expect("a permutation names each packet once"))
        .collect();
    for o in &mut group.objects {
        for p in o.packets.iter_mut().chain(&mut o.components) {
            *p = new_number[*p];
        }
    }
}

fn runs_of(order: &[usize]) -> Vec<[u32; 2]> {
    let mut runs: Vec<[u32; 2]> = Vec::new();
    for &p in order {
        match runs.last_mut() {
            Some([first, count]) if (*first + *count) as usize == p => *count += 1,
            _ => runs.push([p as u32, 1]),
        }
    }
    runs
}

/// The layout that turns `plain` (what the operations gave on `base`) into
/// `mine`, when the two hold the same group.
fn layout_between(plain: &[u8], mine: &[u8], base: &[u8]) -> Option<Layout> {
    let mut given = SimGroup::parse(plain).ok()?;
    let mut wanted = SimGroup::parse(mine).ok()?;
    if given.objects.len() != wanted.objects.len() || given.packets.len() != wanted.packets.len() {
        return None;
    }
    // Packets: the same packet is the one with the same first user.
    let wanted_users: HashMap<(usize, usize), usize> = first_users(&wanted)?
        .into_iter()
        .enumerate()
        .map(|(number, user)| (user, number))
        .collect();
    let mut order = vec![usize::MAX; given.packets.len()];
    for (number, user) in first_users(&given)?.into_iter().enumerate() {
        let slot = order.get_mut(*wanted_users.get(&user)?)?;
        if *slot != usize::MAX {
            return None;
        }
        *slot = number;
    }
    let identity = order.iter().enumerate().all(|(new, &old)| new == old);
    let packets = if identity {
        Vec::new()
    } else {
        reorder_packets(&mut given, &order);
        runs_of(&order)
    };
    // The packets are in the same order now; an object may still use
    // another one of two equal packets.
    let mut users = Vec::new();
    for (i, (ours, theirs)) in given.objects.iter().zip(&wanted.objects).enumerate() {
        if (ours.packets.len(), ours.components.is_some())
            != (theirs.packets.len(), theirs.components.is_some())
        {
            return None;
        }
        let slots = ours.packets.iter().chain(&ours.components);
        for (slot, (&p, &q)) in slots
            .zip(theirs.packets.iter().chain(&theirs.components))
            .enumerate()
        {
            if p != q {
                if !same_packet(&given.packets[p], &given.packets[q]) {
                    return None;
                }
                users.push([i as u32, slot as u32, q as u32]);
            }
        }
    }
    for &[object, slot, packet] in &users {
        *slot_mut(&mut given.objects[object as usize], slot as usize)? = packet as usize;
    }

    // Aux: every byte of the wanted region a pointer covers comes from the
    // byte the same pointer covers in the given one.
    let given_aux = std::mem::take(&mut given.aux);
    let wanted_aux = std::mem::take(&mut wanted.aux);
    let mut source = vec![usize::MAX; wanted_aux.len()];
    let pointers: Vec<(usize, usize, usize)> = {
        let from = aux_refs(&mut given);
        let to = aux_refs(&mut wanted);
        if from.len() != to.len() {
            return None;
        }
        from.into_iter()
            .zip(to)
            .map(|((from, size), (to, wanted_size))| {
                (size == wanted_size).then_some((*from, *to, size))
            })
            .collect::<Option<_>>()?
    };
    for &(from, to, size) in &pointers {
        if from + size > given_aux.len() || to + size > wanted_aux.len() {
            return None;
        }
        for k in 0..size {
            if source[to + k] == usize::MAX && given_aux[from + k] == wanted_aux[to + k] {
                source[to + k] = from + k;
            }
        }
    }
    let mut aux = Vec::new();
    if given_aux != wanted_aux {
        let mut at = 0;
        while at < source.len() {
            let mut end = at + 1;
            if source[at] == usize::MAX {
                while end < source.len() && source[end] == usize::MAX {
                    end += 1;
                }
                aux.push(Piece::Literal(Hex::text(&wanted_aux[at..end])));
            } else {
                while end < source.len() && source[end] == source[end - 1].wrapping_add(1) {
                    end += 1;
                }
                aux.push(Piece::Run([source[at] as u32, (end - at) as u32]));
            }
            at = end;
        }
        if aux.is_empty() {
            // An empty region cannot be told from "leave it as it is".
            return None;
        }
    }
    // The region may hold the same bytes twice: a pointer that uses another
    // place than its run gives is named with its place.
    let plan = AuxPlan::new(&given_aux, &aux).ok()?;
    let aux_pointers = pointers
        .iter()
        .enumerate()
        .filter(|&(_, &(from, to, size))| plan.target(from, size) != Some(to))
        .map(|(number, &(_, to, _))| [number as u32, to as u32])
        .collect();

    // Strings: by text.
    let given_strings = std::mem::take(&mut given.strings);
    let wanted_strings = std::mem::take(&mut wanted.strings);
    let mut strings = Vec::new();
    if given_strings != wanted_strings {
        let given_list = string_list(&given_strings)?;
        let mut number: HashMap<&[u8], usize> = HashMap::new();
        for (i, (_, text)) in given_list.iter().enumerate() {
            number.entry(text).or_insert(i);
        }
        for (_, text) in string_list(&wanted_strings)? {
            match (number.get(text), strings.last_mut()) {
                (Some(&n), Some(Piece::Run([first, count]))) if (*first + *count) as usize == n => {
                    *count += 1;
                }
                (Some(&n), _) => strings.push(Piece::Run([n as u32, 1])),
                (None, _) => strings.push(Piece::Literal(String::from_utf8(text.to_vec()).ok()?)),
            }
        }
        if strings.is_empty() {
            // No strings at all cannot be told from "leave it as it is".
            return None;
        }
    }
    let plan = StringPlan::new(&given_strings, &strings).ok()?;
    let (from, to) = (string_refs(&mut given), string_refs(&mut wanted));
    if from.len() != to.len() {
        return None;
    }
    let string_pointers = from
        .into_iter()
        .zip(to)
        .enumerate()
        .filter(|(_, (from, to))| plan.target(**from) != Some(**to))
        .map(|(number, (_, to))| [number as u32, *to as u32])
        .collect();
    Some(Layout {
        base: sha256_hex(base),
        packets,
        users,
        aux,
        aux_pointers,
        strings,
        string_pointers,
    })
}

/// Moves the data of `group` (as written by the operations) to where the
/// layout puts it. Pointers follow their data; no value changes.
fn apply_layout(group: &mut SimGroup, layout: &Layout) -> Result<()> {
    if !layout.packets.is_empty() {
        let count = group.packets.len();
        let mut order = Vec::with_capacity(count);
        let mut seen = vec![false; count];
        for &[first, run] in &layout.packets {
            for p in (first as usize..).take(run as usize) {
                match seen.get_mut(p) {
                    Some(taken) if !*taken => *taken = true,
                    _ => return Err(layout_error("the packet order is not a permutation")),
                }
                order.push(p);
            }
        }
        if order.len() != count {
            return Err(layout_error("the packet order leaves packets out"));
        }
        reorder_packets(group, &order);
    }
    for &[object, slot, packet] in &layout.users {
        let packet = packet as usize;
        let current = group
            .objects
            .get_mut(object as usize)
            .and_then(|o| slot_mut(o, slot as usize))
            .map(|p| *p)
            .ok_or_else(|| layout_error("an object or a slot of it is not there"))?;
        // The object keeps its values: only an equal packet may take the place.
        match group.packets.get(packet) {
            Some(other) if same_packet(&group.packets[current], other) => {}
            _ => return Err(layout_error("an object would get another packet")),
        }
        *slot_mut(&mut group.objects[object as usize], slot as usize).expect("found above") =
            packet;
    }

    // A pointer goes where its data went. Whatever the layout says, the
    // bytes it points to afterwards are the bytes it pointed to before.
    if !layout.aux.is_empty() || !layout.aux_pointers.is_empty() {
        let old = std::mem::take(&mut group.aux);
        let plan = AuxPlan::new(&old, &layout.aux)?;
        let exceptions = pointer_exceptions(&layout.aux_pointers);
        for (number, (at, size)) in aux_refs(group).into_iter().enumerate() {
            let to = match exceptions.get(&number) {
                Some(&to) => to,
                None => plan
                    .target(*at, size)
                    .ok_or_else(|| layout_error("an aux block is in no run"))?,
            };
            match (block(&old, *at, size), block(&plan.region, to, size)) {
                (Some(before), Some(after)) if before == after => *at = to,
                _ => return Err(layout_error("a pointer would get another aux block")),
            }
        }
        group.aux = plan.region;
    }

    if !layout.strings.is_empty() || !layout.string_pointers.is_empty() {
        let old = std::mem::take(&mut group.strings);
        let plan = StringPlan::new(&old, &layout.strings)?;
        let exceptions = pointer_exceptions(&layout.string_pointers);
        for (number, at) in string_refs(group).into_iter().enumerate() {
            let to = match exceptions.get(&number) {
                Some(&to) => to,
                None => plan
                    .target(*at)
                    .ok_or_else(|| layout_error("a string is in no run"))?,
            };
            match (string_at(&old, *at), string_at(&plan.region, to)) {
                (Some(before), Some(after)) if before == after => *at = to,
                _ => return Err(layout_error("a pointer would get another string")),
            }
        }
        group.strings = plan.region;
    }
    Ok(())
}

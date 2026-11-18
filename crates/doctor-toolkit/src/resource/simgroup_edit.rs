//! Editing SimGroups: attribute values, transforms, models and links.
//!
//! Every edit works on the parsed [`SimGroup`] and keeps it consistent, so
//! [`SimGroup::encode`] writes a valid resource after any number of edits.
//!
//! - **Shared packets are copied on write.** Objects with equal attribute
//!   values of a class share one packet. Changing the value for one object
//!   gives that object its own copy; when the result equals another packet
//!   of the class, the object shares that one instead, and a packet nothing
//!   uses any more is removed.
//! - **Strings and aux blocks are appended.** A new string or GUID, vector
//!   or matrix value reuses equal data already in its region or is appended
//!   to it; the value points there. Aux blocks are shared too, so a value is
//!   never changed in place. On write, blocks nothing refers to are dropped
//!   and the string region is laid out again.
//! - **Zero is the default.** An attribute equal to zero (or an empty
//!   string) is not stored: its bit in the default mask is set.
//! - **Objects are appended or removed.** A duplicate shares the packets of
//!   its source; a copy from another group brings its packets along; a
//!   removed object takes the packets only it used with it.

use std::collections::HashMap;

use crate::resource::neoguid;
use crate::resource::simgroup::{
    ATTR_LOAD_MATRIX, ATTR_SET_MODEL, Attr, CLASS_ENTITY, CLASS_SYSTEM_COMMANDS, Guid, Hash32,
    Kind, Packet, Schema, SimGroup, Target, Value,
};
use crate::{Error, Result};

/// A new object GUID: a canonical NeoGuid of the tool's machine name
/// ([`neoguid::MachineName::TOOLKIT`]) with keys derived from `seed`
/// ([`neoguid::new_guid`]); GUIDs for which `taken` is true are skipped. The
/// same seed gives the same GUID.
pub fn new_object_guid(seed: &str, taken: impl Fn(&Guid) -> bool) -> Guid {
    neoguid::new_guid(&neoguid::MachineName::TOOLKIT, seed, |g| {
        g.resource_id().is_some() || taken(g)
    })
}

/// A transform split into position, rotation and scale.
///
/// The matrix is row-major with rows right, up, at and position (Y up); the
/// rows of the 3×3 part are the rotated axes times the scale. The rotation is
/// in degrees about the fixed X, then Y, then Z axes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub position: [f32; 3],
    pub rotation: [f32; 3],
    pub scale: [f32; 3],
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            position: [0.0; 3],
            rotation: [0.0; 3],
            scale: [1.0; 3],
        }
    }
}

impl Transform {
    /// Splits a matrix. Shear is lost; a mirrored matrix gets a negative X scale.
    pub fn from_matrix(m: &[[f32; 4]; 4]) -> Self {
        let row = |i: usize| [m[i][0] as f64, m[i][1] as f64, m[i][2] as f64];
        let len = |r: [f64; 3]| (r[0] * r[0] + r[1] * r[1] + r[2] * r[2]).sqrt();
        let mut rows = [row(0), row(1), row(2)];
        let mut scale = rows.map(len);
        let det = rows[0][0] * (rows[1][1] * rows[2][2] - rows[1][2] * rows[2][1])
            - rows[0][1] * (rows[1][0] * rows[2][2] - rows[1][2] * rows[2][0])
            + rows[0][2] * (rows[1][0] * rows[2][1] - rows[1][1] * rows[2][0]);
        if det < 0.0 {
            scale[0] = -scale[0];
        }
        for (r, s) in rows.iter_mut().zip(scale) {
            if s != 0.0 {
                *r = r.map(|v| v / s);
            }
        }
        // rows = Rx·Ry·Rz in row-vector form; its transpose c is the
        // column-vector rotation Rz·Ry·Rx.
        let c = |i: usize, j: usize| rows[j][i];
        let sin_y = (-c(2, 0)).clamp(-1.0, 1.0);
        let y = sin_y.asin();
        let (x, z) = if sin_y.abs() < 0.999_999 {
            (c(2, 1).atan2(c(2, 2)), c(1, 0).atan2(c(0, 0)))
        } else {
            (0.0, (-c(0, 1)).atan2(c(1, 1)))
        };
        Self {
            position: [m[3][0], m[3][1], m[3][2]],
            rotation: [x, y, z].map(|a| a.to_degrees() as f32),
            scale: scale.map(|s| s as f32),
        }
    }

    /// Builds the matrix; the last column is `(0, 0, 0, 1)`.
    pub fn to_matrix(&self) -> [[f32; 4]; 4] {
        let [x, y, z] = self.rotation.map(|a| (a as f64).to_radians());
        let (sx, cx, sy, cy, sz, cz) = (x.sin(), x.cos(), y.sin(), y.cos(), z.sin(), z.cos());
        // Column-vector Rz·Ry·Rx.
        let c = [
            [cz * cy, cz * sy * sx - sz * cx, cz * sy * cx + sz * sx],
            [sz * cy, sz * sy * sx + cz * cx, sz * sy * cx - cz * sx],
            [-sy, cy * sx, cy * cx],
        ];
        let mut m = [[0.0f32; 4]; 4];
        for (i, row) in m.iter_mut().take(3).enumerate() {
            for (j, v) in row.iter_mut().take(3).enumerate() {
                *v = (c[j][i] * self.scale[i] as f64) as f32;
            }
        }
        m[3] = [self.position[0], self.position[1], self.position[2], 1.0];
        m
    }
}

/// One attribute whose value differs between two versions of a SimGroup.
#[derive(Debug, Clone, PartialEq)]
pub struct AttrDiff {
    pub object: usize,
    /// Slot in [`crate::resource::simgroup::Object::packets`].
    pub slot: usize,
    pub class: u32,
    pub index: u16,
    pub name: Option<&'static str>,
    pub old: Option<Value>,
    pub new: Option<Value>,
}

/// Compares two versions of a group object by object (by position) and
/// lists the attributes whose values differ. Objects whose class, GUID,
/// links or packet classes differ are listed in the second result.
pub fn compare(old: &SimGroup, new: &SimGroup) -> (Vec<AttrDiff>, Vec<usize>) {
    let mut attrs = Vec::new();
    let mut other = Vec::new();
    for i in 0..old.objects.len().min(new.objects.len()) {
        if old.objects[i].guid != new.objects[i].guid
            || !compare_objects(old, i, new, i, i, &mut attrs)
        {
            other.push(i);
        }
    }
    let common = old.objects.len().min(new.objects.len());
    other.extend(common..old.objects.len().max(new.objects.len()));
    (attrs, other)
}

/// Compares object `a` of `old` with object `b` of `new` and appends the
/// attributes whose values differ to `attrs`, as attributes of object
/// `object`. Returns `false`, appending nothing, when the class, the links,
/// the packet classes or the components of the two differ: their attributes
/// cannot be told apart then.
fn compare_objects(
    old: &SimGroup,
    a: usize,
    new: &SimGroup,
    b: usize,
    object: usize,
    attrs: &mut Vec<AttrDiff>,
) -> bool {
    let (a, b) = (&old.objects[a], &new.objects[b]);
    let classes = |g: &SimGroup, o| -> Vec<u32> {
        g.object_packets(o).map(|p| p.class.0).collect::<Vec<_>>()
    };
    let components = |g: &SimGroup, o: &crate::resource::simgroup::Object| {
        o.components
            .map(|c| g.packets[c].attrs.iter().map(|a| a.raw).collect::<Vec<_>>())
    };
    if (a.class, &a.links) != (b.class, &b.links)
        || classes(old, a) != classes(new, b)
        || components(old, a) != components(new, b)
    {
        return false;
    }
    for (slot, (&pa, &pb)) in a.packets.iter().zip(&b.packets).enumerate() {
        let (pa, pb) = (&old.packets[pa], &new.packets[pb]);
        let mut indices: Vec<u16> = pa.attrs.iter().chain(&pb.attrs).map(|a| a.index).collect();
        indices.sort_unstable();
        indices.dedup();
        for index in indices {
            let find = |attrs: &[Attr]| attrs.iter().find(|a| a.index == index).cloned();
            let (x, y) = (find(&pa.attrs), find(&pb.attrs));
            let same = match (&x, &y) {
                (Some(x), Some(y)) => same_attr(x, y),
                (None, None) => true,
                _ => false,
            };
            if !same {
                attrs.push(AttrDiff {
                    object,
                    slot,
                    class: pb.class.0,
                    index,
                    name: x.as_ref().or(y.as_ref()).and_then(|a| a.name),
                    old: x.map(|a| a.value),
                    new: y.map(|a| a.value),
                });
            }
        }
    }
    true
}

/// Two versions of a group compared with their objects matched by GUID
/// ([`compare_by_guid`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GuidComparison {
    /// Objects both versions have: `(index in old, index in new)`, in the
    /// order of `old`.
    pub matched: Vec<(usize, usize)>,
    /// Attributes whose values differ in matched objects;
    /// [`AttrDiff::object`] is the index in `new`.
    pub attrs: Vec<AttrDiff>,
    /// Matched objects whose class, links, packet classes or components
    /// differ: `(index in old, index in new)`.
    pub other: Vec<(usize, usize)>,
    /// Indices in `old` of the objects `new` does not have.
    pub removed: Vec<usize>,
    /// Indices in `new` of the objects `old` does not have.
    pub added: Vec<usize>,
}

/// Compares two versions of a group with the objects matched by GUID, so an
/// object added or removed in the middle does not shift the comparison of
/// the others as it does in [`compare`]. When a GUID occurs several times in
/// a version, its occurrences are matched in order.
pub fn compare_by_guid(old: &SimGroup, new: &SimGroup) -> GuidComparison {
    let mut by_guid: HashMap<Guid, std::collections::VecDeque<usize>> = HashMap::new();
    for (i, o) in new.objects.iter().enumerate() {
        by_guid.entry(o.guid).or_default().push_back(i);
    }
    let mut out = GuidComparison::default();
    let mut taken = vec![false; new.objects.len()];
    for (a, o) in old.objects.iter().enumerate() {
        match by_guid.get_mut(&o.guid).and_then(|q| q.pop_front()) {
            Some(b) => {
                taken[b] = true;
                out.matched.push((a, b));
                if !compare_objects(old, a, new, b, b, &mut out.attrs) {
                    out.other.push((a, b));
                }
            }
            None => out.removed.push(a),
        }
    }
    out.added = (0..new.objects.len()).filter(|&b| !taken[b]).collect();
    out
}

/// A value for people: matrices as position, rotation and scale.
pub fn describe(value: Option<&Value>) -> String {
    match value {
        None => "default".to_owned(),
        Some(Value::Matrix(m)) => {
            let t = Transform::from_matrix(m);
            let v = |a: [f32; 3]| format!("({}, {}, {})", a[0], a[1], a[2]);
            format!(
                "position {} rotation {} scale {}",
                v(t.position),
                v(t.rotation.map(|a| (a * 1000.0).round() / 1000.0)),
                v(t.scale.map(|a| (a * 10000.0).round() / 10000.0))
            )
        }
        Some(Value::Guid(g)) => match g.resource_id() {
            Some(id) => format!("resource {id:08x}"),
            None => g.to_string(),
        },
        Some(v) => v.to_string(),
    }
}

/// The text form of a value that [`parse_value`] reads back: numbers,
/// hexadecimal hashes, strings as they are, a GUID as 32 hexadecimal digits
/// (8 for a resource reference), vectors and matrices as comma-separated numbers.
pub fn edit_text(value: &Value) -> String {
    let list = |v: &[f32]| {
        v.iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    match value {
        Value::Int(v) => v.to_string(),
        Value::Float(v) => v.to_string(),
        Value::Hash(v) => v.to_string(),
        Value::String(s) => s.clone(),
        Value::Guid(g) => match g.resource_id() {
            Some(id) => format!("{id:08x}"),
            None => g.to_string(),
        },
        Value::Vec3(v) => list(v),
        Value::Matrix(m) => list(m.as_flattened()),
        Value::Bytes(b) => b.iter().map(|x| format!("{x:02x}")).collect(),
    }
}

/// Reads a value of `kind` from its text form (see [`edit_text`]). Empty
/// text is the default (`None`). Ints also accept `true` and `false`.
pub fn parse_value(kind: Kind, text: &str) -> Result<Option<Value>> {
    let raw = text;
    let text = text.trim();
    if raw.is_empty() || (kind != Kind::String && text.is_empty()) {
        return Ok(None);
    }
    let bad = |what: &str| Error::InvalidArgument(format!("`{text}` is not {what}"));
    let hex = |t: &str| u32::from_str_radix(t.trim_start_matches("0x"), 16);
    let floats = |n: usize| -> Result<Vec<f32>> {
        let parts: Vec<&str> = text.split(',').map(str::trim).collect();
        if parts.len() != n {
            return Err(bad(&format!("{n} comma-separated numbers")));
        }
        parts
            .iter()
            .map(|p| p.parse::<f32>().map_err(|_| bad("a list of numbers")))
            .collect()
    };
    Ok(Some(match kind {
        Kind::Int => match text {
            "true" => Value::Int(1),
            "false" => Value::Int(0),
            _ => Value::Int(
                text.parse::<i32>()
                    .or_else(|_| text.parse::<u32>().map(|v| v as i32))
                    .map_err(|_| bad("an integer"))?,
            ),
        },
        Kind::Float => Value::Float(text.parse().map_err(|_| bad("a number"))?),
        Kind::Hash => Value::Hash(Hash32(hex(text).map_err(|_| bad("a hexadecimal hash"))?)),
        Kind::String => Value::String(raw.to_owned()),
        Kind::Guid if text.len() == 32 => {
            let bytes: Vec<u8> = (0..16)
                .map(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16))
                .collect::<std::result::Result<_, _>>()
                .map_err(|_| bad("a GUID"))?;
            Value::Guid(Guid(bytes.try_into().expect("16 bytes")))
        }
        Kind::Guid => Value::Guid(Guid::from_resource_id(
            hex(text).map_err(|_| bad("a GUID or a resource id"))?,
        )),
        Kind::Vec3 => Value::Vec3(floats(3)?.try_into().expect("3 numbers")),
        Kind::Matrix => {
            let m = floats(16)?;
            Value::Matrix(std::array::from_fn(|r| {
                std::array::from_fn(|c| m[4 * r + c])
            }))
        }
    }))
}

/// Whether two values can be stored in the same attribute.
fn same_storage(a: &Value, b: &Value) -> bool {
    let class = |v: &Value| match v {
        Value::Int(_) | Value::Float(_) | Value::Hash(_) => 0,
        Value::String(_) => 1,
        Value::Guid(_) => 2,
        Value::Vec3(_) => 3,
        Value::Matrix(_) => 4,
        Value::Bytes(_) => 5,
    };
    class(a) == class(b)
}

fn kind_accepts(kind: Kind, value: &Value) -> bool {
    matches!(
        (kind, value),
        (
            Kind::Int | Kind::Float | Kind::Hash,
            Value::Int(_) | Value::Float(_) | Value::Hash(_)
        ) | (Kind::String, Value::String(_))
            | (Kind::Guid, Value::Guid(_))
            | (Kind::Vec3, Value::Vec3(_))
            | (Kind::Matrix, Value::Matrix(_))
    )
}

/// The stored word of a scalar value.
fn scalar_bits(value: &Value) -> Option<u32> {
    match value {
        Value::Int(v) => Some(*v as u32),
        Value::Float(v) => Some(v.to_bits()),
        Value::Hash(v) => Some(v.0),
        _ => None,
    }
}

/// Whether a value is the default (zero) and therefore not stored.
fn is_default(value: &Value) -> bool {
    match value {
        Value::String(s) => s.is_empty(),
        v => scalar_bits(v) == Some(0),
    }
}

fn floats_le(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// Whether two stored attributes hold the same value.
fn same_attr(a: &Attr, b: &Attr) -> bool {
    a.index == b.index
        && match (scalar_bits(&a.value), scalar_bits(&b.value)) {
            (Some(x), Some(y)) => x == y && a.target.is_none() == b.target.is_none(),
            (None, None) => a.value == b.value,
            _ => false,
        }
}

impl SimGroup {
    /// How many objects use packet `packet`, as an attribute packet or as
    /// their component list.
    pub fn packet_users(&self, packet: usize) -> usize {
        self.objects
            .iter()
            .map(|o| {
                o.packets.iter().filter(|&&p| p == packet).count()
                    + usize::from(o.components == Some(packet))
            })
            .sum()
    }

    /// Kind of attribute `index` of `class`: the GF2 schema's, or that of a
    /// stored value of the attribute in this group.
    pub fn attr_kind(&self, class: u32, index: u16) -> Option<Kind> {
        if let Some(info) = Schema::gf2().get(class, index) {
            return Some(info.kind);
        }
        let value = self
            .packets
            .iter()
            .filter(|p| p.class.0 == class)
            .flat_map(|p| &p.attrs)
            .find(|a| a.index == index)
            .map(|a| &a.value)?;
        Some(match value {
            Value::Int(_) => Kind::Int,
            Value::Float(_) => Kind::Float,
            Value::Hash(_) | Value::Bytes(_) => return None,
            Value::String(_) => Kind::String,
            Value::Guid(_) => Kind::Guid,
            Value::Vec3(_) => Kind::Vec3,
            Value::Matrix(_) => Kind::Matrix,
        })
    }

    /// Sets attribute `index` of the packet in slot `slot` of `object`
    /// (`Object::packets[slot]`); `None` or zero makes it the default.
    /// Returns whether anything changed.
    ///
    /// The value must be of the attribute's kind: a stored value keeps its
    /// storage (int, float and hash are interchangeable; string, GUID,
    /// vector and matrix are not), and an unset attribute follows the schema.
    pub fn set_attr(
        &mut self,
        object: usize,
        slot: usize,
        index: u16,
        value: Option<Value>,
    ) -> Result<bool> {
        let o = self
            .objects
            .get(object)
            .ok_or_else(|| Error::InvalidArgument(format!("no object {object}")))?;
        let &p = o.packets.get(slot).ok_or_else(|| {
            Error::InvalidArgument(format!("object {object} has no packet {slot}"))
        })?;
        let packet = &self.packets[p];
        let class = packet.class.0;
        if index >= packet.attr_count {
            return Err(Error::InvalidArgument(format!(
                "class {} has {} attribute(s), not {}",
                packet.class,
                packet.attr_count,
                index + 1
            )));
        }
        let current = packet.attrs.iter().find(|a| a.index == index);
        let value = value.filter(|v| !is_default(v));
        if let Some(new) = &value {
            let accepted = match (current, self.attr_kind(class, index)) {
                (Some(c), _) => same_storage(&c.value, new),
                (None, Some(kind)) => kind_accepts(kind, new),
                (None, None) => !matches!(new, Value::Bytes(_)),
            };
            if !accepted {
                return Err(Error::InvalidArgument(format!(
                    "attribute {index} of class {} cannot hold a {} value",
                    packet.class,
                    new.kind_name()
                )));
            }
            if let Value::String(s) = new
                && s.contains('\0')
            {
                return Err(Error::InvalidArgument("string contains a NUL byte".into()));
            }
        }
        let unchanged = match (current, &value) {
            (None, None) => true,
            (Some(a), Some(v)) => match (scalar_bits(&a.value), scalar_bits(v)) {
                (Some(x), Some(y)) => x == y && a.target.is_none(),
                (None, None) => a.value == *v,
                _ => false,
            },
            _ => false,
        };
        if unchanged {
            return Ok(false);
        }
        let name = current
            .and_then(|a| a.name)
            .or_else(|| Schema::gf2().get(class, index).and_then(|i| i.name));
        let attr = value.map(|value| self.make_attr(index, name, value));

        let q = self.writable_packet(object, slot);
        let attrs = &mut self.packets[q].attrs;
        let at = attrs.partition_point(|a| a.index < index);
        let present = attrs.get(at).is_some_and(|a| a.index == index);
        match (attr, present) {
            (Some(attr), true) => attrs[at] = attr,
            (Some(attr), false) => attrs.insert(at, attr),
            (None, true) => {
                attrs.remove(at);
            }
            (None, false) => {}
        }
        self.share_packet(object, slot);
        Ok(true)
    }

    /// Sets attribute `index` of class `class` of `object`; see [`Self::set_attr`].
    pub fn set_class_attr(
        &mut self,
        object: usize,
        class: u32,
        index: u16,
        value: Option<Value>,
    ) -> Result<bool> {
        let slot = self.slot_of(object, class)?;
        self.set_attr(object, slot, index, value)
    }

    /// Sets the world transform (`CSystemCommands.CMD_LoadMatrix`). The
    /// object must have a `CSystemCommands` packet.
    pub fn set_transform(&mut self, object: usize, matrix: [[f32; 4]; 4]) -> Result<bool> {
        self.set_class_attr(
            object,
            CLASS_SYSTEM_COMMANDS,
            ATTR_LOAD_MATRIX,
            Some(Value::Matrix(matrix)),
        )
    }

    /// Moves `object` to `position`, keeping its rotation and scale. An
    /// object without a matrix gets an identity rotation.
    pub fn set_position(&mut self, object: usize, position: [f32; 3]) -> Result<bool> {
        let o = self
            .objects
            .get(object)
            .ok_or_else(|| Error::InvalidArgument(format!("no object {object}")))?;
        let mut m = self
            .transform(o)
            .unwrap_or_else(|| Transform::default().to_matrix());
        m[3][..3].copy_from_slice(&position);
        self.set_transform(object, m)
    }

    /// Shows the `metamodel` resource with id `model` (`Entity.CMD_SetModel`);
    /// `None` clears the model.
    pub fn set_model(&mut self, object: usize, model: Option<u32>) -> Result<bool> {
        let value = model.map(|id| Value::Guid(Guid::from_resource_id(id)));
        self.set_class_attr(object, CLASS_ENTITY, ATTR_SET_MODEL, value)
    }

    /// Replaces link `link` of `object` (a resource the object needs).
    pub fn set_link(&mut self, object: usize, link: usize, guid: Guid) -> Result<bool> {
        let o = self
            .objects
            .get(object)
            .ok_or_else(|| Error::InvalidArgument(format!("no object {object}")))?;
        let current = *o
            .links
            .get(link)
            .ok_or_else(|| Error::InvalidArgument(format!("object {object} has no link {link}")))?;
        if current == guid {
            return Ok(false);
        }
        let at = self.intern_aux(&guid.0);
        let o = &mut self.objects[object];
        o.links[link] = guid;
        o.link_refs[link] = at;
        Ok(true)
    }

    /// Slot of the packet of `class` in `object`.
    pub fn slot_of(&self, object: usize, class: u32) -> Result<usize> {
        let o = self
            .objects
            .get(object)
            .ok_or_else(|| Error::InvalidArgument(format!("no object {object}")))?;
        o.packets
            .iter()
            .position(|&p| self.packets[p].class.0 == class)
            .ok_or_else(|| {
                Error::InvalidArgument(format!(
                    "object {object} has no packet of class {}",
                    Hash32(class)
                ))
            })
    }

    fn make_attr(&mut self, index: u16, name: Option<&'static str>, value: Value) -> Attr {
        let (raw, target) = match &value {
            v @ (Value::Int(_) | Value::Float(_) | Value::Hash(_)) => {
                (scalar_bits(v).expect("scalar"), None)
            }
            Value::String(s) => (0, Some(Target::String(self.intern_string(s)))),
            Value::Guid(g) => (0, Some(Target::Aux(self.intern_aux(&g.0)))),
            Value::Vec3(v) => (0, Some(Target::Aux(self.intern_aux(&floats_le(v))))),
            Value::Matrix(m) => (
                0,
                Some(Target::Aux(self.intern_aux(&floats_le(m.as_flattened())))),
            ),
            Value::Bytes(b) => (0, Some(Target::Aux(self.intern_aux(b)))),
        };
        let resource = match &value {
            Value::Guid(g) => g.resource_id().map(Hash32),
            _ => None,
        };
        Attr {
            index,
            name,
            raw,
            value,
            resource,
            target,
        }
    }

    /// Offset of `bytes` in the aux region at a 4-byte boundary, appended
    /// when not there yet.
    fn intern_aux(&mut self, bytes: &[u8]) -> usize {
        self.relayout = true;
        let n = bytes.len();
        if n > 0 && self.aux.len() >= n {
            let found = (0..=self.aux.len() - n)
                .step_by(4)
                .find(|&at| &self.aux[at..at + n] == bytes);
            if let Some(at) = found {
                return at;
            }
        }
        self.aux.resize(self.aux.len().next_multiple_of(4), 0);
        let at = self.aux.len();
        self.aux.extend_from_slice(bytes);
        at
    }

    /// Offset of `text` in the string region, appended when not there yet.
    fn intern_string(&mut self, text: &str) -> usize {
        self.relayout = true;
        let bytes = text.as_bytes();
        let mut at = 0;
        while at < self.strings.len() {
            let end = self.strings[at..]
                .iter()
                .position(|&b| b == 0)
                .map_or(self.strings.len(), |e| at + e);
            if at > 0 && self.strings[at - 1] == 0 && &self.strings[at..end] == bytes {
                return at;
            }
            at = end + 1;
        }
        let at = self.strings.len();
        self.strings.extend_from_slice(bytes);
        self.strings.push(0);
        at
    }

    /// Index of a packet in slot `slot` of `object` that only this object
    /// uses: the packet itself, or a new copy of it.
    fn writable_packet(&mut self, object: usize, slot: usize) -> usize {
        let p = self.objects[object].packets[slot];
        if self.packet_users(p) == 1 {
            return p;
        }
        let mut copy = self.packets[p].clone();
        copy.offset = 0;
        self.packets.push(copy);
        let q = self.packets.len() - 1;
        self.objects[object].packets[slot] = q;
        q
    }

    /// After an edit of the packet in slot `slot` of `object`: share an
    /// equal packet if there is one, and drop the edited one if nothing
    /// uses it any more.
    fn share_packet(&mut self, object: usize, slot: usize) {
        let q = self.objects[object].packets[slot];
        let packet = &self.packets[q];
        let equal = self.packets.iter().enumerate().position(|(i, other)| {
            i != q
                && other.class == packet.class
                && other.attr_count == packet.attr_count
                && other.attrs.len() == packet.attrs.len()
                && other
                    .attrs
                    .iter()
                    .zip(&packet.attrs)
                    .all(|(a, b)| same_attr(a, b))
        });
        if let Some(r) = equal {
            self.objects[object].packets[slot] = r;
            if self.packet_users(q) == 0 {
                self.remove_packet(q);
            }
        }
    }

    // ------------------------------------------------------------ objects

    /// Appends a copy of object `source` with GUID `guid`; returns its index.
    ///
    /// The copy shares every packet, the component list and the links of its
    /// source, so the group only grows by the object record and the GUID. A
    /// later edit of the copy gives it its own packets (copy on write).
    pub fn duplicate_object(&mut self, source: usize, guid: Guid) -> Result<usize> {
        let object = self
            .objects
            .get(source)
            .ok_or_else(|| Error::InvalidArgument(format!("no object {source}")))?
            .clone();
        self.check_new_guid(&guid)?;
        let mut copy = object;
        copy.offset = 0;
        copy.guid = guid;
        copy.guid_ref = self.intern_aux(&guid.0);
        self.objects.push(copy);
        Ok(self.objects.len() - 1)
    }

    /// Appends a copy of object `source` of another group `from`, with GUID
    /// `guid`; returns its index. Its packets are copied into this group and
    /// shared with equal packets already here.
    pub fn copy_object_from(
        &mut self,
        from: &SimGroup,
        source: usize,
        guid: Guid,
    ) -> Result<usize> {
        let object = from
            .objects
            .get(source)
            .ok_or_else(|| Error::InvalidArgument(format!("no object {source}")))?;
        self.check_new_guid(&guid)?;
        let packets = object
            .packets
            .iter()
            .map(|&p| self.import_packet(&from.packets[p]))
            .collect();
        let components = object
            .components
            .map(|c| self.import_packet(&from.packets[c]));
        let link_refs = object.links.iter().map(|l| self.intern_aux(&l.0)).collect();
        let guid_ref = self.intern_aux(&guid.0);
        self.objects.push(crate::resource::simgroup::Object {
            offset: 0,
            flags: object.flags,
            flags2: object.flags2,
            class: object.class,
            guid,
            links: object.links.clone(),
            guid_ref,
            link_refs,
            components,
            packets,
        });
        Ok(self.objects.len() - 1)
    }

    /// A group without objects.
    pub fn empty() -> Self {
        Self {
            version: 1,
            group_hash: 0,
            flags: 1,
            objects: Vec::new(),
            packets: Vec::new(),
            aux: Vec::new(),
            strings: vec![0],
            relayout: true,
        }
    }

    /// Appends an object of class `class` with GUID `guid` whose packets
    /// `packets` (class hash and attribute count, base class first) hold no
    /// values; returns its index. Empty packets are shared with equal ones.
    /// The flags are those of the game's logic objects (3 and 1, no links,
    /// no components).
    pub fn push_object(&mut self, class: u32, guid: Guid, packets: &[(u32, u16)]) -> Result<usize> {
        self.check_new_guid(&guid)?;
        let packets = packets
            .iter()
            .map(|&(c, n)| {
                self.import_packet(&Packet {
                    offset: 0,
                    class: Hash32(c),
                    class_name: crate::resource::names::sim_classes().get(c),
                    attr_count: n,
                    attrs: Vec::new(),
                })
            })
            .collect();
        let guid_ref = self.intern_aux(&guid.0);
        self.objects.push(crate::resource::simgroup::Object {
            offset: 0,
            flags: 3,
            flags2: 1,
            class,
            guid,
            links: Vec::new(),
            guid_ref,
            link_refs: Vec::new(),
            components: None,
            packets,
        });
        Ok(self.objects.len() - 1)
    }

    /// Removes object `object` and the packets only it used; returns it.
    /// Indices of the later objects drop by one. GUID references to the
    /// object from other objects are not changed (see [`Self::references_to`]).
    pub fn remove_object(&mut self, object: usize) -> Result<crate::resource::simgroup::Object> {
        if object >= self.objects.len() {
            return Err(Error::InvalidArgument(format!("no object {object}")));
        }
        let removed = self.objects.remove(object);
        // Its aux blocks are dropped by the next encode.
        self.relayout = true;
        let mut used: Vec<usize> = removed
            .packets
            .iter()
            .copied()
            .chain(removed.components)
            .filter(|&p| self.packet_users(p) == 0)
            .collect();
        used.sort_unstable();
        used.dedup();
        for &p in used.iter().rev() {
            self.remove_packet(p);
        }
        Ok(removed)
    }

    /// Attributes of other objects that hold `guid`: `(object, slot, index)`.
    pub fn references_to(&self, guid: &Guid) -> Vec<(usize, usize, u16)> {
        let mut out = Vec::new();
        for (i, o) in self.objects.iter().enumerate() {
            for (slot, &p) in o.packets.iter().enumerate() {
                for a in &self.packets[p].attrs {
                    if a.value == Value::Guid(*guid) {
                        out.push((i, slot, a.index));
                    }
                }
            }
        }
        out
    }

    fn check_new_guid(&self, guid: &Guid) -> Result<()> {
        if guid.is_nil() || guid.resource_id().is_some() {
            return Err(Error::InvalidArgument(format!(
                "{guid} is not an object GUID"
            )));
        }
        if self.objects.iter().any(|o| o.guid == *guid) {
            return Err(Error::InvalidArgument(format!(
                "an object with GUID {guid} is already in the group"
            )));
        }
        Ok(())
    }

    /// Adds a packet of another group to this one: its pointer values are
    /// interned here. Returns an equal packet of this group if there is one.
    fn import_packet(&mut self, packet: &Packet) -> usize {
        let attrs: Vec<Attr> = packet
            .attrs
            .iter()
            .map(|a| match a.target {
                None => Attr {
                    target: None,
                    ..a.clone()
                },
                Some(_) => self.make_attr(a.index, a.name, a.value.clone()),
            })
            .collect();
        let new = Packet {
            offset: 0,
            class: packet.class,
            class_name: packet.class_name,
            attr_count: packet.attr_count,
            attrs,
        };
        let equal = self.packets.iter().position(|p| {
            p.class == new.class
                && p.attr_count == new.attr_count
                && p.attrs.len() == new.attrs.len()
                && p.attrs.iter().zip(&new.attrs).all(|(a, b)| same_attr(a, b))
        });
        equal.unwrap_or_else(|| {
            self.packets.push(new);
            self.packets.len() - 1
        })
    }

    fn remove_packet(&mut self, q: usize) {
        self.packets.remove(q);
        let shift = |i: &mut usize| {
            if *i > q {
                *i -= 1;
            }
        };
        for o in &mut self.objects {
            o.packets.iter_mut().for_each(shift);
            if let Some(c) = &mut o.components {
                shift(c);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::simgroup::tests::{sample, schema};

    /// The sample group with a second object that shares both packets.
    fn two_objects() -> Vec<u8> {
        let mut group = SimGroup::parse_with(&sample(), &schema()).unwrap();
        let mut second = group.objects[0].clone();
        second.flags2 = 0;
        group.objects.push(second);
        group.encode()
    }

    fn parse(data: &[u8]) -> SimGroup {
        SimGroup::parse_with(data, &schema()).unwrap()
    }

    fn value(group: &SimGroup, object: usize, slot: usize, index: u16) -> Option<Value> {
        let p = group.objects[object].packets[slot];
        let attrs = &group.packets[p].attrs;
        attrs
            .iter()
            .find(|a| a.index == index)
            .map(|a| a.value.clone())
    }

    #[test]
    fn shared_packets_are_copied_on_write_and_shared_again() {
        let data = two_objects();
        let mut group = parse(&data);
        assert_eq!(group.packets.len(), 2);
        assert_eq!(group.packet_users(0), 2);
        assert!(group.set_attr(0, 0, 2, Some(Value::Int(42))).unwrap());
        assert_eq!(group.packets.len(), 3, "object 0 got its own copy");
        let edited = parse(&group.encode());
        assert_eq!(value(&edited, 0, 0, 2), Some(Value::Int(42)));
        assert_eq!(value(&edited, 1, 0, 2), Some(Value::Int(-7)));
        assert_eq!(value(&edited, 0, 0, 0), Some(Value::String("hello".into())));
        // Back to the old value: the copy merges into the shared packet again.
        assert!(group.set_attr(0, 0, 2, Some(Value::Int(-7))).unwrap());
        assert_eq!(group.packets.len(), 2);
        assert_eq!(group.encode(), data);
        // No-op edits change nothing.
        assert!(!group.set_attr(0, 0, 2, Some(Value::Int(-7))).unwrap());
        assert!(!group.set_attr(0, 0, 1, None).unwrap());
        assert_eq!(group.encode(), data);
    }

    #[test]
    fn strings_and_defaults() {
        let data = two_objects();
        let mut group = parse(&data);
        group
            .set_attr(1, 0, 0, Some(Value::String("a longer name".into())))
            .unwrap();
        group.set_attr(1, 0, 1, Some(Value::Float(2.5))).unwrap();
        group.set_attr(1, 0, 2, None).unwrap();
        let bytes = group.encode();
        let edited = parse(&bytes);
        assert_eq!(
            value(&edited, 1, 0, 0),
            Some(Value::String("a longer name".into()))
        );
        assert_eq!(
            value(&edited, 1, 0, 1),
            Some(Value::Hash(Hash32(2.5f32.to_bits())))
        );
        assert_eq!(value(&edited, 1, 0, 2), None);
        assert_eq!(value(&edited, 0, 0, 0), Some(Value::String("hello".into())));
        assert_eq!(&edited.strings[..], b"\0hello\0a longer name\0\0\0\0");
        assert_eq!(edited.encode(), bytes, "the new layout is stable");
        // An empty string is the default; the unused string disappears.
        group
            .set_attr(1, 0, 0, Some(Value::String(String::new())))
            .unwrap();
        let edited = parse(&group.encode());
        assert_eq!(value(&edited, 1, 0, 0), None);
        assert_eq!(&edited.strings[..], b"\0hello\0\0");
    }

    #[test]
    fn aux_values_are_appended_and_unused_blocks_dropped() {
        let data = two_objects();
        let original = parse(&data);
        let mut group = original.clone();
        let mut m = Transform {
            position: [10.0, 20.0, 30.0],
            rotation: [0.0, 45.0, 0.0],
            scale: [1.0; 3],
        }
        .to_matrix();
        group.set_attr(0, 1, 1, Some(Value::Matrix(m))).unwrap();
        group.set_attr(1, 1, 1, Some(Value::Matrix(m))).unwrap();
        assert_eq!(group.packets.len(), 2, "both objects share the new packet");
        let edited = parse(&group.encode());
        assert_eq!(value(&edited, 1, 1, 1), Some(Value::Matrix(m)));
        assert_eq!(edited.aux.len(), original.aux.len(), "old matrix dropped");
        m[3][0] = -1.0;
        group.set_attr(1, 1, 1, Some(Value::Matrix(m))).unwrap();
        let g = Guid::from_resource_id(0x1234_5678);
        group.set_attr(1, 1, 0, Some(Value::Guid(g))).unwrap();
        let edited = parse(&group.encode());
        assert_eq!(
            value(&edited, 0, 1, 1).unwrap(),
            value(&group, 0, 1, 1).unwrap()
        );
        assert_eq!(value(&edited, 1, 1, 1), Some(Value::Matrix(m)));
        let attr = &edited.packets[edited.objects[1].packets[1]].attrs[0];
        assert_eq!(attr.resource, Some(Hash32(0x1234_5678)));
        assert_eq!(edited.objects[1].guid, original.objects[1].guid);
        // Links.
        assert!(group.objects[0].links.is_empty());
        assert!(group.set_link(0, 0, g).is_err());
    }

    #[test]
    fn objects_are_duplicated_copied_and_removed() {
        let data = two_objects();
        let original = parse(&data);
        let mut group = original.clone();
        let taken = |g: &Guid| group.objects.iter().any(|o| o.guid == *g);
        let guid = new_object_guid("test", taken);
        assert_eq!(&guid.0[8..], b"DOCTORTK");
        assert_eq!(guid.0[3], b'X', "byte 3 is the last letter of XDOCTORTKX");
        assert!(crate::resource::neoguid::parts(&guid).is_some());
        assert_eq!(guid, new_object_guid("test", |_| false), "deterministic");
        assert_ne!(guid, new_object_guid("test", |g| *g == guid));

        // A duplicate shares the packets and only adds its GUID.
        let copy = group.duplicate_object(0, guid).unwrap();
        assert_eq!(copy, 2);
        assert!(group.duplicate_object(0, guid).is_err(), "GUID in use");
        assert!(
            group
                .duplicate_object(0, Guid::from_resource_id(5))
                .is_err()
        );
        assert_eq!(group.packets.len(), 2);
        let bytes = group.encode();
        let edited = parse(&bytes);
        assert_eq!(edited.objects.len(), 3);
        assert_eq!(edited.objects[2].guid, guid);
        assert_eq!(edited.objects[2].packets, edited.objects[0].packets);
        assert_eq!(edited.aux.len(), original.aux.len() + 16);
        assert_eq!(edited.encode(), bytes, "stable");

        // Moving the copy gives it its own packet; the source keeps its matrix.
        let moved = Transform {
            position: [5.0, 6.0, 7.0],
            ..Default::default()
        }
        .to_matrix();
        group
            .set_attr(copy, 1, 1, Some(Value::Matrix(moved)))
            .unwrap();
        let edited = parse(&group.encode());
        assert_eq!(value(&edited, 2, 1, 1), Some(Value::Matrix(moved)));
        assert_eq!(value(&edited, 0, 1, 1), value(&original, 0, 1, 1));

        // Removing it again gives the original bytes.
        let removed = group.remove_object(copy).unwrap();
        assert_eq!(removed.guid, guid);
        assert_eq!(group.packets.len(), 2, "its own packet went with it");
        assert_eq!(group.encode(), data);
        assert!(group.remove_object(7).is_err());

        // A copy from another group brings its packets and strings along.
        let mut other = parse(&data);
        other
            .set_attr(1, 0, 0, Some(Value::String("elsewhere".into())))
            .unwrap();
        let mut group = original.clone();
        let copied = group.copy_object_from(&other, 1, guid).unwrap();
        assert_eq!(group.packets.len(), 3, "one new packet, one shared");
        let edited = parse(&group.encode());
        let name = edited.object_name(&edited.objects[copied]);
        assert_eq!(name, Some("elsewhere"));
        assert_eq!(edited.object_name(&edited.objects[0]), Some("hello"));
        assert_eq!(
            edited.objects[copied].packets[1],
            edited.objects[0].packets[1]
        );

        // Removing every object leaves an empty group that parses.
        let mut group = original.clone();
        group.remove_object(1).unwrap();
        group.remove_object(0).unwrap();
        assert!(group.packets.is_empty());
        let empty = parse(&group.encode());
        assert!(empty.objects.is_empty());
    }

    #[test]
    fn versions_are_compared_by_guid() {
        let guid = |n: u8| Guid([n, 0, 0, b'X', 0, 0, 0, 0, 68, 79, 67, 84, 79, 82, 84, 75]);
        // Three objects: the sample's and two copies.
        let mut old = parse(&sample());
        old.duplicate_object(0, guid(1)).unwrap();
        old.duplicate_object(0, guid(2)).unwrap();
        let old = parse(&old.encode());
        // The middle one goes, the last one changes, a new one comes.
        let mut new = old.clone();
        new.remove_object(1).unwrap();
        new.set_attr(1, 0, 2, Some(Value::Int(42))).unwrap();
        new.duplicate_object(0, guid(3)).unwrap();
        let new = parse(&new.encode());

        let by_guid = compare_by_guid(&old, &new);
        assert_eq!(by_guid.matched, [(0, 0), (2, 1)]);
        assert_eq!(by_guid.removed, [1]);
        assert_eq!(by_guid.added, [2]);
        assert!(by_guid.other.is_empty());
        assert_eq!(by_guid.attrs.len(), 1);
        let diff = &by_guid.attrs[0];
        assert_eq!((diff.object, diff.slot, diff.index), (1, 0, 2));
        assert_eq!(diff.old, Some(Value::Int(-7)));
        assert_eq!(diff.new, Some(Value::Int(42)));
        // By position the removal shifts everything after it.
        let (attrs, other) = compare(&old, &new);
        assert!(attrs.is_empty());
        assert_eq!(other, [1, 2]);
        // The same version: nothing.
        let same = compare_by_guid(&old, &old);
        assert_eq!(same.matched, [(0, 0), (1, 1), (2, 2)]);
        assert!(same.attrs.is_empty() && same.removed.is_empty() && same.added.is_empty());

        // A GUID that occurs twice: its objects are matched in order, and
        // an object of another class is not compared attribute by attribute.
        let twice = parse(&two_objects());
        let mut changed = twice.clone();
        changed.objects[1].class ^= 1;
        changed.objects.remove(0);
        let compared = compare_by_guid(&twice, &changed);
        assert_eq!(compared.matched, [(0, 0)]);
        assert_eq!(compared.other, [(0, 0)]);
        assert_eq!(compared.removed, [1]);
        assert!(compared.attrs.is_empty() && compared.added.is_empty());
    }

    #[test]
    fn references_to_objects_are_found() {
        let mut group = parse(&two_objects());
        let target = group.objects[0].guid;
        assert!(group.references_to(&target).is_empty());
        group.set_attr(1, 1, 0, Some(Value::Guid(target))).unwrap();
        assert_eq!(group.references_to(&target), vec![(1, 1, 0)]);
    }

    #[test]
    fn rejects_values_of_another_kind() {
        let mut group = parse(&two_objects());
        let before = group.clone();
        assert!(group.set_attr(0, 0, 0, Some(Value::Int(1))).is_err());
        assert!(
            group
                .set_attr(0, 1, 1, Some(Value::Vec3([0.0; 3])))
                .is_err()
        );
        assert!(group.set_attr(0, 0, 3, Some(Value::Int(1))).is_err());
        assert!(group.set_attr(0, 2, 0, None).is_err());
        assert!(group.set_attr(9, 0, 0, None).is_err());
        assert!(
            group
                .set_attr(0, 0, 0, Some(Value::String("a\0b".into())))
                .is_err()
        );
        assert_eq!(group, before);
    }

    #[test]
    fn values_round_trip_through_text() {
        let m = Transform {
            position: [1.5, 2.0, -3.0],
            rotation: [0.0, 30.0, 0.0],
            scale: [1.0; 3],
        }
        .to_matrix();
        for (kind, value) in [
            (Kind::Int, Value::Int(-5)),
            (Kind::Float, Value::Float(0.1)),
            (Kind::Hash, Value::Hash(Hash32(0xFF80_0000))),
            (Kind::String, Value::String(" spaced ".into())),
            (Kind::Guid, Value::Guid(Guid::from_resource_id(0x2F4F_CBD4))),
            (
                Kind::Guid,
                Value::Guid(Guid(*b"\x01\x02\x03\x04\x05\x06\x07\x08CHRIST42")),
            ),
            (Kind::Vec3, Value::Vec3([1.0, -2.5, 3.25])),
            (Kind::Matrix, Value::Matrix(m)),
        ] {
            let text = edit_text(&value);
            assert_eq!(parse_value(kind, &text).unwrap(), Some(value), "{text}");
        }
        assert_eq!(parse_value(Kind::Int, "true").unwrap(), Some(Value::Int(1)));
        assert_eq!(parse_value(Kind::Float, " ").unwrap(), None);
        assert!(parse_value(Kind::Int, "1.5").is_err());
        assert!(parse_value(Kind::Vec3, "1, 2").is_err());
        assert!(parse_value(Kind::Guid, "xyz").is_err());
    }

    #[test]
    fn transforms_round_trip() {
        for t in [
            Transform::default(),
            Transform {
                position: [1.0, -2.0, 3.5],
                rotation: [10.0, -35.0, 120.0],
                scale: [0.75, 4.0, 0.25],
            },
            Transform {
                position: [0.0; 3],
                rotation: [0.0, 90.0, 0.0],
                scale: [1.0; 3],
            },
        ] {
            let m = t.to_matrix();
            let back = Transform::from_matrix(&m);
            let m2 = back.to_matrix();
            for (a, b) in m.as_flattened().iter().zip(m2.as_flattened()) {
                assert!((a - b).abs() < 1e-4, "{t:?} -> {back:?}");
            }
            for (a, b) in t.scale.iter().zip(back.scale) {
                assert!((a - b).abs() < 1e-5);
            }
        }
        // Yaw of 90° about Y turns the right axis into -Z (row form).
        let m = Transform {
            rotation: [0.0, 90.0, 0.0],
            ..Default::default()
        }
        .to_matrix();
        assert!(
            (m[0][2] + 1.0).abs() < 1e-6 && m[0][0].abs() < 1e-6,
            "{m:?}"
        );
    }
}

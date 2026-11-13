//! The author's edits of SimGroup objects, kept apart from the exported JSON
//! (`patches/<stream>/simgroup/<name>.toml`).
//!
//! A patch changes objects of the group by GUID or name and adds new objects
//! copied from a template. A new object's GUID comes from its name
//! ([`super::guid::derived`]), so it is the same on every build. An entry
//! whose object is no longer in the group is a warning, not an error: the
//! rest of the patch still applies.
//!
//! ```toml
//! [[object]]
//! name = "door_01"             # or guid = "<32 hexadecimal digits>"
//! position = [10.0, 0.0, -4.5]
//! rotation = [0.0, 90.0, 0.0]  # degrees about X, Y, Z
//! set = { "Entity.CMD_SetModel" = "2f4fcbd4" }
//!
//! [[new]]
//! name = "doctor_lamp_1"       # sets CMD_SetName when the template has it
//! copy = "lamp_02"             # template: name or GUID of an object of the group
//! position = [12.0, 0.0, -4.5]
//! ```

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::resource::simgroup::{Guid, SimGroup, Value};
use crate::resource::simgroup_edit::{Transform, parse_value};
use crate::{Error, Result};

/// A patch file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SimGroupPatch {
    #[serde(default, rename = "object", skip_serializing_if = "Vec::is_empty")]
    pub objects: Vec<ObjectEdit>,
    #[serde(default, rename = "new", skip_serializing_if = "Vec::is_empty")]
    pub new: Vec<NewObject>,
}

/// Changes of one object of the group.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ObjectEdit {
    /// GUID of the object, 32 hexadecimal digits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guid: Option<String>,
    /// Name of the object (`CMD_SetName`), when there is no GUID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(flatten)]
    pub change: Change,
}

/// A new object: a copy of a template object of the group.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NewObject {
    /// Name of the object; its GUID is derived from it.
    pub name: String,
    /// Name or GUID of the object to copy.
    pub copy: String,
    #[serde(flatten)]
    pub change: Change,
}

/// What to change in an object.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Change {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<[f32; 3]>,
    /// Degrees about the fixed X, then Y, then Z axes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<[f32; 3]>,
    /// Attributes: `Class.attribute` (names or hexadecimal class and index)
    /// → value as `simgroup-set` reads it; an empty value is the default.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub set: BTreeMap<String, String>,
}

/// What a patch did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PatchReport {
    /// Edits whose object is not in the group, and the like.
    pub warnings: Vec<String>,
    /// New objects: name and GUID.
    pub added: Vec<(String, Guid)>,
    /// Objects changed (existing ones).
    pub changed: usize,
}

impl SimGroupPatch {
    pub fn parse(text: &str) -> Result<Self> {
        toml::from_str(text).map_err(|e| Error::InvalidArgument(e.to_string()))
    }

    pub fn is_empty(&self) -> bool {
        self.objects.is_empty() && self.new.is_empty()
    }

    /// Applies the patch to `group`. `scope` names the project and the group
    /// for the GUIDs of new objects.
    pub fn apply(&self, group: &mut SimGroup, scope: &str) -> Result<PatchReport> {
        let mut report = PatchReport::default();
        for (i, edit) in self.objects.iter().enumerate() {
            let at = format!("object {}", i + 1);
            let wanted = match (&edit.guid, &edit.name) {
                (Some(g), _) => g.clone(),
                (None, Some(n)) => n.clone(),
                (None, None) => {
                    return Err(Error::InvalidArgument(format!(
                        "{at}: give `guid` or `name`"
                    )));
                }
            };
            let Some(object) = find_object(group, &wanted)? else {
                report.warnings.push(format!(
                    "{at}: no object `{wanted}` in the group any more; the entry is skipped"
                ));
                continue;
            };
            if apply_change(group, object, &edit.change).map_err(|e| super::at(&at, e))? {
                report.changed += 1;
            }
        }
        let mut names = std::collections::HashSet::new();
        for (i, new) in self.new.iter().enumerate() {
            let at = format!("new object {} (`{}`)", i + 1, new.name);
            if new.name.trim().is_empty() {
                return Err(Error::InvalidArgument(format!("{at}: the name is empty")));
            }
            if !names.insert(new.name.to_ascii_lowercase()) {
                return Err(Error::InvalidArgument(format!(
                    "{at}: two new objects have this name"
                )));
            }
            let Some(template) = find_object(group, &new.copy)? else {
                report.warnings.push(format!(
                    "{at}: no template object `{}` in the group; the object is not added",
                    new.copy
                ));
                continue;
            };
            let taken = |g: &Guid| group.objects.iter().any(|o| o.guid == *g);
            let guid = super::guid::derived(scope, &new.name, taken);
            let object = group.duplicate_object(template, guid)?;
            set_attribute_named(group, object, "CMD_SetName", &new.name)?;
            apply_change(group, object, &new.change).map_err(|e| super::at(&at, e))?;
            report.added.push((new.name.clone(), guid));
        }
        Ok(report)
    }
}

/// The object with GUID `wanted` (32 hexadecimal digits) or name `wanted`.
fn find_object(group: &SimGroup, wanted: &str) -> Result<Option<usize>> {
    let text = wanted.trim();
    if text.len() == 32 && text.bytes().all(|b| b.is_ascii_hexdigit()) {
        let bytes: Vec<u8> = (0..16)
            .map(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16).expect("hexadecimal"))
            .collect();
        let guid = Guid(bytes.try_into().expect("16 bytes"));
        return Ok(group.objects.iter().position(|o| o.guid == guid));
    }
    let found: Vec<usize> = (0..group.objects.len())
        .filter(|&i| {
            group
                .object_name(&group.objects[i])
                .is_some_and(|n| n.eq_ignore_ascii_case(text))
        })
        .collect();
    match found.as_slice() {
        [] => Ok(None),
        [one] => Ok(Some(*one)),
        _ => Err(Error::InvalidArgument(format!(
            "{} objects are named `{text}`; use the GUID",
            found.len()
        ))),
    }
}

/// Applies a change to one object. Returns whether anything changed.
fn apply_change(group: &mut SimGroup, object: usize, change: &Change) -> Result<bool> {
    let mut changed = false;
    for (target, text) in &change.set {
        changed |= set_attribute(group, object, target, text)?;
    }
    if change.position.is_some() || change.rotation.is_some() || change.scale.is_some() {
        let current = group
            .transform(&group.objects[object])
            .ok_or_else(|| Error::InvalidArgument("the object has no transform".into()))?;
        let mut m = current;
        if change.rotation.is_some() || change.scale.is_some() {
            let mut t = Transform::from_matrix(&current);
            if let Some(r) = change.rotation {
                t.rotation = r;
            }
            if let Some(s) = change.scale {
                t.scale = s;
            }
            m = t.to_matrix();
            m[3] = current[3];
        }
        if let Some(p) = change.position {
            m[3][..3].copy_from_slice(&p);
        }
        changed |= group.set_transform(object, m)?;
    }
    Ok(changed)
}

/// Sets `Class.attribute` of `object` from text.
fn set_attribute(group: &mut SimGroup, object: usize, target: &str, text: &str) -> Result<bool> {
    let (class, attr) = target
        .split_once('.')
        .ok_or_else(|| Error::InvalidArgument(format!("`{target}`: `Class.attribute` expected")))?;
    let hex = |t: &str| u32::from_str_radix(t.trim_start_matches("0x"), 16).ok();
    let o = &group.objects[object];
    let slot = o
        .packets
        .iter()
        .position(|&p| {
            let packet = &group.packets[p];
            packet
                .class_name
                .is_some_and(|n| n.eq_ignore_ascii_case(class))
                || hex(class) == Some(packet.class.0)
        })
        .ok_or_else(|| {
            Error::InvalidArgument(format!("`{target}`: the object has no `{class}` packet"))
        })?;
    let packet = &group.packets[o.packets[slot]];
    let class_hash = packet.class.0;
    let schema = crate::resource::simgroup::Schema::gf2();
    let index = match attr.parse::<u16>() {
        Ok(index) => index,
        Err(_) => (0..packet.attr_count)
            .find(|&i| {
                schema
                    .get(class_hash, i)
                    .and_then(|info| info.name)
                    .is_some_and(|n| n.eq_ignore_ascii_case(attr))
            })
            .ok_or_else(|| {
                Error::InvalidArgument(format!("`{target}`: `{class}` has no attribute `{attr}`"))
            })?,
    };
    let kind = group.attr_kind(class_hash, index).ok_or_else(|| {
        Error::InvalidArgument(format!(
            "`{target}`: the kind of the attribute is not known"
        ))
    })?;
    let value = parse_value(kind, text).map_err(|e| super::at(&format!("`{target}`"), e))?;
    group.set_attr(object, slot, index, value)
}

/// Sets the attribute named `name` (in any packet of the object) to the
/// string `text`; nothing when the object has no such attribute.
fn set_attribute_named(group: &mut SimGroup, object: usize, name: &str, text: &str) -> Result<()> {
    let schema = crate::resource::simgroup::Schema::gf2();
    let o = &group.objects[object];
    let found = o.packets.iter().enumerate().find_map(|(slot, &p)| {
        let packet = &group.packets[p];
        (0..packet.attr_count)
            .find(|&i| {
                schema
                    .get(packet.class.0, i)
                    .and_then(|info| info.name)
                    .is_some_and(|n| n == name)
            })
            .map(|i| (slot, i))
    });
    if let Some((slot, index)) = found {
        group.set_attr(object, slot, index, Some(Value::String(text.to_owned())))?;
    }
    Ok(())
}

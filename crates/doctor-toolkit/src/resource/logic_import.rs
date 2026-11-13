//! Edited JSON back into game-logic resources.
//!
//! [`apply_json`] takes a document in the form [`super::logic::to_json`]
//! writes, compares it with the resource and writes only the values that
//! differ. Structure is matched by position (objects, packets, states,
//! parts, variables) and checked by class, GUID or name, so a document
//! exported from another resource is refused rather than half applied.
//!
//! | Type | What can change |
//! |---|---|
//! | `simgroup` | attribute values (a removed attribute becomes the default), `position`, `model` |
//! | `metamodel` | property values, condition values |
//! | `variabledictionary` | `kind` and `value` of each variable |
//! | `partsdb` | everything: the document is the whole database (parts, variants, sets, ...) |

use serde_json::Value as Json;

use crate::resource::logic::{
    TYPE_METAMODEL, TYPE_PARTSDB, TYPE_SIMGROUP, TYPE_VARIABLE_DICTIONARY,
};
use crate::resource::metamodel::{MetaModel, PropValue};
use crate::resource::metamodel_edit;
use crate::resource::partsdb::PartsDb;
use crate::resource::simgroup::{Guid, Hash32, SimGroup, Value};
use crate::resource::variabledictionary::VariableDictionary;
use crate::{Error, Result};

/// The result of [`apply_json`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Imported {
    /// The new resource data; equal to the input when nothing changed.
    pub data: Vec<u8>,
    /// One line per changed value.
    pub changes: Vec<String>,
}

/// Whether [`apply_json`] supports resources of `type_id`.
pub fn is_supported(type_id: u32) -> bool {
    [
        TYPE_SIMGROUP,
        TYPE_METAMODEL,
        TYPE_VARIABLE_DICTIONARY,
        TYPE_PARTSDB,
    ]
    .contains(&type_id)
}

/// Applies the values of `doc` to the resource `data` of type `type_id`.
pub fn apply_json(type_id: u32, data: &[u8], doc: &Json) -> Result<Imported> {
    match type_id {
        TYPE_SIMGROUP => simgroup(data, doc),
        TYPE_METAMODEL => metamodel(data, doc),
        TYPE_VARIABLE_DICTIONARY => variables(data, doc),
        TYPE_PARTSDB => parts_database(data, doc),
        _ => Err(Error::InvalidArgument(format!(
            "type {:08x} cannot be imported from JSON",
            type_id
        ))),
    }
}

/// A parts database is written whole from the document.
fn parts_database(data: &[u8], doc: &Json) -> Result<Imported> {
    let old = PartsDb::parse(data)?;
    let new: PartsDb = serde_json::from_value(doc.clone())
        .map_err(|e| bad(format!("not a parts database: {e}")))?;
    if new.name != old.name {
        return Err(bad(format!(
            "the document is database `{}`, the resource is `{}`",
            new.name, old.name
        )));
    }
    let mut changes = Vec::new();
    let mut table = |what: &str, a: usize, b: usize, same: bool| {
        if a != b {
            changes.push(format!("{what}: {a} -> {b}"));
        } else if !same {
            changes.push(format!("{what}: changed"));
        }
    };
    table(
        "slots",
        old.slots.len(),
        new.slots.len(),
        old.slots == new.slots,
    );
    table(
        "parts",
        old.parts.len(),
        new.parts.len(),
        old.parts == new.parts,
    );
    table(
        "variants",
        old.variants.len(),
        new.variants.len(),
        old.variants == new.variants,
    );
    table(
        "presets",
        old.presets.len(),
        new.presets.len(),
        old.presets == new.presets,
    );
    table(
        "blenders",
        old.blenders.len(),
        new.blenders.len(),
        old.blenders == new.blenders,
    );
    table("sets", old.sets.len(), new.sets.len(), old.sets == new.sets);
    table(
        "groups",
        old.groups.len(),
        new.groups.len(),
        old.groups == new.groups,
    );
    if new.kind != old.kind {
        changes.push(format!("kind: {} -> {}", old.kind, new.kind));
    }
    let data = if changes.is_empty() {
        data.to_vec()
    } else {
        new.encode()?
    };
    Ok(Imported { data, changes })
}

fn bad(what: impl std::fmt::Display) -> Error {
    Error::InvalidArgument(what.to_string())
}

fn field<'a>(json: &'a Json, name: &str, at: &str) -> Result<&'a Json> {
    json.get(name)
        .ok_or_else(|| bad(format!("{at}: missing `{name}`")))
}

fn array<'a>(json: &'a Json, name: &str, at: &str) -> Result<&'a Vec<Json>> {
    field(json, name, at)?
        .as_array()
        .ok_or_else(|| bad(format!("{at}: `{name}` must be a list")))
}

fn hex_u32(json: &Json, at: &str) -> Result<u32> {
    match json {
        Json::String(s) => u32::from_str_radix(s.trim_start_matches("0x"), 16)
            .map_err(|_| bad(format!("{at}: `{s}` is not a hexadecimal number"))),
        Json::Number(n) => n
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| bad(format!("{at}: {n} is not a 32-bit number"))),
        _ => Err(bad(format!("{at}: hexadecimal string expected"))),
    }
}

fn hex_bytes(text: &str, at: &str) -> Result<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return Err(bad(format!("{at}: odd number of hexadecimal digits")));
    }
    (0..text.len() / 2)
        .map(|i| {
            u8::from_str_radix(&text[2 * i..2 * i + 2], 16)
                .map_err(|_| bad(format!("{at}: `{text}` is not hexadecimal")))
        })
        .collect()
}

fn int(json: &Json, at: &str) -> Result<i32> {
    match json {
        Json::Bool(b) => Ok(*b as i32),
        // Accepts the signed and the unsigned reading of 32 bits.
        Json::Number(n) => n
            .as_i64()
            .or_else(|| n.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64))
            .filter(|v| (i32::MIN as i64..=u32::MAX as i64).contains(v))
            .map(|v| v as i32)
            .ok_or_else(|| bad(format!("{at}: {n} is not a 32-bit integer"))),
        _ => Err(bad(format!("{at}: integer expected"))),
    }
}

/// A float; `None` for `null` (NaN and infinities are written as `null`).
fn float(json: &Json, at: &str) -> Result<Option<f32>> {
    match json {
        Json::Null => Ok(None),
        Json::Number(n) => Ok(n.as_f64().map(|f| f as f32)),
        _ => Err(bad(format!("{at}: number expected"))),
    }
}

fn floats<const N: usize>(json: &Json, at: &str) -> Result<Option<[f32; N]>> {
    let items = json
        .as_array()
        .filter(|a| a.len() == N)
        .ok_or_else(|| bad(format!("{at}: list of {N} numbers expected")))?;
    let mut out = [0.0; N];
    for (o, item) in out.iter_mut().zip(items) {
        match float(item, at)? {
            Some(v) => *o = v,
            None => return Ok(None),
        }
    }
    Ok(Some(out))
}

fn matrix(json: &Json, at: &str) -> Result<Option<[[f32; 4]; 4]>> {
    let rows = json
        .as_array()
        .filter(|a| a.len() == 4)
        .ok_or_else(|| bad(format!("{at}: 4 rows of 4 numbers expected")))?;
    let mut m = [[0.0; 4]; 4];
    for (row, item) in m.iter_mut().zip(rows) {
        match floats::<4>(item, at)? {
            Some(r) => *row = r,
            None => return Ok(None),
        }
    }
    Ok(Some(m))
}

fn guid(json: &Json, at: &str) -> Result<Guid> {
    let text = json
        .as_str()
        .ok_or_else(|| bad(format!("{at}: GUID string expected")))?;
    let bytes = hex_bytes(text, at)?;
    Ok(Guid(bytes.try_into().map_err(|_| {
        bad(format!("{at}: a GUID has 32 hexadecimal digits"))
    })?))
}

/// A SimGroup value from `{"type": ..., "value": ...}`; `Ok(None)` when the
/// value cannot be compared (a `null` float).
fn sim_value(attr: &Json, at: &str) -> Result<Option<Value>> {
    let kind = field(attr, "type", at)?
        .as_str()
        .ok_or_else(|| bad(format!("{at}: `type` must be a string")))?;
    let value = attr.get("value").unwrap_or(&Json::Null);
    Ok(Some(match kind {
        "int" | "bool" => Value::Int(int(value, at)?),
        "float" => match float(value, at)? {
            Some(v) => Value::Float(v),
            None => return Ok(None),
        },
        "hash" => Value::Hash(Hash32(hex_u32(value, at)?)),
        "string" => Value::String(
            value
                .as_str()
                .ok_or_else(|| bad(format!("{at}: string expected")))?
                .to_owned(),
        ),
        "guid" => Value::Guid(guid(value, at)?),
        "vec3" => match floats::<3>(value, at)? {
            Some(v) => Value::Vec3(v),
            None => return Ok(None),
        },
        "matrix" => match matrix(value, at)? {
            Some(m) => Value::Matrix(m),
            None => return Ok(None),
        },
        "bytes" => Value::Bytes(hex_bytes(
            value
                .as_str()
                .ok_or_else(|| bad(format!("{at}: hexadecimal string expected")))?,
            at,
        )?),
        other => return Err(bad(format!("{at}: unknown value type `{other}`"))),
    }))
}

/// Whether two SimGroup values store the same data.
fn same_sim_value(a: &Value, b: &Value) -> bool {
    let bits = |v: &Value| match v {
        Value::Int(x) => Some(*x as u32),
        Value::Float(x) => Some(x.to_bits()),
        Value::Hash(x) => Some(x.0),
        _ => None,
    };
    match (bits(a), bits(b)) {
        (Some(x), Some(y)) => x == y,
        (None, None) => a == b,
        _ => false,
    }
}

fn show(value: Option<&Value>) -> String {
    value.map_or_else(|| "default".to_owned(), |v| v.to_string())
}

fn simgroup(data: &[u8], doc: &Json) -> Result<Imported> {
    let original = SimGroup::parse(data)?;
    let mut group = original.clone();
    let objects = array(doc, "objects", "document")?;
    if objects.len() != group.objects.len() {
        return Err(bad(format!(
            "the document has {} object(s), the SimGroup {}",
            objects.len(),
            group.objects.len()
        )));
    }
    let mut changes = Vec::new();
    for (i, json) in objects.iter().enumerate() {
        let at = format!("object {i}");
        let object = &original.objects[i];
        if hex_u32(field(json, "class", &at)?, &at)? != object.class
            || json
                .get("guid")
                .is_some_and(|g| guid(g, &at).ok() != Some(object.guid))
        {
            return Err(bad(format!(
                "{at}: class or GUID differ; the document belongs to another SimGroup"
            )));
        }
        let label = format!(
            "object {i} ({})",
            object
                .class_name()
                .map_or_else(|| Hash32(object.class).to_string(), str::to_owned)
        );
        let packets = array(json, "packets", &at)?;
        if packets.len() != object.packets.len() {
            return Err(bad(format!(
                "{at}: {} packet(s) instead of {}",
                packets.len(),
                object.packets.len()
            )));
        }
        for (slot, pj) in packets.iter().enumerate() {
            let at = format!("{at} packet {slot}");
            let packet = &original.packets[object.packets[slot]];
            if hex_u32(field(pj, "class", &at)?, &at)? != packet.class.0 {
                return Err(bad(format!("{at}: class differs")));
            }
            let mut wanted = Vec::new();
            for aj in array(pj, "attrs", &at)? {
                let index = field(aj, "index", &at)?
                    .as_u64()
                    .and_then(|v| u16::try_from(v).ok())
                    .ok_or_else(|| bad(format!("{at}: bad attribute index")))?;
                let at = format!("{at} attribute {index}");
                wanted.push((index, sim_value(aj, &at)?));
            }
            let mut indices: Vec<u16> = wanted.iter().map(|w| w.0).collect();
            indices.extend(packet.attrs.iter().map(|a| a.index));
            indices.sort_unstable();
            indices.dedup();
            for index in indices {
                let current = packet.attrs.iter().find(|a| a.index == index);
                let new = match wanted.iter().find(|w| w.0 == index) {
                    Some((_, None)) => continue, // a value JSON cannot hold
                    Some((_, Some(v))) => Some(v.clone()),
                    None => None,
                };
                let same = match (current, &new) {
                    (None, None) => true,
                    (Some(c), Some(n)) => same_sim_value(&c.value, n),
                    _ => false,
                };
                if same {
                    continue;
                }
                let name = current
                    .and_then(|a| a.name)
                    .map_or_else(|| index.to_string(), str::to_owned);
                let class = packet
                    .class_name
                    .map_or_else(|| packet.class.to_string(), str::to_owned);
                if group.set_attr(i, slot, index, new.clone())? {
                    changes.push(format!(
                        "{label} {class}.{name}: {} -> {}",
                        show(current.map(|a| &a.value)),
                        show(new.as_ref())
                    ));
                }
            }
        }
        // Position and model win over the matrix and GUID when they changed too.
        if let Some(p) = json.get("position").filter(|p| !p.is_null()) {
            let at = format!("{at} position");
            let old = original
                .transform(object)
                .map(|m| [m[3][0], m[3][1], m[3][2]]);
            if let Some(p) = floats::<3>(p, &at)?
                && old != Some(p)
                && group.set_position(i, p)?
            {
                changes.push(format!("{label} position: {old:?} -> {p:?}"));
            }
        }
        if let Some(m) = json.get("model").filter(|m| !m.is_null()) {
            let model = hex_u32(m, &format!("{at} model"))?;
            let old = original.model(object);
            if old != Some(model) && group.set_model(i, Some(model))? {
                changes.push(format!(
                    "{label} model: {} -> {model:08x}",
                    old.map_or_else(|| "none".to_owned(), |m| format!("{m:08x}"))
                ));
            }
        }
    }
    let data = if changes.is_empty() {
        data.to_vec()
    } else {
        group.encode()
    };
    Ok(Imported { data, changes })
}

/// A metamodel value; `Ok(None)` when it cannot be compared (a `null` float).
fn prop_value(json: &Json, at: &str) -> Result<Option<PropValue>> {
    let kind = field(json, "type", at)?
        .as_str()
        .ok_or_else(|| bad(format!("{at}: `type` must be a string")))?;
    let value = json.get("value").unwrap_or(&Json::Null);
    let index = |v: &Json| {
        v.as_u64()
            .map(|v| v as usize)
            .ok_or_else(|| bad(format!("{at}: index expected")))
    };
    Ok(Some(match kind {
        "bool" => PropValue::Bool(
            value
                .as_bool()
                .ok_or_else(|| bad(format!("{at}: true or false expected")))?,
        ),
        "int" => PropValue::Int(int(value, at)?),
        "float" => match float(value, at)? {
            Some(v) => PropValue::Float(v),
            None => return Ok(None),
        },
        "string" => PropValue::String(
            value
                .as_str()
                .ok_or_else(|| bad(format!("{at}: string expected")))?
                .to_owned(),
        ),
        "resource" => PropValue::Resource(index(value)?),
        "object" => PropValue::Object(index(value)?),
        "matrix" => match matrix(value, at)? {
            Some(m) => PropValue::Matrix(m),
            None => return Ok(None),
        },
        "color" => PropValue::Color(Hash32(hex_u32(value, at)?)),
        "vec3" => match floats::<3>(value, at)? {
            Some(v) => PropValue::Vec3(v),
            None => return Ok(None),
        },
        "array" => PropValue::Array(
            value
                .as_array()
                .ok_or_else(|| bad(format!("{at}: list of u32 values expected")))?
                .iter()
                .map(|v| hex_u32(v, at).map(Hash32))
                .collect::<Result<_>>()?,
        ),
        // Exports made before type 0x100 was read as a u32 array wrote it as
        // four floats; such a value cannot be compared and is left alone.
        "vec4" => return Ok(None),
        "null" => PropValue::Null,
        "unknown" => PropValue::Unknown(Hash32(hex_u32(value, at)?)),
        other => return Err(bad(format!("{at}: unknown value type `{other}`"))),
    }))
}

fn same_prop(a: &PropValue, b: &PropValue) -> bool {
    match (a, b) {
        (PropValue::Float(x), PropValue::Float(y)) => x.to_bits() == y.to_bits(),
        _ => a == b,
    }
}

fn metamodel(data: &[u8], doc: &Json) -> Result<Imported> {
    let model = MetaModel::parse(data)?;
    let mut wanted: Vec<Option<(PropValue, String)>> = vec![None; model.properties.len()];
    let mut conditions: Vec<Option<u32>> = vec![None; model.conditions.len()];
    let mut want = |index: usize, json: &Json, at: &str| -> Result<()> {
        let property = &model.properties[index];
        let name = field(json, "name", at)?.as_str().unwrap_or_default();
        if name != property.name {
            return Err(bad(format!(
                "{at}: property `{name}` instead of `{}`",
                property.name
            )));
        }
        let Some(value) = prop_value(json, at)? else {
            return Ok(());
        };
        match &wanted[index] {
            Some((other, place)) if !same_prop(other, &value) => Err(bad(format!(
                "{at}: the property is shared with {place} and gets two different values"
            ))),
            _ => {
                wanted[index] = Some((value, at.to_owned()));
                Ok(())
            }
        }
    };
    let lists = [
        (
            "states",
            model
                .states
                .iter()
                .map(|s| &s.properties)
                .collect::<Vec<_>>(),
        ),
        ("parts", model.parts.iter().map(|p| &p.properties).collect()),
        (
            "sub_objects",
            model.sub_objects.iter().map(|p| &p.properties).collect(),
        ),
    ];
    for (name, owners) in lists {
        let items = match doc.get(name) {
            Some(items) => items
                .as_array()
                .ok_or_else(|| bad(format!("`{name}` must be a list")))?,
            None if owners.is_empty() => continue,
            None => return Err(bad(format!("document: missing `{name}`"))),
        };
        if items.len() != owners.len() {
            return Err(bad(format!(
                "`{name}` has {} entries instead of {}",
                items.len(),
                owners.len()
            )));
        }
        for (k, (item, props)) in items.iter().zip(owners).enumerate() {
            let at = format!("{name}[{k}]");
            let list = array(item, "properties", &at)?;
            if list.len() != props.len() {
                return Err(bad(format!("{at}: number of properties differs")));
            }
            for (j, (pj, &index)) in list.iter().zip(props).enumerate() {
                want(index, pj, &format!("{at}.properties[{j}]"))?;
            }
        }
    }
    let states = array(doc, "states", "document")?;
    for (k, (sj, state)) in states.iter().zip(&model.states).enumerate() {
        let at = format!("states[{k}]");
        let list = array(sj, "conditions", &at)?;
        if list.len() != state.conditions.len() {
            return Err(bad(format!("{at}: number of conditions differs")));
        }
        for (j, (cj, &index)) in list.iter().zip(&state.conditions).enumerate() {
            let at = format!("{at}.conditions[{j}]");
            let condition = &model.conditions[index];
            let kind = model.variables.get(condition.variable).map(|v| v.kind);
            let value = field(cj, "value", &at)?;
            let raw = match kind {
                Some(1) => value
                    .as_bool()
                    .map(u32::from)
                    .ok_or_else(|| bad(format!("{at}: true or false expected")))?,
                Some(4) => match float(value, &at)? {
                    Some(f) => f.to_bits(),
                    None => continue,
                },
                _ => int(value, &at)? as u32,
            };
            conditions[index] = Some(raw);
        }
    }

    let mut data = data.to_vec();
    let mut changes = Vec::new();
    for (index, wanted) in wanted.into_iter().enumerate() {
        let property = &model.properties[index];
        if let Some((value, at)) = wanted
            && !same_prop(&property.value, &value)
        {
            data = metamodel_edit::set_property(&data, index, &value)
                .map_err(|e| bad(format!("{at}: {e}")))?;
            changes.push(format!(
                "{at} {}.{}: {:?} -> {value:?}",
                property.class, property.name, property.value
            ));
        }
    }
    for (index, raw) in conditions.into_iter().enumerate() {
        let condition = &model.conditions[index];
        if let Some(raw) = raw
            && raw != condition.value
        {
            data = metamodel_edit::set_condition(&data, index, raw)?;
            let variable = model
                .variables
                .get(condition.variable)
                .map_or("?", |v| v.name.as_str());
            changes.push(format!(
                "condition {index} on {variable}: {:#x} -> {raw:#x}",
                condition.value
            ));
        }
    }
    Ok(Imported { data, changes })
}

fn variables(data: &[u8], doc: &Json) -> Result<Imported> {
    let mut dict = VariableDictionary::parse(data)?;
    let items = array(doc, "variables", "document")?;
    if items.len() != dict.variables.len() {
        return Err(bad(format!(
            "the document has {} variable(s), the dictionary {}; adding and removing is not supported",
            items.len(),
            dict.variables.len()
        )));
    }
    let mut changes = Vec::new();
    for (k, (item, var)) in items.iter().zip(&mut dict.variables).enumerate() {
        let at = format!("variables[{k}]");
        let name = field(item, "name", &at)?.as_str().unwrap_or_default();
        if name != var.name || hex_u32(field(item, "hash", &at)?, &at)? != var.hash.0 {
            return Err(bad(format!(
                "{at}: `{name}` instead of `{}`; renaming is not supported",
                var.name
            )));
        }
        for (key, slot) in [("kind", &mut var.kind), ("value", &mut var.value)] {
            let new = int(field(item, key, &at)?, &at)? as u32;
            if new != *slot {
                changes.push(format!("{} {key}: {} -> {new}", var.name, *slot));
                *slot = new;
            }
        }
    }
    let data = if changes.is_empty() {
        data.to_vec()
    } else {
        dict.encode()
    };
    Ok(Imported { data, changes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::logic::to_json;

    #[test]
    fn unchanged_documents_change_nothing() {
        let sim = crate::resource::simgroup::tests::sample();
        let mm = crate::resource::metamodel::tests::sample();
        for (type_id, data) in [(TYPE_SIMGROUP, sim), (TYPE_METAMODEL, mm)] {
            let doc = to_json(type_id, &data).unwrap().unwrap();
            let imported = apply_json(type_id, &data, &doc).unwrap();
            assert!(imported.changes.is_empty(), "{:?}", imported.changes);
            assert_eq!(imported.data, data);
        }
    }

    #[test]
    fn parts_databases_are_written_whole() {
        let db = crate::resource::partsdb::tests::sample_db();
        let data = db.encode().unwrap();
        let mut doc = to_json(TYPE_PARTSDB, &data).unwrap().unwrap();
        doc["stream"] = "gf2/x".into();
        let same = apply_json(TYPE_PARTSDB, &data, &doc).unwrap();
        assert!(same.changes.is_empty());
        assert_eq!(same.data, data);
        doc["variants"][1]["name"] = "fedora".into();
        doc["sets"][0]["choices"][1]["variants"] = serde_json::json!([1]);
        let imported = apply_json(TYPE_PARTSDB, &data, &doc).unwrap();
        assert_eq!(imported.changes, vec!["variants: changed", "sets: changed"]);
        let back = PartsDb::parse(&imported.data).unwrap();
        assert_eq!(back.variants[1].name, "fedora");
        doc["name"] = "other".into();
        assert!(apply_json(TYPE_PARTSDB, &data, &doc).is_err());
    }

    #[test]
    fn simgroup_values_position_and_model() {
        let data = crate::resource::simgroup::tests::sample();
        let mut doc = to_json(TYPE_SIMGROUP, &data).unwrap().unwrap();
        // The sample's schema is not the game's: under GF2's schema packet A
        // (class 11111111) is unknown, so its string stays a string by region.
        let attrs = &mut doc["objects"][0]["packets"][0]["attrs"];
        attrs[0]["value"] = "renamed".into();
        attrs.as_array_mut().unwrap().remove(1);
        let imported = apply_json(TYPE_SIMGROUP, &data, &doc).unwrap();
        assert_eq!(imported.changes.len(), 2, "{:?}", imported.changes);
        let group = SimGroup::parse(&imported.data).unwrap();
        let packet = &group.packets[group.objects[0].packets[0]];
        assert_eq!(packet.attrs.len(), 1);
        assert_eq!(packet.attrs[0].value, Value::String("renamed".into()));
        // A document of another group is refused.
        doc["objects"][0]["guid"] = "00".repeat(16).into();
        assert!(apply_json(TYPE_SIMGROUP, &data, &doc).is_err());
    }

    #[test]
    fn metamodel_properties_and_conditions() {
        let data = crate::resource::metamodel::tests::sample();
        let mut doc = to_json(TYPE_METAMODEL, &data).unwrap().unwrap();
        doc["states"][0]["properties"][0]["value"][3][0] = 9.5.into();
        doc["states"][0]["conditions"][0]["value"] = 12.0.into();
        let imported = apply_json(TYPE_METAMODEL, &data, &doc).unwrap();
        assert_eq!(imported.changes.len(), 2, "{:?}", imported.changes);
        let model = MetaModel::parse(&imported.data).unwrap();
        let PropValue::Matrix(m) = model.properties[0].value else {
            panic!("matrix expected");
        };
        assert_eq!(m[3][0], 9.5);
        assert_eq!(model.conditions[0].value, 12f32.to_bits());
        doc["parts"][0]["properties"][0]["type"] = "int".into();
        assert!(apply_json(TYPE_METAMODEL, &data, &doc).is_err());
    }

    #[test]
    fn variable_values() {
        use crate::resource::variabledictionary::Variable;
        let dict = VariableDictionary {
            version: 4,
            variables: vec![Variable {
                name: "Bridge_Var".into(),
                hash: Hash32(7),
                kind: 1,
                value: 0,
                name_offset: 0,
            }],
        };
        let data = dict.encode();
        let mut doc = serde_json::to_value(&dict).unwrap();
        doc["variables"][0]["value"] = 1.into();
        let imported = apply_json(TYPE_VARIABLE_DICTIONARY, &data, &doc).unwrap();
        assert_eq!(imported.changes, ["Bridge_Var value: 0 -> 1"]);
        let parsed = VariableDictionary::parse(&imported.data).unwrap();
        assert_eq!(parsed.variables[0].value, 1);
        doc["variables"][0]["name"] = "Other".into();
        assert!(apply_json(TYPE_VARIABLE_DICTIONARY, &data, &doc).is_err());
    }
}

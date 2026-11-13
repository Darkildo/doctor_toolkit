//! Editing metamodels in place: property values and condition thresholds.
//!
//! A metamodel is a memory image with absolute pointers, so there is no
//! general writer; edits patch the resource instead. Scalars (bool, int,
//! float, colour) and condition values are overwritten in place. A matrix,
//! vector or u32 array is overwritten in place when no other property points
//! to it (and an array is not longer than before), otherwise it is appended;
//! a new string is always appended. Appended data
//! goes to the end of the resource (16-byte aligned) and the size at 0x08 is
//! updated; nothing else moves, so every other pointer stays valid.

use crate::io::{self, Endian};
use crate::resource::metamodel::{MetaModel, PropValue};
use crate::{Error, Result};

const STRIDE_RESOURCE: usize = 0x20;
const STRIDE_PROPERTY: usize = 0x18;
const STRIDE_CONDITION: usize = 0x0C;
const STRIDE_OBJECT: usize = 0x24;

fn table(data: &[u8], t: usize) -> Result<usize> {
    Ok(io::read_u32(data, 0x34 + 4 * t, Endian::Little)? as usize)
}

fn put(data: &mut [u8], at: usize, value: u32) {
    data[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

/// Appends `bytes` at a 16-byte boundary and updates the size; returns the offset.
fn append(data: &mut Vec<u8>, bytes: &[u8]) -> Result<u32> {
    data.resize(data.len().next_multiple_of(16), 0);
    let at = u32::try_from(data.len())
        .map_err(|_| Error::InvalidArgument("metamodel larger than 4 GiB".into()))?;
    data.extend_from_slice(bytes);
    let size = data.len() as u32;
    put(data, 8, size);
    Ok(at)
}

fn floats_le(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// Sets the value of property `index` (an entry of [`MetaModel::properties`])
/// and returns the new resource. The value must have the property's type;
/// [`PropValue::Null`] clears a pointer.
pub fn set_property(data: &[u8], index: usize, value: &PropValue) -> Result<Vec<u8>> {
    let model = MetaModel::parse(data)?;
    let property = model
        .properties
        .get(index)
        .ok_or_else(|| Error::InvalidArgument(format!("no property {index}")))?;
    if property.value == *value {
        return Ok(data.to_vec());
    }
    let properties = table(data, 3)?;
    let raw_of = |k: usize| io::read_u32(data, properties + STRIDE_PROPERTY * k, Endian::Little);
    let entry = properties + STRIDE_PROPERTY * index;
    let current = raw_of(index)?;
    // Whether another property's data overlaps this one: equal blocks are
    // stored once, and vectors may lie inside matrices.
    let blob_size = |k: usize| match &model.properties[k].value {
        PropValue::Matrix(_) => 64,
        PropValue::Vec3(_) => 12,
        PropValue::Array(values) => 4 + 4 * values.len(),
        _ => 0,
    };
    let size = blob_size(index);
    let range = current as usize..current as usize + size;
    let shared = (0..model.properties.len()).any(|k| {
        let other = blob_size(k);
        let at = raw_of(k).unwrap_or(0) as usize;
        k != index && other > 0 && at != 0 && at < range.end && range.start < at + other
    });
    let mut out = data.to_vec();
    let mismatch = || {
        Error::InvalidArgument(format!(
            "property {} ({}) has type {:#x}, not {}",
            index,
            property.name,
            property.type_flag,
            value_name(value)
        ))
    };
    let blob = |out: &mut Vec<u8>, bytes: &[u8]| -> Result<u32> {
        if current != 0 && !shared && bytes.len() <= size {
            let at = current as usize;
            out[at..at + bytes.len()].copy_from_slice(bytes);
            Ok(current)
        } else {
            append(out, bytes)
        }
    };
    let raw = match (property.type_flag, value) {
        (0x01, PropValue::Bool(b)) => *b as u32,
        (0x02, PropValue::Int(v)) => *v as u32,
        (0x04, PropValue::Float(v)) => v.to_bits(),
        (0x40, PropValue::Color(c)) => c.0,
        (0x08 | 0x10 | 0x20 | 0x80 | 0x100 | 0x200, PropValue::Null) => 0,
        (0x08, PropValue::String(s)) => {
            if s.contains('\0') {
                return Err(Error::InvalidArgument("string contains a NUL byte".into()));
            }
            let mut bytes = s.as_bytes().to_vec();
            bytes.push(0);
            append(&mut out, &bytes)?
        }
        (0x10, PropValue::Resource(i)) => {
            if *i >= model.resources.len() {
                return Err(Error::InvalidArgument(format!("no resource entry {i}")));
            }
            (table(data, 0)? + STRIDE_RESOURCE * i) as u32
        }
        (0x200, PropValue::Object(i)) => {
            if *i >= model.sub_objects.len() {
                return Err(Error::InvalidArgument(format!("no sub-object {i}")));
            }
            (table(data, 6)? + STRIDE_OBJECT * i) as u32
        }
        (0x20, PropValue::Matrix(m)) => blob(&mut out, &floats_le(m.as_flattened()))?,
        (0x80, PropValue::Vec3(v)) => blob(&mut out, &floats_le(v))?,
        (0x100, PropValue::Array(values)) => {
            let count = u32::try_from(values.len())
                .map_err(|_| Error::InvalidArgument("array too long".into()))?;
            let bytes: Vec<u8> = std::iter::once(count)
                .chain(values.iter().map(|v| v.0))
                .flat_map(u32::to_le_bytes)
                .collect();
            blob(&mut out, &bytes)?
        }
        _ => return Err(mismatch()),
    };
    put(&mut out, entry, raw);
    MetaModel::parse(&out)?;
    Ok(out)
}

/// Sets the stored value of condition `index` (an entry of
/// [`MetaModel::conditions`]): the bits of a bool, int or float according to
/// the kind of its variable.
pub fn set_condition(data: &[u8], index: usize, raw: u32) -> Result<Vec<u8>> {
    let model = MetaModel::parse(data)?;
    if index >= model.conditions.len() {
        return Err(Error::InvalidArgument(format!("no condition {index}")));
    }
    let mut out = data.to_vec();
    put(&mut out, table(data, 4)? + STRIDE_CONDITION * index, raw);
    Ok(out)
}

fn value_name(value: &PropValue) -> &'static str {
    match value {
        PropValue::Bool(_) => "bool",
        PropValue::Int(_) => "int",
        PropValue::Float(_) => "float",
        PropValue::String(_) => "string",
        PropValue::Resource(_) => "resource",
        PropValue::Matrix(_) => "matrix",
        PropValue::Color(_) => "color",
        PropValue::Vec3(_) => "vec3",
        PropValue::Array(_) => "array",
        PropValue::Object(_) => "object",
        PropValue::Null => "null",
        PropValue::Unknown(_) => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::metamodel::tests::sample;

    #[test]
    fn edits_scalars_blobs_strings_and_references() {
        let data = sample();
        let model = MetaModel::parse(&data).unwrap();
        assert!(matches!(model.properties[0].value, PropValue::Matrix(_)));
        let mut m = [[0.0f32; 4]; 4];
        m[3] = [1.0, 2.0, 3.0, 1.0];
        // The matrix is not shared: written in place.
        let edited = set_property(&data, 0, &PropValue::Matrix(m)).unwrap();
        assert_eq!(edited.len(), data.len());
        let parsed = MetaModel::parse(&edited).unwrap();
        assert_eq!(parsed.properties[0].value, PropValue::Matrix(m));
        assert_eq!(parsed.properties[1], model.properties[1]);
        // Wrong types are refused; equal values change nothing.
        assert!(set_property(&data, 0, &PropValue::Int(1)).is_err());
        assert!(set_property(&data, 1, &PropValue::Resource(1)).is_err());
        assert_eq!(
            set_property(&data, 1, &PropValue::Resource(0)).unwrap(),
            data
        );
        let cleared = set_property(&data, 1, &PropValue::Null).unwrap();
        assert_eq!(
            MetaModel::parse(&cleared).unwrap().properties[1].value,
            PropValue::Null
        );
        // A condition threshold.
        let edited = set_condition(&data, 0, 25f32.to_bits()).unwrap();
        let parsed = MetaModel::parse(&edited).unwrap();
        assert_eq!(parsed.conditions[0].value, 25f32.to_bits());
    }

    #[test]
    fn appends_shared_blobs_and_strings() {
        let mut data = sample();
        // Make property 1 a second matrix pointing to the same blob as property 0.
        data[0xDC..0xE0].copy_from_slice(&0x1C0u32.to_le_bytes());
        data[0xF0..0xF4].copy_from_slice(&0x20u32.to_le_bytes());
        let model = MetaModel::parse(&data).unwrap();
        assert_eq!(model.properties[0].value, model.properties[1].value);
        let m = [[2.0f32; 4]; 4];
        let edited = set_property(&data, 0, &PropValue::Matrix(m)).unwrap();
        assert_eq!(edited.len(), data.len().next_multiple_of(16) + 64);
        assert_eq!(
            io::read_u32(&edited, 8, Endian::Little).unwrap() as usize,
            edited.len()
        );
        let parsed = MetaModel::parse(&edited).unwrap();
        assert_eq!(parsed.properties[0].value, PropValue::Matrix(m));
        assert_eq!(parsed.properties[1].value, model.properties[1].value);
    }

    /// Makes property 1 of the sample a u32 array (type 0x100) at 0x1A0,
    /// as `ComponentList` in the game.
    fn with_array() -> Vec<u8> {
        let mut data = sample();
        let put =
            |d: &mut Vec<u8>, at: usize, v: u32| d[at..at + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut data, 0xDC, 0x1A0);
        put(&mut data, 0xF0, 0x100);
        for (k, v) in [3, 0x9F3F_A445, 1, 0xDC51_EBB1].into_iter().enumerate() {
            put(&mut data, 0x1A0 + 4 * k, v);
        }
        data
    }

    #[test]
    fn arrays_are_u32_lists() {
        use crate::resource::simgroup::Hash32;
        let data = with_array();
        let model = MetaModel::parse(&data).unwrap();
        let values = vec![Hash32(0x9F3F_A445), Hash32(1), Hash32(0xDC51_EBB1)];
        assert_eq!(model.properties[1].value, PropValue::Array(values.clone()));
        let json = serde_json::to_value(&model.properties[1]).unwrap();
        assert_eq!(json["type"], "array");
        assert_eq!(json["value"][0], "9f3fa445");
        // The same value changes nothing; a shorter array is written in place.
        assert_eq!(
            set_property(&data, 1, &PropValue::Array(values.clone())).unwrap(),
            data
        );
        let edited = set_property(&data, 1, &PropValue::Array(vec![Hash32(7)])).unwrap();
        assert_eq!(edited.len(), data.len());
        assert_eq!(
            MetaModel::parse(&edited).unwrap().properties[1].value,
            PropValue::Array(vec![Hash32(7)])
        );
        // A longer one is appended; the rest of the resource stays.
        let longer: Vec<Hash32> = (0..5).map(Hash32).collect();
        let edited = set_property(&data, 1, &PropValue::Array(longer.clone())).unwrap();
        assert_eq!(edited.len(), data.len().next_multiple_of(16) + 24);
        assert_eq!(
            &edited[..0x1A0],
            &{
                let mut d = data[..0x1A0].to_vec();
                d[0xDC..0xE0]
                    .copy_from_slice(&(data.len().next_multiple_of(16) as u32).to_le_bytes());
                d[8..12].copy_from_slice(&(edited.len() as u32).to_le_bytes());
                d
            }[..]
        );
        let parsed = MetaModel::parse(&edited).unwrap();
        assert_eq!(parsed.properties[1].value, PropValue::Array(longer));
        assert_eq!(parsed.properties[0], model.properties[0]);
        // Four floats are not an array.
        assert!(set_property(&data, 1, &PropValue::Vec3([0.0; 3])).is_err());
        // A count past the end is an error, not a panic.
        let mut bad = data.clone();
        bad[0x1A0..0x1A4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(MetaModel::parse(&bad).is_err());
    }
}

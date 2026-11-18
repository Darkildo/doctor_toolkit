//! Deltas of parts databases: parts and variants added and variants allowed
//! in sets, everything addressed by name.
//!
//! ```json
//! {"format":1,"kind":"partsdb","ops":[
//!   {"op":"add_part","name":"doctor_hat","slot":"a_hat","models":["11111111","22222222","00000000","00000000","00000000"]},
//!   {"op":"add_variant","name":"doctor_hat","parts":["doctor_hat"]},
//!   {"op":"allow","set":"fam_granados","slot":"a_hat","variant":"doctor_hat"}
//! ]}
//! ```
//!
//! Indices are not stable between versions of a database (a mod below may
//! add its own parts), names are. Any other change — a slot, a preset, a
//! blender, a set or a group added, a part edited — keeps the database
//! whole.

use serde::{Deserialize, Serialize};

use super::{Applied, Delta, FORMAT, Kind, Report, check_head, checked};
use crate::resource::partsdb::{MODEL_SLOTS, Part, PartsDb, Set};
use crate::resource::simgroup::Hash32;
use crate::{Error, Result};

/// A delta of a parts database.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartsDelta {
    pub format: u32,
    pub kind: Kind,
    pub ops: Vec<Op>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// One operation on a parts database.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Op {
    /// Adds a part (one model in every level of detail).
    AddPart {
        name: String,
        /// Name of the slot the part occupies.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        slot: Option<String>,
        models: [Hash32; MODEL_SLOTS],
        #[serde(default, skip_serializing_if = "String::is_empty")]
        label: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tags: Vec<String>,
    },
    /// Adds a variant: a named combination of parts, the parts by name.
    AddVariant { name: String, parts: Vec<String> },
    /// Lets a set pick a variant in a slot; with `only` the variant becomes
    /// the only choice of the slot.
    Allow {
        set: String,
        slot: String,
        variant: String,
        #[serde(default, skip_serializing_if = "is_false")]
        only: bool,
    },
}

impl Op {
    /// What the operation addresses, as the notes of [`apply`] name it.
    pub fn target(&self) -> String {
        match self {
            Self::AddPart { name, .. } => format!("part `{name}`"),
            Self::AddVariant { name, .. } => format!("variant `{name}`"),
            Self::Allow {
                set, slot, variant, ..
            } => format!("variant `{variant}` in slot `{slot}` of set `{set}`"),
        }
    }
}

/// The delta that turns `base` into `mine`, or `None` when the change is
/// more than parts and variants added at the end and variants allowed, or
/// the delta does not give `mine` back byte for byte.
pub fn diff(base: &[u8], mine: &[u8]) -> Result<Option<PartsDelta>> {
    let old = PartsDb::parse(base)?;
    let new = PartsDb::parse(mine)?;
    let Some(ops) = ops_between(&old, &new) else {
        return Ok(None);
    };
    let delta = PartsDelta {
        format: FORMAT,
        kind: Kind::PartsDb,
        ops,
    };
    Ok(match checked(Delta::PartsDb(delta), base, mine) {
        Some(Delta::PartsDb(delta)) => Some(delta),
        _ => None,
    })
}

fn ops_between(old: &PartsDb, new: &PartsDb) -> Option<Vec<Op>> {
    let same_rest = |a: &Set, b: &Set| {
        (&a.name, &a.materials, &a.presets) == (&b.name, &b.materials, &b.presets)
    };
    if (
        &old.name,
        old.kind,
        &old.slots,
        &old.presets,
        &old.blenders,
        &old.groups,
    ) != (
        &new.name,
        new.kind,
        &new.slots,
        &new.presets,
        &new.blenders,
        &new.groups,
    ) || !new.parts.starts_with(&old.parts)
        || !new.variants.starts_with(&old.variants)
        || old.sets.len() != new.sets.len()
        || !old.sets.iter().zip(&new.sets).all(|(a, b)| same_rest(a, b))
    {
        return None;
    }
    let mut ops = Vec::new();
    for part in &new.parts[old.parts.len()..] {
        ops.push(Op::AddPart {
            name: part.name.clone(),
            slot: match part.slot {
                Some(slot) => Some(new.slots.get(slot)?.clone()),
                None => None,
            },
            models: part.models,
            label: part.label.clone(),
            tags: part.tags.clone(),
        });
    }
    for variant in &new.variants[old.variants.len()..] {
        ops.push(Op::AddVariant {
            name: variant.name.clone(),
            parts: variant
                .parts
                .iter()
                .map(|&p| new.parts.get(p as usize).map(|part| part.name.clone()))
                .collect::<Option<_>>()?,
        });
    }
    for (a, b) in old.sets.iter().zip(&new.sets) {
        if b.choices.len() < a.choices.len() {
            return None;
        }
        for (k, choice) in b.choices.iter().enumerate() {
            let before: &[usize] = match a.choices.get(k) {
                Some(was) if was.slot != choice.slot => return None,
                Some(was) => &was.variants,
                None => &[],
            };
            let allow = |variant: usize, only: bool| {
                Some(Op::Allow {
                    set: b.name.clone(),
                    slot: new.slots.get(choice.slot)?.clone(),
                    variant: new.variants.get(variant)?.name.clone(),
                    only,
                })
            };
            if choice.variants.starts_with(before) {
                // A new choice with no variants cannot be made by allowing one.
                if choice.variants.is_empty() && a.choices.get(k).is_none() {
                    return None;
                }
                for &variant in &choice.variants[before.len()..] {
                    ops.push(allow(variant, false)?);
                }
            } else if let [variant] = choice.variants[..] {
                ops.push(allow(variant, true)?);
            } else {
                return None;
            }
        }
    }
    Some(ops)
}

/// Applies `delta` to the database `base`. An operation that names a slot,
/// a part, a variant or a set the database does not have is skipped and
/// listed; a new part or variant whose name is taken is an error.
pub fn apply(delta: &PartsDelta, base: &[u8]) -> Result<Applied> {
    check_head(delta.format, delta.kind, Kind::PartsDb)?;
    for (i, op) in delta.ops.iter().enumerate() {
        // The database keeps its strings NUL-terminated.
        let names: Vec<&String> = match op {
            Op::AddPart {
                name, label, tags, ..
            } => [name, label].into_iter().chain(tags).collect(),
            Op::AddVariant { name, .. } => vec![name],
            Op::Allow { .. } => Vec::new(),
        };
        if names.iter().any(|name| name.contains('\0')) {
            return Err(Error::InvalidArgument(format!(
                "operation {}: a name contains a NUL byte",
                i + 1
            )));
        }
    }
    let mut db = PartsDb::parse(base)?;
    let mut report = Report::default();
    for (i, op) in delta.ops.iter().enumerate() {
        let taken = |what: &str, name: &str| {
            Error::InvalidArgument(format!(
                "operation {}: the database already has a {what} named `{name}`",
                i + 1
            ))
        };
        match op {
            Op::AddPart {
                name,
                slot,
                models,
                label,
                tags,
            } => {
                let target = op.target();
                if db.part_index(name).is_some() {
                    return Err(taken("part", name));
                }
                let slot = match slot {
                    None => None,
                    Some(slot) => match db.slot_index(slot) {
                        Some(index) => Some(index),
                        None => {
                            report.skip(i, target, format!("the database has no slot `{slot}`"));
                            continue;
                        }
                    },
                };
                db.add_part(Part {
                    name: name.clone(),
                    slot,
                    models: *models,
                    label: label.clone(),
                    tags: tags.clone(),
                })?;
                report.applied();
            }
            Op::AddVariant { name, parts } => {
                let target = op.target();
                if db.variant_index(name).is_some() {
                    return Err(taken("variant", name));
                }
                let found: std::result::Result<Vec<u16>, &String> = parts
                    .iter()
                    .map(|part| db.part_index(part).map(|p| p as u16).ok_or(part))
                    .collect();
                match found {
                    Ok(parts) => {
                        db.add_variant(name, parts)?;
                        report.applied();
                    }
                    Err(part) => {
                        report.skip(i, target, format!("the database has no part `{part}`"));
                    }
                }
            }
            Op::Allow {
                set,
                slot,
                variant,
                only,
            } => {
                let target = op.target();
                let missing = match (
                    db.set_index(set),
                    db.slot_index(slot),
                    db.variant_index(variant),
                ) {
                    (Some(set), Some(slot), Some(variant)) => {
                        db.allow_variant(set, slot, variant, *only)?;
                        report.applied();
                        continue;
                    }
                    (None, _, _) => format!("set `{set}`"),
                    (_, None, _) => format!("slot `{slot}`"),
                    (_, _, None) => format!("variant `{variant}`"),
                };
                report.skip(i, target, format!("the database has no {missing}"));
            }
        }
    }
    let data = if report.any_applied() {
        db.encode()?
    } else {
        base.to_vec()
    };
    Ok(report.finish(data))
}

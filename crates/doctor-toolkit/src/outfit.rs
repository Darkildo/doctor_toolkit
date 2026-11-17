//! New clothes in a parts database (`partsdb`): a part with its meshes, a
//! variant made of it, and the sets that may wear the variant.
//!
//! One description of the operation for every surface: the New outfit
//! wizard of the editor, `partsdb add-part` and `partsdb allow` of the
//! command line, and the tools `partsdb_add_part` and `partsdb_allow` of
//! the MCP server. [`add_part`] and [`allow`] change a [`PartsDb`] in
//! memory; [`record_part`] and [`record_allow`] read the database as the
//! workspace has it, change it and record one undoable operation.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::game::{GameInstance, StreamFile};
use crate::resource::partsdb::{MODEL_SLOTS, Part, PartsDb};
use crate::resource::simgroup::Hash32;
use crate::workspace::Workspace;
use crate::{Error, Result};

const TYPE_MESH: u32 = crate::hash::sdbm(b"mesh");

/// A new part with everything resolved to indices of the database.
#[derive(Debug, Clone, PartialEq)]
pub struct PartSpec {
    pub name: String,
    pub slot: usize,
    /// The mesh of each level of detail (0 = none).
    pub models: [Hash32; MODEL_SLOTS],
    pub tags: Vec<String>,
    /// Name of the variant; empty takes the part name.
    pub variant: String,
    /// Sets that may wear the variant.
    pub sets: Vec<usize>,
    /// The variant becomes the only choice of the slot in those sets.
    pub only: bool,
}

/// What [`add_part`] added.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Added {
    pub part: usize,
    pub variant: usize,
    pub variant_name: String,
    /// Sets whose choices changed.
    pub sets_changed: usize,
}

/// Adds the part of `spec`, a variant made of it, and lets the sets wear it.
pub fn add_part(db: &mut PartsDb, spec: &PartSpec) -> Result<Added> {
    if db.part_index(&spec.name).is_some() {
        return Err(Error::InvalidArgument(format!(
            "`{}` already has a part `{}`",
            db.name, spec.name
        )));
    }
    let part = db.add_part(Part {
        name: spec.name.clone(),
        slot: Some(spec.slot),
        models: spec.models,
        label: String::new(),
        tags: spec.tags.clone(),
    })?;
    let variant_name = match spec.variant.trim() {
        "" => spec.name.clone(),
        name => name.to_owned(),
    };
    let variant = db.add_variant(&variant_name, vec![part as u16])?;
    let mut sets_changed = 0;
    for &set in &spec.sets {
        sets_changed += db.allow_variant(set, spec.slot, variant, spec.only)? as usize;
    }
    Ok(Added {
        part,
        variant,
        variant_name,
        sets_changed,
    })
}

/// Lets `sets` wear `variant` in `slot` (default: the slot of the variant's
/// first part). Returns the number of sets that changed.
pub fn allow(
    db: &mut PartsDb,
    variant: usize,
    slot: Option<usize>,
    sets: &[usize],
    only: bool,
) -> Result<usize> {
    let slot = match slot {
        Some(slot) => slot,
        None => db
            .variants
            .get(variant)
            .ok_or_else(|| Error::InvalidArgument(format!("no variant {variant}")))?
            .parts
            .iter()
            .find_map(|&p| db.parts[p as usize].slot)
            .ok_or_else(|| {
                Error::InvalidArgument("the variant's parts have no slot; give the slot".into())
            })?,
    };
    let mut changed = 0;
    for &set in sets {
        changed += db.allow_variant(set, slot, variant, only)? as usize;
    }
    Ok(changed)
}

/// Sets by name or index; every set when `all` or when none is named.
pub fn set_indices(db: &PartsDb, names: &[String], all: bool) -> Result<Vec<usize>> {
    if all || names.is_empty() {
        return Ok((0..db.sets.len()).collect());
    }
    names
        .iter()
        .map(|n| {
            db.set_index(n)
                .or_else(|| n.parse().ok().filter(|&i: &usize| i < db.sets.len()))
                .ok_or_else(|| Error::InvalidArgument(format!("no set `{n}` in `{}`", db.name)))
        })
        .collect()
}

fn slot_index(db: &PartsDb, slot: &str) -> Result<usize> {
    db.slot_index(slot)
        .or_else(|| slot.parse().ok().filter(|&i: &usize| i < db.slots.len()))
        .ok_or_else(|| {
            Error::InvalidArgument(format!("no slot `{slot}`; slots: {}", db.slots.join(", ")))
        })
}

/// Mesh names of a stream by resource id.
pub fn mesh_names(file: &StreamFile) -> HashMap<u32, String> {
    let mut names = HashMap::new();
    for (index, r) in file.layout().resources.iter().enumerate() {
        if r.header.type_id != TYPE_MESH {
            continue;
        }
        if let Ok(data) = file.resource_data(index)
            && let Ok(at) = crate::io::read_u32(&data, 0x20, crate::io::Endian::Little)
            && let Ok(name) = crate::io::cstr(&data, at as usize)
        {
            names.insert(r.header.ids[3], String::from_utf8_lossy(name).into_owned());
        }
    }
    names
}

/// A new part as a surface describes it: names and hexadecimal ids.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct NewPart {
    /// Name of the new part, e.g. `mod_fedora`.
    pub name: String,
    /// Slot of the part by name (`upper_body`, `a_hat`) or number.
    pub slot: String,
    /// The mesh of each level of detail (at most three: the model, `_lod1`,
    /// `_lod2`): a resource id (hexadecimal, the last id of the `mesh`
    /// resource) or the name of a mesh in the stream of the database. A
    /// mesh made by `import_model` with `name` is given by its id.
    pub meshes: Vec<String>,
    /// Culling tags of the part, e.g. `hatCulled`.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Name of the new variant [default: the part name].
    #[serde(default)]
    pub variant: Option<String>,
    /// Sets that may wear the variant, by name or number; none = every set.
    #[serde(default)]
    pub sets: Vec<String>,
    /// Make the variant the only choice of its slot in those sets.
    #[serde(default)]
    pub only: bool,
}

impl NewPart {
    /// Resolves names against `db`; `meshes` are the mesh names of the
    /// stream of the database ([`mesh_names`]). The second value lists the
    /// mesh ids that are not among them (new in the workspace, or in
    /// another stream).
    pub fn resolve(
        &self,
        db: &PartsDb,
        meshes: &HashMap<u32, String>,
    ) -> Result<(PartSpec, Vec<u32>)> {
        if self.meshes.is_empty() {
            return Err(Error::InvalidArgument("a part needs a mesh".into()));
        }
        if self.meshes.len() > MODEL_SLOTS {
            return Err(Error::InvalidArgument(format!(
                "at most {MODEL_SLOTS} meshes per part"
            )));
        }
        if self.name.trim().is_empty() {
            return Err(Error::InvalidArgument("a part needs a name".into()));
        }
        let slot = slot_index(db, &self.slot)?;
        let mut models = [Hash32(0); MODEL_SLOTS];
        let mut unknown = Vec::new();
        for (model, text) in models.iter_mut().zip(&self.meshes) {
            let by_name = meshes
                .iter()
                .find(|(_, n)| n.eq_ignore_ascii_case(text))
                .map(|(&id, _)| id);
            let id = match by_name {
                Some(id) => id,
                None => {
                    u32::from_str_radix(text.trim().trim_start_matches("0x"), 16).map_err(|_| {
                        Error::InvalidArgument(format!(
                            "`{text}` is neither a mesh name in the stream nor a hexadecimal id"
                        ))
                    })?
                }
            };
            if !meshes.contains_key(&id) {
                unknown.push(id);
            }
            *model = Hash32(id);
        }
        Ok((
            PartSpec {
                name: self.name.clone(),
                slot,
                models,
                tags: self.tags.clone(),
                variant: self.variant.clone().unwrap_or_default(),
                sets: set_indices(db, &self.sets, false)?,
                only: self.only,
            },
            unknown,
        ))
    }
}

/// Letting sets wear an existing variant, as a surface describes it.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AllowVariant {
    /// Name of the variant.
    pub variant: String,
    /// Slot by name or number [default: the slot of the variant's first part].
    #[serde(default)]
    pub slot: Option<String>,
    /// Sets by name or number; none = every set.
    #[serde(default)]
    pub sets: Vec<String>,
    /// Make the variant the only choice of the slot in those sets.
    #[serde(default)]
    pub only: bool,
}

/// What [`record_part`] and [`record_allow`] did.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Recorded {
    /// Name of the database.
    pub database: String,
    pub slot: String,
    pub part: Option<usize>,
    pub variant: usize,
    pub variant_name: String,
    pub sets_changed: usize,
    /// Mesh ids of the part that are not in the game files of the stream.
    pub meshes_not_in_stream: Vec<String>,
    /// Label of the operation; `None` when nothing changed.
    pub operation: Option<String>,
}

fn current_db(game: &GameInstance, ws: &Workspace, at: (usize, usize)) -> Result<PartsDb> {
    PartsDb::parse(&ws.current_data(game, at.0, at.1)?)
}

/// Adds a part to the parts database at `at` (stream and resource) as the
/// workspace has it: one undoable operation.
pub fn record_part(
    game: &GameInstance,
    ws: &mut Workspace,
    at: (usize, usize),
    new: &NewPart,
) -> Result<Recorded> {
    let mut db = current_db(game, ws, at)?;
    let names = mesh_names(&game.open_stream(at.0)?);
    let (spec, unknown) = new.resolve(&db, &names)?;
    let added = add_part(&mut db, &spec)?;
    let label = format!("Add part {} to parts database {}", spec.name, db.name);
    ws.set_resource(game, at.0, at.1, db.encode()?, &label)?;
    Ok(Recorded {
        slot: db.slots[spec.slot].clone(),
        database: db.name,
        part: Some(added.part),
        variant: added.variant,
        variant_name: added.variant_name,
        sets_changed: added.sets_changed,
        meshes_not_in_stream: unknown.iter().map(|id| format!("{id:08x}")).collect(),
        operation: Some(label),
    })
}

/// Lets sets of the parts database at `at` wear an existing variant: one
/// undoable operation, or none when every set already has it.
pub fn record_allow(
    game: &GameInstance,
    ws: &mut Workspace,
    at: (usize, usize),
    params: &AllowVariant,
) -> Result<Recorded> {
    let mut db = current_db(game, ws, at)?;
    let variant = db
        .variant_index(&params.variant)
        .ok_or_else(|| Error::InvalidArgument(format!("no variant `{}`", params.variant)))?;
    let slot = params
        .slot
        .as_deref()
        .map(|s| slot_index(&db, s))
        .transpose()?;
    let sets = set_indices(&db, &params.sets, false)?;
    let sets_changed = allow(&mut db, variant, slot, &sets, params.only)?;
    let slot = slot
        .or_else(|| {
            db.variants[variant]
                .parts
                .iter()
                .find_map(|&p| db.parts[p as usize].slot)
        })
        .map(|s| db.slots[s].clone())
        .unwrap_or_default();
    let operation = if sets_changed == 0 {
        None
    } else {
        let label = format!(
            "Allow variant {} in parts database {}",
            params.variant, db.name
        );
        ws.set_resource(game, at.0, at.1, db.encode()?, &label)?;
        Some(label)
    };
    Ok(Recorded {
        database: db.name,
        slot,
        part: None,
        variant,
        variant_name: params.variant.clone(),
        sets_changed,
        meshes_not_in_stream: Vec::new(),
        operation,
    })
}

/// The parts database called `name` (or at resource index `name`) of a
/// stream, as the workspace has it.
pub fn find_db(
    game: &GameInstance,
    ws: Option<&Workspace>,
    stream: usize,
    name: &str,
) -> Result<Option<(usize, PartsDb)>> {
    const TYPE_PARTSDB: u32 = crate::hash::sdbm(b"partsdb");
    let file = game.open_stream(stream)?;
    for (index, r) in file.layout().resources.iter().enumerate() {
        if r.header.type_id != TYPE_PARTSDB {
            continue;
        }
        let data = match ws {
            Some(ws) => ws.current_data(game, stream, index)?,
            None => file.resource_data(index)?,
        };
        let db = PartsDb::parse(&data)?;
        if name.parse::<usize>() == Ok(index) || db.name.eq_ignore_ascii_case(name) {
            return Ok(Some((index, db)));
        }
    }
    Ok(None)
}

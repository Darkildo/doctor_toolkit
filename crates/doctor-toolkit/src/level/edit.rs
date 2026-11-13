//! Level edits recorded in a workspace: move, duplicate, remove and add
//! objects, and clone the model a new object shows.
//!
//! Every function reads the SimGroup as the workspace has it (so edits
//! build on each other), changes it with [`crate::resource::simgroup_edit`]
//! and records the new data as one undoable operation. Cloning a model is a
//! second operation (new resources) recorded before the SimGroup edit.
//!
//! New objects get their collision from their model's `CollisionHavok4Part`:
//! a clone keeps pointing at the original `hkx`, unless it is made with
//! [`ModelChoice::CloneWithCollision`] ([`clone_model_with_collision`]), which
//! gives the clone a new `hkx` of its own built from its meshes.

use crate::container::ustream::{Resource, Storage, StreamLayout};
use crate::create::{self, CloneOptions, Remap, UsedIds};
use crate::game::{GameInstance, ResourceIndex};
use crate::import::collision as new_collision;
use crate::resource::hkx::collision::{CollisionKind, CollisionMesh};
use crate::resource::simgroup::{Guid, SimGroup};
use crate::resource::simgroup_edit::{Transform, new_object_guid};
use crate::workspace::{NewResourceEdit, ResourceKey, StorageChoice, Workspace};
use crate::{Error, Result};
use std::sync::Arc;

use super::{Location, RowMatrix, TYPE_METAMODEL, TYPE_SIMGROUP, compat};
use crate::resource::metamodel::MetaModel;

/// A SimGroup as the workspace has it.
pub fn current_group(game: &GameInstance, ws: &Workspace, group: Location) -> Result<SimGroup> {
    let data = ws.current_data(game, group.stream, group.resource)?;
    SimGroup::parse(&data)
}

fn label_of(group: &SimGroup, object: usize) -> String {
    let o = &group.objects[object];
    o.class_name()
        .map_or_else(|| format!("{:08x}", o.class), str::to_owned)
}

fn check_group(game: &GameInstance, group: Location) -> Result<String> {
    let stream = game.open_stream(group.stream)?;
    let header = stream
        .layout()
        .resources
        .get(group.resource)
        .ok_or_else(|| Error::InvalidArgument(format!("no resource {}", group.resource)))?
        .header;
    if header.type_id != TYPE_SIMGROUP {
        return Err(Error::InvalidArgument(format!(
            "resource {} is not a SimGroup",
            group.resource
        )));
    }
    Ok(game.streams()[group.stream].stream_name())
}

fn record(
    game: &GameInstance,
    ws: &mut Workspace,
    group: Location,
    data: &SimGroup,
    label: &str,
) -> Result<bool> {
    ws.set_resource(game, group.stream, group.resource, data.encode(), label)
}

/// Sets the world matrix of an object. Returns whether anything changed.
pub fn set_transform(
    game: &GameInstance,
    ws: &mut Workspace,
    group: Location,
    object: usize,
    matrix: RowMatrix,
) -> Result<bool> {
    let name = check_group(game, group)?;
    let mut g = current_group(game, ws, group)?;
    if !g.set_transform(object, matrix)? {
        return Ok(false);
    }
    let label = format!("Move object {object} ({}) in {name}", label_of(&g, object));
    record(game, ws, group, &g, &label)
}

/// A new object GUID that no object of `taken` and of the group has.
pub fn new_guid(group: &SimGroup, seed: &str, taken: &dyn Fn(&Guid) -> bool) -> Guid {
    new_object_guid(seed, |g| {
        taken(g) || group.objects.iter().any(|o| o.guid == *g)
    })
}

/// Duplicates object `object` (sharing its packets) and moves the copy by
/// `offset`. Returns the index and GUID of the copy.
pub fn duplicate(
    game: &GameInstance,
    ws: &mut Workspace,
    group: Location,
    object: usize,
    offset: [f32; 3],
    taken: &dyn Fn(&Guid) -> bool,
) -> Result<(usize, Guid)> {
    let name = check_group(game, group)?;
    let mut g = current_group(game, ws, group)?;
    let seed = format!("{name}/{object}/copy{}", g.objects.len());
    let guid = new_guid(&g, &seed, taken);
    let copy = g.duplicate_object(object, guid)?;
    if let Some(mut m) = g.transform(&g.objects[copy]) {
        for (i, d) in offset.iter().enumerate() {
            m[3][i] += d;
        }
        g.set_transform(copy, m)?;
    }
    let label = format!(
        "Duplicate object {object} ({}) in {name} as {copy}",
        label_of(&g, copy)
    );
    record(game, ws, group, &g, &label)?;
    Ok((copy, guid))
}

/// Removes an object. Returns how many attributes of other objects of the
/// group still refer to its GUID (they are left as they are).
pub fn remove(
    game: &GameInstance,
    ws: &mut Workspace,
    group: Location,
    object: usize,
) -> Result<usize> {
    let name = check_group(game, group)?;
    let mut g = current_group(game, ws, group)?;
    let class = label_of(&g, object.min(g.objects.len().saturating_sub(1)));
    let removed = g.remove_object(object)?;
    let dangling = g.references_to(&removed.guid).len();
    let label = format!("Remove object {object} ({class}) from {name}");
    record(game, ws, group, &g, &label)?;
    Ok(dangling)
}

/// The model a new object shows.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelChoice {
    /// The template's model.
    Template,
    /// An existing `metamodel` by id; it must be in the SimGroup's stream or
    /// an ancestor for the game to find it.
    Existing(u32),
    /// A clone of the metamodel at this location, made in the SimGroup's
    /// stream together with the meshes it uses from its own stream.
    Clone(Location),
    /// As `Clone`, and the clone gets new collision of this kind built from
    /// the given geometry, or from its meshes when `None`.
    CloneWithCollision(Location, CollisionKind, Option<Arc<CollisionMesh>>),
}

/// What [`add`] did.
#[derive(Debug, Clone, PartialEq)]
pub struct Added {
    pub object: usize,
    pub guid: Guid,
    pub model: Option<u32>,
    /// Keys of the cloned resources (metamodel first).
    pub cloned: Vec<ResourceKey>,
}

/// Adds an object to SimGroup `group`: a copy of `template` (object
/// `template.1` of the group at `template.0`, possibly another group) at
/// `position`, showing `model`.
///
/// With `check_model` a model the object cannot show
/// ([`super::compat::model_problem`]) is refused before anything is recorded.
#[allow(clippy::too_many_arguments)]
pub fn add(
    game: &GameInstance,
    index: &ResourceIndex,
    ws: &mut Workspace,
    group: Location,
    template: (Location, usize),
    model: ModelChoice,
    position: Option<[f32; 3]>,
    taken: &dyn Fn(&Guid) -> bool,
    check_model: bool,
) -> Result<Added> {
    let name = check_group(game, group)?;
    let source = if template.0 == group {
        None
    } else {
        check_group(game, template.0)?;
        Some(current_group(game, ws, template.0)?)
    };
    let mut g = current_group(game, ws, group)?;
    let seed = format!("{name}/new{}", g.objects.len());
    let guid = new_guid(&g, &seed, taken);
    let object = match &source {
        None => g.duplicate_object(template.1, guid)?,
        Some(from) => g.copy_object_from(from, template.1, guid)?,
    };
    if check_model {
        let shown = match &model {
            ModelChoice::Template => None,
            ModelChoice::Existing(id) => compat::find_metamodel(game, index, Some(ws), *id)?,
            ModelChoice::Clone(at) | ModelChoice::CloneWithCollision(at, ..) => Some(
                MetaModel::parse(&game.open_stream(at.stream)?.resource_data(at.resource)?)?,
            ),
        };
        if let Some(problem) = shown.and_then(|mm| compat::model_problem(&g, object, &mm)) {
            return Err(Error::InvalidArgument(problem));
        }
    }
    // The clone is recorded first: the SimGroup edit refers to it.
    let (model_id, cloned) = match model {
        ModelChoice::Template => (None, Vec::new()),
        ModelChoice::Existing(id) => (Some(id), Vec::new()),
        ModelChoice::Clone(at) => {
            let (id, keys) = clone_model(game, index, ws, at, &name)?;
            (Some(id), keys)
        }
        ModelChoice::CloneWithCollision(at, kind, mesh) => {
            let collision = Some((kind, mesh.as_deref()));
            let (id, keys) = clone_model_with_collision(game, index, ws, at, &name, collision)?;
            (Some(id), keys)
        }
    };
    if let Some(id) = model_id {
        g.set_model(object, Some(id))?;
    }
    if let Some(p) = position {
        let mut m = g
            .transform(&g.objects[object])
            .unwrap_or_else(|| Transform::default().to_matrix());
        m[3][..3].copy_from_slice(&p);
        g.set_transform(object, m)?;
    }
    let label = format!(
        "Add object {object} ({}) to {name}{}",
        label_of(&g, object),
        model_id.map_or_else(String::new, |m| format!(" with model {m:08x}"))
    );
    record(game, ws, group, &g, &label)?;
    Ok(Added {
        object,
        guid,
        model: model_id.or_else(|| g.model(&g.objects[object])),
        cloned,
    })
}

/// Clones the metamodel at `at` into stream `target` with the resources it
/// refers to in its own stream (meshes and their `meshvolatile`, nested
/// metamodels); materials, textures and collision stay shared. Returns the
/// id of the clone and the keys of all clones.
pub fn clone_model(
    game: &GameInstance,
    index: &ResourceIndex,
    ws: &mut Workspace,
    at: Location,
    target: &str,
) -> Result<(u32, Vec<ResourceKey>)> {
    clone_model_with_collision(game, index, ws, at, target, None)
}

/// As [`clone_model`]; with `collision`, the clone also gets a new `hkx` of
/// that kind built from the given geometry or, when `None`, from the meshes
/// of the metamodel at `at` (as the workspace has them). The `hkx` is added
/// before the clones and the collision parts of the clone point at it
/// ([`crate::import::collision::attach`]). The body keeps the EA material of
/// the original collision.
pub fn clone_model_with_collision(
    game: &GameInstance,
    index: &ResourceIndex,
    ws: &mut Workspace,
    at: Location,
    target: &str,
    collision: Option<(CollisionKind, Option<&CollisionMesh>)>,
) -> Result<(u32, Vec<ResourceKey>)> {
    let stream = game.open_stream(at.stream)?;
    let layout = stream.layout();
    let header = layout
        .resources
        .get(at.resource)
        .ok_or_else(|| Error::InvalidArgument(format!("no resource {}", at.resource)))?
        .header;
    if header.type_id != TYPE_METAMODEL {
        return Err(Error::InvalidArgument(format!(
            "resource {} is not a metamodel",
            at.resource
        )));
    }
    let data = stream.resource_data(at.resource)?;
    let mut indices = vec![at.resource];
    for t in create::references(TYPE_METAMODEL, &data)? {
        if let create::Target::Guid(g) = t
            && let Some(i) = layout.resources.iter().position(|r| {
                r.header.ids == g && [create::TYPE_MESH, TYPE_METAMODEL].contains(&r.header.type_id)
            })
            && !indices.contains(&i)
        {
            indices.push(i);
        }
    }
    let mut used: UsedIds = ws.used_ids(index);
    let source_name = game.streams()[at.stream].stream_name();
    let options = CloneOptions {
        seed: format!("{target}/{source_name}/{}", at.resource),
        rewrite_references: true,
        storage: Some(Storage::RefPack),
    };
    let (mut clones, remap): (Vec<_>, Remap) =
        create::clone_resources(&stream, &indices, &mut used, &options)?;
    let mut extra = Vec::new();
    if let Some((kind, given)) = collision {
        let mesh = match given {
            Some(mesh) => mesh.clone(),
            None => new_collision::mesh_from_metamodel(game, ws, at.stream, at.resource)?,
        };
        let source = ws.current_data(game, at.stream, at.resource)?;
        let material = new_collision::current_material(game, ws, at.stream, &source);
        let clone = clones
            .iter_mut()
            .find(|c| c.source == Some(at.resource))
            .ok_or_else(|| Error::InvalidArgument("the metamodel was not cloned".into()))?;
        let seed = format!("{}:collision", options.seed);
        let new = new_collision::attach(&clone.data, &mesh, kind, material, &mut used, &seed)?;
        clone.data = new.metamodel;
        extra.push(new.resource);
    }
    let new_id = remap
        .guids
        .get(&header.ids)
        .map(|ids| ids[3])
        .ok_or_else(|| Error::InvalidArgument("the metamodel was not cloned".into()))?;
    // In the same stream the clones follow their sources; elsewhere they go
    // before the first SimGroup so that the targets come first. The place is
    // a resource of the original stream: the game file may hold resources
    // the workspace added and applied, and those cannot be places.
    let target_index = game
        .find_stream(target)
        .ok_or_else(|| Error::InvalidArgument(format!("stream {target} is not in the game")))?;
    let original = {
        let path = game.original_path(&game.streams()[target_index]);
        let data = std::fs::read(&path).map_err(|e| Error::io(&path, e))?;
        StreamLayout::parse(&data)?
    };
    let is_original = |r: &&Resource| {
        ResourceKey::new(target, &r.header)
            .find_in(&original)
            .is_some()
    };
    let before = if target_index == at.stream {
        let last = clones.iter().filter_map(|c| c.source).max().unwrap_or(0);
        layout
            .resources
            .iter()
            .skip(last + 1)
            .find(|r| is_original(r))
            .map(|r| ResourceKey::new(target, &r.header))
    } else {
        let target_stream = game.open_stream(target_index)?;
        target_stream
            .layout()
            .resources
            .iter()
            .filter(|r| r.header.type_id == TYPE_SIMGROUP)
            .find(|r| is_original(r))
            .map(|r| ResourceKey::new(target, &r.header))
    };
    let clones: Vec<_> = extra.into_iter().chain(clones).collect();
    let additions: Vec<NewResourceEdit> = clones
        .iter()
        .map(|c| NewResourceEdit {
            stream: target.to_owned(),
            header: c.header,
            data: c.data.clone(),
            storage: StorageChoice::Refpack,
            before: before.clone(),
        })
        .collect();
    let keys = clones
        .iter()
        .map(|c| ResourceKey::new(target, &c.header))
        .collect();
    let label = format!(
        "Clone model {:08x} of {source_name} into {target} as {new_id:08x} ({} resource(s))",
        header.ids[3],
        additions.len()
    );
    drop(stream);
    ws.add_resources(game, &label, &additions)?;
    Ok((new_id, keys))
}

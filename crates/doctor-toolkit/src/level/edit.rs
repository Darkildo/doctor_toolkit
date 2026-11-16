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
use crate::create::{self, CloneOptions, NewResource, Remap, UsedIds};
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

/// A SimGroup to edit: a resource of a game stream, or a SimGroup the
/// workspace added (to a stream it creates or to a game stream) and that is
/// not in the game files yet.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum GroupAt {
    Game(Location),
    Added(ResourceKey),
}

impl From<Location> for GroupAt {
    fn from(at: Location) -> Self {
        Self::Game(at)
    }
}

/// A SimGroup at `at` as the workspace has it.
pub fn current_group_at(game: &GameInstance, ws: &Workspace, at: &GroupAt) -> Result<SimGroup> {
    match at {
        GroupAt::Game(group) => current_group(game, ws, *group),
        GroupAt::Added(key) => {
            let data = ws
                .data(key)?
                .ok_or_else(|| Error::InvalidArgument(format!("{key} is not in the workspace")))?;
            SimGroup::parse(&data)
        }
    }
}

/// The stream name of the SimGroup at `at`, checked to be a SimGroup.
pub fn group_stream(game: &GameInstance, ws: &Workspace, at: &GroupAt) -> Result<String> {
    match at {
        GroupAt::Game(group) => check_group(game, *group),
        GroupAt::Added(key) => {
            if key.type_id != TYPE_SIMGROUP {
                return Err(Error::InvalidArgument(format!("{key} is not a SimGroup")));
            }
            match ws.change(key) {
                Some(c) if c.is_added() => Ok(key.stream.clone()),
                _ => Err(Error::InvalidArgument(format!(
                    "{key} is not a SimGroup added by the workspace"
                ))),
            }
        }
    }
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

/// Records new data of the SimGroup at `group` as one undoable operation
/// labelled `label`. Returns whether anything changed.
pub fn set_group(
    game: &GameInstance,
    ws: &mut Workspace,
    group: &GroupAt,
    data: &SimGroup,
    label: &str,
) -> Result<bool> {
    match group {
        GroupAt::Game(at) => ws.set_resource(game, at.stream, at.resource, data.encode(), label),
        GroupAt::Added(key) => Ok(ws
            .change_resources_and_added(
                game,
                label,
                &[],
                &[],
                &[(key.clone(), Some(data.encode()))],
            )?
            .is_some()),
    }
}

/// Sets the world matrix of an object. Returns whether anything changed.
pub fn set_transform(
    game: &GameInstance,
    ws: &mut Workspace,
    group: Location,
    object: usize,
    matrix: RowMatrix,
) -> Result<bool> {
    set_transform_at(game, ws, &GroupAt::Game(group), object, matrix)
}

/// [`set_transform`] of a SimGroup anywhere ([`GroupAt`]).
pub fn set_transform_at(
    game: &GameInstance,
    ws: &mut Workspace,
    group: &GroupAt,
    object: usize,
    matrix: RowMatrix,
) -> Result<bool> {
    let name = group_stream(game, ws, group)?;
    let mut g = current_group_at(game, ws, group)?;
    if !g.set_transform(object, matrix)? {
        return Ok(false);
    }
    let label = format!("Move object {object} ({}) in {name}", label_of(&g, object));
    set_group(game, ws, group, &g, &label)
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
    duplicate_at(game, ws, &GroupAt::Game(group), object, offset, taken)
}

/// [`duplicate`] in a SimGroup anywhere ([`GroupAt`]).
pub fn duplicate_at(
    game: &GameInstance,
    ws: &mut Workspace,
    group: &GroupAt,
    object: usize,
    offset: [f32; 3],
    taken: &dyn Fn(&Guid) -> bool,
) -> Result<(usize, Guid)> {
    let name = group_stream(game, ws, group)?;
    let mut g = current_group_at(game, ws, group)?;
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
    set_group(game, ws, group, &g, &label)?;
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
    remove_at(game, ws, &GroupAt::Game(group), object)
}

/// [`remove`] from a SimGroup anywhere ([`GroupAt`]).
pub fn remove_at(
    game: &GameInstance,
    ws: &mut Workspace,
    group: &GroupAt,
    object: usize,
) -> Result<usize> {
    let name = group_stream(game, ws, group)?;
    let mut g = current_group_at(game, ws, group)?;
    let class = label_of(&g, object.min(g.objects.len().saturating_sub(1)));
    let removed = g.remove_object(object)?;
    let dangling = g.references_to(&removed.guid).len();
    let label = format!("Remove object {object} ({class}) from {name}");
    set_group(game, ws, group, &g, &label)?;
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
    add_at(
        game,
        index,
        ws,
        &GroupAt::Game(group),
        (&GroupAt::Game(template.0), template.1),
        model,
        position,
        taken,
        check_model,
    )
}

/// [`add`] to a SimGroup anywhere ([`GroupAt`]), with a template object
/// from any SimGroup. A model cloned for it goes into the group's stream.
#[allow(clippy::too_many_arguments)]
pub fn add_at(
    game: &GameInstance,
    index: &ResourceIndex,
    ws: &mut Workspace,
    group: &GroupAt,
    template: (&GroupAt, usize),
    model: ModelChoice,
    position: Option<[f32; 3]>,
    taken: &dyn Fn(&Guid) -> bool,
    check_model: bool,
) -> Result<Added> {
    let name = group_stream(game, ws, group)?;
    let source = if template.0 == group {
        None
    } else {
        group_stream(game, ws, template.0)?;
        Some(current_group_at(game, ws, template.0)?)
    };
    let mut g = current_group_at(game, ws, group)?;
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
    set_group(game, ws, group, &g, &label)?;
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
    let before = match game.find_stream(target) {
        Some(target_index) => {
            place_for_clones(game, layout, at.stream, target_index, target, &clones)?
        }
        // A stream the workspace creates holds its resources in the order
        // they were added, SimGroups last.
        None if ws.new_stream(target).is_some() => None,
        None => {
            return Err(Error::InvalidArgument(format!(
                "stream {target} is neither in the game nor new"
            )));
        }
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

/// Where clones of resources of stream `source` go in the game stream
/// `target_index`: after their sources in the same stream, else before the
/// first SimGroup so that the targets come first. The place is a resource of
/// the original stream: the game file may hold resources the workspace added
/// and applied, and those cannot be places.
fn place_for_clones(
    game: &GameInstance,
    layout: &StreamLayout,
    source: usize,
    target_index: usize,
    target: &str,
    clones: &[NewResource],
) -> Result<Option<ResourceKey>> {
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
    let before = if target_index == source {
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
    Ok(before)
}

/// What [`add_group`] made.
#[derive(Debug, Clone, PartialEq)]
pub struct NewGroup {
    /// The key of the new SimGroup.
    pub key: ResourceKey,
    /// Objects copied into it.
    pub objects: usize,
    /// GUIDs of the copied objects: (original, copy).
    pub guids: Vec<(Guid, Guid)>,
    /// GUID attributes and links of the copies that pointed at copied
    /// objects and now point at their copies.
    pub rewired: usize,
}

/// Adds a new SimGroup to `stream` (a game stream or one the workspace
/// creates), as one undoable operation: empty, or with copies of objects of
/// `copy` (its objects `objects`, every one when `None`). Copies get new
/// GUIDs, and GUID attributes and links between the copied objects follow
/// them, so the new group can be loaded with the original.
pub fn add_group(
    game: &GameInstance,
    index: &ResourceIndex,
    ws: &mut Workspace,
    stream: &str,
    copy: Option<(&GroupAt, Option<&[usize]>)>,
    taken: &dyn Fn(&Guid) -> bool,
) -> Result<NewGroup> {
    use crate::resource::simgroup::Value;
    let stream = crate::workspace::key::normalize_stream(stream);
    if game.find_stream(&stream).is_none() && ws.new_stream(&stream).is_none() {
        return Err(Error::InvalidArgument(format!(
            "stream {stream} is neither in the game nor new"
        )));
    }
    let mut g = SimGroup::empty();
    // The group hash is the table-of-contents hash of the stream, as in
    // every SimGroup of the game (`docs/formats/simgroup.md` §1).
    g.group_hash = match ws.new_stream(&stream) {
        Some(new) => new.hash,
        None => game.stream_hash(&stream)?.unwrap_or(0),
    };
    let mut guids: Vec<(Guid, Guid)> = Vec::new();
    let mut rewired = 0;
    if let Some((from, objects)) = copy {
        group_stream(game, ws, from)?;
        let source = current_group_at(game, ws, from)?;
        g.version = source.version;
        g.flags = source.flags;
        let list: Vec<usize> = match objects {
            Some(list) => list.to_vec(),
            None => (0..source.objects.len()).collect(),
        };
        for &o in &list {
            let old = source
                .objects
                .get(o)
                .ok_or_else(|| {
                    Error::InvalidArgument(format!(
                        "the SimGroup has {} object(s), not {}",
                        source.objects.len(),
                        o + 1
                    ))
                })?
                .guid;
            let seed = format!("{stream}/group/{o}");
            let new = new_guid(&g, &seed, taken);
            g.copy_object_from(&source, o, new)?;
            guids.push((old, new));
        }
        let map: std::collections::HashMap<Guid, Guid> = guids.iter().copied().collect();
        for o in 0..g.objects.len() {
            let mut sets = Vec::new();
            for (slot, &p) in g.objects[o].packets.iter().enumerate() {
                for a in &g.packets[p].attrs {
                    if let Value::Guid(old) = a.value
                        && let Some(new) = map.get(&old)
                    {
                        sets.push((slot, a.index, *new));
                    }
                }
            }
            for (slot, attr, new) in sets {
                if g.set_attr(o, slot, attr, Some(Value::Guid(new)))? {
                    rewired += 1;
                }
            }
            let links: Vec<(usize, Guid)> = g.objects[o]
                .links
                .iter()
                .enumerate()
                .filter_map(|(i, l)| map.get(l).map(|n| (i, *n)))
                .collect();
            for (i, new) in links {
                if g.set_link(o, i, new)? {
                    rewired += 1;
                }
            }
        }
    }
    let mut used = ws.used_ids(index);
    let seed = format!("{stream}:simgroup:{}", ws.operations().len());
    let ids = used.new_ids(
        create::IdForm::for_type(TYPE_SIMGROUP),
        TYPE_SIMGROUP,
        &[0; 4],
        &seed,
    );
    let defaults = create::type_defaults(TYPE_SIMGROUP);
    let header = crate::container::ustream::ResourceHeader {
        mem_policy: defaults.mem_policy,
        alignment: defaults.alignment,
        type_id: TYPE_SIMGROUP,
        ids,
        data_size: 0,
    };
    let key = ResourceKey::new(&stream, &header);
    let label = match copy {
        None => format!("Add an empty SimGroup to {stream}"),
        Some(_) => format!(
            "Add a SimGroup with {} copied object(s) to {stream}",
            guids.len()
        ),
    };
    ws.add_resources(
        game,
        &label,
        &[NewResourceEdit {
            stream: stream.clone(),
            header,
            data: g.encode(),
            storage: StorageChoice::Refpack,
            before: None,
        }],
    )?;
    Ok(NewGroup {
        key,
        objects: guids.len(),
        guids,
        rewired,
    })
}

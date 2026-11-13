//! New collision for a model, recorded in the workspace.
//!
//! A metamodel refers to its collision through `CollisionHavok4Part`
//! entries: T0 resource references of type [`TYPE_HKX`] or
//! [`TYPE_HKX_REFERENCE`] whose GUID is the GUID of an `hkx` resource. [`attach`] builds a new `hkx`
//! ([`crate::resource::hkx::collision`]) under new ids and points every
//! collision reference of a metamodel at it; [`plan`] does that for a
//! metamodel of a stream and records it as one workspace operation (the new
//! `hkx` goes right before the metamodel, so it loads first).
//!
//! The geometry comes from glTF ([`mesh_from_gltf`]) or from the meshes the
//! metamodel shows as the workspace has them ([`mesh_from_metamodel`]), for
//! example after `model-import`.
//!
//! API for other stages:
//!
//! - stage 10 (`model-import`): [`crate::import::model::ImportOptions::collision`]
//!   builds collision from the imported glTF in the same operation;
//! - stage 11 (new objects with a cloned model):
//!   [`crate::level::edit::clone_model_with_collision`] clones the model and
//!   gives the clone its own collision built from its meshes;
//! - anything else: [`attach`] on metamodel data with a [`CollisionMesh`].

use serde::Serialize;

use crate::container::ustream::Storage;
use crate::create::{IdForm, NewResource, Remap, TYPE_METAMODEL, UsedIds};
use crate::game::{GameInstance, ResourceIndex};
use crate::import::gltf::GltfModel;
use crate::resource::hkx::collision::{
    self, CollisionKind, CollisionMesh, CollisionSummary, DEFAULT_MATERIAL,
};
use crate::resource::hkx::physics::EaMaterial;
use crate::resource::hkx::{TYPE_HKX, TYPE_HKX_REFERENCE};
use crate::resource::mesh::{Model, TYPE_MESH};
use crate::resource::metamodel::MetaModel;
use crate::workspace::{NewResourceEdit, ResourceEdit, ResourceKey, StorageChoice, Workspace};
use crate::{Error, Result};

/// GUIDs of the collision `hkx` resources a metamodel refers to, without repeats.
pub fn collision_references(metamodel: &[u8]) -> Result<Vec<[u32; 4]>> {
    let model = MetaModel::parse(metamodel)?;
    let mut out: Vec<[u32; 4]> = Vec::new();
    for r in &model.resources {
        if r.type_id.0 == TYPE_HKX_REFERENCE || r.type_id.0 == TYPE_HKX {
            let ids = r.ids.map(|h| h.0);
            if !out.contains(&ids) {
                out.push(ids);
            }
        }
    }
    Ok(out)
}

/// GUIDs of the meshes a metamodel shows.
pub fn mesh_references(metamodel: &[u8]) -> Result<Vec<[u32; 4]>> {
    let model = MetaModel::parse(metamodel)?;
    Ok(model
        .resources
        .iter()
        .filter(|r| r.type_id.0 == TYPE_MESH)
        .map(|r| r.ids.map(|h| h.0))
        .collect())
}

/// All triangle parts of a glTF model (node transforms applied).
pub fn mesh_from_gltf(model: &GltfModel) -> CollisionMesh {
    let mut mesh = CollisionMesh::default();
    for part in &model.parts {
        mesh.append(&part.geometry.positions, &part.geometry.triangles);
    }
    mesh
}

/// All draw records of a game mesh, in model space.
pub fn mesh_from_model(model: &Model) -> Result<CollisionMesh> {
    let mut mesh = CollisionMesh::default();
    for record in &model.records {
        let vertices = model.vertices(record)?;
        let triangles = model.triangles(record)?;
        mesh.append(&vertices.positions, &triangles);
    }
    Ok(mesh)
}

/// A new collision `hkx` and the metamodel pointing at it.
#[derive(Debug, Clone)]
pub struct NewCollision {
    pub resource: NewResource,
    /// The metamodel data with its collision references rewritten.
    pub metamodel: Vec<u8>,
    /// Collision references rewritten.
    pub references: usize,
    pub summary: CollisionSummary,
}

/// Builds collision for `mesh` as a new `hkx` resource (ids from `used`,
/// derived from `seed`) and points every collision reference of `metamodel`
/// at it. `material` is the EA material of the body. Fails when the
/// metamodel has no collision part: adding one is not supported.
pub fn attach(
    metamodel: &[u8],
    mesh: &CollisionMesh,
    kind: CollisionKind,
    material: EaMaterial,
    used: &mut UsedIds,
    seed: &str,
) -> Result<NewCollision> {
    let old = collision_references(metamodel)?;
    if old.is_empty() {
        return Err(Error::InvalidArgument(
            "the metamodel has no collision part (CollisionHavok4Part); \
             only metamodels with one can get new collision"
                .into(),
        ));
    }
    let name = MetaModel::parse(metamodel)
        .ok()
        .and_then(|m| m.parts.first().map(|p| p.name.clone()))
        .unwrap_or_else(|| "collision".into());
    let (bytes, summary) = collision::build(mesh, kind, material, &name)?;
    let ids = used.new_ids(IdForm::for_type(TYPE_HKX), TYPE_HKX, &[0; 4], seed);
    let resource = NewResource::new(TYPE_HKX, ids, bytes, Storage::RefPack)?;
    let mut data = metamodel.to_vec();
    let remap = Remap {
        guids: old.iter().map(|g| (*g, ids)).collect(),
        names: Default::default(),
    };
    let references = crate::create::rewrite_references(TYPE_METAMODEL, &mut data, &remap)?;
    Ok(NewCollision {
        resource,
        metamodel: data,
        references,
        summary,
    })
}

/// The first EA material of the collision a metamodel uses now (looked up
/// in the stream), or the default material.
pub fn current_material(
    game: &GameInstance,
    ws: &Workspace,
    stream: usize,
    metamodel: &[u8],
) -> EaMaterial {
    let Ok(refs) = collision_references(metamodel) else {
        return DEFAULT_MATERIAL;
    };
    let Ok(file) = game.open_stream(stream) else {
        return DEFAULT_MATERIAL;
    };
    for (i, r) in file.layout().resources.iter().enumerate() {
        if r.header.type_id == TYPE_HKX
            && refs.contains(&r.header.ids)
            && let Ok(data) = ws.current_data(game, stream, i)
        {
            return collision::template_material(&data);
        }
    }
    DEFAULT_MATERIAL
}

/// The meshes metamodel `resource` of `stream` shows (as the workspace has
/// them, found in the same stream), as one collision mesh.
pub fn mesh_from_metamodel(
    game: &GameInstance,
    ws: &Workspace,
    stream: usize,
    resource: usize,
) -> Result<CollisionMesh> {
    let data = ws.current_data(game, stream, resource)?;
    let meshes = mesh_references(&data)?;
    let file = game.open_stream(stream)?;
    let layout = file.layout();
    let mut mesh = CollisionMesh::default();
    // The first mesh is the finest level of detail.
    let Some(first) = meshes.first() else {
        return Err(Error::InvalidArgument("the metamodel shows no mesh".into()));
    };
    let index = layout
        .resources
        .iter()
        .position(|r| r.header.type_id == TYPE_MESH && r.header.ids == *first)
        .ok_or_else(|| {
            Error::InvalidArgument(format!(
                "mesh {:08x} of the metamodel is not in its stream",
                first[3]
            ))
        })?;
    let mesh_data = ws.current_data(game, stream, index)?;
    let volatile = match crate::resource::mesh::volatile_for(layout, index) {
        Some(v) => ws.current_data(game, stream, v)?,
        None => Vec::new(),
    };
    let model = Model::parse(&mesh_data, &volatile)?;
    let part = mesh_from_model(&model)?;
    mesh.append(&part.positions, &part.triangles);
    Ok(mesh)
}

/// What [`plan`] does.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CollisionReport {
    pub metamodel: String,
    pub added: String,
    pub references: usize,
    #[serde(flatten)]
    pub collision: CollisionSummary,
}

/// A collision change ready to be recorded.
#[derive(Debug, Clone)]
pub struct CollisionPlan {
    pub report: CollisionReport,
    pub label: String,
    /// The new `hkx` data, for a preview.
    pub hkx: Vec<u8>,
    edits: Vec<(usize, Vec<ResourceEdit>)>,
    additions: Vec<NewResourceEdit>,
}

impl CollisionPlan {
    /// Records the change as one undoable operation.
    pub fn commit(&self, ws: &mut Workspace, game: &GameInstance) -> Result<Option<u64>> {
        ws.change_resources(game, &self.label, &self.edits, &self.additions)
    }
}

/// Works out new collision for metamodel `resource` of `stream` built from
/// `mesh`. The body keeps the EA material of the current collision.
pub fn plan(
    game: &GameInstance,
    ws: &Workspace,
    index: &ResourceIndex,
    stream: usize,
    resource: usize,
    mesh: &CollisionMesh,
    kind: CollisionKind,
) -> Result<CollisionPlan> {
    let file = game.open_stream(stream)?;
    let layout = file.layout();
    let header = layout
        .resources
        .get(resource)
        .ok_or_else(|| Error::InvalidArgument(format!("no resource {resource}")))?
        .header;
    if header.type_id != TYPE_METAMODEL {
        return Err(Error::InvalidArgument(format!(
            "resource {resource} is not a metamodel"
        )));
    }
    let name = game.streams()[stream].stream_name();
    let data = ws.current_data(game, stream, resource)?;
    let material = current_material(game, ws, stream, &data);
    let mut used = ws.used_ids(index);
    let seed = format!("{name}/{}/collision", header.ids[3]);
    let new = attach(&data, mesh, kind, material, &mut used, &seed)?;
    let metamodel_key = ResourceKey::new(&name, &header);
    let added = ResourceKey::new(&name, &new.resource.header);
    // The new hkx goes right before the metamodel. A metamodel the workspace
    // added is not in the original stream: the hkx goes where it goes (just
    // after it, before the same original resource).
    let before = match ws.change(&metamodel_key).and_then(|c| c.added.as_ref()) {
        Some(added) => added.before.clone(),
        None => Some(metamodel_key.clone()),
    };
    let label = format!(
        "New {} collision {:08x} for metamodel {:08x} in {name}",
        match kind {
            CollisionKind::Mesh => "mesh",
            CollisionKind::Hull => "hull",
            CollisionKind::Box => "box",
        },
        new.resource.header.ids[3],
        header.ids[3]
    );
    Ok(CollisionPlan {
        report: CollisionReport {
            metamodel: metamodel_key.to_string(),
            added: added.to_string(),
            references: new.references,
            collision: new.summary.clone(),
        },
        label,
        hkx: new.resource.data.clone(),
        edits: vec![(
            stream,
            vec![ResourceEdit {
                index: resource,
                data: new.metamodel,
                storage: StorageChoice::Keep,
            }],
        )],
        additions: vec![NewResourceEdit {
            stream: name,
            header: new.resource.header,
            data: new.resource.data,
            storage: StorageChoice::Refpack,
            before,
        }],
    })
}

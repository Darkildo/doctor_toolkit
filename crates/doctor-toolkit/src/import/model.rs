//! Importing a model into the game: geometry from glTF, skeleton binding,
//! materials and textures, recorded in the workspace.
//!
//! [`plan`] works out everything without touching the workspace and returns
//! an [`ImportPlan`] with a summary and the new mesh (for a preview);
//! [`ImportPlan::commit`] records it as one undoable operation.
//!
//! Two modes:
//!
//! - **replace** (default): the target mesh (or every mesh of the target
//!   metamodel, its levels of detail) gets the imported geometry under its
//!   own ids; everything that uses the model shows the new one;
//! - **new** (`ImportOptions::new_name`): the target (metamodel or mesh) is
//!   cloned with its meshes under new ids ([`crate::create::clone_resources`])
//!   and the clones get the imported geometry; `retarget` points SimGroups or
//!   metamodels of the stream at the clone.
//!
//! Materials: a glTF material named like a material of the target mesh, with
//! no textures or with the same pixels as the game's textures, reuses that
//! material. Otherwise the material (the named one, or the target's most used
//! one) is cloned with a new name hash, and its base colour and normal map are
//! cloned and replaced by the glTF images. New materials and textures go into
//! the target's stream before the mesh, so they load first.

use std::collections::HashMap;

use serde::Serialize;

use crate::container::ustream::{Storage, StreamLayout};
use crate::create::{
    self, CloneOptions, NewResource, ReferenceChecker, Remap, TYPE_MATERIAL, TYPE_METAMODEL,
    Target as RefTarget, UsedIds,
};
use crate::export::gltf::AssetSource;
use crate::game::{GameAssets, GameInstance, ResourceIndex};
use crate::hash::crc32;
use crate::import::collision as new_collision;
use crate::import::gltf::GltfModel;
use crate::resource::hkx::collision::{CollisionKind, CollisionSummary};
use crate::resource::material::{Material, TextureRole};
use crate::resource::mesh::{Model, TYPE_MESH, TYPE_MESH_VOLATILE};
use crate::resource::mesh_build::{self, BuildOptions, PartGeometry, RecordKind};
use crate::resource::mesh_image::MeshImage;
use crate::resource::names::bone_names;
use crate::resource::rcb::{Mat4, Skeleton};
use crate::resource::texture::{self, TYPE_TG4D, TYPE_TG4H, Texture};
use crate::resource::{metamodel::MetaModel, pixels, tg4};
use crate::workspace::{NewResourceEdit, ResourceEdit, ResourceKey, StorageChoice, Workspace};
use crate::{Error, Result};

/// Options of [`plan`].
#[derive(Debug, Clone, Default)]
pub struct ImportOptions {
    /// Clone the target into a new model set with this name instead of
    /// replacing it.
    pub new_name: Option<String>,
    /// Resources of the target stream (SimGroup, metamodel) to point at the
    /// new set; only with `new_name`.
    pub retarget: Vec<usize>,
    /// Vertex format of every record instead of the target's.
    pub format: Option<u8>,
    /// Also build new collision of this kind from the imported geometry and
    /// point the metamodel at it (needs a metamodel target with a
    /// `CollisionHavok4Part`; see [`crate::import::collision`]).
    pub collision: Option<CollisionKind>,
}

/// One part of the import, for the summary.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PartReport {
    pub name: String,
    pub kind: String,
    pub vertices: usize,
    pub triangles: usize,
    pub material: String,
    /// Whether the material is new (cloned with the imported textures).
    pub new_material: bool,
}

/// What an import does.
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct ImportSummary {
    pub mode: String,
    /// Meshes replaced or created (resource keys).
    pub meshes: Vec<String>,
    pub parts: Vec<PartReport>,
    /// Draw records of the first mesh.
    pub records: usize,
    pub skeleton: Option<String>,
    /// glTF joints with weights, all mapped to skeleton bones.
    pub bones_mapped: usize,
    pub materials_reused: usize,
    pub materials_new: usize,
    pub textures_imported: usize,
    /// Resources added to the workspace.
    pub added: Vec<String>,
    /// Resources changed (other than the meshes).
    pub retargeted: Vec<String>,
    /// New collision, when asked for.
    pub collision: Option<CollisionSummary>,
    pub warnings: Vec<String>,
}

/// An import ready to be recorded.
#[derive(Debug, Clone)]
pub struct ImportPlan {
    pub summary: ImportSummary,
    /// The first new mesh, for a preview.
    pub mesh: MeshImage,
    pub label: String,
    edits: Vec<(usize, Vec<ResourceEdit>)>,
    additions: Vec<NewResourceEdit>,
}

impl ImportPlan {
    /// The first new mesh as the viewers read it.
    pub fn preview(&self) -> Result<Model> {
        let (mesh, volatile) = self.mesh.encode()?;
        Model::parse(&mesh, &volatile)
    }

    /// Materials and textures for a preview: the new ones of the plan first,
    /// then `game`.
    pub fn assets<'a>(&'a self, game: &'a mut dyn AssetSource) -> PlanAssets<'a> {
        PlanAssets { plan: self, game }
    }

    /// The materials the import adds (clones with the imported textures).
    pub fn new_materials(&self) -> Vec<Material> {
        self.additions
            .iter()
            .filter(|a| a.header.type_id == TYPE_MATERIAL)
            .filter_map(|a| Material::parse(&a.data).ok())
            .collect()
    }

    /// The textures the import adds.
    pub fn new_textures(&self) -> Vec<Texture> {
        self.additions
            .iter()
            .filter(|a| a.header.type_id == TYPE_TG4H)
            .filter_map(|h| {
                let d = self.additions.iter().find(|d| {
                    d.header.type_id == TYPE_TG4D && d.header.ids[3] == h.header.ids[3]
                })?;
                Some(Texture {
                    header: tg4::Tg4Header::parse(&h.data).ok()?,
                    data: d.data.clone(),
                })
            })
            .collect()
    }

    /// Ids (`ids[3]`) of the meshes the import replaces or creates.
    pub fn mesh_ids(&self) -> Vec<u32> {
        self.summary
            .meshes
            .iter()
            .filter_map(|k| k.parse::<ResourceKey>().ok())
            .map(|k| k.ids[3])
            .collect()
    }

    /// Sets float parameter `param` of the new material with name hash
    /// `material` (a tint colour, say) before the plan is recorded.
    pub fn set_material_floats(
        &mut self,
        material: u32,
        param: &str,
        values: &[f32],
    ) -> Result<()> {
        let addition = self
            .additions
            .iter_mut()
            .filter(|a| a.header.type_id == TYPE_MATERIAL)
            .find(|a| a.data.get(8..12) == Some(&material.to_le_bytes()[..]))
            .ok_or_else(|| {
                Error::InvalidArgument(format!("the import adds no material {material:08x}"))
            })?;
        addition.data = Material::set_floats(&addition.data, param, values)?;
        Ok(())
    }

    /// Records the import as one undoable operation. Returns its sequence number.
    pub fn commit(&self, workspace: &mut Workspace, game: &GameInstance) -> Result<Option<u64>> {
        workspace.change_resources(game, &self.label, &self.edits, &self.additions)
    }
}

/// [`ImportPlan::assets`].
pub struct PlanAssets<'a> {
    plan: &'a ImportPlan,
    game: &'a mut dyn AssetSource,
}

impl AssetSource for PlanAssets<'_> {
    fn material(&mut self, hash: u32) -> Option<Material> {
        self.plan
            .additions
            .iter()
            .filter(|a| a.header.type_id == TYPE_MATERIAL)
            .find(|a| a.data.get(8..12) == Some(&hash.to_le_bytes()[..]))
            .and_then(|a| Material::parse(&a.data).ok())
            .or_else(|| self.game.material(hash))
    }

    fn texture(&mut self, hash: u32) -> Option<Texture> {
        let find = |type_id: u32| {
            self.plan
                .additions
                .iter()
                .find(|a| a.header.type_id == type_id && a.header.ids[3] == hash)
        };
        match (find(TYPE_TG4H), find(TYPE_TG4D)) {
            (Some(h), Some(d)) => Some(Texture {
                header: tg4::Tg4Header::parse(&h.data).ok()?,
                data: d.data.clone(),
            }),
            _ => self.game.texture(hash),
        }
    }

    fn skeleton(&mut self, bones: &[(u32, u32)]) -> Option<Skeleton> {
        self.game.skeleton(bones)
    }
}

/// The SDBM hash a joint name stands for: a known bone name, or the 8
/// hexadecimal digits the exporters write for bones without a known name
/// (USD paths start them with `_`: `_1234abcd`).
pub fn joint_hash(name: &str) -> u32 {
    let direct = crate::hash::sdbm(name.as_bytes());
    if bone_names().get(direct).is_some() {
        return direct;
    }
    let lower = crate::hash::sdbm(name.to_ascii_lowercase().as_bytes());
    if bone_names().get(lower).is_some() {
        return lower;
    }
    let hex = name.strip_prefix('_').unwrap_or(name);
    if hex.len() == 8
        && let Ok(hash) = u32::from_str_radix(hex, 16)
    {
        return hash;
    }
    lower
}

struct Target {
    stream_name: String,
    /// The resource named on the command line (mesh or metamodel).
    resource: usize,
    /// Mesh resources to rebuild and their meshvolatile.
    meshes: Vec<(usize, Option<usize>)>,
}

fn find_target(game: &GameInstance, stream: usize, resource: usize) -> Result<Target> {
    let file = game.open_stream(stream)?;
    let layout = file.layout();
    let header = layout
        .resources
        .get(resource)
        .ok_or_else(|| Error::InvalidArgument(format!("no resource {resource}")))?
        .header;
    let stream_name = game.streams()[stream].stream_name();
    let meshes: Vec<usize> = match header.type_id {
        TYPE_MESH => vec![resource],
        TYPE_METAMODEL => {
            let meta = MetaModel::parse(&file.resource_data(resource)?)?;
            let mut out = Vec::new();
            for r in meta.resources.iter().filter(|r| r.type_id.0 == TYPE_MESH) {
                let ids = r.ids.map(|h| h.0);
                if let Some(i) = layout
                    .resources
                    .iter()
                    .position(|x| x.header.type_id == TYPE_MESH && x.header.ids == ids)
                    && !out.contains(&i)
                {
                    out.push(i);
                }
            }
            if out.is_empty() {
                return Err(Error::InvalidArgument(format!(
                    "metamodel {resource} of {stream_name} has no mesh in this stream"
                )));
            }
            out
        }
        other => {
            return Err(Error::InvalidArgument(format!(
                "resource {resource} of {stream_name} is a {}, not a mesh or metamodel",
                crate::resource::names::type_tags().display(other)
            )));
        }
    };
    let meshes = meshes
        .into_iter()
        .map(|m| (m, crate::resource::mesh::volatile_for(layout, m)))
        .collect();
    Ok(Target {
        stream_name,
        resource,
        meshes,
    })
}

/// Works out an import of `model` into resource `resource` (a mesh or a
/// metamodel) of stream `stream`.
#[allow(clippy::too_many_arguments)]
pub fn plan(
    game: &GameInstance,
    workspace: &Workspace,
    index: &ResourceIndex,
    assets: &mut GameAssets,
    stream: usize,
    resource: usize,
    model: &GltfModel,
    options: &ImportOptions,
) -> Result<ImportPlan> {
    if model.parts.is_empty() {
        return Err(Error::InvalidArgument(
            "the model file has no triangle meshes".into(),
        ));
    }
    let target = find_target(game, stream, resource)?;
    let file = game.open_stream(stream)?;
    let layout = file.layout();
    let key_of = |i: usize| ResourceKey::new(&target.stream_name, &layout.resources[i].header);
    // New resources go before a resource of the original stream: the game
    // file may hold resources the workspace added and applied (clones made
    // earlier), and those cannot be places. `place_from(i)` is the first
    // original resource at or after `i` of the game file.
    let original = {
        let path = game.original_path(&game.streams()[stream]);
        let data = std::fs::read(&path).map_err(|e| Error::io(&path, e))?;
        StreamLayout::parse(&data)?
    };
    let place_from = |from: usize| -> Option<ResourceKey> {
        (from..layout.resources.len())
            .map(key_of)
            .find(|k| k.find_in(&original).is_some())
    };
    let mut summary = ImportSummary {
        mode: if options.new_name.is_some() {
            "new"
        } else {
            "replace"
        }
        .into(),
        warnings: model.warnings.clone(),
        ..Default::default()
    };
    if !options.retarget.is_empty() && options.new_name.is_none() {
        return Err(Error::InvalidArgument(
            "retargeting needs a new model set (--new NAME)".into(),
        ));
    }
    let mut used = workspace.used_ids(index);

    // Current data of the target meshes.
    let mut templates = Vec::new();
    for &(mesh, volatile) in &target.meshes {
        let data = workspace.current_data(game, stream, mesh)?;
        let vol = match volatile {
            Some(v) => workspace.current_data(game, stream, v)?,
            None => Vec::new(),
        };
        templates.push(MeshImage::parse(&data, &vol)?);
    }
    let first_template = &templates[0];

    // Skeleton.
    let mut parts: Vec<PartGeometry> = model.parts.iter().map(|p| p.geometry.clone()).collect();
    let skinned = parts.iter().any(|p| p.skin.is_some());
    let mut skeleton_hashes = None;
    if skinned {
        let skeleton = resolve_skeleton(assets, first_template, model, &mut parts, &mut summary)?;
        skeleton_hashes = Some(skeleton.bones.iter().map(|b| b.hash).collect::<Vec<u32>>());
    }

    // Materials.
    let template_materials = material_counts(first_template);
    let mut materials: Vec<Option<(u32, bool, String)>> = vec![None; model.materials.len() + 1];
    let mut additions: Vec<NewResource> = Vec::new();
    let mut base_of: HashMap<u32, u32> = HashMap::new();
    let mut checker = ReferenceChecker::new(game)?;
    let seed = format!(
        "{}:{}:{}",
        target.stream_name,
        options.new_name.as_deref().unwrap_or(""),
        model.name
    );
    for part in &model.parts {
        let slot = part.material.map_or(model.materials.len(), |m| m);
        if materials[slot].is_some() {
            continue;
        }
        let gltf_material = part.material.and_then(|m| model.materials.get(m));
        let resolved = resolve_material(
            game,
            assets,
            &mut checker,
            stream,
            &template_materials,
            gltf_material,
            &mut used,
            &format!("{seed}:{slot}"),
            &mut additions,
            &mut summary,
        )?;
        base_of.insert(resolved.0, resolved.3);
        materials[slot] = Some((resolved.0, resolved.1, resolved.2));
    }
    for (part, geometry) in model.parts.iter().zip(parts.iter_mut()) {
        let slot = part.material.map_or(model.materials.len(), |m| m);
        geometry.material_hash = materials[slot].as_ref().expect("resolved above").0;
    }

    // Meshes.
    let mut built = Vec::new();
    for template in &templates {
        let (image, warnings) = mesh_build::build(
            &parts,
            &BuildOptions {
                name: options
                    .new_name
                    .clone()
                    .unwrap_or_else(|| template.name.clone()),
                skeleton: skeleton_hashes.clone(),
                template: Some(template),
                // Template records are found by the material a new one was cloned from.
                template_materials: base_of.clone(),
                format: options.format,
            },
        )?;
        for w in warnings {
            if !summary.warnings.contains(&w) {
                summary.warnings.push(w);
            }
        }
        built.push(image);
    }
    let first = built[0].clone();
    summary.records = first.records.len();
    for (part, geometry) in model.parts.iter().zip(&parts) {
        let slot = part.material.map_or(model.materials.len(), |m| m);
        let (_, new, name) = materials[slot].clone().expect("resolved above");
        let kind = first
            .records
            .iter()
            .find(|r| r.name == geometry.name)
            .map(RecordKind::of);
        summary.parts.push(PartReport {
            name: geometry.name.clone(),
            kind: kind.map_or("?".into(), |k| format!("{k:?}").to_lowercase()),
            vertices: geometry.positions.len(),
            triangles: geometry.triangles.len(),
            material: name,
            new_material: new,
        });
    }

    // Workspace changes.
    let first_mesh_place = place_from(target.meshes[0].0);
    let mut new_edits: Vec<NewResourceEdit> = additions
        .iter()
        .map(|r| new_edit(&target.stream_name, r, first_mesh_place.clone()))
        .collect();
    let mut edits: Vec<ResourceEdit> = Vec::new();
    match &options.new_name {
        None => {
            for ((mesh, volatile), image) in target.meshes.iter().zip(&built) {
                let (data, vol) = image.encode()?;
                edits.push(ResourceEdit {
                    index: *mesh,
                    data,
                    storage: StorageChoice::Keep,
                });
                match volatile {
                    Some(v) => edits.push(ResourceEdit {
                        index: *v,
                        data: vol,
                        storage: StorageChoice::Keep,
                    }),
                    None => {
                        // The mesh had no meshvolatile: add one right after it.
                        let header = layout.resources[*mesh].header;
                        let ids = header.ids.map(|i| !i);
                        let resource =
                            NewResource::new(TYPE_MESH_VOLATILE, ids, vol, Storage::Raw)?;
                        new_edits.push(new_edit(
                            &target.stream_name,
                            &resource,
                            place_from(mesh + 1),
                        ));
                    }
                }
                summary.meshes.push(key_of(*mesh).to_string());
            }
            if let Some(kind) = options.collision {
                if layout.resources[target.resource].header.type_id == TYPE_METAMODEL {
                    let data = workspace.current_data(game, stream, target.resource)?;
                    let material = new_collision::current_material(game, workspace, stream, &data);
                    let new = new_collision::attach(
                        &data,
                        &new_collision::mesh_from_gltf(model),
                        kind,
                        material,
                        &mut used,
                        &format!("{seed}:collision"),
                    )?;
                    edits.push(ResourceEdit {
                        index: target.resource,
                        data: new.metamodel,
                        storage: StorageChoice::Keep,
                    });
                    new_edits.insert(
                        0,
                        new_edit(
                            &target.stream_name,
                            &new.resource,
                            // Right before the metamodel; after it when the
                            // metamodel is itself an addition.
                            match workspace
                                .change(&key_of(target.resource))
                                .and_then(|c| c.added.as_ref())
                            {
                                Some(added) => added.before.clone(),
                                None => Some(key_of(target.resource)),
                            },
                        ),
                    );
                    summary.collision = Some(new.summary);
                } else {
                    summary
                        .warnings
                        .push("collision needs a metamodel target; none was built".into());
                }
            }
        }
        Some(name) => {
            let mut sources = vec![target.resource];
            sources.extend(target.meshes.iter().map(|m| m.0));
            let (mut clones, remap) = create::clone_resources(
                &file,
                &sources,
                &mut used,
                &CloneOptions {
                    seed: format!("{seed}:clone"),
                    rewrite_references: true,
                    storage: None,
                },
            )?;
            for clone in &mut clones {
                let Some(source) = clone.source else { continue };
                let Some(n) = target
                    .meshes
                    .iter()
                    .position(|m| m.0 == source || m.1 == Some(source))
                else {
                    continue;
                };
                let (data, vol) = built[n].encode()?;
                clone.data = if clone.header.type_id == TYPE_MESH {
                    summary
                        .meshes
                        .push(ResourceKey::new(&target.stream_name, &clone.header).to_string());
                    data
                } else {
                    vol
                };
                clone.header.data_size = clone.data.len() as u32;
            }
            if clones
                .iter()
                .all(|c| c.header.type_id != TYPE_MESH_VOLATILE)
            {
                summary.warnings.push(format!(
                    "{name}: the cloned mesh has no meshvolatile; vertex data may be missing"
                ));
            }
            let last = clones.iter().filter_map(|c| c.source).max().unwrap_or(0);
            let before = place_from(last + 1);
            if let Some(kind) = options.collision {
                match clones.iter_mut().find(|c| {
                    c.source == Some(target.resource) && c.header.type_id == TYPE_METAMODEL
                }) {
                    Some(clone) => {
                        let material =
                            new_collision::current_material(game, workspace, stream, &clone.data);
                        let new = new_collision::attach(
                            &clone.data,
                            &new_collision::mesh_from_gltf(model),
                            kind,
                            material,
                            &mut used,
                            &format!("{seed}:collision"),
                        )?;
                        clone.data = new.metamodel;
                        new_edits.push(new_edit(
                            &target.stream_name,
                            &new.resource,
                            before.clone(),
                        ));
                        summary.collision = Some(new.summary);
                    }
                    None => summary
                        .warnings
                        .push("collision needs a metamodel target; none was built".into()),
                }
            }
            for clone in &clones {
                new_edits.push(new_edit(&target.stream_name, clone, before.clone()));
            }
            for &r in &options.retarget {
                let header = layout
                    .resources
                    .get(r)
                    .ok_or_else(|| Error::InvalidArgument(format!("no resource {r} to retarget")))?
                    .header;
                let mut data = workspace.current_data(game, stream, r)?;
                let changed = create::rewrite_references(
                    header.type_id,
                    &mut data,
                    &Remap {
                        guids: remap.guids.clone(),
                        names: HashMap::new(),
                    },
                )?;
                if changed == 0 {
                    summary.warnings.push(format!(
                        "{}: no reference to the target to point at the new model",
                        key_of(r)
                    ));
                    continue;
                }
                summary
                    .retargeted
                    .push(format!("{} ({changed} reference(s))", key_of(r)));
                edits.push(ResourceEdit {
                    index: r,
                    data,
                    storage: StorageChoice::Keep,
                });
            }
        }
    }
    // A clone of a mesh with a volatile companion has its volatile right after it.
    summary.added = new_edits
        .iter()
        .map(|e| {
            ResourceKey {
                stream: e.stream.clone(),
                type_id: e.header.type_id,
                ids: e.header.ids,
            }
            .to_string()
        })
        .collect();
    let label = match &options.new_name {
        Some(name) => format!(
            "Import model {name} from {} into {}",
            model.name, target.stream_name
        ),
        None => format!(
            "Import model {} over {}",
            model.name,
            key_of(target.meshes[0].0)
        ),
    };
    Ok(ImportPlan {
        summary,
        mesh: first,
        label,
        edits: if edits.is_empty() {
            Vec::new()
        } else {
            vec![(stream, edits)]
        },
        additions: new_edits,
    })
}

fn new_edit(stream: &str, resource: &NewResource, before: Option<ResourceKey>) -> NewResourceEdit {
    NewResourceEdit {
        stream: stream.to_owned(),
        header: resource.header,
        data: resource.data.clone(),
        storage: match resource.storage {
            Storage::Raw => StorageChoice::Raw,
            Storage::RefPack => StorageChoice::Refpack,
        },
        before,
    }
}

/// Material hashes of a mesh with the number of records using each, most used first.
fn material_counts(mesh: &MeshImage) -> Vec<u32> {
    let mut counts: Vec<(u32, usize)> = Vec::new();
    for r in &mesh.records {
        let h = r.u32_at(0x0C);
        match counts.iter_mut().find(|c| c.0 == h) {
            Some(c) => c.1 += 1,
            None => counts.push((h, 1)),
        }
    }
    counts.sort_by_key(|c| std::cmp::Reverse(c.1));
    counts.into_iter().map(|c| c.0).collect()
}

/// Maps the joints of skinned parts (indices into `model.joints`) to
/// bones of `skeleton` by name. Returns the joints whose bind position
/// differs from the skeleton's (by more than 1 mm).
pub fn bind_to_skeleton(
    model: &GltfModel,
    skeleton: &Skeleton,
    parts: &mut [PartGeometry],
) -> Result<Vec<String>> {
    let index_of: HashMap<u32, usize> = skeleton
        .bones
        .iter()
        .enumerate()
        .map(|(i, b)| (b.hash, i))
        .collect();
    let mut mapped: Vec<Option<usize>> = vec![None; model.joints.len()];
    let mut differ = Vec::new();
    for p in parts.iter_mut() {
        let Some(skin) = &mut p.skin else { continue };
        for (j, w) in skin.iter_mut() {
            for k in 0..4 {
                if w[k] <= 0.0 {
                    j[k] = 0;
                    continue;
                }
                let joint_index = j[k] as usize;
                let joint = model.joints.get(joint_index).ok_or_else(|| {
                    Error::InvalidArgument(format!(
                        "part `{}`: joint {joint_index} missing",
                        p.name
                    ))
                })?;
                let bone = match mapped[joint_index] {
                    Some(b) => b,
                    None => {
                        let b = *index_of.get(&joint_hash(&joint.name)).ok_or_else(|| {
                            Error::InvalidArgument(format!(
                                "bone `{}` is not in skeleton `{}`",
                                joint.name, skeleton.name
                            ))
                        })?;
                        // Joint positions in the bind pose: other programs
                        // turn bone axes (Blender), which does not move vertices.
                        let at = |m: &Mat4| {
                            crate::resource::rcb::translation(&crate::resource::rcb::invert_affine(
                                m,
                            ))
                        };
                        let (p, q) = (at(&joint.inverse_bind), at(&skeleton.bones[b].inverse_bind));
                        let delta = (0..3).map(|i| (p[i] - q[i]).abs()).fold(0.0f32, f32::max);
                        if delta > 1e-3 {
                            differ.push(joint.name.clone());
                        }
                        mapped[joint_index] = Some(b);
                        b
                    }
                };
                j[k] = bone as u16;
            }
        }
    }
    Ok(differ)
}

/// Finds the game skeleton for the glTF joints and maps the parts' joints to it.
fn resolve_skeleton(
    assets: &mut GameAssets,
    template: &MeshImage,
    model: &GltfModel,
    parts: &mut [PartGeometry],
    summary: &mut ImportSummary,
) -> Result<Skeleton> {
    // Joints that carry weight.
    let mut used = vec![false; model.joints.len()];
    for p in parts.iter() {
        for (j, w) in p.skin.iter().flatten() {
            for k in 0..4 {
                if w[k] > 0.0 {
                    used[j[k] as usize] = true;
                }
            }
        }
    }
    let hashes: Vec<u32> = model.joints.iter().map(|j| joint_hash(&j.name)).collect();
    let wanted: Vec<u32> = (0..hashes.len())
        .filter(|&i| used[i])
        .map(|i| hashes[i])
        .collect();
    let covers = |s: &Skeleton| wanted.iter().all(|h| s.bones.iter().any(|b| b.hash == *h));
    let from_template = if template.bones.is_empty() {
        None
    } else {
        crate::export::gltf::AssetSource::skeleton(assets, &template.bones)
    };
    let skeleton = match from_template.filter(|s| covers(s)) {
        Some(s) => s,
        None => {
            let candidates: Vec<&Skeleton> = assets
                .rigs()
                .iter()
                .flat_map(|r| &r.skeletons)
                .filter(|s| covers(s))
                .collect();
            match candidates.into_iter().max_by_key(|s| s.bones.len()) {
                Some(s) => s.clone(),
                None => {
                    // Report the bones no skeleton knows, or those missing from the best one.
                    let rigs = assets.rigs();
                    let best = rigs.iter().flat_map(|r| &r.skeletons).max_by_key(|s| {
                        wanted
                            .iter()
                            .filter(|h| s.bones.iter().any(|b| b.hash == **h))
                            .count()
                    });
                    let missing: Vec<String> = (0..hashes.len())
                        .filter(|&i| used[i])
                        .filter(|&i| {
                            best.is_none_or(|s| !s.bones.iter().any(|b| b.hash == hashes[i]))
                        })
                        .map(|i| model.joints[i].name.clone())
                        .collect();
                    return Err(Error::InvalidArgument(format!(
                        "no game skeleton has the bone(s) {}{}",
                        missing.join(", "),
                        best.map_or(String::new(), |s| format!(
                            " (closest: skeleton `{}` with {} bones; bone names are in data/gf2/bone_names.txt)",
                            s.name,
                            s.bones.len()
                        ))
                    )));
                }
            }
        }
    };
    let differ = bind_to_skeleton(model, &skeleton, parts)?;
    if !differ.is_empty() {
        summary.warnings.push(format!(
            "bind pose of {} joint(s) differs from skeleton `{}` ({}…); vertices are used as they are",
            differ.len(),
            skeleton.name,
            differ.iter().take(3).cloned().collect::<Vec<_>>().join(", ")
        ));
    }
    summary.skeleton = Some(format!(
        "{} ({} bones)",
        if skeleton.name.is_empty() {
            format!("{:08x}", skeleton.name_hash)
        } else {
            skeleton.name.clone()
        },
        skeleton.bones.len()
    ));
    summary.bones_mapped = used.iter().filter(|&&u| u).count();
    Ok(skeleton)
}

/// Resolves one glTF material: `(hash used by the records, new?, display
/// name, hash of the material it is based on)`.
///
/// The base is the target's material with the glTF material's name, else
/// any game material with that name (CRC-32 of the lower-case name; copied
/// unchanged into the target stream when no stream loaded with it has it),
/// else the target's most used material. The base is reused when the glTF
/// material brings no image or the same pixels; otherwise it is cloned with
/// new textures.
#[allow(clippy::too_many_arguments)]
fn resolve_material(
    game: &GameInstance,
    assets: &mut GameAssets,
    checker: &mut ReferenceChecker<'_>,
    stream: usize,
    template_materials: &[u32],
    gltf_material: Option<&crate::import::gltf::GltfMaterial>,
    used: &mut UsedIds,
    seed: &str,
    additions: &mut Vec<NewResource>,
    summary: &mut ImportSummary,
) -> Result<(u32, bool, String, u32)> {
    let mut known: Vec<(u32, Material)> = Vec::new();
    for &h in template_materials {
        if let Some(m) = crate::export::gltf::AssetSource::material(assets, h) {
            known.push((h, m));
        }
    }
    let mut named = gltf_material.and_then(|g| {
        known
            .iter()
            .find(|(_, m)| m.name.eq_ignore_ascii_case(&g.name))
            .cloned()
    });
    if named.is_none()
        && let Some(g) = gltf_material
    {
        // A material of another model, found by its name.
        let hash = crc32(g.name.to_ascii_lowercase().as_bytes());
        if let Some(m) = crate::export::gltf::AssetSource::material(assets, hash) {
            copy_if_unreachable(game, assets, checker, stream, hash, &m, additions, summary)?;
            named = Some((hash, m));
        }
    }
    let (base_hash, base) = match named.clone().or_else(|| known.first().cloned()) {
        Some(found) => found,
        None => {
            let hash = template_materials.first().copied().unwrap_or(0);
            summary.warnings.push(format!(
                "material {hash:08x} of the target is not in the game; records keep it"
            ));
            return Ok((hash, false, format!("{hash:08x}"), hash));
        }
    };
    let Some(gm) = gltf_material else {
        summary.materials_reused += 1;
        return Ok((base_hash, false, base.name.clone(), base_hash));
    };
    // Images that differ from the base material's textures.
    type Rgba = (usize, usize, Vec<u8>);
    let mut replace: Vec<(TextureRole, u32, Rgba)> = Vec::new();
    for (role, image) in [
        (TextureRole::Diffuse, gm.base_color.as_ref()),
        (TextureRole::Normal, gm.normal.as_ref()),
    ] {
        let Some(image) = image else { continue };
        let decoded = image.rgba()?;
        let Some(texture_hash) = base.texture(role) else {
            summary.warnings.push(format!(
                "material `{}` has no {role:?} texture; the image `{}` is not used",
                base.name, image.name
            ));
            continue;
        };
        let current =
            crate::export::gltf::AssetSource::texture(assets, texture_hash).and_then(|t| {
                let (w, h, mut rgba) = t.rgba(0).ok()?;
                if role == TextureRole::Normal && t.header.format() == tg4::Format::Dxt5Nm {
                    pixels::rebuild_normal_map(&mut rgba);
                }
                Some((w, h, rgba))
            });
        if named.is_some() && current.as_ref() == Some(&decoded) {
            continue;
        }
        replace.push((role, texture_hash, decoded));
    }
    if named.is_some() && replace.is_empty() {
        summary.materials_reused += 1;
        return Ok((base_hash, false, base.name.clone(), base_hash));
    }
    // A new material cloned from the base, with new textures.
    let (s, r) = assets.material_location(base_hash).ok_or_else(|| {
        Error::InvalidArgument(format!("material `{}` not found in the game", base.name))
    })?;
    let source = game.open_stream(s)?;
    let (mut clone, remap) = create::clone_resources(
        &source,
        &[r],
        used,
        &CloneOptions {
            seed: format!("{seed}:material"),
            rewrite_references: false,
            storage: None,
        },
    )?;
    let mut material = clone.remove(0);
    let new_hash = remap.names[&base_hash];
    let mut texture_remap = Remap::default();
    for (role, texture_hash, (w, h, rgba)) in replace {
        let (ts, tr) = assets.texture_location(texture_hash).ok_or_else(|| {
            Error::InvalidArgument(format!("texture {texture_hash:08x} not found in the game"))
        })?;
        let tstream = game.open_stream(ts)?;
        let (pair, tremap) = create::clone_resources(
            &tstream,
            &[tr],
            used,
            &CloneOptions {
                seed: format!("{seed}:{role:?}"),
                rewrite_references: false,
                storage: None,
            },
        )?;
        let header_res = pair
            .iter()
            .find(|p| p.header.type_id == TYPE_TG4H)
            .ok_or_else(|| Error::InvalidArgument("texture header missing".into()))?;
        let header = tg4::Tg4Header::parse(&header_res.data)?;
        let new_texture: Texture = texture::from_rgba(&header, w, h, rgba, None)?;
        for mut res in pair {
            res.data = if res.header.type_id == TYPE_TG4H {
                new_texture.header.to_bytes()?
            } else if res.header.type_id == TYPE_TG4D {
                new_texture.data.clone()
            } else {
                res.data
            };
            res.header.data_size = res.data.len() as u32;
            additions.push(res);
        }
        texture_remap.names.extend(tremap.names);
        summary.textures_imported += 1;
    }
    create::rewrite_references(TYPE_MATERIAL, &mut material.data, &texture_remap)?;
    // Textures the clone keeps must be loaded with it too.
    for (_, hash, _) in base.textures() {
        if !texture_remap.names.contains_key(&hash) {
            copy_texture_if_unreachable(game, assets, checker, stream, hash, additions)?;
        }
    }
    additions.push(material);
    summary.materials_new += 1;
    Ok((
        new_hash,
        true,
        format!("{} (new, from {})", gm.name, base.name),
        base_hash,
    ))
}

/// Copies a material, unchanged and under its own ids, into the target
/// stream when no stream loaded with it provides it; its textures likewise.
/// The game shares resources between streams this way (same GUID, same data).
#[allow(clippy::too_many_arguments)]
fn copy_if_unreachable(
    game: &GameInstance,
    assets: &mut GameAssets,
    checker: &mut ReferenceChecker<'_>,
    stream: usize,
    hash: u32,
    material: &Material,
    additions: &mut Vec<NewResource>,
    summary: &mut ImportSummary,
) -> Result<()> {
    for (_, texture, _) in material.textures() {
        copy_texture_if_unreachable(game, assets, checker, stream, texture, additions)?;
    }
    let added = additions.iter().any(|a| {
        a.header.type_id == TYPE_MATERIAL && a.data.get(8..12) == Some(&hash.to_le_bytes()[..])
    });
    if added || checker.provides(stream, &RefTarget::Material(hash))? {
        return Ok(());
    }
    let (s, r) = assets
        .material_location(hash)
        .ok_or_else(|| Error::InvalidArgument(format!("material {hash:08x} not found")))?;
    additions.push(copy_resource(game, s, r)?);
    summary.warnings.push(format!(
        "material `{}` comes from another model: copied into the target stream",
        material.name
    ));
    Ok(())
}

fn copy_texture_if_unreachable(
    game: &GameInstance,
    assets: &mut GameAssets,
    checker: &mut ReferenceChecker<'_>,
    stream: usize,
    hash: u32,
    additions: &mut Vec<NewResource>,
) -> Result<()> {
    let added = additions
        .iter()
        .any(|a| a.header.type_id == TYPE_TG4H && a.header.ids[3] == hash);
    if added || checker.provides(stream, &RefTarget::Texture(hash))? {
        return Ok(());
    }
    // Engine textures are in no stream; the game provides them.
    let Some((s, r)) = assets.texture_location(hash) else {
        return Ok(());
    };
    let file = game.open_stream(s)?;
    let pair = texture::pair_of(file.layout(), r)
        .ok_or_else(|| Error::InvalidArgument(format!("texture {hash:08x} has no data")))?;
    additions.push(copy_resource(game, s, pair.header)?);
    additions.push(copy_resource(game, s, pair.data)?);
    Ok(())
}

/// A resource of the game under its own ids and with its data.
fn copy_resource(game: &GameInstance, stream: usize, index: usize) -> Result<NewResource> {
    let file = game.open_stream(stream)?;
    let header = file.layout().resources[index].header;
    let data = file.resource_data(index)?;
    Ok(NewResource {
        header,
        storage: if file.layout().is_compressed(index) {
            Storage::RefPack
        } else {
            Storage::Raw
        },
        data,
        source: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::sdbm;
    use crate::import::gltf::{GltfJoint, GltfPart};
    use crate::resource::rcb::Bone;

    fn bone(name: &'static str, parent: Option<usize>) -> Bone {
        Bone {
            hash: sdbm(name.as_bytes()),
            name: Some(name),
            parent,
            bind_index: -1,
            bind_type: 1,
            inverse_bind: [
                1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
            ],
        }
    }

    fn model(joints: &[&str]) -> GltfModel {
        GltfModel {
            name: "m".into(),
            parts: vec![GltfPart {
                name: "p".into(),
                object: "m".into(),
                material: None,
                geometry: PartGeometry {
                    positions: vec![[0.0; 3]; 2],
                    skin: Some(vec![
                        ([0, 1, 0, 0], [0.5, 0.5, 0.0, 0.0]),
                        ([1, 0, 0, 0], [1.0, 0.0, 0.0, 0.0]),
                    ]),
                    ..Default::default()
                },
            }],
            joints: joints
                .iter()
                .map(|n| GltfJoint {
                    name: (*n).into(),
                    parent: None,
                    local: bone("m_root", None).inverse_bind,
                    inverse_bind: bone("m_root", None).inverse_bind,
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn joint_names_map_to_bone_hashes() {
        assert_eq!(joint_hash("m_pelvis"), sdbm(b"m_pelvis"));
        assert_eq!(joint_hash("M_PELVIS"), sdbm(b"m_pelvis"));
        // Bones without a known name are exported as their hash.
        assert_eq!(joint_hash("1234abcd"), 0x1234_ABCD);
        assert_eq!(joint_hash("_1234abcd"), 0x1234_ABCD);
    }

    #[test]
    fn skins_bind_by_bone_name() {
        let skeleton = Skeleton {
            name_hash: 0,
            name: "s".into(),
            bones: vec![
                bone("m_root", None),
                bone("m_pelvis", Some(0)),
                bone("l_wrist", Some(1)),
            ],
        };
        let gltf = model(&["l_wrist", "m_pelvis"]);
        let mut parts: Vec<PartGeometry> = gltf.parts.iter().map(|p| p.geometry.clone()).collect();
        let differ = bind_to_skeleton(&gltf, &skeleton, &mut parts).unwrap();
        assert!(differ.is_empty());
        let skin = parts[0].skin.as_ref().unwrap();
        assert_eq!(skin[0].0, [2, 1, 0, 0]);
        assert_eq!(skin[1].0, [1, 0, 0, 0]);

        let unknown = model(&["l_wrist", "tail_01"]);
        let mut parts: Vec<PartGeometry> =
            unknown.parts.iter().map(|p| p.geometry.clone()).collect();
        let error = bind_to_skeleton(&unknown, &skeleton, &mut parts).unwrap_err();
        assert!(error.to_string().contains("tail_01"), "{error}");
    }
}

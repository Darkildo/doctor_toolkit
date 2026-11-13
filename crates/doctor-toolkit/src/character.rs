//! Characters (and vehicles) assembled from a parts database (`partsdb`).
//!
//! [`PartsDb::assemble`] picks the variants, preset item and material
//! candidates of one set; [`Character::load`] finds the meshes of the chosen
//! parts by resource id and [`Character::to_glb`] writes them as one glTF
//! file: one node per part, skinned to one skeleton, with material swaps and
//! blender morphs applied.

use std::collections::HashMap;

use crate::export::gltf::{AssetSource, SceneModel};
use crate::game::{GameInstance, ResourceIndex};
use crate::hash::sdbm;
use crate::resource::mesh::{self, Model};
use crate::resource::partsdb::{Assembly, PartsDb, Selection};
use crate::{Error, Result};

const TYPE_MESH: u32 = sdbm(b"mesh");

/// Finds `mesh` resources by id, preferring one stream.
pub struct MeshLocator {
    by_id: HashMap<u32, Vec<(usize, usize)>>,
}

impl MeshLocator {
    pub fn new(index: &ResourceIndex) -> Self {
        let mut by_id: HashMap<u32, Vec<(usize, usize)>> = HashMap::new();
        for e in &index.entries {
            if e.header.type_id == TYPE_MESH {
                by_id
                    .entry(e.header.ids[3])
                    .or_default()
                    .push((e.stream as usize, e.resource as usize));
            }
        }
        Self { by_id }
    }

    /// `(stream, resource)` of mesh `id`: in `near` if it is there, else the first one.
    pub fn find(&self, id: u32, near: usize) -> Option<(usize, usize)> {
        let list = self.by_id.get(&id)?;
        list.iter()
            .find(|l| l.0 == near)
            .or_else(|| list.first())
            .copied()
    }
}

/// A part of an assembled character with its model.
#[derive(Debug, Clone)]
pub struct CharacterPart {
    /// Index into [`PartsDb::parts`].
    pub part: usize,
    pub name: String,
    pub mesh_id: u32,
    /// `(stream, resource)` of the mesh.
    pub location: (usize, usize),
    pub model: Model,
    /// Materials replaced for this part (`from → to`).
    pub materials: HashMap<u32, u32>,
}

/// A character ready to draw or export.
#[derive(Debug, Clone)]
pub struct Character {
    pub name: String,
    pub assembly: Assembly,
    pub parts: Vec<CharacterPart>,
    /// Morph target weights `(SDBM of the name, weight)`.
    pub morphs: Vec<(u32, f32)>,
    /// Parts whose mesh was not found: `(part name, mesh id)`.
    pub missing: Vec<(String, u32)>,
}

impl Character {
    /// Assembles set `set` of `db` and loads the meshes of level of detail
    /// (or damage state) `lod`, falling back to the first model a part has.
    pub fn load(
        game: &GameInstance,
        meshes: &MeshLocator,
        near_stream: usize,
        db: &PartsDb,
        set: usize,
        selection: &Selection,
        lod: usize,
    ) -> Result<Self> {
        Self::load_with(
            game,
            meshes,
            near_stream,
            db,
            set,
            selection,
            lod,
            &HashMap::new(),
        )
    }

    /// [`Self::load`] with meshes that are not in the game files yet (new
    /// in the workspace or about to be imported), by id. Their parts get the
    /// location `(usize::MAX, id)`.
    #[allow(clippy::too_many_arguments)]
    pub fn load_with(
        game: &GameInstance,
        meshes: &MeshLocator,
        near_stream: usize,
        db: &PartsDb,
        set: usize,
        selection: &Selection,
        lod: usize,
        extra: &HashMap<u32, Model>,
    ) -> Result<Self> {
        let assembly = db.assemble(set, selection)?;
        let mut parts = Vec::new();
        let mut missing = Vec::new();
        let mut open: HashMap<usize, crate::game::StreamFile> = HashMap::new();
        for &p in &assembly.parts {
            let part = &db.parts[p];
            let Some(id) = part
                .models
                .get(lod)
                .map(|h| h.0)
                .filter(|&h| h != 0)
                .or_else(|| part.mesh_ids().next())
            else {
                continue;
            };
            let (location, model) = if let Some(model) = extra.get(&id) {
                ((usize::MAX, id as usize), model.clone())
            } else {
                let Some(location) = meshes.find(id, near_stream) else {
                    missing.push((part.name.clone(), id));
                    continue;
                };
                let stream = match open.entry(location.0) {
                    std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
                    std::collections::hash_map::Entry::Vacant(e) => {
                        e.insert(game.open_stream(location.0)?)
                    }
                };
                (location, mesh::load(stream, location.1)?)
            };
            let mut materials = HashMap::new();
            for record in &model.records {
                let to = PartsDb::resolve_material(&assembly, p, record.material_hash);
                if to != record.material_hash {
                    materials.insert(record.material_hash, to);
                }
            }
            parts.push(CharacterPart {
                part: p,
                name: part.name.clone(),
                mesh_id: id,
                location,
                model,
                materials,
            });
        }
        let morphs = assembly
            .morphs
            .iter()
            .map(|(name, w)| (sdbm(name.to_ascii_lowercase().as_bytes()), *w))
            .collect();
        Ok(Self {
            name: db.sets[set].name.clone(),
            assembly,
            parts,
            morphs,
            missing,
        })
    }

    /// The whole character as binary glTF.
    pub fn to_glb(&self, assets: &mut dyn AssetSource) -> Result<Vec<u8>> {
        crate::export::gltf::write_glb(&self.to_scene(assets)?)
    }

    /// The whole character as a scene, for any model format
    /// ([`crate::scene::write`]).
    pub fn to_scene(&self, assets: &mut dyn AssetSource) -> Result<crate::scene::Scene> {
        if self.parts.is_empty() {
            return Err(Error::InvalidArgument(format!(
                "set `{}` has no part with a mesh",
                self.name
            )));
        }
        let models: Vec<SceneModel<'_>> = self
            .parts
            .iter()
            .map(|p| SceneModel {
                model: &p.model,
                name: p.name.clone(),
                materials: p.materials.clone(),
                morphs: self.morphs.clone(),
            })
            .collect();
        crate::scene::game::build(&models, None, &[], assets)
    }
}

/// A selection from `slot=variant` and `preset=item` texts (names or indices).
pub fn parse_selection(db: &PartsDb, set: usize, choices: &[String]) -> Result<Selection> {
    let mut selection = Selection::default();
    let set = db
        .sets
        .get(set)
        .ok_or_else(|| Error::InvalidArgument(format!("no set {set}")))?;
    for text in choices {
        let (key, value) = text
            .split_once('=')
            .ok_or_else(|| Error::InvalidArgument(format!("`{text}`: KEY=VALUE expected")))?;
        if let Some(slot) = db.slot_index(key) {
            let variant = db
                .variant_index(value)
                .or_else(|| value.parse().ok())
                .ok_or_else(|| Error::InvalidArgument(format!("no variant `{value}`")))?;
            selection.variants.insert(slot, variant);
        } else if let Some(k) = set
            .presets
            .iter()
            .position(|c| db.presets[c.preset].name.eq_ignore_ascii_case(key))
        {
            let choice = &set.presets[k];
            let items = &db.presets[choice.preset].items;
            let item = choice
                .items
                .iter()
                .position(|&i| items[i].name.eq_ignore_ascii_case(value))
                .or_else(|| value.parse().ok())
                .ok_or_else(|| Error::InvalidArgument(format!("no item `{value}`")))?;
            selection.items.insert(k, item);
        } else if key.eq_ignore_ascii_case("tag") {
            selection.tags.push(value.to_owned());
        } else {
            return Err(Error::InvalidArgument(format!(
                "`{key}` is neither a slot nor a preset of the set"
            )));
        }
    }
    Ok(selection)
}

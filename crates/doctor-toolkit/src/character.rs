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

/// Meshes, materials and textures a workspace adds (to streams of the game
/// or to new ones) and that are not in the game files yet: what a character
/// with a part made in the workspace is drawn with before `apply`
/// ([`Character::load_with`] takes the meshes).
#[derive(Debug, Clone, Default)]
pub struct WorkspaceAssets {
    /// Meshes by id.
    pub meshes: HashMap<u32, Model>,
    pub materials: Vec<crate::resource::material::Material>,
    pub textures: Vec<crate::resource::texture::Texture>,
}

impl WorkspaceAssets {
    /// Reads the resources `ws` adds; the ones that do not parse are left out.
    pub fn of(ws: &crate::workspace::Workspace) -> Self {
        use crate::resource::texture::{TYPE_TG4D, TYPE_TG4H, Texture};
        use crate::resource::tg4::Tg4Header;
        let mut out = Self::default();
        let data = |stream: &str, type_id: u32, ids: [u32; 4]| {
            let key = crate::workspace::ResourceKey {
                stream: stream.to_owned(),
                type_id,
                ids,
            };
            ws.data(&key).ok().flatten()
        };
        for change in ws.changes().filter(|c| c.is_added()) {
            let key = &change.key;
            let Ok(Some(bytes)) = ws.data(key) else {
                continue;
            };
            if key.type_id == TYPE_MESH {
                let volatile = data(
                    &key.stream,
                    crate::resource::mesh::TYPE_MESH_VOLATILE,
                    key.ids.map(|i| !i),
                )
                .unwrap_or_default();
                if let Ok(model) = Model::parse(&bytes, &volatile) {
                    out.meshes.insert(key.ids[3], model);
                }
            } else if key.type_id == crate::create::TYPE_MATERIAL {
                if let Ok(material) = crate::resource::material::Material::parse(&bytes) {
                    out.materials.push(material);
                }
            } else if key.type_id == TYPE_TG4H
                && let Ok(header) = Tg4Header::parse(&bytes)
            {
                let mut ids = key.ids;
                ids[1] = TYPE_TG4D;
                if let Some(pixels) = data(&key.stream, TYPE_TG4D, ids)
                    && pixels.len() == header.data_size as usize
                {
                    out.textures.push(Texture {
                        header,
                        data: pixels,
                    });
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::container::ustream::ResourceHeader;
    use crate::testing as mini;
    use crate::workspace::{NewResourceEdit, StorageChoice, Workspace};

    /// A mesh, a material and a texture added to a stream of the game are
    /// read from the workspace before anything is written.
    #[test]
    fn workspace_assets_are_what_the_workspace_adds() {
        let dir = tempfile::tempdir().unwrap();
        mini::write_game(dir.path()).unwrap();
        let game = GameInstance::open(dir.path()).unwrap();
        let mut ws = Workspace::for_game(&game, Some(&dir.path().join("ws"))).unwrap();
        assert!(WorkspaceAssets::of(&ws).meshes.is_empty());
        let (mesh, volatile) = mini::quad_mesh("hat", mini::MATERIAL_NAME).unwrap();
        let header = |tag: &[u8], ids: [u32; 4]| ResourceHeader {
            mem_policy: sdbm(b"default"),
            alignment: 0x10,
            type_id: sdbm(tag),
            ids,
            data_size: 0,
        };
        let add = |header: ResourceHeader, data: Vec<u8>| NewResourceEdit {
            stream: mini::LEVEL.into(),
            header,
            data,
            storage: StorageChoice::Raw,
            before: None,
        };
        let id = 0x4444;
        let tex = 0x77;
        let pixels = vec![0u8; mini::texture_header().data_size as usize];
        let mut additions = vec![
            add(header(b"mesh", [id; 4]), mesh),
            add(
                header(b"material", [0x5151; 4]),
                mini::material("felt", tex),
            ),
            add(
                header(b"tg4h", [0, sdbm(b"tg4h"), 7, tex]),
                mini::texture_header().to_bytes().unwrap(),
            ),
            add(header(b"tg4d", [0, sdbm(b"tg4d"), 7, tex]), pixels),
        ];
        if !volatile.is_empty() {
            additions.push(add(header(b"meshvolatile", [!id; 4]), volatile));
        }
        ws.add_resources(&game, "A hat", &additions).unwrap();
        let added = WorkspaceAssets::of(&ws);
        assert_eq!(added.meshes.keys().copied().collect::<Vec<_>>(), [id]);
        assert_eq!(added.materials.len(), 1);
        assert_eq!(added.textures.len(), 1);
        // The index of the game does not have the mesh: the locator misses it.
        let index = game.build_index(|_| {}).0;
        assert_eq!(MeshLocator::new(&index).find(id, 0), None);
    }
}

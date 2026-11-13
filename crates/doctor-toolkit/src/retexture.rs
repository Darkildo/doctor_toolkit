//! The textures a model draws with, for retexturing it.
//!
//! A model is a mesh, a metamodel (its meshes) or a character (the meshes
//! of the parts of a parts database set). Every draw record names a
//! material; the material names its textures by sampler, which gives each
//! texture its role ([`TextureRole`]). A texture can be in several streams:
//! [`TextureUse::copies`] lists every `tg4h` with its name hash, so that a
//! new image can replace all of them.

use std::collections::HashMap;

use crate::character::MeshLocator;
use crate::export::gltf::AssetSource;
use crate::game::{GameAssets, GameInstance, ResourceIndex};
use crate::resource::material::TextureRole;
use crate::resource::mesh::{self, TYPE_MESH};
use crate::resource::texture::{TYPE_TG4H, TexturePair, pair_of};
use crate::{Error, Result};

const TYPE_METAMODEL: u32 = crate::hash::sdbm(b"metamodel");

/// One texture of a model.
#[derive(Debug, Clone, PartialEq)]
pub struct TextureUse {
    /// Name hash (the last id of its `tg4h` and `tg4d`).
    pub hash: u32,
    /// Name in the texture header; empty when the texture is not in the game.
    pub name: String,
    pub role: TextureRole,
    /// Sampler of the first material that uses it, e.g. `g_BaseSampler`.
    pub sampler: String,
    /// Names of the materials that use it.
    pub materials: Vec<String>,
    /// `(stream, tg4h resource)` of every copy in the game.
    pub copies: Vec<(usize, usize)>,
    /// Size of the first copy.
    pub size: (u32, u32),
}

impl TextureUse {
    /// The header and data resources of each copy.
    pub fn pairs(&self, game: &GameInstance) -> Result<Vec<(usize, TexturePair)>> {
        self.copies
            .iter()
            .map(|&(stream, resource)| {
                let file = game.open_stream(stream)?;
                let pair = pair_of(file.layout(), resource).ok_or_else(|| {
                    Error::InvalidArgument(format!(
                        "texture {:08x} in {} has no data resource",
                        self.hash,
                        game.streams()[stream].rel_path
                    ))
                })?;
                Ok((stream, pair))
            })
            .collect()
    }
}

/// A short English name of a role.
pub fn role_name(role: TextureRole) -> &'static str {
    match role {
        TextureRole::Diffuse => "colour",
        TextureRole::Normal => "normal map",
        TextureRole::Specular => "specular",
        TextureRole::Gloss => "gloss",
        TextureRole::Detail => "detail",
        TextureRole::Mask => "mask",
        TextureRole::Other => "other",
    }
}

/// The meshes of a mesh or metamodel resource `(stream, resource)`, found in
/// the stream itself first.
pub fn meshes_of(
    game: &GameInstance,
    locator: &MeshLocator,
    at: (usize, usize),
) -> Result<Vec<(usize, usize)>> {
    let file = game.open_stream(at.0)?;
    let header = file
        .layout()
        .resources
        .get(at.1)
        .ok_or_else(|| Error::InvalidArgument(format!("no resource {}", at.1)))?
        .header;
    match header.type_id {
        TYPE_MESH => Ok(vec![at]),
        TYPE_METAMODEL => {
            let refs = crate::import::collision::mesh_references(&file.resource_data(at.1)?)?;
            let mut out = Vec::new();
            for ids in refs {
                if let Some(found) = locator.find(ids[3], at.0)
                    && !out.contains(&found)
                {
                    out.push(found);
                }
            }
            Ok(out)
        }
        other => Err(Error::InvalidArgument(format!(
            "{} is neither a mesh nor a metamodel",
            crate::resource::names::type_tags().display(other)
        ))),
    }
}

/// Every `tg4h` of the game by texture name hash.
pub fn texture_copies(index: &ResourceIndex) -> HashMap<u32, Vec<(usize, usize)>> {
    let mut out: HashMap<u32, Vec<(usize, usize)>> = HashMap::new();
    for e in &index.entries {
        if e.header.type_id == TYPE_TG4H {
            out.entry(e.header.ids[3])
                .or_default()
                .push((e.stream as usize, e.resource as usize));
        }
    }
    out
}

/// The textures of `meshes`, through their materials (after the material
/// replacements `swaps`, as a character set makes them), in order of first use.
pub fn textures_of(
    game: &GameInstance,
    index: &ResourceIndex,
    assets: &mut GameAssets,
    meshes: &[(usize, usize)],
    swaps: &HashMap<u32, u32>,
) -> Result<Vec<TextureUse>> {
    let copies = texture_copies(index);
    let mut materials: Vec<u32> = Vec::new();
    for &(stream, resource) in meshes {
        let file = game.open_stream(stream)?;
        let model = mesh::load(&file, resource)?;
        for record in &model.records {
            let hash = swaps
                .get(&record.material_hash)
                .copied()
                .unwrap_or(record.material_hash);
            if !materials.contains(&hash) {
                materials.push(hash);
            }
        }
    }
    let mut out: Vec<TextureUse> = Vec::new();
    for hash in materials {
        let Some(material) = assets.material(hash) else {
            continue;
        };
        for (role, texture, param) in material.textures() {
            if let Some(known) = out.iter_mut().find(|t| t.hash == texture) {
                if !known.materials.contains(&material.name) {
                    known.materials.push(material.name.clone());
                }
                continue;
            }
            let found = copies.get(&texture).cloned().unwrap_or_default();
            let (name, size) = match assets.texture(texture) {
                Some(t) => (
                    t.header.name.clone(),
                    (t.header.width.into(), t.header.height.into()),
                ),
                None => (String::new(), (0, 0)),
            };
            out.push(TextureUse {
                hash: texture,
                name,
                role,
                sampler: param.name.clone(),
                materials: vec![material.name.clone()],
                copies: found,
                size,
            });
        }
    }
    Ok(out)
}

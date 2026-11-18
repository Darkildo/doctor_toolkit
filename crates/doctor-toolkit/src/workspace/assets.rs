//! Materials and textures as the workspace has them: what a model of the
//! workspace is drawn and exported with before `apply`.

use std::collections::HashMap;

use crate::create::TYPE_MATERIAL;
use crate::export::gltf::AssetSource;
use crate::resource::material::Material;
use crate::resource::rcb::{Rig, Skeleton};
use crate::resource::texture::{TYPE_TG4D, TYPE_TG4H, Texture};
use crate::resource::tg4::Tg4Header;

use super::{ResourceKey, Workspace};

/// An [`AssetSource`] that gives the materials and textures the workspace
/// adds or changes (a clone made by a model import, a recoloured material,
/// an imported texture) and everything else from the game.
pub struct EditedAssets<'a> {
    workspace: &'a Workspace,
    game: &'a mut dyn AssetSource,
    /// Name hash of a material -> its key.
    materials: HashMap<u32, ResourceKey>,
    /// Name hash of a texture -> the keys of its header and its pixels.
    headers: HashMap<u32, ResourceKey>,
    pixels: HashMap<u32, ResourceKey>,
}

impl<'a> EditedAssets<'a> {
    pub fn new(workspace: &'a Workspace, game: &'a mut dyn AssetSource) -> Self {
        let mut materials = HashMap::new();
        let mut headers = HashMap::new();
        let mut pixels = HashMap::new();
        for change in workspace.changes() {
            let key = &change.key;
            match key.type_id {
                // The name hash is in the data (offset 8).
                TYPE_MATERIAL => {
                    if let Ok(Some(data)) = workspace.data(key)
                        && let Some(hash) = data.get(8..12)
                    {
                        let hash = u32::from_le_bytes(hash.try_into().expect("four bytes"));
                        materials.insert(hash, key.clone());
                    }
                }
                TYPE_TG4H => {
                    headers.insert(key.ids[3], key.clone());
                }
                TYPE_TG4D => {
                    pixels.insert(key.ids[3], key.clone());
                }
                _ => {}
            }
        }
        Self {
            workspace,
            game,
            materials,
            headers,
            pixels,
        }
    }

    /// Whether the workspace has anything a model could be drawn with.
    pub fn is_empty(&self) -> bool {
        self.materials.is_empty() && self.headers.is_empty() && self.pixels.is_empty()
    }

    fn data(&self, key: Option<&ResourceKey>) -> Option<Vec<u8>> {
        self.workspace.data(key?).ok().flatten()
    }
}

impl AssetSource for EditedAssets<'_> {
    fn material(&mut self, hash: u32) -> Option<Material> {
        self.data(self.materials.get(&hash))
            .and_then(|d| Material::parse(&d).ok())
            .or_else(|| self.game.material(hash))
    }

    fn texture(&mut self, hash: u32) -> Option<Texture> {
        let header = self
            .data(self.headers.get(&hash))
            .and_then(|d| Tg4Header::parse(&d).ok());
        let data = self.data(self.pixels.get(&hash));
        match (header, data) {
            (None, None) => self.game.texture(hash),
            (Some(header), Some(data)) => Some(Texture { header, data }),
            // Only one half is changed: the other one is the game's.
            (header, data) => {
                let game = self.game.texture(hash)?;
                Some(Texture {
                    header: header.unwrap_or(game.header),
                    data: data.unwrap_or(game.data),
                })
            }
        }
    }

    fn texture_version(&mut self, hash: u32) -> u32 {
        let mut version = 0u32;
        for key in [self.headers.get(&hash), self.pixels.get(&hash)]
            .into_iter()
            .flatten()
        {
            if let Some(change) = self.workspace.change(key) {
                version = version
                    .rotate_left(5)
                    .wrapping_add(crate::hash::crc32(change.payload.sha256.as_bytes()));
            }
        }
        match version {
            0 => self.game.texture_version(hash),
            v => v,
        }
    }

    fn skeleton(&mut self, bones: &[(u32, u32)]) -> Option<Skeleton> {
        self.game.skeleton(bones)
    }

    fn rigs(&mut self) -> &[Rig] {
        self.game.rigs()
    }

    fn rig_skeleton(&mut self, rig: Option<u32>, bones: &[(u32, u32)]) -> Option<Skeleton> {
        self.game.rig_skeleton(rig, bones)
    }
}

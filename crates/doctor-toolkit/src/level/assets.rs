//! Materials and textures of a level, found like the game finds them:
//! materials by the name hash in their data among the streams of the scope,
//! textures by name hash (the last id of `tg4h`), nearest stream first.

use std::collections::HashMap;

use rayon::prelude::*;

use crate::game::ResourceIndex;
use crate::resource::material::Material;
use crate::resource::texture::{TYPE_TG4H, Texture, pair_of};

use super::{LevelSource, Location, TYPE_MATERIAL};

/// Where the materials and textures of a level are.
#[derive(Debug, Default)]
pub struct LevelAssets {
    materials: HashMap<u32, Location>,
    textures: HashMap<u32, Location>,
}

impl LevelAssets {
    /// Reads the name hash of every material in the scope (in parallel) and
    /// maps every texture of the game.
    pub fn new(source: &LevelSource, index: &ResourceIndex) -> Self {
        let rank = |stream: usize| source.scope.rank(stream);
        let found: Vec<(u32, Location)> = source
            .scope
            .streams
            .par_iter()
            .flat_map_iter(|&s| {
                let Ok(stream) = source.stream(s) else {
                    return Vec::new();
                };
                source
                    .lookup
                    .in_stream(s, TYPE_MATERIAL)
                    .filter_map(|r| {
                        let data = stream.resource_data(r).ok()?;
                        let hash = crate::io::read_u32(&data, 8, crate::io::Endian::Little).ok()?;
                        Some((
                            hash,
                            Location {
                                stream: s,
                                resource: r,
                            },
                        ))
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        let mut materials: HashMap<u32, Location> = HashMap::new();
        for (hash, location) in found {
            let keep = materials
                .get(&hash)
                .is_some_and(|old| rank(old.stream) <= rank(location.stream));
            if !keep {
                materials.insert(hash, location);
            }
        }
        let mut textures: HashMap<u32, Location> = HashMap::new();
        for e in index
            .entries
            .iter()
            .filter(|e| e.header.type_id == TYPE_TG4H)
        {
            let location = Location {
                stream: e.stream as usize,
                resource: e.resource as usize,
            };
            let keep = textures
                .get(&e.header.ids[3])
                .is_some_and(|old| rank(old.stream) <= rank(location.stream));
            if !keep {
                textures.insert(e.header.ids[3], location);
            }
        }
        Self {
            materials,
            textures,
        }
    }

    pub fn material_count(&self) -> usize {
        self.materials.len()
    }

    pub fn material(&self, source: &LevelSource, hash: u32) -> Option<Material> {
        let l = self.materials.get(&hash)?;
        let data = source
            .stream(l.stream)
            .ok()?
            .resource_data(l.resource)
            .ok()?;
        Material::parse(&data).ok()
    }

    pub fn texture(&self, source: &LevelSource, hash: u32) -> Option<Texture> {
        let l = self.textures.get(&hash)?;
        let stream = source.stream(l.stream).ok()?;
        let pair = pair_of(stream.layout(), l.resource)?;
        Texture::load(stream.data(), stream.layout(), pair).ok()
    }
}

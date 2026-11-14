//! Loads a level on a worker thread: the level model first, then its meshes
//! and textures converted to renderer data in parallel and sent in batches,
//! so the scene fills in while the interface stays responsive.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use doctor_toolkit::game::{GameInstance, ResourceIndex};
use doctor_toolkit::level::{Found, Level, LevelAssets, LevelSource, Lookup, Overrides, Scope};
use doctor_toolkit::resource::material::TextureRole;
use doctor_toolkit::resource::mesh::Model;
use doctor_toolkit::resource::texture::Texture;
use doctor_toolkit::resource::tg4::{Format, Kind};
use rayon::prelude::*;

use crate::render::{MaterialData, MeshData, MeshKey, TextureData, TextureKey, Vertex};

/// Meshes converted per batch.
const BATCH: usize = 32;
/// Key namespaces: level meshes and textures do not mix with the model viewer's.
const MESH_TAG: u64 = 0x4C56_0000_0000_0000;
const OVERRIDE_TAG: u64 = 0x0001_0000_0000_0000;
const TEXTURE_TAG: u64 = 0x4C54_0000_0000_0000;

/// Renderer key of a level mesh.
pub fn mesh_key(id: u32, found: Option<Found>) -> MeshKey {
    let tag = match found {
        Some(Found::Override(..)) => MESH_TAG | OVERRIDE_TAG,
        _ => MESH_TAG,
    };
    tag | id as u64
}

pub fn texture_key(hash: u32) -> TextureKey {
    TEXTURE_TAG | hash as u64
}

/// Converted meshes and the textures they need (upload the textures first).
pub struct Batch {
    pub textures: Vec<(TextureKey, TextureData)>,
    pub meshes: Vec<(u32, MeshKey, Arc<MeshData>)>,
    pub failed: Vec<(u32, String)>,
}

pub enum Msg {
    Level {
        level: Box<Level>,
        assets: Arc<LevelAssets>,
        elapsed: Duration,
    },
    Batch(Batch),
    Done {
        elapsed: Duration,
        meshes: usize,
        textures: usize,
        triangles: usize,
    },
    Error(String),
}

/// A running load; dropping it cancels the work.
pub struct LoadJob {
    pub rx: Receiver<Msg>,
    pub done: Arc<AtomicUsize>,
    pub total: Arc<AtomicUsize>,
    cancel: Arc<AtomicBool>,
}

impl Drop for LoadJob {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// What a load needs from the application.
#[derive(Clone)]
pub struct LoadInput {
    pub game: Arc<GameInstance>,
    pub index: Arc<ResourceIndex>,
    pub lookup: Arc<Lookup>,
    pub parents: Arc<HashMap<String, Vec<String>>>,
    pub overrides: Overrides,
    /// Largest texture side uploaded; bigger textures start at a smaller mip.
    pub texture_limit: u32,
}

/// Loads the level of `streams` and then its meshes.
pub fn start(input: LoadInput, streams: Vec<usize>, ctx: egui::Context) -> LoadJob {
    let (tx, rx) = channel();
    let job = LoadJob {
        rx,
        done: Arc::default(),
        total: Arc::default(),
        cancel: Arc::default(),
    };
    let (done, total, cancel) = (job.done.clone(), job.total.clone(), job.cancel.clone());
    std::thread::spawn(move || {
        let started = Instant::now();
        let scope = Scope::new(&input.game, &input.parents, &streams);
        let source = Arc::new(LevelSource::new(
            input.game.clone(),
            input.lookup.clone(),
            input.overrides.clone(),
            scope,
        ));
        let level = match Level::load(source.clone(), &streams) {
            Ok(level) => level,
            Err(error) => {
                let _ = tx.send(Msg::Error(error.to_string()));
                ctx.request_repaint();
                return;
            }
        };
        let assets = Arc::new(LevelAssets::new(&source, &input.index));
        let meshes: Vec<(u32, Option<Found>)> = level
            .meshes
            .iter()
            .filter(|m| m.found.is_some())
            .map(|m| (m.id, m.found))
            .collect();
        let _ = tx.send(Msg::Level {
            level: Box::new(level),
            assets: assets.clone(),
            elapsed: started.elapsed(),
        });
        ctx.request_repaint();
        convert(
            &source,
            &assets,
            &meshes,
            input.texture_limit,
            &tx,
            &ctx,
            &done,
            &total,
            &cancel,
            started,
        );
    });
    job
}

/// Converts more meshes of a loaded level (after an edit added models).
pub fn more(
    source: Arc<LevelSource>,
    assets: Arc<LevelAssets>,
    meshes: Vec<(u32, Option<Found>)>,
    texture_limit: u32,
    ctx: egui::Context,
) -> LoadJob {
    let (tx, rx) = channel();
    let job = LoadJob {
        rx,
        done: Arc::default(),
        total: Arc::default(),
        cancel: Arc::default(),
    };
    let (done, total, cancel) = (job.done.clone(), job.total.clone(), job.cancel.clone());
    std::thread::spawn(move || {
        convert(
            &source,
            &assets,
            &meshes,
            texture_limit,
            &tx,
            &ctx,
            &done,
            &total,
            &cancel,
            Instant::now(),
        );
    });
    job
}

/// A mesh with the texture hashes it uses, or why it failed.
type Converted = (u32, Option<Found>, Result<(MeshData, Vec<u32>), String>);

#[allow(clippy::too_many_arguments)]
fn convert(
    source: &LevelSource,
    assets: &LevelAssets,
    meshes: &[(u32, Option<Found>)],
    texture_limit: u32,
    tx: &Sender<Msg>,
    ctx: &egui::Context,
    done: &AtomicUsize,
    total: &AtomicUsize,
    cancel: &AtomicBool,
    started: Instant,
) {
    total.store(meshes.len(), Ordering::Relaxed);
    let materials: Mutex<HashMap<u32, Option<u32>>> = Mutex::default();
    let mut sent_textures: HashSet<u32> = HashSet::new();
    let (mut mesh_count, mut texture_count, mut triangles) = (0, 0, 0);
    for chunk in meshes.chunks(BATCH) {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let converted: Vec<Converted> = chunk
            .par_iter()
            .map(|&(id, found)| {
                let result = source
                    .mesh(id)
                    .map_err(|e| e.to_string())
                    .and_then(|model| mesh_data(&model, source, assets, &materials));
                (id, found, result)
            })
            .collect();
        let mut wanted = Vec::new();
        for (_, _, r) in &converted {
            if let Ok((_, textures)) = r {
                for &t in textures {
                    if sent_textures.insert(t) {
                        wanted.push(t);
                    }
                }
            }
        }
        let textures: Vec<(TextureKey, TextureData)> = wanted
            .par_iter()
            .filter_map(|&hash| {
                let texture = assets.texture(source, hash)?;
                Some((texture_key(hash), texture_data(&texture, texture_limit)?))
            })
            .collect();
        let mut batch = Batch {
            textures,
            meshes: Vec::new(),
            failed: Vec::new(),
        };
        for (id, found, r) in converted {
            match r {
                Ok((data, _)) => {
                    triangles += data.triangle_count();
                    batch.meshes.push((id, mesh_key(id, found), Arc::new(data)));
                }
                Err(error) => batch.failed.push((id, error)),
            }
        }
        mesh_count += batch.meshes.len();
        texture_count += batch.textures.len();
        done.fetch_add(chunk.len(), Ordering::Relaxed);
        if tx.send(Msg::Batch(batch)).is_err() {
            return;
        }
        ctx.request_repaint();
    }
    let _ = tx.send(Msg::Done {
        elapsed: started.elapsed(),
        meshes: mesh_count,
        textures: texture_count,
        triangles,
    });
    ctx.request_repaint();
}

/// One part per texture (draw records of one texture are merged); returns
/// the texture hashes the parts use.
fn mesh_data(
    model: &Model,
    source: &LevelSource,
    assets: &LevelAssets,
    materials: &Mutex<HashMap<u32, Option<u32>>>,
) -> Result<(MeshData, Vec<u32>), String> {
    let mut data = MeshData::default();
    let mut textures = Vec::new();
    let mut groups: Vec<PartGroup> = Vec::new();
    for record in &model.records {
        let vertices = model.vertices(record).map_err(|e| e.to_string())?;
        let triangles = model.triangles(record).map_err(|e| e.to_string())?;
        let mut out: Vec<Vertex> = vertices
            .positions
            .iter()
            .enumerate()
            .map(|(i, &position)| Vertex {
                position,
                normal: vertices.normals.as_ref().map_or([0.0; 3], |n| n[i]),
                uv: vertices.uv0.as_ref().map_or([0.0; 2], |uv| uv[i]),
                color: vertices.colors.as_ref().map_or([255; 4], |c| c[i]),
                ..Vertex::default()
            })
            .collect();
        let indices: Vec<u32> = triangles.into_iter().flatten().collect();
        if vertices.normals.is_none() {
            MeshData::compute_normals(&mut out, &indices);
        }
        let cached = materials
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&record.material_hash)
            .copied();
        let diffuse = match cached {
            Some(d) => d,
            None => {
                let d = assets
                    .material(source, record.material_hash)
                    .and_then(|m| m.texture(TextureRole::Diffuse));
                materials
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(record.material_hash, d);
                d
            }
        };
        let texture = diffuse.filter(|_| vertices.uv0.is_some());
        if let Some(t) = texture
            && !textures.contains(&t)
        {
            textures.push(t);
        }
        // Records with the same texture become one part: one draw call.
        let group = match groups.iter().position(|(t, ..)| *t == texture) {
            Some(g) => g,
            None => {
                groups.push((texture, String::new(), Vec::new(), Vec::new()));
                groups.len() - 1
            }
        };
        let (_, names, vertices_out, indices_out) = &mut groups[group];
        if !names.is_empty() {
            names.push('+');
        }
        names.push_str(&record.name);
        let base = vertices_out.len() as u32;
        vertices_out.extend(out);
        indices_out.extend(indices.iter().map(|i| i + base));
    }
    for (texture, names, vertices, indices) in groups {
        let material = MaterialData {
            texture: texture.map(texture_key),
            color: if texture.is_some() {
                [1.0; 4]
            } else {
                MaterialData::default().color
            },
            surface: None,
        };
        data.push_part(names, vertices, &indices, material);
    }
    Ok((data, textures))
}

/// Records of one texture: the texture, part names, vertices, indices.
type PartGroup = (Option<u32>, String, Vec<Vertex>, Vec<u32>);

/// Mip levels of a flat texture from the first one not larger than `limit`
/// (also the textures of the Content Browser's previews).
pub(crate) fn texture_data(texture: &Texture, limit: u32) -> Option<TextureData> {
    let header = &texture.header;
    if header.kind() != Kind::Flat {
        return None;
    }
    let (w, h) = (header.width as u32, header.height as u32);
    let count = header.mip_count();
    let first = (0..count)
        .find(|&m| (w >> m).max(1).max((h >> m).max(1)) <= limit)
        .unwrap_or(count.saturating_sub(1));
    let mut mips = Vec::new();
    for mip in first..count {
        let Ok((_, _, mut rgba)) = texture.rgba_level(0, mip) else {
            break;
        };
        if header.format() == Format::Dxt5Nm {
            doctor_toolkit::resource::pixels::rebuild_normal_map(&mut rgba);
        }
        mips.push(rgba);
    }
    (!mips.is_empty()).then(|| TextureData {
        name: header.name.clone(),
        width: (w >> first).max(1),
        height: (h >> first).max(1),
        srgb: header.is_srgb(),
        mips,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_do_not_collide() {
        assert_ne!(mesh_key(5, None), 5);
        assert_ne!(
            mesh_key(5, None),
            mesh_key(5, Some(Found::Override(0, true)))
        );
        assert_ne!(texture_key(5), mesh_key(5, None));
    }
}

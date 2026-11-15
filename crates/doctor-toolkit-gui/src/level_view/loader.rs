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
use doctor_toolkit::resource::mesh::{Model, RENDER_FLAG_ALPHA_SORT, RENDER_FLAG_NO_DEPTH};
use doctor_toolkit::resource::texture::Texture;
use doctor_toolkit::resource::tg4::{Format, Kind};
use doctor_toolkit::resource::treemodel::TreeModel;
use doctor_toolkit::resource::world_shading::{WorldModel, WorldShading};
use rayon::prelude::*;

use crate::render::tree::{TreeMaterial, tree_mesh, tree_part_material};
use crate::render::{
    MaterialData, MeshData, MeshKey, Surface, SurfaceModel, TextureData, TextureKey, Vertex,
    WorldParams,
};

/// Meshes converted per batch.
const BATCH: usize = 32;
/// Key namespaces: level meshes and textures do not mix with the model viewer's.
const MESH_TAG: u64 = 0x4C56_0000_0000_0000;
const OVERRIDE_TAG: u64 = 0x0001_0000_0000_0000;
const TEXTURE_TAG: u64 = 0x4C54_0000_0000_0000;
const TREE_TAG: u64 = 0x4C52_0000_0000_0000;

/// Renderer key of a level mesh.
pub fn mesh_key(id: u32, found: Option<Found>) -> MeshKey {
    let tag = match found {
        Some(Found::Override(..)) => MESH_TAG | OVERRIDE_TAG,
        _ => MESH_TAG,
    };
    tag | id as u64
}

/// Renderer key of a level tree model.
pub fn tree_key(id: u32, found: Option<Found>) -> MeshKey {
    let tag = match found {
        Some(Found::Override(..)) => TREE_TAG | OVERRIDE_TAG,
        _ => TREE_TAG,
    };
    tag | id as u64
}

/// Something of a level the loader converts: a mesh or a tree model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Mesh(u32, Option<Found>),
    Tree(u32, Option<Found>),
}

impl Part {
    pub fn key(&self) -> MeshKey {
        match *self {
            Part::Mesh(id, found) => mesh_key(id, found),
            Part::Tree(id, found) => tree_key(id, found),
        }
    }

    fn id(&self) -> u32 {
        match *self {
            Part::Mesh(id, _) | Part::Tree(id, _) => id,
        }
    }
}

/// The meshes and trees of a level that are in the game files.
pub fn parts(level: &Level) -> Vec<Part> {
    let meshes = level.meshes.iter().filter(|m| m.found.is_some());
    let trees = level.trees.iter().filter(|m| m.found.is_some());
    meshes
        .map(|m| Part::Mesh(m.id, m.found))
        .chain(trees.map(|t| Part::Tree(t.id, t.found)))
        .collect()
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
        let meshes = parts(&level);
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
    meshes: Vec<Part>,
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
type Converted = (Part, Result<(MeshData, Vec<u32>), String>);

#[allow(clippy::too_many_arguments)]
fn convert(
    source: &LevelSource,
    assets: &LevelAssets,
    meshes: &[Part],
    texture_limit: u32,
    tx: &Sender<Msg>,
    ctx: &egui::Context,
    done: &AtomicUsize,
    total: &AtomicUsize,
    cancel: &AtomicBool,
    started: Instant,
) {
    total.store(meshes.len(), Ordering::Relaxed);
    let materials: Mutex<HashMap<u32, Arc<LevelMaterial>>> = Mutex::default();
    let tree_materials: Mutex<HashMap<u32, Option<TreeMaterial>>> = Mutex::default();
    let mut sent_textures: HashSet<u32> = HashSet::new();
    let (mut mesh_count, mut texture_count, mut triangles) = (0, 0, 0);
    for chunk in meshes.chunks(BATCH) {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let converted: Vec<Converted> = chunk
            .par_iter()
            .map(|&part| {
                let result = match part {
                    Part::Mesh(id, _) => source
                        .mesh(id)
                        .map_err(|e| e.to_string())
                        .and_then(|model| mesh_data(&model, source, assets, &materials)),
                    Part::Tree(id, _) => source
                        .tree(id)
                        .map_err(|e| e.to_string())
                        .map(|tree| tree_data(&tree, source, assets, &tree_materials)),
                };
                (part, result)
            })
            .collect();
        let mut wanted = Vec::new();
        for (_, r) in &converted {
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
        for (part, r) in converted {
            match r {
                Ok((data, _)) => {
                    triangles += data.triangle_count();
                    batch.meshes.push((part.id(), part.key(), Arc::new(data)));
                }
                Err(error) => batch.failed.push((part.id(), error)),
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

/// What the level view draws of a material: its base texture (all the
/// simple shading uses), whether it is the sky dome, and how the game shades
/// it (world surfaces, `docs/analysis/renderer.md` §10.7).
#[derive(Debug, Default)]
struct LevelMaterial {
    diffuse: Option<u32>,
    sky: bool,
    world: Option<WorldShading>,
}

impl LevelMaterial {
    fn of(material: Option<&doctor_toolkit::resource::material::Material>) -> Self {
        let Some(m) = material else {
            return Self::default();
        };
        Self {
            diffuse: m.texture(TextureRole::Diffuse),
            sky: m.shaders.iter().any(|s| is_sky(s.0)),
            world: WorldShading::from_material(m),
        }
    }
}

/// The renderer's surface of a world material.
fn world_surface(w: &WorldShading) -> Surface {
    let key = |t: Option<u32>| t.map(texture_key);
    let model = match w.model {
        WorldModel::Basic => SurfaceModel::WorldBasic,
        WorldModel::Building => SurfaceModel::Building,
        WorldModel::Detail => SurfaceModel::Detail,
        WorldModel::Panorama => SurfaceModel::Panorama,
        WorldModel::Overlay => SurfaceModel::Overlay,
        WorldModel::Window => SurfaceModel::Window,
        WorldModel::Glass => SurfaceModel::Glass,
        WorldModel::Water => SurfaceModel::Water,
    };
    let identity = [1.0, 1.0, 0.0, 0.0];
    // Windows bind their front map, panoramas their tint mask, as the
    // detail texture at the base UV.
    let (detail, detail_uv) = match w.model {
        WorldModel::Window | WorldModel::Glass => (w.base, identity),
        WorldModel::Panorama => (w.detail, identity),
        _ => (w.detail, w.detail_uv),
    };
    let mut surface = Surface {
        model,
        blend: w.model == WorldModel::Overlay,
        normal: key(w.normal),
        gloss: key(w.gloss),
        detail_normal: key(w.detail_normal),
        detail: key(detail),
        mask: key(w.mask),
        exponent_map: key(w.exponent_map),
        tint: w.tint,
        detail_uv,
        detail_mask_uv: w.mask_uv,
        specular: w.specular,
        base_tint: w.base_tint,
        detail_tint: w.detail_tint,
        world: WorldParams {
            mask_scale: w.mask_scale,
            normal_scale: w.normal_scale,
            exponent: w.exponent,
            overlay: w.overlay,
            reflection: w.reflection,
            specular_masked: w.specular_masked,
            specular_lit: w.specular_lit,
            reflection_masked: w.reflection_masked,
            alpha_test: w.alpha_test,
            glow: w.glow,
        },
        ..Surface::default()
    };
    settle(&mut surface, w.model);
    surface
}

/// Clears what the world shader does not use for this surface (`scene.wgsl`,
/// `world_fragment`), so that records that look the same share a part and a
/// bind group: each part is a draw call and each bind group change costs
/// CPU time (`docs/analysis/editor-performance.md` §10).
fn settle(s: &mut Surface, model: WorldModel) {
    let identity = [1.0, 1.0, 0.0, 0.0];
    let w = &mut s.world;
    // A highlight masked by a missing (black) gloss map is none.
    if w.specular_masked && s.gloss.is_none() {
        s.specular = [0.0; 3];
    }
    if s.specular == [0.0; 3] {
        w.specular_masked = false;
        w.specular_lit = false;
        w.exponent = 0.0;
        s.exponent_map = None;
    }
    if w.reflection_masked && s.gloss.is_none() {
        w.reflection = [0.0; 2];
    }
    if w.reflection == [0.0; 2] {
        w.reflection_masked = false;
    }
    let gloss_used = (s.specular != [0.0; 3] && (w.specular_masked || s.exponent_map.is_some()))
        || w.reflection_masked;
    if !gloss_used {
        s.gloss = None;
        s.exponent_map = None;
    }
    if s.normal.is_none() && s.detail_normal.is_none() {
        w.normal_scale = 1.0;
    }
    if s.detail.is_none() && s.detail_normal.is_none() {
        s.detail_uv = identity;
    }
    if s.mask.is_none() {
        s.detail_mask_uv = identity;
    }
    // Only the two-layer model reads the detail tint and the mask; only the
    // tinted models the base tint; only the overlay its strength.
    if model != WorldModel::Detail {
        s.detail_tint = [1.0; 4];
        s.mask = None;
        s.detail_mask_uv = identity;
        w.mask_scale = 1.0;
    }
    if !matches!(
        model,
        WorldModel::Building | WorldModel::Detail | WorldModel::Panorama | WorldModel::Water
    ) {
        s.base_tint = [1.0; 4];
    }
    // The world shader reads only the alpha of `g_Tint`.
    s.tint = [1.0, 1.0, 1.0, s.tint[3]];
}

/// One part per material as the renderer sees it (draw records that look
/// the same are merged); returns the texture hashes the parts use.
fn mesh_data(
    model: &Model,
    source: &LevelSource,
    assets: &LevelAssets,
    materials: &Mutex<HashMap<u32, Arc<LevelMaterial>>>,
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
            .cloned();
        let material = match cached {
            Some(m) => m,
            None => {
                let m = Arc::new(LevelMaterial::of(
                    assets.material(source, record.material_hash).as_ref(),
                ));
                materials
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(record.material_hash, m.clone());
                m
            }
        };
        let has_uv = vertices.uv0.is_some();
        let texture = material.diffuse.filter(|_| has_uv);
        // Overlay layers (decals, `*_ov_N`) and sorted glass are blended
        // over what is drawn before them (`docs/formats/geometry.md`).
        let layer = record.overlay_id > 0
            || record.render_flags & (RENDER_FLAG_NO_DEPTH | RENDER_FLAG_ALPHA_SORT) != 0;
        let surface = material.world.as_ref().map(|w| {
            let mut s = world_surface(w);
            s.blend |= layer;
            s
        });
        let used = texture.into_iter().chain(
            material
                .world
                .iter()
                .flat_map(|w| w.textures())
                .filter(|_| has_uv),
        );
        for t in used {
            if !textures.contains(&t) {
                textures.push(t);
            }
        }
        let material = MaterialData {
            texture: texture.map(texture_key),
            color: if texture.is_some() {
                [1.0; 4]
            } else {
                MaterialData::default().color
            },
            surface: surface.map(Box::new),
            sky: material.sky,
        };
        // Records that look the same become one part: one draw call.
        let group = match groups.iter().position(|(m, ..)| *m == material) {
            Some(g) => g,
            None => {
                groups.push((material, String::new(), Vec::new(), Vec::new()));
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
    // Parts of one base texture next to each other, in the order the
    // texture first appears: the simple look draws them with one material,
    // in the order it drew the records of a texture before world surfaces.
    let first: Vec<usize> = groups
        .iter()
        .map(|(m, ..)| {
            groups
                .iter()
                .position(|(o, ..)| o.texture == m.texture && o.sky == m.sky)
                .unwrap_or(0)
        })
        .collect();
    let mut order: Vec<usize> = (0..groups.len()).collect();
    order.sort_by_key(|&i| first[i]);
    let mut groups: Vec<Option<PartGroup>> = groups.into_iter().map(Some).collect();
    for i in order {
        if let Some((material, names, vertices, indices)) = groups[i].take() {
            data.push_part(names, vertices, &indices, material);
        }
    }
    data.layer_coplanar_parts();
    Ok((data, textures))
}

/// A tree model with the texture hashes it uses.
fn tree_data(
    tree: &TreeModel,
    source: &LevelSource,
    assets: &LevelAssets,
    materials: &Mutex<HashMap<u32, Option<TreeMaterial>>>,
) -> (MeshData, Vec<u32>) {
    let mut textures = Vec::new();
    let data = tree_mesh(tree, |hash| {
        let cached = materials
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&hash)
            .copied();
        let material = cached.unwrap_or_else(|| {
            let m = assets.material(source, hash).as_ref().map(TreeMaterial::of);
            materials
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(hash, m);
            m
        });
        let texture = material.and_then(|m| m.texture);
        if let Some(t) = texture
            && !textures.contains(&t)
        {
            textures.push(t);
        }
        tree_part_material(
            texture.map(texture_key),
            material.map_or([1.0; 4], |m| m.tint),
        )
    });
    (data, textures)
}

/// Records drawn alike: material, names of the records, vertices, indices.
type PartGroup = (MaterialData, String, Vec<Vertex>, Vec<u32>);

/// Whether a material's colour pass is one of the sky's shaders (`sky_dome`,
/// `sky_clouds`): shading as in the game draws its own sky instead.
fn is_sky(shader: u32) -> bool {
    let hash = |name: &str| doctor_toolkit::hash::crc32(name.as_bytes());
    shader == hash("sky_dome") || shader == hash("sky_clouds")
}

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
    fn world_surfaces_bind_their_layers() {
        use doctor_toolkit::hash::crc32;
        use doctor_toolkit::resource::material::{Material, Param, ParamValue};
        let param = |name: &str, value| Param {
            hash: crc32(name.as_bytes()),
            name: name.into(),
            source: String::new(),
            value,
        };
        let material = |lit: &str, params: Vec<Param>| Material {
            version: 19,
            name: "test".into(),
            name_hash: 0,
            flags: 0,
            shaders: [
                (0, 0, 0),
                (crc32(lit.as_bytes()), 0, params.len() as u8),
                (0, 0, 0),
                (0, 0, 0),
            ],
            shadow_shader: 0,
            params,
        };
        let window = material(
            "windowglow",
            vec![param("g_FrontMap", ParamValue::Texture(9))],
        );
        let level = LevelMaterial::of(Some(&window));
        let surface = world_surface(level.world.as_ref().unwrap());
        assert_eq!(surface.model, SurfaceModel::Window);
        assert_eq!(level.diffuse, None, "the simple look keeps no texture");
        assert_eq!(surface.detail, Some(texture_key(9)), "front map");
        assert_eq!(surface.detail_uv, [1.0, 1.0, 0.0, 0.0]);

        let overlay = material("gf_overlay_vertex_ao", Vec::new());
        let surface = world_surface(LevelMaterial::of(Some(&overlay)).world.as_ref().unwrap());
        assert!(surface.blend && surface.model == SurfaceModel::Overlay);
        let trunk = material("treetrunk", Vec::new());
        assert!(LevelMaterial::of(Some(&trunk)).world.is_none());
    }

    #[test]
    fn keys_do_not_collide() {
        assert_ne!(mesh_key(5, None), 5);
        assert_ne!(
            mesh_key(5, None),
            mesh_key(5, Some(Found::Override(0, true)))
        );
        assert_ne!(texture_key(5), mesh_key(5, None));
        assert_ne!(tree_key(5, None), mesh_key(5, None));
        assert_ne!(tree_key(5, None), texture_key(5));
    }
}

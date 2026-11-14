//! Making preview images on a worker thread.
//!
//! - Textures: the smallest mip level not smaller than [`SIZE`], decoded
//!   and halved down to it, opaque.
//! - Models: a metamodel expanded into its meshes (the finest level of
//!   detail; meshes found from the stream's scope, the workspace's first),
//!   or one mesh, or one tree model with the trees of a metamodel
//!   (`treemodel`), converted as the model viewer converts them (game-like
//!   surfaces: tint masks, skin, hair; textures at most [`SIZE`] wide) and
//!   drawn by the scene renderer of the model and level views into an
//!   offscreen target of [`SIZE`]² on a transparent background, then read
//!   back.

use std::collections::HashMap;
use std::sync::Mutex;

use doctor_toolkit::level::model::{self, Marker, Piece};
use doctor_toolkit::level::{IDENTITY, TYPE_METAMODEL, TYPE_TREE};
use doctor_toolkit::resource::texture::{self, TYPE_TG4D, TYPE_TG4H, Texture};
use doctor_toolkit::resource::tg4::{Format, Tg4Header};
use doctor_toolkit::workspace::ResourceKey;

use super::preview::{DataHash, Kind, PreviewJob, PreviewSource};
use crate::level_view::scene::flatten;
use crate::render::{GpuContext, Instance, OrbitCamera, Scene, SceneRenderer, Viewport, math};

/// Width and height of a preview.
pub const SIZE: u32 = 256;
/// Meshes drawn at most for one model.
const MAX_MESHES: usize = 48;
/// Textures kept by the preview renderer before it starts afresh.
const TEXTURE_BUDGET: usize = 384 << 20;
/// Mesh keys of previews: apart from the views' keys.
const MESH_TAG: u64 = 0x5052_0000_0000_0000;

/// A preview image: RGBA, not premultiplied, with the bounds of a model.
#[derive(Debug, Clone, PartialEq)]
pub struct Thumbnail {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub bounds: Option<[[f32; 3]; 2]>,
}

/// The renderer of the previews: on the window's device, or on one of its
/// own made on first use when the window has none.
#[derive(Default)]
pub struct GpuSlot {
    context: Option<GpuContext>,
    renderer: Option<SceneRenderer>,
    /// Look for a device when the window gave none.
    headless: bool,
    tried: bool,
    next_key: u64,
}

impl GpuSlot {
    pub fn new(context: Option<GpuContext>, headless: bool) -> Self {
        Self {
            context,
            headless,
            ..Self::default()
        }
    }

    fn renderer(&mut self) -> Option<&mut SceneRenderer> {
        if self.renderer.is_none() {
            if self.context.is_none() && self.headless && !self.tried {
                self.tried = true;
                self.context = GpuContext::headless();
            }
            let mut renderer = SceneRenderer::new(self.context.clone()?);
            renderer.set_mesh_budget(64);
            self.renderer = Some(renderer);
        }
        self.renderer.as_mut()
    }
}

/// The hash of what a preview shows: the stored bytes of the resource (and
/// of a texture's data), or the workspace's version.
pub fn data_hash(source: &PreviewSource, job: &PreviewJob) -> Result<u64, String> {
    let mut h = DataHash::default();
    if let Some(version) = &job.id.version {
        h.write(version.as_bytes());
        return Ok(h.finish());
    }
    let at = job.at.ok_or("no data")?;
    let stream = source.stream(at.stream)?;
    let layout = stream.layout();
    let mut resources = vec![at.resource];
    if job.kind == Kind::Texture
        && let Some(pair) = texture::pair_of(layout, at.resource)
    {
        resources.push(pair.data);
    }
    for r in resources {
        h.word(
            layout
                .resources
                .get(r)
                .ok_or("no such resource")?
                .header
                .data_size as u64,
        );
        for block in layout.data_blocks(r) {
            h.write(&stream.data()[block.body()]);
        }
    }
    Ok(h.finish())
}

/// The key of a texture's data part in the workspace.
fn data_key(source: &PreviewSource, header: &ResourceKey) -> Option<ResourceKey> {
    source
        .ws
        .keys()
        .find(|k| k.stream == header.stream && k.type_id == TYPE_TG4D && k.ids[3] == header.ids[3])
        .cloned()
}

/// A texture as the workspace or the game has it.
fn load_texture(source: &PreviewSource, job: &PreviewJob) -> Result<Texture, String> {
    let key = &job.id.key;
    let ws_header = source.ws_data(key);
    let ws_data = data_key(source, key).and_then(|k| source.ws_data(&k));
    let from_game = || -> Result<Texture, String> {
        let at = job.at.ok_or("not in the game files")?;
        let stream = source.stream(at.stream)?;
        let pair = texture::pair_of(stream.layout(), at.resource).ok_or("no texture data")?;
        Texture::load(stream.data(), stream.layout(), pair).map_err(|e| e.to_string())
    };
    if ws_header.is_none() && ws_data.is_none() {
        return from_game();
    }
    let game = from_game().ok();
    let header = match ws_header {
        Some(bytes) => Tg4Header::parse(&bytes).map_err(|e| e.to_string())?,
        None => game.as_ref().ok_or("no texture header")?.header.clone(),
    };
    let data = match ws_data {
        Some(bytes) => bytes.to_vec(),
        None => game.ok_or("no texture data")?.data,
    };
    Ok(Texture { header, data })
}

/// The preview of a texture.
pub fn texture(source: &PreviewSource, job: &PreviewJob) -> Result<Thumbnail, String> {
    debug_assert_eq!(job.id.key.type_id, TYPE_TG4H);
    let texture = load_texture(source, job)?;
    let header = &texture.header;
    let (w, h) = (header.width as u32, header.height as u32);
    let count = header.mip_count();
    // The smallest level that is still at least SIZE wide (or the first).
    let mip = (0..count)
        .rev()
        .find(|&m| (w >> m).max(h >> m) >= SIZE)
        .unwrap_or(0);
    let (mut width, mut height, mut rgba) = texture
        .rgba_level(0, mip)
        .or_else(|_| texture.rgba_level(0, 0))
        .map_err(|e| e.to_string())?;
    if header.format() == Format::Dxt5Nm {
        doctor_toolkit::resource::pixels::rebuild_normal_map(&mut rgba);
    }
    while width.max(height) > SIZE as usize {
        (width, height, rgba) = doctor_toolkit::resource::pixels::downsample(width, height, &rgba);
    }
    for p in rgba.chunks_exact_mut(4) {
        p[3] = 255;
    }
    Ok(Thumbnail {
        width: width as u32,
        height: height as u32,
        rgba,
        bounds: None,
    })
}

/// The meshes of a model at its finest level of detail, and its trees:
/// a metamodel expanded, or one mesh, or one tree model.
fn pieces(context: &super::preview::LevelContext, job: &PreviewJob) -> (Vec<Piece>, Vec<Marker>) {
    let id = job.id.key.ids[3];
    match job.id.key.type_id {
        TYPE_METAMODEL => {}
        TYPE_TREE => {
            return (
                Vec::new(),
                vec![Marker {
                    resource: id,
                    matrix: IDENTITY,
                }],
            );
        }
        _ => {
            return (
                vec![Piece {
                    mesh: id,
                    matrix: IDENTITY,
                    lod: None,
                }],
                Vec::new(),
            );
        }
    }
    let desc = model::expand(id, &mut &context.source);
    let nearest = desc
        .pieces
        .iter()
        .map(|p| p.lod.map_or(0.0, |l| l[0]))
        .fold(f32::INFINITY, f32::min);
    let pieces = desc
        .pieces
        .into_iter()
        .filter(|p| p.lod.map_or(0.0, |l| l[0]) <= nearest)
        .collect();
    (pieces, desc.trees)
}

/// The preview of a model (a metamodel, a mesh or a tree model).
pub fn model(
    source: &PreviewSource,
    job: &PreviewJob,
    gpu: &Mutex<GpuSlot>,
) -> Result<Thumbnail, String> {
    let key = &job.id.key;
    let stream = job
        .at
        .map(|l| l.stream)
        .or_else(|| source.game.find_stream(&key.stream))
        .ok_or("the stream is not in the game files")?;
    let context = source.level(stream);
    let (pieces, trees) = pieces(&context, job);
    if pieces.is_empty() && trees.is_empty() {
        return Err("no meshes".to_owned());
    }
    // Meshes parsed on this thread, converted and uploaded as the model
    // viewer does it (materials and textures found across the game).
    let mut models = Vec::new();
    let mut failed = None;
    for piece in &pieces {
        if models.iter().any(|(id, _)| *id == piece.mesh) || models.len() >= MAX_MESHES {
            continue;
        }
        match context.source.mesh(piece.mesh) {
            Ok(model) => models.push((piece.mesh, model)),
            Err(error) => failed = Some(error.to_string()),
        }
    }
    let mut tree_models = Vec::new();
    for tree in &trees {
        if tree_models.iter().any(|(id, _)| *id == tree.resource) || tree_models.len() >= MAX_MESHES
        {
            continue;
        }
        match context.source.tree(tree.resource) {
            Ok(model) => tree_models.push((tree.resource, model)),
            Err(error) => failed = Some(error.to_string()),
        }
    }
    if models.is_empty() && tree_models.is_empty() {
        return Err(failed.unwrap_or_else(|| "no meshes".to_owned()));
    }
    let mut slot = gpu.lock().unwrap_or_else(|e| e.into_inner());
    let first_key = slot.next_key;
    slot.next_key += (models.len() + tree_models.len()) as u64;
    let renderer = slot.renderer().ok_or("no GPU for previews")?;
    let mut assets = source.assets();
    let mut keys = HashMap::new();
    for (i, (id, model)) in models.iter().enumerate() {
        let key = MESH_TAG | (first_key + i as u64);
        let options = crate::model_view::LoadOptions::default();
        match crate::model_view::load_limited(
            model,
            key,
            &mut *assets,
            renderer,
            &options,
            Some(SIZE),
        ) {
            Ok(loaded) => {
                keys.insert(*id, (key, loaded.mesh.bounds()));
            }
            Err(error) => failed = Some(error.to_string()),
        }
    }
    let mut tree_keys = HashMap::new();
    for (i, (id, tree)) in tree_models.iter().enumerate() {
        let key = MESH_TAG | (first_key + (models.len() + i) as u64);
        let loaded =
            crate::model_view::load_tree(tree, "", key, &mut *assets, renderer, Some(SIZE));
        tree_keys.insert(*id, (key, loaded.mesh.bounds()));
    }
    drop(assets);
    if keys.is_empty() && tree_keys.is_empty() {
        return Err(failed.unwrap_or_else(|| "no meshes".to_owned()));
    }
    let mut scene = Scene::default();
    let mut bounds = math::Aabb::EMPTY;
    let placed = pieces
        .iter()
        .filter_map(|p| Some((keys.get(&p.mesh)?, &p.matrix)))
        .chain(
            trees
                .iter()
                .filter_map(|t| Some((tree_keys.get(&t.resource)?, &t.matrix))),
        );
    for ((key, mesh_bounds), matrix) in placed {
        let transform = flatten(matrix);
        bounds.union(&mesh_bounds.transformed(&transform));
        scene.instances.push(Instance::new(*key, transform));
    }
    if bounds.is_empty() {
        return Err("empty model".to_owned());
    }
    scene.options.background = [0.0; 4];
    scene.options.frustum_culling = false;
    let camera = OrbitCamera::framing(&bounds);
    let view = camera.view(1.0);
    let mut viewport = Viewport::new();
    renderer.render(&mut viewport, [SIZE, SIZE], &scene, &view);
    let (width, height, mut rgba) = renderer
        .read_pixels(&viewport)
        .ok_or("cannot read the image back")?;
    if renderer.texture_bytes() > TEXTURE_BUDGET {
        slot.renderer = None;
    }
    drop(slot);
    unpremultiply(&mut rgba);
    Ok(Thumbnail {
        width,
        height,
        rgba,
        bounds: Some([bounds.min, bounds.max]),
    })
}

/// The edges of the transparent background come out of the multisampled
/// target premultiplied by their coverage.
fn unpremultiply(rgba: &mut [u8]) {
    for p in rgba.chunks_exact_mut(4) {
        let a = p[3] as u32;
        if a > 0 && a < 255 {
            for c in &mut p[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_are_unpremultiplied() {
        let mut px = vec![50, 100, 0, 128, 10, 20, 30, 255, 0, 0, 0, 0];
        unpremultiply(&mut px);
        assert_eq!(px, [100, 199, 0, 128, 10, 20, 30, 255, 0, 0, 0, 0]);
    }
}

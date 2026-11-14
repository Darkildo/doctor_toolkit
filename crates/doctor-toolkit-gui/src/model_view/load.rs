//! Conversion of game models into renderer data, with the names the viewer shows.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use doctor_toolkit::export::gltf::{AssetSource, morphed_positions, vertex_skin};
use doctor_toolkit::game::StreamFile;
use doctor_toolkit::resource::material::TextureRole;
use doctor_toolkit::resource::mesh::{self, FORMAT_NAMES, Model};
use doctor_toolkit::resource::rcb::{Skeleton, translation};
use doctor_toolkit::resource::shading::{Blend, Family, Shading};
use doctor_toolkit::resource::texture::Texture;
use doctor_toolkit::resource::tg4::{Format, Kind};

use crate::render::{
    MaterialData, MeshData, MeshKey, SceneRenderer, Surface, SurfaceModel, TextureData, Vertex,
};

/// What the viewer shows about one part (draw record).
#[derive(Debug, Clone, PartialEq)]
pub struct PartInfo {
    pub name: String,
    /// The material drawn (after swaps).
    pub material_hash: u32,
    /// The material the draw record names, when a swap replaced it.
    pub swapped_from: Option<u32>,
    /// Shading inputs of the material, for the material inspector.
    pub shading: Option<Shading>,
    /// Shader slot names of the material (depth, lit, normals, low quality) and shadow shader.
    pub shaders: Vec<String>,
    /// Every parameter once: `(name, value, source)`.
    pub params: Vec<(String, String, String)>,
    pub material: Option<String>,
    /// `(role, texture name or hash)` of every texture of the material.
    pub textures: Vec<(String, String)>,
    pub format: &'static str,
    pub vertices: usize,
    pub triangles: usize,
    /// Maximum bone influences per vertex; `None` for static geometry.
    pub influences: Option<u8>,
    pub rigid_bone: Option<u8>,
    /// Skeleton bones of the palette, in palette order.
    pub palette: Vec<u8>,
}

/// A bone of the matched skeleton in the bind pose.
#[derive(Debug, Clone, PartialEq)]
pub struct BoneInfo {
    pub name: String,
    pub parent: Option<usize>,
    pub position: [f32; 3],
}

/// A model prepared for the renderer.
pub struct LoadedModel {
    pub key: MeshKey,
    pub name: String,
    pub mesh: Arc<MeshData>,
    pub parts: Vec<PartInfo>,
    pub skeleton_name: Option<String>,
    pub bones: Vec<BoneInfo>,
    /// Entries of the mesh bone table.
    pub bone_table: usize,
    pub textured_materials: usize,
}

/// What the viewer knows about one material.
struct MaterialInfo {
    name: Option<String>,
    /// `(role, texture name)`.
    textures: Vec<(String, String)>,
    /// Uploaded base texture.
    diffuse: Option<u64>,
    shading: Option<Shading>,
    surface: Option<Surface>,
    shaders: Vec<String>,
    params: Vec<(String, String, String)>,
}

fn shader_label(hash: u32) -> String {
    match doctor_toolkit::resource::shading::shader(hash) {
        Some(s) => s.name.to_owned(),
        None if hash == 0 => "-".to_owned(),
        None => format!("{hash:08x}"),
    }
}

/// Parameters of a material for display; repeated ones (the same parameter
/// of another shader slot) once.
fn param_rows(
    material: &doctor_toolkit::resource::material::Material,
    texture_names: &HashMap<u32, String>,
) -> Vec<(String, String, String)> {
    use doctor_toolkit::resource::material::ParamValue;
    let mut rows: Vec<(String, String, String)> = Vec::new();
    for p in &material.params {
        let value = match &p.value {
            ParamValue::Floats(v) => v
                .iter()
                .map(|f| format!("{f:.4}"))
                .collect::<Vec<_>>()
                .join(", "),
            ParamValue::Texture(h) => texture_names
                .get(h)
                .cloned()
                .unwrap_or_else(|| format!("{h:08x}")),
            ParamValue::Unknown(kind) => format!("type {kind}"),
        };
        let source = if p.source == "(null)" {
            String::new()
        } else {
            p.source.clone()
        };
        if !rows.iter().any(|r| r.0 == p.name && r.1 == value) {
            rows.push((p.name.clone(), value, source));
        }
    }
    rows
}

/// Cache key of mesh resource `index` of a stream.
pub fn mesh_key(stream: &StreamFile, index: usize) -> MeshKey {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    stream.path().hash(&mut hasher);
    index.hash(&mut hasher);
    hasher.finish()
}

/// All mip levels of a flat texture as RGBA; `None` for cube and volume textures.
pub fn texture_data(texture: &Texture) -> Option<TextureData> {
    let header = &texture.header;
    if header.kind() != Kind::Flat {
        return None;
    }
    let mut mips = Vec::new();
    for mip in 0..header.mip_count() {
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
        width: header.width as u32,
        height: header.height as u32,
        srgb: header.is_srgb(),
        mips,
    })
}

fn role_name(role: TextureRole) -> String {
    format!("{role:?}").to_ascii_lowercase()
}

/// Uploads texture `hash` unless it is there (from the first mip level not
/// wider than `limit`, when given); returns its key when present.
fn upload(
    hash: u32,
    assets: &mut dyn AssetSource,
    renderer: &mut SceneRenderer,
    srgb: &mut Option<bool>,
    limit: Option<u32>,
) -> Option<u64> {
    let key = hash as u64;
    if !renderer.has_texture(key) {
        let texture = assets.texture(hash)?;
        let data = match limit {
            Some(limit) => crate::level_view::loader::texture_data(&texture, limit),
            None => texture_data(&texture),
        }?;
        *srgb = Some(data.srgb);
        renderer.upload_texture(key, &data);
    }
    renderer.has_texture(key).then_some(key)
}

/// The renderer's surface for a material's shading inputs.
fn surface(
    shading: &Shading,
    assets: &mut dyn AssetSource,
    renderer: &mut SceneRenderer,
    base_srgb: Option<bool>,
    limit: Option<u32>,
) -> Surface {
    let mut ignore = None;
    let mut tex =
        |hash: Option<u32>| hash.and_then(|h| upload(h, assets, renderer, &mut ignore, limit));
    let model = match shading.family {
        Family::TintMask => SurfaceModel::TintMask,
        Family::Hair => SurfaceModel::Hair,
        Family::Skin => SurfaceModel::Skin,
        _ => SurfaceModel::Basic,
    };
    // Masks are used as stored; colour textures that are not sampled as
    // sRGB hold gamma-encoded colour the shaders raise to 2.2.
    let base_gamma = match model {
        SurfaceModel::TintMask | SurfaceModel::Hair => 1.0,
        _ if base_srgb == Some(true) => 1.0,
        _ => 2.2,
    };
    Surface {
        model,
        blend: shading.blend == Blend::Alpha,
        normal: tex(shading.normal),
        gloss: tex(shading.gloss),
        detail_normal: tex(shading.detail_normal),
        detail_mask: tex(shading.detail_mask),
        detail_gloss: tex(shading.detail_gloss),
        base_gamma,
        tints: shading.tints,
        tint: shading.tint,
        skin_tint: shading.skin_tint,
        detail_uv: shading.detail_uv,
        detail_mask_uv: shading.detail_mask_uv,
        specular: shading.specular,
        specular_power: shading.specular_power,
        detail_mask_strength: shading.detail_mask_strength,
        detail_normal_strength: shading.detail_normal_strength,
    }
}

/// How a model is drawn: material swaps and morph weights (characters).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LoadOptions {
    /// Materials drawn instead of the ones the draw records name.
    pub swaps: HashMap<u32, u32>,
    /// `(SDBM of the morph target, weight)`.
    pub morphs: Vec<(u32, f32)>,
}

/// Reads mesh resource `index` and uploads what the renderer does not have yet.
pub fn load_model(
    stream: &StreamFile,
    index: usize,
    assets: &mut dyn AssetSource,
    renderer: &mut SceneRenderer,
) -> doctor_toolkit::Result<LoadedModel> {
    let model = mesh::load(stream, index)?;
    load_parsed(&model, mesh_key(stream, index), assets, renderer)
}

/// Uploads a parsed model under `key` (a model that is not in a stream, such
/// as an import preview, needs a key of its own).
pub fn load_parsed(
    model: &Model,
    key: MeshKey,
    assets: &mut dyn AssetSource,
    renderer: &mut SceneRenderer,
) -> doctor_toolkit::Result<LoadedModel> {
    load_with(model, key, assets, renderer, &LoadOptions::default())
}

/// [`load_parsed`] with material swaps and morphs; `key` must differ for
/// every distinct `options`.
pub fn load_with(
    model: &Model,
    key: MeshKey,
    assets: &mut dyn AssetSource,
    renderer: &mut SceneRenderer,
    options: &LoadOptions,
) -> doctor_toolkit::Result<LoadedModel> {
    load_limited(model, key, assets, renderer, options, None)
}

/// [`load_with`] with textures from the first mip level not wider than
/// `limit` (the previews of the Content Browser).
pub fn load_limited(
    model: &Model,
    key: MeshKey,
    assets: &mut dyn AssetSource,
    renderer: &mut SceneRenderer,
    options: &LoadOptions,
    limit: Option<u32>,
) -> doctor_toolkit::Result<LoadedModel> {
    let drawn = |hash: u32| options.swaps.get(&hash).copied().unwrap_or(hash);
    let skeleton = if model.bones.is_empty() {
        None
    } else {
        assets.skeleton(&model.bones)
    };

    // Materials: names, texture names and the diffuse texture on the GPU.
    let mut materials: HashMap<u32, MaterialInfo> = HashMap::new();
    let mut texture_names: HashMap<u32, String> = HashMap::new();
    for record in &model.records {
        let hash = drawn(record.material_hash);
        if materials.contains_key(&hash) {
            continue;
        }
        let entry = match assets.material(hash) {
            None => MaterialInfo {
                name: None,
                textures: Vec::new(),
                diffuse: None,
                shading: None,
                surface: None,
                shaders: Vec::new(),
                params: Vec::new(),
            },
            Some(material) => {
                let mut textures = Vec::new();
                for (role, hash, _) in material.textures() {
                    let name = match texture_names.get(&hash) {
                        Some(name) => name.clone(),
                        None => {
                            let name = assets
                                .texture(hash)
                                .map(|t| t.header.name.clone())
                                .unwrap_or_else(|| format!("{hash:08x}"));
                            texture_names.insert(hash, name.clone());
                            name
                        }
                    };
                    textures.push((role_name(role), name));
                }
                let mut srgb = None;
                let diffuse = material
                    .texture(TextureRole::Diffuse)
                    .and_then(|hash| upload(hash, assets, renderer, &mut srgb, limit));
                let shading = Shading::from_material(&material);
                let surface = surface(&shading, assets, renderer, srgb, limit);
                let mut shaders: Vec<String> =
                    material.shaders.iter().map(|s| shader_label(s.0)).collect();
                shaders.push(shader_label(material.shadow_shader));
                MaterialInfo {
                    name: Some(material.name.clone()),
                    textures,
                    diffuse,
                    shading: Some(shading),
                    surface: Some(surface),
                    shaders,
                    params: param_rows(&material, &texture_names),
                }
            }
        };
        materials.insert(hash, entry);
    }

    let mesh = match renderer.has_mesh(key) {
        true => renderer.mesh_data(key).cloned(),
        false => None,
    };
    let mesh = match mesh {
        Some(mesh) => mesh,
        None => {
            let data = Arc::new(mesh_data(model, skeleton.as_ref(), &materials, options)?);
            renderer.upload_mesh(key, data.clone());
            data
        }
    };

    let parts = model
        .records
        .iter()
        .zip(&mesh.parts)
        .map(|(record, part)| {
            let hash = drawn(record.material_hash);
            let info = &materials[&hash];
            PartInfo {
                name: record.name.clone(),
                material_hash: hash,
                swapped_from: (hash != record.material_hash).then_some(record.material_hash),
                shading: info.shading.clone(),
                shaders: info.shaders.clone(),
                params: info.params.clone(),
                material: info.name.clone(),
                textures: info.textures.clone(),
                format: FORMAT_NAMES
                    .get(record.format as usize)
                    .copied()
                    .unwrap_or("?"),
                vertices: record.vertex_count as usize,
                triangles: part.index_count as usize / 3,
                influences: (record.influences != 0xFF).then_some(record.influences),
                rigid_bone: record.rigid_bone,
                palette: record.palette.iter().map(|p| p.0).collect(),
            }
        })
        .collect();
    let bones = skeleton
        .as_ref()
        .map(|s| {
            s.bind_globals()
                .iter()
                .zip(&s.bones)
                .map(|(m, b)| BoneInfo {
                    name: b.display_name(),
                    parent: b.parent,
                    position: translation(m),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(LoadedModel {
        key,
        name: model.name.clone(),
        mesh,
        parts,
        skeleton_name: skeleton.as_ref().map(|s| {
            if s.name.is_empty() {
                format!("{:08x}", s.name_hash)
            } else {
                s.name.clone()
            }
        }),
        bones,
        bone_table: model.bones.len(),
        textured_materials: materials.values().filter(|m| m.diffuse.is_some()).count(),
    })
}

/// One part per draw record, in record order (empty records stay as empty parts).
fn mesh_data(
    model: &Model,
    skeleton: Option<&Skeleton>,
    materials: &HashMap<u32, MaterialInfo>,
    options: &LoadOptions,
) -> doctor_toolkit::Result<MeshData> {
    let mut data = MeshData::default();
    let bone_count = skeleton.map_or(256, |s| s.bones.len());
    for record in &model.records {
        let mut source = model.vertices(record)?;
        morphed_positions(model, record, &mut source.positions, &options.morphs)?;
        let triangles = model.triangles(record)?;
        let skin = vertex_skin(&record.palette, record.rigid_bone, &source, bone_count);
        let mut vertices: Vec<Vertex> = source
            .positions
            .iter()
            .enumerate()
            .map(|(i, &position)| {
                let (joints, weights) = skin[i];
                Vertex {
                    position,
                    normal: source.normals.as_ref().map_or([0.0; 3], |n| n[i]),
                    uv: source.uv0.as_ref().map_or([0.0; 2], |uv| uv[i]),
                    color: source.colors.as_ref().map_or([255; 4], |c| c[i]),
                    joints: joints.map(|j| j.min(255) as u8),
                    weights: weights.map(|w| (w.clamp(0.0, 1.0) * 255.0).round() as u8),
                    part: 0,
                }
            })
            .collect();
        let indices: Vec<u32> = triangles.into_iter().flatten().collect();
        if source.normals.is_none() {
            MeshData::compute_normals(&mut vertices, &indices);
        }
        let info = materials.get(
            &options
                .swaps
                .get(&record.material_hash)
                .copied()
                .unwrap_or(record.material_hash),
        );
        let texture = info
            .and_then(|m| m.diffuse)
            .filter(|_| source.uv0.is_some());
        let material = MaterialData {
            texture,
            color: if texture.is_some() {
                [1.0; 4]
            } else {
                MaterialData::default().color
            },
            surface: info
                .and_then(|m| m.surface.clone())
                .filter(|_| source.uv0.is_some())
                .map(Box::new),
        };
        data.push_part(record.name.clone(), vertices, &indices, material);
    }
    Ok(data)
}

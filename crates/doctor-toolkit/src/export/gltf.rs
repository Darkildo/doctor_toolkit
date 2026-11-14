//! Export of models and animation clips to binary glTF 2.0 (`.glb`).
//!
//! Game models become a [`Scene`] ([`crate::scene::game`]), which
//! [`write_glb`] writes: one node and mesh per object, one primitive per
//! part, a skin when the scene has joints (one node per joint with its
//! bind-pose local matrix, the inverse bind matrices, `JOINTS_0`/`WEIGHTS_0`
//! on skinned primitives) and one animation per clip. Positions are written
//! as stored: the game uses Y up, like glTF.

use std::collections::HashMap;

use serde_json::{Value, json};

use crate::resource::material::Material;
use crate::resource::mesh::{Model, Vertices};
use crate::resource::mesh_build::PartGeometry;
use crate::resource::rcb::Skeleton;
use crate::resource::texture::Texture;
use crate::scene::{Scene, SceneImage, SceneMaterial};
use crate::{Error, Result};

/// Looks up materials and textures referenced by a model.
pub trait AssetSource {
    /// The material whose name hash is `hash`.
    fn material(&mut self, hash: u32) -> Option<Material>;
    /// The texture whose name hash is `hash`.
    fn texture(&mut self, hash: u32) -> Option<Texture>;
    /// A skeleton matching a mesh bone table `(bone name hash, skeleton index)`.
    fn skeleton(&mut self, _bones: &[(u32, u32)]) -> Option<Skeleton> {
        None
    }
    /// Every rig of the game (skeletons and bone sets, for animations).
    fn rigs(&mut self) -> &[crate::resource::rcb::Rig] {
        &[]
    }
    /// The skeleton of rig `rig` (resource id, as a character's SimGroup
    /// object names it) matching a mesh bone table; [`Self::skeleton`]
    /// when the rig is not known or has none.
    fn rig_skeleton(&mut self, rig: Option<u32>, bones: &[(u32, u32)]) -> Option<Skeleton> {
        let found = rig.and_then(|id| {
            self.rigs()
                .iter()
                .filter(|r| r.id == id)
                .find_map(|r| r.skeleton_for(bones))
                .cloned()
        });
        found.or_else(|| self.skeleton(bones))
    }
}

/// An [`AssetSource`] without assets: geometry only.
pub struct NoAssets;

impl AssetSource for NoAssets {
    fn material(&mut self, _: u32) -> Option<Material> {
        None
    }
    fn texture(&mut self, _: u32) -> Option<Texture> {
        None
    }
}

const ARRAY_BUFFER: u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;
const FLOAT: u32 = 5126;
const UNSIGNED_BYTE: u32 = 5121;
const UNSIGNED_SHORT: u32 = 5123;
const UNSIGNED_INT: u32 = 5125;

#[derive(Default)]
struct Builder {
    bin: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
    images: Vec<Value>,
    textures: Vec<Value>,
    materials: Vec<Value>,
    image_index: HashMap<(usize, u64), usize>,
}

impl Builder {
    fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let mut view =
            json!({ "buffer": 0, "byteOffset": self.bin.len(), "byteLength": bytes.len() });
        if let Some(target) = target {
            view["target"] = json!(target);
        }
        self.bin.extend_from_slice(bytes);
        self.views.push(view);
        self.views.len() - 1
    }

    fn accessor(
        &mut self,
        bytes: &[u8],
        target: Option<u32>,
        component: u32,
        kind: &str,
        count: usize,
        extra: Value,
    ) -> usize {
        let view = self.view(bytes, target);
        let mut accessor =
            json!({ "bufferView": view, "componentType": component, "count": count, "type": kind });
        if let (Value::Object(a), Value::Object(e)) = (&mut accessor, extra) {
            a.extend(e);
        }
        self.accessors.push(accessor);
        self.accessors.len() - 1
    }

    fn floats<const N: usize>(
        &mut self,
        values: &[[f32; N]],
        kind: &str,
        with_bounds: bool,
    ) -> usize {
        let bytes: Vec<u8> = values
            .iter()
            .flatten()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        let extra = if with_bounds {
            let min: Vec<f32> = (0..N)
                .map(|i| values.iter().map(|v| v[i]).fold(f32::INFINITY, f32::min))
                .collect();
            let max: Vec<f32> = (0..N)
                .map(|i| {
                    values
                        .iter()
                        .map(|v| v[i])
                        .fold(f32::NEG_INFINITY, f32::max)
                })
                .collect();
            json!({ "min": min, "max": max })
        } else {
            json!({})
        };
        self.accessor(&bytes, Some(ARRAY_BUFFER), FLOAT, kind, values.len(), extra)
    }

    /// Adds an image (once) and a texture using it.
    fn texture(&mut self, image: &SceneImage) -> usize {
        let key = image.key();
        let source = match self.image_index.get(&key) {
            Some(&i) => i,
            None => {
                let view = self.view(&image.bytes, None);
                let mime = match image.extension() {
                    "jpg" => "image/jpeg",
                    _ => "image/png",
                };
                self.images
                    .push(json!({ "bufferView": view, "mimeType": mime, "name": image.name }));
                self.image_index.insert(key, self.images.len() - 1);
                self.textures
                    .push(json!({ "source": self.images.len() - 1 }));
                self.images.len() - 1
            }
        };
        // Textures and images are added together, one texture per image.
        source
    }

    fn material(&mut self, material: &SceneMaterial) -> usize {
        let mut pbr = json!({ "metallicFactor": 0.0, "roughnessFactor": 1.0 });
        if material.base_color_factor != [1.0; 4] {
            pbr["baseColorFactor"] = json!(material.base_color_factor);
        }
        let mut entry = json!({ "name": material.name });
        if let Some(image) = &material.base_color {
            pbr["baseColorTexture"] = json!({ "index": self.texture(image) });
            if material.alpha {
                entry["alphaMode"] = json!("MASK");
            }
        } else if material.alpha {
            entry["alphaMode"] = json!("BLEND");
        }
        if let Some(image) = &material.normal {
            entry["normalTexture"] = json!({ "index": self.texture(image) });
        }
        entry["pbrMetallicRoughness"] = pbr;
        self.materials.push(entry);
        self.materials.len() - 1
    }

    fn primitive(
        &mut self,
        vertices: &PartGeometry,
        material: Option<usize>,
        skin: Option<&[([u16; 4], [f32; 4])]>,
    ) -> Value {
        let mut attributes = serde_json::Map::new();
        attributes.insert(
            "POSITION".into(),
            json!(self.floats(&vertices.positions, "VEC3", true)),
        );
        if let Some(normals) = &vertices.normals {
            attributes.insert("NORMAL".into(), json!(self.floats(normals, "VEC3", false)));
            if let (Some(tangents), Some(_)) = (&vertices.tangents, &vertices.uv0) {
                attributes.insert(
                    "TANGENT".into(),
                    json!(self.floats(tangents, "VEC4", false)),
                );
            }
        }
        if let Some(uv) = &vertices.uv0 {
            attributes.insert("TEXCOORD_0".into(), json!(self.floats(uv, "VEC2", false)));
        }
        if let Some(uv) = &vertices.uv1 {
            attributes.insert("TEXCOORD_1".into(), json!(self.floats(uv, "VEC2", false)));
        }
        if let Some(colors) = &vertices.colors {
            let bytes: Vec<u8> = colors.iter().flatten().copied().collect();
            let index = self.accessor(
                &bytes,
                Some(ARRAY_BUFFER),
                UNSIGNED_BYTE,
                "VEC4",
                colors.len(),
                json!({ "normalized": true }),
            );
            attributes.insert("COLOR_0".into(), json!(index));
        }
        if let Some(skin) = skin {
            let bytes: Vec<u8> = skin
                .iter()
                .flat_map(|(j, _)| j.iter().flat_map(|i| i.to_le_bytes()))
                .collect();
            let joints = self.accessor(
                &bytes,
                Some(ARRAY_BUFFER),
                UNSIGNED_SHORT,
                "VEC4",
                skin.len(),
                json!({}),
            );
            attributes.insert("JOINTS_0".into(), json!(joints));
            let weights: Vec<[f32; 4]> = skin.iter().map(|(_, w)| *w).collect();
            attributes.insert(
                "WEIGHTS_0".into(),
                json!(self.floats(&weights, "VEC4", false)),
            );
        }
        let bytes: Vec<u8> = vertices
            .triangles
            .iter()
            .flatten()
            .flat_map(|i| i.to_le_bytes())
            .collect();
        let indices = self.accessor(
            &bytes,
            Some(ELEMENT_ARRAY_BUFFER),
            UNSIGNED_INT,
            "SCALAR",
            vertices.triangles.len() * 3,
            json!({}),
        );
        let mut primitive = json!({ "attributes": attributes, "indices": indices, "mode": 4 });
        if let Some(material) = material {
            primitive["material"] = json!(material);
        }
        primitive
    }
}

/// Joints (skeleton bone indices) and normalised weights of every vertex of a record.
///
/// `palette` and `rigid_bone` come from the draw record.
pub fn vertex_skin(
    palette: &[(u8, u8)],
    rigid_bone: Option<u8>,
    vertices: &Vertices,
    bone_count: usize,
) -> Vec<([u16; 4], [f32; 4])> {
    let n = vertices.positions.len();
    let bone = |b: u8| {
        if (b as usize) < bone_count {
            b as u16
        } else {
            0
        }
    };
    match (&vertices.joints, &vertices.weights) {
        (Some(joints), Some(weights)) => joints
            .iter()
            .zip(weights)
            .map(|(j, w)| {
                let mut out_j = [0u16; 4];
                let mut out_w = [0f32; 4];
                for k in 0..4 {
                    if w[k] > 0.0
                        && let Some(&(skeleton_bone, _)) = palette.get(j[k] as usize)
                    {
                        out_j[k] = bone(skeleton_bone);
                        out_w[k] = w[k];
                    }
                }
                let sum: f32 = out_w.iter().sum();
                if sum > 0.0 {
                    out_w = out_w.map(|x| x / sum);
                } else {
                    out_w = [1.0, 0.0, 0.0, 0.0];
                }
                (out_j, out_w)
            })
            .collect(),
        _ => {
            let b = rigid_bone.map(bone).unwrap_or(0);
            vec![([b, 0, 0, 0], [1.0, 0.0, 0.0, 0.0]); n]
        }
    }
}

/// One model of a scene written by [`models_to_glb`].
#[derive(Debug, Clone)]
pub struct SceneModel<'a> {
    pub model: &'a Model,
    /// Node and mesh name.
    pub name: String,
    /// Materials drawn instead of the ones the draw records name (`from → to`).
    pub materials: HashMap<u32, u32>,
    /// Morph target weights applied to the positions: `(SDBM of the target, weight)`.
    pub morphs: Vec<(u32, f32)>,
}

impl<'a> SceneModel<'a> {
    pub fn new(model: &'a Model) -> Self {
        Self {
            model,
            name: model.name.clone(),
            materials: HashMap::new(),
            morphs: Vec::new(),
        }
    }
}

/// Converts a model to a binary glTF file.
pub fn model_to_glb(model: &Model, source: &mut dyn AssetSource) -> Result<Vec<u8>> {
    models_to_glb(&[SceneModel::new(model)], source)
}

/// An animation clip for [`scene_to_glb`].
#[derive(Debug, Clone)]
pub struct SceneAnimation<'a> {
    pub name: String,
    /// The skeleton the track bone indices refer to; mapped to the scene's
    /// skeleton by bone hash.
    pub skeleton: &'a Skeleton,
    pub tracks: Vec<crate::resource::anim::BoneTrack>,
    pub frame_rate: f32,
}

/// Converts models sharing one skeleton (the parts of a character) to one
/// binary glTF file: a node per model, all skinned to the largest skeleton.
/// Joints of models matched to another skeleton are mapped by bone hash.
pub fn models_to_glb(models: &[SceneModel<'_>], source: &mut dyn AssetSource) -> Result<Vec<u8>> {
    scene_to_glb(models, None, &[], source)
}

/// Adds the morph targets of a record to its positions with the given weights.
pub fn morphed_positions(
    model: &Model,
    record: &crate::resource::mesh::DrawRecord,
    positions: &mut [[f32; 3]],
    weights: &[(u32, f32)],
) -> Result<()> {
    if weights.iter().all(|w| w.1 == 0.0) {
        return Ok(());
    }
    for morph in model.morphs(record)? {
        let Some(&(_, w)) = weights.iter().find(|(h, _)| *h == morph.hash) else {
            continue;
        };
        for (v, d) in morph.deltas {
            if let Some(p) = positions.get_mut(v as usize) {
                for k in 0..3 {
                    p[k] += d[k] * w;
                }
            }
        }
    }
    Ok(())
}

/// [`models_to_glb`] with animations. `skeleton` replaces the skeleton found
/// from the models (it is required when there are no models).
pub fn scene_to_glb(
    models: &[SceneModel<'_>],
    skeleton: Option<&Skeleton>,
    animations: &[SceneAnimation<'_>],
    source: &mut dyn AssetSource,
) -> Result<Vec<u8>> {
    let scene = crate::scene::game::build(models, skeleton, animations, source)?;
    write_glb(&scene)
}

/// Writes a scene as binary glTF.
pub fn write_glb(scene: &Scene) -> Result<Vec<u8>> {
    let mut builder = Builder::default();
    let materials: Vec<usize> = scene
        .materials
        .iter()
        .map(|m| builder.material(m))
        .collect();
    let mut meshes: Vec<Value> = Vec::new();
    let mut mesh_nodes: Vec<Value> = Vec::new();
    let mut k = 0;
    while k < scene.parts.len() {
        let object = &scene.parts[k].object;
        let mut end = k + 1;
        while end < scene.parts.len() && scene.parts[end].object == *object {
            end += 1;
        }
        let mut primitives = Vec::new();
        let mut skinned = false;
        for part in &scene.parts[k..end] {
            let g = &part.geometry;
            if g.triangles.is_empty() {
                continue;
            }
            let skin = g.skin.as_deref().filter(|_| !scene.joints.is_empty());
            skinned |= skin.is_some();
            let material = part.material.and_then(|m| materials.get(m).copied());
            primitives.push(builder.primitive(g, material, skin));
        }
        let name = if object.is_empty() {
            &scene.parts[k].name
        } else {
            object
        };
        if !primitives.is_empty() {
            let mut node = json!({ "mesh": meshes.len(), "name": name });
            if skinned {
                node["skin"] = json!(0);
            }
            meshes.push(json!({ "name": name, "primitives": primitives }));
            mesh_nodes.push(node);
        }
        k = end;
    }
    if meshes.is_empty() && scene.joints.is_empty() {
        return Err(Error::InvalidArgument(format!(
            "model `{}` has no triangles",
            scene.name
        )));
    }
    let first_bone = mesh_nodes.len();
    let mut scene_nodes: Vec<usize> = (0..mesh_nodes.len()).collect();
    let mut nodes = mesh_nodes;
    let mut skins = Vec::new();
    if !scene.joints.is_empty() {
        let joints = &scene.joints;
        let bytes: Vec<u8> = joints
            .iter()
            .flat_map(|j| j.inverse_bind)
            .flat_map(|f| f.to_le_bytes())
            .collect();
        let ibm = builder.accessor(&bytes, None, FLOAT, "MAT4", joints.len(), json!({}));
        let mut children = vec![Vec::new(); joints.len()];
        let mut roots = Vec::new();
        for (i, joint) in joints.iter().enumerate() {
            match joint.parent {
                Some(p) => children[p].push(first_bone + i),
                None => roots.push(first_bone + i),
            }
        }
        for (i, joint) in joints.iter().enumerate() {
            let mut node = json!({ "name": joint.name, "matrix": joint.local });
            if !children[i].is_empty() {
                node["children"] = json!(children[i]);
            }
            nodes.push(node);
        }
        skins.push(json!({
            "name": scene.name,
            "inverseBindMatrices": ibm,
            "joints": (first_bone..first_bone + joints.len()).collect::<Vec<_>>(),
            "skeleton": roots.first().copied().unwrap_or(first_bone),
        }));
        scene_nodes.extend(roots);
    }
    let mut gltf_animations = Vec::new();
    for clip in &scene.clips {
        let mut samplers = Vec::new();
        let mut channels = Vec::new();
        let rate = if clip.frame_rate > 0.0 {
            clip.frame_rate
        } else {
            30.0
        };
        let mut inputs: HashMap<usize, usize> = HashMap::new();
        for track in &clip.tracks {
            if track.joint >= scene.joints.len() {
                continue;
            }
            let node = first_bone + track.joint;
            let mut channel = |builder: &mut Builder, n: usize, output: usize, path: &str| {
                let input = *inputs.entry(n).or_insert_with(|| {
                    let times: Vec<[f32; 1]> = (0..n).map(|f| [f as f32 / rate]).collect();
                    builder.floats(&times, "SCALAR", true)
                });
                samplers
                    .push(json!({ "input": input, "output": output, "interpolation": "LINEAR" }));
                channels.push(json!({
                    "sampler": samplers.len() - 1,
                    "target": { "node": node, "path": path }
                }));
            };
            if !track.rotations.is_empty() {
                let output = builder.floats(&track.rotations, "VEC4", false);
                channel(&mut builder, track.rotations.len(), output, "rotation");
            }
            if !track.translations.is_empty() {
                let output = builder.floats(&track.translations, "VEC3", false);
                channel(
                    &mut builder,
                    track.translations.len(),
                    output,
                    "translation",
                );
            }
            if !track.scales.is_empty() {
                let output = builder.floats(&track.scales, "VEC3", false);
                channel(&mut builder, track.scales.len(), output, "scale");
            }
        }
        if !channels.is_empty() {
            gltf_animations
                .push(json!({ "name": clip.name, "samplers": samplers, "channels": channels }));
        }
    }
    while !builder.bin.len().is_multiple_of(4) {
        builder.bin.push(0);
    }
    let mut document = json!({
        "asset": { "version": "2.0", "generator": concat!("doctor_toolkit ", env!("CARGO_PKG_VERSION")) },
        "scene": 0,
        "scenes": [{ "nodes": scene_nodes }],
        "nodes": nodes,
        "buffers": [{ "byteLength": builder.bin.len() }],
        "bufferViews": builder.views,
        "accessors": builder.accessors,
    });
    for (key, list) in [
        ("meshes", meshes),
        ("images", builder.images),
        ("textures", builder.textures),
        ("materials", builder.materials),
        ("skins", skins),
        ("animations", gltf_animations),
    ] {
        if !list.is_empty() {
            document[key] = Value::Array(list);
        }
    }
    pack_glb(&document, &builder.bin)
}

/// Packs a glTF document and its binary buffer into a `.glb` file.
fn pack_glb(document: &Value, bin: &[u8]) -> Result<Vec<u8>> {
    let mut json_bytes =
        serde_json::to_vec(document).map_err(|e| Error::InvalidArgument(e.to_string()))?;
    while !json_bytes.len().is_multiple_of(4) {
        json_bytes.push(b' ');
    }
    let total = 12 + 8 + json_bytes.len() + 8 + bin.len();
    let mut glb = Vec::with_capacity(total);
    glb.extend_from_slice(b"glTF");
    glb.extend_from_slice(&2u32.to_le_bytes());
    glb.extend_from_slice(&(total as u32).to_le_bytes());
    glb.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    glb.extend_from_slice(b"JSON");
    glb.extend_from_slice(&json_bytes);
    glb.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    glb.extend_from_slice(b"BIN\0");
    glb.extend_from_slice(bin);
    Ok(glb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skin_maps_palette_indices_to_skeleton_bones() {
        let vertices = Vertices {
            positions: vec![[0.0; 3]; 2],
            joints: Some(vec![[1, 0, 9, 0], [0, 0, 0, 0]]),
            weights: Some(vec![[0.5, 0.25, 0.0, 0.0], [0.0; 4]]),
            ..Default::default()
        };
        let palette = [(7, 0), (40, 1)];
        let skin = vertex_skin(&palette, None, &vertices, 41);
        assert_eq!(skin[0].0, [40, 7, 0, 0]);
        assert!((skin[0].1[0] - 2.0 / 3.0).abs() < 1e-6);
        assert!((skin[0].1.iter().sum::<f32>() - 1.0).abs() < 1e-6);
        // No weights at all: bound to the root with weight 1.
        assert_eq!(skin[1], ([0, 0, 0, 0], [1.0, 0.0, 0.0, 0.0]));
        // Bones outside the skeleton fall back to the root.
        assert_eq!(
            vertex_skin(&palette, None, &vertices, 10)[0].0,
            [0, 7, 0, 0]
        );
    }

    #[test]
    fn rigid_records_use_their_bone() {
        let vertices = Vertices {
            positions: vec![[0.0; 3]; 3],
            ..Default::default()
        };
        let skin = vertex_skin(&[], Some(27), &vertices, 102);
        assert_eq!(skin, vec![([27, 0, 0, 0], [1.0, 0.0, 0.0, 0.0]); 3]);
        assert_eq!(vertex_skin(&[], None, &vertices, 102)[0].0, [0; 4]);
    }
}

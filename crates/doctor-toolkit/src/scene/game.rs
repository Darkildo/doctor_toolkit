//! Game models, skeletons and animation clips → [`Scene`].
//!
//! Positions are taken as stored: the game uses Y up and metres. Each draw
//! record becomes one part. When the [`AssetSource`] finds a skeleton
//! (`rcb`) matching the mesh bone table, the scene gets its joints (bind-pose
//! locals and the rig's inverse bind matrices) and every part gets bone
//! weights: per-vertex palette indices are mapped through the record palette
//! (`palette[i].0`), records bound to one bone use that bone with weight 1,
//! and static records use bone 0.

use std::collections::HashMap;

use super::{JointTrack, Scene, SceneClip, SceneImage, SceneJoint, SceneMaterial, ScenePart};
use crate::export::gltf::{
    AssetSource, SceneAnimation, SceneModel, morphed_positions, vertex_skin,
};
use crate::resource::material::TextureRole;
use crate::resource::mesh_build::PartGeometry;
use crate::resource::pixels;
use crate::resource::rcb::Skeleton;
use crate::resource::shading::{Family, Shading};
use crate::resource::tg4::Format;
use crate::{Error, Result};

#[derive(Default)]
struct Materials {
    list: Vec<SceneMaterial>,
    index: HashMap<u32, Option<usize>>,
    images: HashMap<(u32, bool), Option<SceneImage>>,
}

impl Materials {
    /// A texture as a PNG image; normal maps are rebuilt to RGB.
    fn image(
        &mut self,
        source: &mut dyn AssetSource,
        hash: u32,
        normal_map: bool,
    ) -> Option<SceneImage> {
        if let Some(found) = self.images.get(&(hash, normal_map)) {
            return found.clone();
        }
        let image = source.texture(hash).and_then(|texture| {
            let (width, height, mut rgba) = texture.rgba(0).ok()?;
            if normal_map && texture.header.format() == Format::Dxt5Nm {
                pixels::rebuild_normal_map(&mut rgba);
            }
            let png = pixels::encode_png(width, height, &rgba).ok()?;
            Some(SceneImage::png(texture.header.name.clone(), png))
        });
        self.images.insert((hash, normal_map), image.clone());
        image
    }

    fn material(&mut self, source: &mut dyn AssetSource, hash: u32) -> Option<usize> {
        if let Some(&index) = self.index.get(&hash) {
            return index;
        }
        let index = source.material(hash).map(|material| {
            let mut entry = SceneMaterial {
                name: material.name.clone(),
                ..Default::default()
            };
            if let Some(diffuse) = material.texture(TextureRole::Diffuse) {
                let alpha_mask = source
                    .texture(diffuse)
                    .is_some_and(|t| t.header.format() == Format::Dxt1a);
                let shading = Shading::from_material(&material);
                match bake_base(source, &shading, diffuse) {
                    Some((image, alpha)) => {
                        entry.base_color = Some(image);
                        entry.alpha = alpha;
                    }
                    None => {
                        entry.base_color = self.image(source, diffuse, false);
                        entry.alpha = alpha_mask && entry.base_color.is_some();
                    }
                }
            }
            if let Some(normal) = material.texture(TextureRole::Normal) {
                entry.normal = self.image(source, normal, true);
            }
            if let Some(specular) = material.texture(TextureRole::Specular) {
                entry.specular = self.image(source, specular, false);
            }
            self.list.push(entry);
            self.list.len() - 1
        });
        self.index.insert(hash, index);
        index
    }
}

/// The colour a game shader makes of a character material's base texture,
/// baked into an RGBA image for other programs (`docs/formats/characters.md`
/// §2): clothes and hair mix their tint colours by the channels of the base
/// texture (a mask), eyes tint the iris (base alpha), hair takes its alpha
/// from the gloss map's green. `None` for other materials (the base texture
/// is the colour) or when a texture is missing; the flag is "has alpha".
fn bake_base(
    source: &mut dyn AssetSource,
    shading: &Shading,
    base: u32,
) -> Option<(SceneImage, bool)> {
    let (width, height, baked) = baked_pixels(source, shading, base)?;
    let texture = source.texture(base)?;
    let png = pixels::encode_png(width, height, &baked).ok()?;
    let name = format!("{}_{}", texture.header.name, shading_suffix(shading.family));
    let alpha = match shading.family {
        Family::Hair => true,
        Family::Eye => false,
        _ => texture.header.format() == Format::Dxt1a,
    };
    Some((SceneImage::png(name, png), alpha))
}

/// The pixels [`bake_base`] writes for base texture `base` of a material
/// with `shading`: `(width, height, RGBA)`. The model importer recognises
/// them as the game's texture (a round trip keeps the mask).
pub(crate) fn baked_pixels(
    source: &mut dyn AssetSource,
    shading: &Shading,
    base: u32,
) -> Option<(usize, usize, Vec<u8>)> {
    if !matches!(
        shading.family,
        Family::TintMask | Family::Hair | Family::Eye
    ) {
        return None;
    }
    let texture = source.texture(base)?;
    let (width, height, rgba) = texture.rgba(0).ok()?;
    let gloss = match shading.family {
        Family::Hair => {
            let gloss = source.texture(shading.gloss?)?;
            Some(gloss.rgba(0).ok()?)
        }
        _ => None,
    };
    let baked = bake_pixels(
        shading,
        width,
        height,
        &rgba,
        texture.header.is_srgb(),
        gloss.as_ref(),
    );
    Some((width, height, baked))
}

fn shading_suffix(family: Family) -> &'static str {
    match family {
        Family::Eye => "eye",
        Family::Hair => "hair",
        _ => "tinted",
    }
}

fn srgb_to_linear(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round() as u8
}

/// [`bake_base`] on pixels: `rgba` is the base texture (sampled as sRGB
/// when `srgb`, as the viewer does), `gloss` the gloss map (any size,
/// sampled at the nearest texel) for hair.
fn bake_pixels(
    shading: &Shading,
    width: usize,
    height: usize,
    rgba: &[u8],
    srgb: bool,
    gloss: Option<&(usize, usize, Vec<u8>)>,
) -> Vec<u8> {
    let [t0, t1, t2] = shading.tints;
    let mut out = Vec::with_capacity(rgba.len());
    for (i, px) in rgba.chunks_exact(4).enumerate() {
        let w = |k: usize| {
            if srgb {
                srgb_to_linear(px[k])
            } else {
                px[k] as f32 / 255.0
            }
        };
        let (colour, alpha): ([f32; 3], u8) = match shading.family {
            Family::Eye => {
                let a = px[3] as f32 / 255.0;
                let colour = std::array::from_fn(|k| {
                    let c = srgb_to_linear(px[k]);
                    c + (c * shading.skin_tint[k] - c) * a
                });
                (colour, 255)
            }
            Family::Hair => {
                let colour = std::array::from_fn(|k| {
                    1.0 - (1.0 - w(0) * t0[k]) * (1.0 - w(1) * t1[k]) * (1.0 - w(2) * t2[k])
                });
                let alpha = gloss.map_or(255, |(gw, gh, g)| {
                    let (x, y) = (i % width, i / width);
                    let (gx, gy) = (x * gw / width.max(1), y * gh / height.max(1));
                    g.get((gy * gw + gx) * 4 + 1).copied().unwrap_or(255)
                });
                (colour, alpha)
            }
            _ => {
                let colour = std::array::from_fn(|k| w(0) * t0[k] + w(1) * t1[k] + w(2) * t2[k]);
                (colour, px[3])
            }
        };
        let tint = shading.tint;
        out.extend(
            colour
                .iter()
                .zip(&tint)
                .map(|(&c, &t)| linear_to_srgb(c * t)),
        );
        out.push(alpha);
    }
    out
}

/// The joints of a game skeleton.
pub fn skeleton_joints(skeleton: &Skeleton) -> Vec<SceneJoint> {
    let locals = skeleton.bind_locals();
    skeleton
        .bones
        .iter()
        .zip(locals)
        .map(|(bone, local)| SceneJoint {
            name: bone.display_name(),
            parent: bone.parent,
            local,
            inverse_bind: bone.inverse_bind,
        })
        .collect()
}

/// Clips of game bone tracks on `skeleton` (the scene's joints).
pub fn clips(skeleton: &Skeleton, animations: &[SceneAnimation<'_>]) -> Vec<SceneClip> {
    animations
        .iter()
        .filter_map(|animation| {
            let tracks: Vec<JointTrack> = animation
                .tracks
                .iter()
                .filter_map(|track| {
                    let hash = animation.skeleton.bones.get(track.bone)?.hash;
                    let joint = skeleton.bones.iter().position(|b| b.hash == hash)?;
                    Some(JointTrack {
                        joint,
                        rotations: track.rotations.clone(),
                        translations: track.translations.clone(),
                        scales: Vec::new(),
                    })
                })
                .filter(|t| !t.rotations.is_empty())
                .collect();
            (!tracks.is_empty()).then(|| SceneClip {
                name: animation.name.clone(),
                frame_rate: animation.frame_rate.max(1.0),
                tracks,
            })
        })
        .collect()
}

/// A scene of one model.
pub fn model_scene(
    model: &crate::resource::mesh::Model,
    source: &mut dyn AssetSource,
) -> Result<Scene> {
    build(&[SceneModel::new(model)], None, &[], source)
}

/// A scene of models sharing one skeleton (the parts of a character): all
/// skinned to the largest skeleton found, joints of models matched to another
/// skeleton mapped by bone hash. `skeleton` replaces the skeleton found from
/// the models (it is required when there are no models).
pub fn build(
    models: &[SceneModel<'_>],
    skeleton: Option<&Skeleton>,
    animations: &[SceneAnimation<'_>],
    source: &mut dyn AssetSource,
) -> Result<Scene> {
    let mut materials = Materials::default();
    let skeletons: Vec<Option<Skeleton>> = models
        .iter()
        .map(|m| {
            if m.model.bones.is_empty() {
                None
            } else {
                source.skeleton(&m.model.bones)
            }
        })
        .collect();
    let main = skeleton.cloned().or_else(|| {
        skeletons
            .iter()
            .flatten()
            .max_by_key(|s| s.bones.len())
            .cloned()
    });
    let mut scene = Scene {
        name: models.first().map(|m| m.name.clone()).unwrap_or_default(),
        ..Default::default()
    };
    for (scene_model, skeleton) in models.iter().zip(&skeletons) {
        let model = scene_model.model;
        // Bone index of the model's skeleton -> bone index of the main one.
        let remap: Option<Vec<u16>> = match (skeleton, &main) {
            (Some(own), Some(main)) => Some(
                own.bones
                    .iter()
                    .map(|b| {
                        main.bones
                            .iter()
                            .position(|m| m.hash == b.hash)
                            .unwrap_or(0) as u16
                    })
                    .collect(),
            ),
            _ => None,
        };
        let bone_count = skeleton.as_ref().map_or(0, |s| s.bones.len());
        let records: Vec<usize> = (0..model.records.len())
            .filter(|&r| {
                model
                    .triangles(&model.records[r])
                    .is_ok_and(|t| !t.is_empty())
            })
            .collect();
        for (k, &r) in records.iter().enumerate() {
            let record = &model.records[r];
            let triangles = model.triangles(record)?;
            let mut vertices = model.vertices(record)?;
            morphed_positions(model, record, &mut vertices.positions, &scene_model.morphs)?;
            let hash = scene_model
                .materials
                .get(&record.material_hash)
                .copied()
                .unwrap_or(record.material_hash);
            let material = materials.material(source, hash);
            let skin = remap.as_ref().map(|remap| {
                let mut skin =
                    vertex_skin(&record.palette, record.rigid_bone, &vertices, bone_count);
                for (joints, _) in &mut skin {
                    for j in joints.iter_mut() {
                        *j = remap.get(*j as usize).copied().unwrap_or(0);
                    }
                }
                skin
            });
            let name = if records.len() > 1 {
                format!("{}.{k}", scene_model.name)
            } else {
                scene_model.name.clone()
            };
            scene.parts.push(ScenePart {
                name: name.clone(),
                object: scene_model.name.clone(),
                material,
                geometry: PartGeometry {
                    name,
                    material_hash: hash,
                    positions: vertices.positions,
                    normals: vertices.normals,
                    tangents: vertices.tangents.filter(|_| vertices.uv0.is_some()),
                    uv0: vertices.uv0,
                    uv1: vertices.uv1,
                    colors: vertices.colors,
                    skin,
                    triangles,
                },
            });
        }
    }
    if scene.parts.is_empty() && main.is_none() {
        let names: Vec<&str> = models.iter().map(|m| m.model.name.as_str()).collect();
        return Err(Error::InvalidArgument(format!(
            "model `{}` has no triangles",
            names.join("`, `")
        )));
    }
    if let Some(skeleton) = &main {
        scene.joints = skeleton_joints(skeleton);
        scene.clips = clips(skeleton, animations);
        if scene.name.is_empty() {
            scene.name = skeleton.name.clone();
        }
    }
    scene.materials = materials.list;
    Ok(scene)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::crc32;
    use crate::resource::material::{Material, Param, ParamValue};

    fn shading(shader: &str, floats: &[(&str, [f32; 4])]) -> Shading {
        let params = floats
            .iter()
            .map(|(name, v)| Param {
                hash: crc32(name.as_bytes()),
                name: (*name).into(),
                source: String::new(),
                value: ParamValue::Floats(v.to_vec()),
            })
            .collect();
        Shading::from_material(&Material {
            version: 19,
            name: "m".into(),
            name_hash: 0,
            flags: 0,
            shaders: [
                (0, 0, 0),
                (crc32(shader.as_bytes()), 0, 0),
                (0, 0, 0),
                (0, 0, 0),
            ],
            shadow_shader: 0,
            params,
        })
    }

    #[test]
    fn eyes_tint_the_iris_and_are_opaque() {
        let s = shading("gf_char_eye", &[("g_EyeTintColor", [0.5, 0.25, 1.0, 1.0])]);
        // Sclera (alpha 0) keeps its colour; the iris (alpha 255) is tinted.
        let px = bake_pixels(
            &s,
            2,
            1,
            &[200, 200, 200, 0, 200, 200, 200, 255],
            true,
            None,
        );
        assert_eq!(&px[..4], &[200, 200, 200, 255]);
        let tinted = |t: f32| linear_to_srgb(srgb_to_linear(200) * t);
        assert_eq!(&px[4..], &[tinted(0.5), tinted(0.25), 200, 255]);
    }

    #[test]
    fn clothes_and_hair_mix_their_tints() {
        let tints = [
            ("g_TintColor0", [1.0, 0.0, 0.0, 1.0]),
            ("g_TintColor1", [0.0, 1.0, 0.0, 1.0]),
            ("g_TintColor2", [0.0, 0.0, 1.0, 1.0]),
        ];
        // A mask of pure red and green: the first two tints.
        let cloth = shading("gf_char_tint_mask_basic", &tints);
        let px = bake_pixels(&cloth, 2, 1, &[255, 0, 0, 255, 0, 255, 0, 128], false, None);
        assert_eq!(px, [255, 0, 0, 255, 0, 255, 0, 128]);
        // Hair: screen layers, alpha from the gloss map's green (2 × 2,
        // sampled at the nearest texel of the 2 × 1 base).
        let hair = shading("gf_char_hair_aniso_tint", &tints);
        let gloss = (
            2,
            2,
            vec![0, 10, 0, 0, 0, 20, 0, 0, 0, 30, 0, 0, 0, 40, 0, 0],
        );
        let px = bake_pixels(
            &hair,
            2,
            1,
            &[255, 255, 0, 0, 0, 0, 0, 0],
            false,
            Some(&gloss),
        );
        assert_eq!(px, [255, 255, 0, 10, 0, 0, 0, 20]);
    }
}

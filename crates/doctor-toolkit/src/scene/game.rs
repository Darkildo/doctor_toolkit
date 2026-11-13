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
                entry.base_color = self.image(source, diffuse, false);
                entry.alpha = alpha_mask && entry.base_color.is_some();
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

//! Reading glTF 2.0 (`.gltf` with external or embedded buffers, `.glb`)
//! into a [`Scene`].
//!
//! Coordinates are taken as they are: the game uses Y up and the same
//! handedness and units as glTF, and the exporter
//! ([`crate::export::gltf`]) writes positions without a transform. Node
//! transforms of unskinned meshes are applied; skinned meshes are in bind
//! space, where glTF ignores the node transform.
//!
//! Every triangle primitive becomes one [`ScenePart`]; strips and fans are
//! turned into lists, points and lines are skipped with a warning. Joints
//! are the nodes of all skins (merged by name; without skins, every node
//! without a mesh), animations are sampled at the game's 29.97 frames per
//! second ([`SAMPLE_RATE`]).

use std::collections::HashMap;
use std::path::Path;

use crate::resource::mesh_build::PartGeometry;
use crate::resource::rcb::{Mat4, multiply};
use crate::scene::anim::{Keys, sample_keys};
use crate::scene::math::{self, IDENTITY};
use crate::scene::{
    JointTrack, Scene, SceneClip, SceneImage, SceneJoint, SceneMaterial, ScenePart,
};
use crate::{Error, Result};

/// An image of a material (see [`SceneImage`]).
pub type GltfImage = SceneImage;
/// A material (see [`SceneMaterial`]).
pub type GltfMaterial = SceneMaterial;
/// One triangle primitive (see [`ScenePart`]).
pub type GltfPart = ScenePart;
/// A joint (see [`SceneJoint`]).
pub type GltfJoint = SceneJoint;
/// A glTF file read into a scene (see [`Scene`]).
pub type GltfModel = Scene;

/// Frame rate animations are sampled at (glTF has times, not frames): the
/// game's, so that clips exported by this toolkit come back key for key.
pub const SAMPLE_RATE: f32 = crate::resource::anim::FRAME_RATE;

fn invalid(message: impl Into<String>) -> Error {
    Error::InvalidArgument(message.into())
}

/// Reads a `.glb` or `.gltf` file. External buffers and images are looked up
/// next to it.
pub fn read_file(path: &Path) -> Result<Scene> {
    let bytes = std::fs::read(path).map_err(|e| Error::io(path, e))?;
    read(&bytes, path.parent())
}

/// Loads an external or data URI; external files must lie next to the model.
fn load_uri(uri: &str, base: Option<&Path>) -> Result<Vec<u8>> {
    if let Some(rest) = uri.strip_prefix("data:") {
        let (_, data) = rest
            .split_once(";base64,")
            .ok_or_else(|| invalid("glTF: data URI is not base64"))?;
        return base64_decode(data);
    }
    let base = base.ok_or_else(|| invalid(format!("glTF: external file `{uri}`")))?;
    let decoded = percent_decode(uri);
    if decoded.contains("..") || decoded.starts_with('/') || decoded.contains(':') {
        return Err(invalid(format!(
            "glTF: refusing file outside the model folder: `{uri}`"
        )));
    }
    let path = base.join(&decoded);
    std::fs::read(&path).map_err(|e| Error::io(&path, e))
}

/// The data of every buffer of a glTF document.
pub(crate) fn load_buffers(gltf: &gltf::Gltf, base: Option<&Path>) -> Result<Vec<Vec<u8>>> {
    let mut buffers: Vec<Vec<u8>> = Vec::new();
    for buffer in gltf.document.buffers() {
        let data = match buffer.source() {
            gltf::buffer::Source::Bin => gltf
                .blob
                .clone()
                .ok_or_else(|| invalid("glTF: binary chunk missing"))?,
            gltf::buffer::Source::Uri(uri) => load_uri(uri, base)?,
        };
        if data.len() < buffer.length() {
            return Err(invalid(format!(
                "glTF: buffer {} has {} bytes, {} expected",
                buffer.index(),
                data.len(),
                buffer.length()
            )));
        }
        buffers.push(data);
    }
    Ok(buffers)
}

/// Reads glTF from memory; `base` resolves relative URIs (none: only
/// embedded data is allowed).
pub fn read(bytes: &[u8], base: Option<&Path>) -> Result<Scene> {
    let gltf = gltf::Gltf::from_slice(bytes).map_err(|e| invalid(format!("glTF: {e}")))?;
    let document = &gltf.document;
    let load_uri = |uri: &str| load_uri(uri, base);
    let buffers = load_buffers(&gltf, base)?;
    let get = |b: gltf::Buffer<'_>| buffers.get(b.index()).map(Vec::as_slice);

    let mut model = Scene::default();
    // Images, then materials.
    let mut images: HashMap<usize, GltfImage> = HashMap::new();
    let mut image = |texture: gltf::Texture<'_>, model: &mut Scene| -> Option<GltfImage> {
        let source = texture.source();
        if let Some(found) = images.get(&source.index()) {
            return Some(found.clone());
        }
        let name = source
            .name()
            .map_or_else(|| format!("image{}", source.index()), str::to_owned);
        let loaded = match source.source() {
            gltf::image::Source::View { view, mime_type } => get(view.buffer())
                .and_then(|b| b.get(view.offset()..view.offset() + view.length()))
                .map(|b| (b.to_vec(), mime_type.to_owned()))
                .ok_or_else(|| invalid(format!("image `{name}`: buffer view out of range"))),
            gltf::image::Source::Uri { uri, mime_type } => {
                load_uri(uri).map(|b| (b, mime_type.unwrap_or("").to_owned()))
            }
        };
        match loaded {
            Ok((bytes, mime_type)) => {
                let img = GltfImage {
                    name,
                    mime_type,
                    bytes,
                };
                images.insert(source.index(), img.clone());
                Some(img)
            }
            Err(e) => {
                model.warnings.push(e.to_string());
                None
            }
        }
    };
    for material in document.materials() {
        let pbr = material.pbr_metallic_roughness();
        let base_color = pbr
            .base_color_texture()
            .and_then(|t| image(t.texture(), &mut model));
        let normal = material
            .normal_texture()
            .and_then(|t| image(t.texture(), &mut model));
        model.materials.push(GltfMaterial {
            name: material.name().map_or_else(
                || format!("material{}", model.materials.len()),
                str::to_owned,
            ),
            base_color,
            normal,
            specular: None,
            base_color_factor: pbr.base_color_factor(),
            alpha: material.alpha_mode() != gltf::material::AlphaMode::Opaque,
        });
    }

    // Node hierarchy: parents and world matrices of every node.
    let node_count = document.nodes().len();
    let mut parent_of: Vec<Option<usize>> = vec![None; node_count];
    for node in document.nodes() {
        for child in node.children() {
            if child.index() != node.index() {
                parent_of[child.index()] = Some(node.index());
            }
        }
    }
    let locals: Vec<Mat4> = document
        .nodes()
        .map(|n| flatten(n.transform().matrix()))
        .collect();
    let mut worlds: Vec<Option<Mat4>> = vec![None; node_count];
    for i in 0..node_count {
        world_of(i, &parent_of, &locals, &mut worlds, 0);
    }
    let world = |i: usize| worlds[i].unwrap_or(IDENTITY);

    // Walk the default scene (or every root node), depth first, in file order.
    let roots: Vec<gltf::Node<'_>> = match document
        .default_scene()
        .or_else(|| document.scenes().next())
    {
        Some(scene) => scene.nodes().collect(),
        None => document
            .nodes()
            .filter(|n| parent_of[n.index()].is_none())
            .collect(),
    };
    let mut order: Vec<gltf::Node<'_>> = Vec::new();
    let mut seen = vec![false; node_count];
    let mut stack: Vec<gltf::Node<'_>> = roots.into_iter().rev().collect();
    while let Some(node) = stack.pop() {
        if std::mem::replace(&mut seen[node.index()], true) {
            continue;
        }
        let children: Vec<_> = node.children().collect();
        for child in children.into_iter().rev() {
            stack.push(child);
        }
        order.push(node);
    }

    // Joints: the nodes of all skins by name, with their parents.
    let mut joint_index: HashMap<String, usize> = HashMap::new();
    let mut joint_node: Vec<usize> = Vec::new();
    let mut joint_names: HashMap<usize, String> = HashMap::new();
    let node_name = |n: &gltf::Node<'_>| {
        n.name()
            .map_or_else(|| format!("joint{}", n.index()), str::to_owned)
    };
    let mut skin_ibms: HashMap<usize, Mat4> = HashMap::new();
    let mut skin_nodes: Vec<usize> = Vec::new();
    for skin in document.skins() {
        let ibms: Vec<Mat4> = skin
            .reader(get)
            .read_inverse_bind_matrices()
            .map(|m| m.map(flatten).collect())
            .unwrap_or_default();
        for (k, joint) in skin.joints().enumerate() {
            skin_ibms
                .entry(joint.index())
                .or_insert_with(|| ibms.get(k).copied().unwrap_or(IDENTITY));
            skin_nodes.push(joint.index());
        }
    }
    let is_joint: Vec<bool> = if document.skins().len() > 0 {
        (0..node_count)
            .map(|i| skin_ibms.contains_key(&i))
            .collect()
    } else {
        document
            .nodes()
            .map(|n| n.mesh().is_none() && n.camera().is_none())
            .collect()
    };
    // Joints in hierarchy order, so that parents come first.
    for node in &order {
        if !is_joint[node.index()] {
            continue;
        }
        let name = node_name(node);
        if joint_index.contains_key(&name) {
            continue;
        }
        let i = node.index();
        let parent = {
            let mut p = parent_of[i];
            while let Some(n) = p {
                if is_joint[n] && joint_names.contains_key(&n) {
                    break;
                }
                p = parent_of[n];
            }
            p
        };
        let local = match parent {
            Some(p) => multiply(&math::invert(&world(p)), &world(i)),
            None => world(i),
        };
        let inverse_bind = skin_ibms
            .get(&i)
            .copied()
            .unwrap_or_else(|| math::invert(&world(i)));
        model.joints.push(SceneJoint {
            name: name.clone(),
            parent: parent
                .and_then(|p| joint_names.get(&p))
                .and_then(|n| joint_index.get(n).copied()),
            local,
            inverse_bind,
        });
        joint_index.insert(name.clone(), model.joints.len() - 1);
        joint_names.insert(i, name);
        joint_node.push(i);
    }
    // Skin joints outside the scene graph walk.
    for &i in &skin_nodes {
        let node = document.nodes().nth(i).expect("skin joint index");
        let name = node_name(&node);
        if !joint_index.contains_key(&name) {
            model.joints.push(SceneJoint {
                name: name.clone(),
                parent: None,
                local: world(i),
                inverse_bind: skin_ibms[&i],
            });
            joint_index.insert(name.clone(), model.joints.len() - 1);
            joint_names.insert(i, name);
            joint_node.push(i);
        }
    }

    for node in &order {
        let Some(mesh) = node.mesh() else { continue };
        let node_world = world(node.index());
        if model.name.is_empty() {
            model.name = mesh.name().or(node.name()).unwrap_or("model").to_owned();
        }
        // Joint numbers of this node's skin → global joint list.
        let skin_joints: Option<Vec<usize>> = node.skin().map(|skin| {
            skin.joints()
                .map(|joint| joint_index[&node_name(&joint)])
                .collect()
        });
        let object = mesh.name().or(node.name()).unwrap_or("part").to_owned();
        let several = mesh.primitives().len() > 1;
        for primitive in mesh.primitives() {
            let name = if several {
                format!("{object}.{}", primitive.index())
            } else {
                object.clone()
            };
            match read_primitive(&primitive, get, &name, skin_joints.as_deref(), &node_world) {
                Ok(Some((geometry, mut warnings))) => {
                    model.warnings.append(&mut warnings);
                    model.parts.push(GltfPart {
                        name,
                        object: object.clone(),
                        material: primitive.material().index(),
                        geometry,
                    });
                }
                Ok(None) => model.warnings.push(format!(
                    "part `{name}`: {:?} primitives are skipped",
                    primitive.mode()
                )),
                Err(e) => return Err(e),
            }
        }
    }

    // Animations of joint nodes.
    let joint_of_node: HashMap<usize, usize> = joint_node
        .iter()
        .enumerate()
        .map(|(j, &n)| (n, j))
        .collect();
    for animation in document.animations() {
        let mut keys: HashMap<usize, [Option<Keys>; 3]> = HashMap::new();
        let mut end = 0f32;
        let mut start = f32::INFINITY;
        for channel in animation.channels() {
            let Some(&joint) = joint_of_node.get(&channel.target().node().index()) else {
                continue;
            };
            let reader = channel.reader(get);
            let Some(times) = reader.read_inputs().map(|t| t.collect::<Vec<f32>>()) else {
                continue;
            };
            let cubic =
                channel.sampler().interpolation() == gltf::animation::Interpolation::CubicSpline;
            let step = channel.sampler().interpolation() == gltf::animation::Interpolation::Step;
            let pick = |v: Vec<Vec<f32>>| -> Vec<Vec<f32>> {
                if cubic {
                    v.chunks(3).filter_map(|c| c.get(1).cloned()).collect()
                } else {
                    v
                }
            };
            let (slot, values) = match reader.read_outputs() {
                Some(gltf::animation::util::ReadOutputs::Translations(t)) => {
                    (1, pick(t.map(|v| v.to_vec()).collect()))
                }
                Some(gltf::animation::util::ReadOutputs::Rotations(r)) => {
                    (0, pick(r.into_f32().map(|v| v.to_vec()).collect()))
                }
                Some(gltf::animation::util::ReadOutputs::Scales(s)) => {
                    (2, pick(s.map(|v| v.to_vec()).collect()))
                }
                _ => continue,
            };
            if values.len() != times.len() || times.is_empty() {
                model.warnings.push(format!(
                    "animation `{}`: a channel has {} keys for {} times; skipped",
                    animation.name().unwrap_or(""),
                    values.len(),
                    times.len()
                ));
                continue;
            }
            end = end.max(*times.last().unwrap_or(&0.0));
            start = start.min(times[0]);
            keys.entry(joint).or_default()[slot] = Some(Keys {
                times,
                values,
                step,
            });
        }
        if keys.is_empty() {
            continue;
        }
        // Clips start at their first key.
        let start = if start.is_finite() {
            start.max(0.0)
        } else {
            0.0
        };
        let frames = (((end - start).max(0.0) * SAMPLE_RATE).round() as usize + 1).min(1 << 20);
        if frames.saturating_mul(keys.len()) > 50_000_000 {
            model.warnings.push(format!(
                "animation `{}` is too long; skipped",
                animation.name().unwrap_or("")
            ));
            continue;
        }
        let mut tracks: Vec<JointTrack> = keys
            .into_iter()
            .map(|(joint, [r, t, s])| {
                // Channels animate the node's own local transform; nodes that
                // are not joints between the joint and its parent joint (an
                // armature node) are a fixed offset in front of it.
                let node = joint_node[joint];
                let rest = math::decompose(&locals[node]);
                let above = model.joints[joint]
                    .parent
                    .map_or(IDENTITY, |p| world(joint_node[p]));
                let offset = match parent_of[node] {
                    Some(p) => multiply(&math::invert(&above), &world(p)),
                    None => math::invert(&above),
                };
                let offset = if offset
                    .iter()
                    .zip(&IDENTITY)
                    .all(|(a, b)| (a - b).abs() < 1e-6)
                {
                    None
                } else {
                    Some(offset)
                };
                let sample = |k: &Option<Keys>, f: usize, rest: &[f32]| -> Vec<f32> {
                    match k {
                        Some(k) => {
                            sample_keys(k, start + f as f32 / SAMPLE_RATE, k.values[0].len() == 4)
                        }
                        None => rest.to_vec(),
                    }
                };
                let mut track = JointTrack {
                    joint,
                    ..Default::default()
                };
                for f in 0..frames {
                    let q = sample(&r, f, &rest.1);
                    let q = math::quat_normalize([q[0], q[1], q[2], q[3]]);
                    let v = sample(&t, f, &rest.0);
                    let tr = [v[0], v[1], v[2]];
                    let sc = if s.is_some() || offset.is_some() {
                        let v = sample(&s, f, &rest.2);
                        [v[0], v[1], v[2]]
                    } else {
                        [1.0; 3]
                    };
                    let (tr, q, sc) = match &offset {
                        Some(o) => math::decompose(&multiply(o, &math::compose(tr, q, sc))),
                        None => (tr, q, sc),
                    };
                    track.rotations.push(q);
                    track.translations.push(tr);
                    if s.is_some() || offset.is_some() {
                        track.scales.push(sc);
                    }
                }
                if track
                    .scales
                    .iter()
                    .all(|s| s.iter().all(|x| (x - 1.0).abs() < 1e-5))
                {
                    track.scales.clear();
                }
                track
            })
            .collect();
        tracks.sort_by_key(|t| t.joint);
        model.clips.push(SceneClip {
            name: animation
                .name()
                .map_or_else(|| format!("animation{}", animation.index()), str::to_owned),
            frame_rate: SAMPLE_RATE,
            tracks,
        });
    }
    Ok(model)
}

fn world_of(
    i: usize,
    parent_of: &[Option<usize>],
    locals: &[Mat4],
    worlds: &mut [Option<Mat4>],
    depth: usize,
) -> Mat4 {
    if let Some(w) = worlds[i] {
        return w;
    }
    let w = match parent_of[i] {
        // A cycle (invalid file) ends at depth 256.
        Some(p) if depth < 256 => multiply(
            &world_of(p, parent_of, locals, worlds, depth + 1),
            &locals[i],
        ),
        _ => locals[i],
    };
    worlds[i] = Some(w);
    w
}

type Warnings = Vec<String>;

fn read_primitive<'a, 's, F>(
    primitive: &'a gltf::Primitive<'a>,
    get: F,
    name: &str,
    skin_joints: Option<&[usize]>,
    world: &Mat4,
) -> Result<Option<(PartGeometry, Warnings)>>
where
    F: Clone + Fn(gltf::Buffer<'a>) -> Option<&'s [u8]>,
{
    use gltf::mesh::Mode;
    let mut warnings = Vec::new();
    let reader = primitive.reader(get);
    let positions: Vec<[f32; 3]> = reader
        .read_positions()
        .ok_or_else(|| invalid(format!("part `{name}` has no positions")))?
        .collect();
    let n = positions.len() as u32;
    let indices: Vec<u32> = match reader.read_indices() {
        Some(i) => i.into_u32().collect(),
        None => (0..n).collect(),
    };
    if indices.iter().any(|&i| i >= n) {
        return Err(invalid(format!("part `{name}`: index out of range")));
    }
    let triangles: Vec<[u32; 3]> = match primitive.mode() {
        Mode::Triangles => indices
            .chunks_exact(3)
            .map(|t| [t[0], t[1], t[2]])
            .collect(),
        Mode::TriangleStrip => indices
            .windows(3)
            .enumerate()
            .map(|(k, w)| {
                if k % 2 == 0 {
                    [w[0], w[1], w[2]]
                } else {
                    [w[1], w[0], w[2]]
                }
            })
            .collect(),
        Mode::TriangleFan => indices
            .windows(2)
            .skip(1)
            .map(|w| [indices[0], w[0], w[1]])
            .collect(),
        _ => return Ok(None),
    };
    let triangles: Vec<[u32; 3]> = triangles
        .into_iter()
        .filter(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2])
        .collect();
    let normals: Option<Vec<[f32; 3]>> = reader.read_normals().map(Iterator::collect);
    let tangents: Option<Vec<[f32; 4]>> = reader.read_tangents().map(Iterator::collect);
    let uv0 = reader.read_tex_coords(0).map(|t| t.into_f32().collect());
    let uv1 = reader.read_tex_coords(1).map(|t| t.into_f32().collect());
    let colors = reader.read_colors(0).map(|c| c.into_rgba_u8().collect());
    let skin = match skin_joints {
        Some(map) => {
            let joints: Option<Vec<[u16; 4]>> =
                reader.read_joints(0).map(|j| j.into_u16().collect());
            let weights: Option<Vec<[f32; 4]>> =
                reader.read_weights(0).map(|w| w.into_f32().collect());
            if reader
                .read_weights(1)
                .is_some_and(|w| w.into_f32().any(|w| w.iter().any(|&x| x > 1e-4)))
            {
                warnings.push(format!(
                    "part `{name}`: more than 4 weights per vertex; only the first 4 are used"
                ));
            }
            match (joints, weights) {
                (Some(j), Some(w)) if j.len() == positions.len() && w.len() == positions.len() => {
                    let mut out = Vec::with_capacity(j.len());
                    for (j, w) in j.iter().zip(&w) {
                        let mut jj = [0u16; 4];
                        let mut ww = [0f32; 4];
                        for k in 0..4 {
                            if w[k] > 0.0 {
                                let global = map.get(j[k] as usize).ok_or_else(|| {
                                    invalid(format!(
                                        "part `{name}`: joint {} not in the skin",
                                        j[k]
                                    ))
                                })?;
                                jj[k] = *global as u16;
                                ww[k] = w[k];
                            }
                        }
                        let sum: f32 = ww.iter().sum();
                        if sum <= 0.0 {
                            return Err(invalid(format!(
                                "part `{name}`: a vertex has no bone weights"
                            )));
                        }
                        out.push((jj, ww.map(|x| x / sum)));
                    }
                    Some(out)
                }
                _ => {
                    warnings.push(format!(
                        "part `{name}`: skinned node without JOINTS_0/WEIGHTS_0; imported unskinned"
                    ));
                    None
                }
            }
        }
        None => None,
    };
    let (positions, normals, tangents) = if skin.is_some() || *world == IDENTITY {
        (positions, normals, tangents)
    } else {
        transform(world, positions, normals, tangents)
    };
    Ok(Some((
        PartGeometry {
            name: name.to_owned(),
            material_hash: 0,
            positions,
            normals,
            tangents,
            uv0,
            uv1,
            colors,
            skin,
            triangles,
        },
        warnings,
    )))
}

type Transformed = (Vec<[f32; 3]>, Option<Vec<[f32; 3]>>, Option<Vec<[f32; 4]>>);

fn transform(
    m: &Mat4,
    positions: Vec<[f32; 3]>,
    normals: Option<Vec<[f32; 3]>>,
    tangents: Option<Vec<[f32; 4]>>,
) -> Transformed {
    let apply = |v: [f32; 3], w: f32| -> [f32; 3] {
        [0, 1, 2].map(|r| m[r] * v[0] + m[4 + r] * v[1] + m[8 + r] * v[2] + m[12 + r] * w)
    };
    // Normals: inverse transpose of the 3×3 part.
    let inv = crate::resource::rcb::invert_affine(m);
    let normal = |v: [f32; 3]| -> [f32; 3] {
        let n =
            [0, 1, 2].map(|r| inv[r * 4] * v[0] + inv[r * 4 + 1] * v[1] + inv[r * 4 + 2] * v[2]);
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-12);
        n.map(|x| x / len)
    };
    let positions = positions.into_iter().map(|p| apply(p, 1.0)).collect();
    let normals = normals.map(|n| n.into_iter().map(normal).collect());
    let det = {
        let a = |r: usize, c: usize| m[c * 4 + r];
        a(0, 0) * (a(1, 1) * a(2, 2) - a(1, 2) * a(2, 1))
            - a(0, 1) * (a(1, 0) * a(2, 2) - a(1, 2) * a(2, 0))
            + a(0, 2) * (a(1, 0) * a(2, 1) - a(1, 1) * a(2, 0))
    };
    let tangents = tangents.map(|t| {
        t.into_iter()
            .map(|t| {
                let v = apply([t[0], t[1], t[2]], 0.0);
                let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-12);
                [
                    v[0] / len,
                    v[1] / len,
                    v[2] / len,
                    if det < 0.0 { -t[3] } else { t[3] },
                ]
            })
            .collect()
    });
    (positions, normals, tangents)
}

fn flatten(m: [[f32; 4]; 4]) -> Mat4 {
    let mut out = [0f32; 16];
    for (c, col) in m.iter().enumerate() {
        out[4 * c..4 * c + 4].copy_from_slice(col);
    }
    out
}

fn base64_decode(text: &str) -> Result<Vec<u8>> {
    let value = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        } as u32)
    };
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0;
    for c in text.bytes() {
        if c == b'=' || c.is_ascii_whitespace() {
            continue;
        }
        let v = value(c).ok_or_else(|| invalid("glTF: bad base64 data"))?;
        acc = acc << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_and_percent() {
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64_decode("AAEC").unwrap(), [0, 1, 2]);
        assert_eq!(percent_decode("a%20b.png"), "a b.png");
    }

    fn base64(data: &[u8]) -> String {
        const ABC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in data.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
            for k in 0..4 {
                if k <= chunk.len() {
                    out.push(ABC[(n >> (18 - 6 * k) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    /// A glTF with one skinned quad (a strip) under a translated node, a
    /// second node with an unskinned triangle, a material with an embedded
    /// PNG and two joints.
    fn sample(extra_weights: bool) -> Vec<u8> {
        let mut bin = Vec::new();
        let positions: [[f32; 3]; 4] = [[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [1., 1., 0.]];
        for p in positions {
            for v in p {
                bin.extend_from_slice(&v.to_le_bytes());
            }
        }
        for i in [0u16, 1, 2, 3] {
            bin.extend_from_slice(&i.to_le_bytes());
        }
        // joints (u8 × 4) and weights (f32 × 4)
        for j in [[0u8, 1, 0, 0], [0, 1, 0, 0], [1, 0, 0, 0], [1, 0, 0, 0]] {
            bin.extend_from_slice(&j);
        }
        for w in [
            [0.5f32, 0.5, 0., 0.],
            [1., 1., 0., 0.],
            [1., 0., 0., 0.],
            [1., 0., 0., 0.],
        ] {
            for v in w {
                bin.extend_from_slice(&v.to_le_bytes());
            }
        }
        let png = crate::resource::pixels::encode_png(2, 2, &[255; 16]).unwrap();
        let weights1 = if extra_weights {
            r#","WEIGHTS_1":5,"JOINTS_1":4"#
        } else {
            ""
        };
        let json = format!(
            r#"{{
  "asset": {{"version": "2.0"}},
  "scene": 0,
  "scenes": [{{"nodes": [0, 3, 1]}}],
  "nodes": [
    {{"name": "body", "mesh": 0, "skin": 0, "translation": [5, 0, 0]}},
    {{"name": "root", "children": [2]}},
    {{"name": "m_pelvis"}},
    {{"name": "prop", "mesh": 1, "translation": [0, 0, 2], "scale": [2, 2, 2]}}
  ],
  "skins": [{{"joints": [1, 2]}}],
  "meshes": [
    {{"name": "body", "primitives": [{{"attributes": {{"POSITION": 0, "JOINTS_0": 2, "WEIGHTS_0": 3{weights1}}}, "indices": 1, "mode": 5, "material": 0}}]}},
    {{"name": "prop", "primitives": [{{"attributes": {{"POSITION": 0}}, "mode": 4}}]}}
  ],
  "materials": [{{"name": "skin_mat", "pbrMetallicRoughness": {{"baseColorTexture": {{"index": 0}}}}}}],
  "textures": [{{"source": 0}}],
  "images": [{{"uri": "data:image/png;base64,{png}"}}],
  "accessors": [
    {{"bufferView": 0, "componentType": 5126, "count": 4, "type": "VEC3", "min": [0,0,0], "max": [1,1,0]}},
    {{"bufferView": 1, "componentType": 5123, "count": 4, "type": "SCALAR"}},
    {{"bufferView": 2, "componentType": 5121, "count": 4, "type": "VEC4"}},
    {{"bufferView": 3, "componentType": 5126, "count": 4, "type": "VEC4"}},
    {{"bufferView": 2, "componentType": 5121, "count": 4, "type": "VEC4"}},
    {{"bufferView": 3, "componentType": 5126, "count": 4, "type": "VEC4"}}
  ],
  "bufferViews": [
    {{"buffer": 0, "byteOffset": 0, "byteLength": 48}},
    {{"buffer": 0, "byteOffset": 48, "byteLength": 8}},
    {{"buffer": 0, "byteOffset": 56, "byteLength": 16}},
    {{"buffer": 0, "byteOffset": 72, "byteLength": 64}}
  ],
  "buffers": [{{"byteLength": {len}, "uri": "data:application/octet-stream;base64,{data}"}}]
}}"#,
            png = base64(&png),
            len = bin.len(),
            data = base64(&bin),
        );
        json.into_bytes()
    }

    #[test]
    fn reads_parts_materials_and_skins() {
        let model = read(&sample(false), None).unwrap();
        assert_eq!(model.name, "body");
        assert_eq!(model.parts.len(), 2);
        let body = &model.parts[0];
        assert_eq!(body.name, "body");
        // The strip 0 1 2 3 gives (0 1 2) and (2 1 3).
        assert_eq!(body.geometry.triangles, vec![[0, 1, 2], [2, 1, 3]]);
        // Skinned: the node transform does not apply.
        assert_eq!(body.geometry.positions[1], [1.0, 0.0, 0.0]);
        let skin = body.geometry.skin.as_ref().unwrap();
        assert_eq!(model.joints.len(), 2);
        assert_eq!(model.joints[1].name, "m_pelvis");
        assert_eq!(skin[0].0[..2], [0, 1]);
        assert_eq!(skin[1].1, [0.5, 0.5, 0.0, 0.0], "weights are normalised");
        let material = &model.materials[body.material.unwrap()];
        assert_eq!(material.name, "skin_mat");
        let (w, h, rgba) = material.base_color.as_ref().unwrap().rgba().unwrap();
        assert_eq!((w, h, rgba.len()), (2, 2, 16));
        // The prop is not skinned: its node transform applies.
        let prop = &model.parts[1];
        assert_eq!(prop.geometry.positions[1], [2.0, 0.0, 2.0]);
        assert!(prop.geometry.skin.is_none());
        assert_eq!(prop.geometry.triangles, vec![[0, 1, 2]]);
        assert!(model.warnings.is_empty(), "{:?}", model.warnings);
    }

    #[test]
    fn warns_about_more_than_four_weights() {
        let model = read(&sample(true), None).unwrap();
        assert!(
            model
                .warnings
                .iter()
                .any(|w| w.contains("more than 4 weights"))
        );
    }

    #[test]
    fn rejects_bad_files() {
        assert!(read(b"not gltf", None).is_err());
        let text = String::from_utf8(sample(false)).unwrap();
        // External files need a folder, and may not leave it.
        let external = text.replacen("data:application/octet-stream;base64,", "../", 1);
        assert!(read(external.as_bytes(), None).is_err());
        assert!(read(external.as_bytes(), Some(std::path::Path::new("/tmp"))).is_err());
    }
}

//! Importing animations into banks (`bnk`): reading glTF animations,
//! fitting them to a game skeleton and writing them as a clip.
//!
//! A file is read into [`SourceAnimation`]s (joints with a rest pose and
//! local samples at the game's 29.97 frames per second). [`retarget`] maps
//! the joints to the bones of the game skeleton by name and transfers each
//! joint's rotation relative to its rest pose in model space, so that a
//! skeleton with other bone axes or proportions still drives the game's
//! bones; translations move by the source's offset from its rest pose,
//! scaled by the ratio of the skeletons' heights. [`import_clip`] encodes
//! the tracks and replaces a clip of a bank or adds a new one with a named
//! entry.
//!
//! glTF is read here; FBX and USD go through the shared scene of stage 25
//! ([`crate::scene::read_file`]) and [`read_scene`].

use std::collections::HashMap;
use std::path::Path;

use serde::Serialize;

use crate::hash::sdbm;
use crate::resource::anim::{
    AnimBank, BoneTrack, FRAME_RATE, SourceAnimation, SourceJoint, bone_set, find_skeleton,
};
use crate::resource::anim_encode::{EncodeOptions, encode_clip};
use crate::resource::anim_image::BankImage;
use crate::resource::rcb::{Mat4, Rig, Skeleton};
use crate::scene::{ModelFormat, Scene};
use crate::{Error, Result};

fn invalid(message: impl Into<String>) -> Error {
    Error::InvalidArgument(message.into())
}

// ---------------------------------------------------------------------------
// Quaternion helpers (x, y, z, w).

type Quat = [f32; 4];
type Vec3 = [f32; 3];

const IDENTITY: Quat = [0.0, 0.0, 0.0, 1.0];

fn qnorm(q: Quat) -> Quat {
    let n = q.iter().map(|v| v * v).sum::<f32>().sqrt();
    if n > 1e-8 { q.map(|v| v / n) } else { IDENTITY }
}

fn slerp(a: Quat, b: Quat, t: f32) -> Quat {
    let mut d: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
    let b = if d < 0.0 {
        d = -d;
        b.map(|v| -v)
    } else {
        b
    };
    if d > 0.9995 {
        return qnorm([0, 1, 2, 3].map(|i| a[i] + (b[i] - a[i]) * t));
    }
    let th = d.min(1.0).acos();
    let s = th.sin();
    let (wa, wb) = (((1.0 - t) * th).sin() / s, (t * th).sin() / s);
    qnorm([0, 1, 2, 3].map(|i| a[i] * wa + b[i] * wb))
}

/// Rotation and translation of an affine column-major matrix (scale removed).
fn decompose(m: &Mat4) -> (Quat, Vec3) {
    let col = |c: usize| {
        let v = [m[4 * c], m[4 * c + 1], m[4 * c + 2]];
        let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-12);
        v.map(|x| x / n)
    };
    let (x, y, z) = (col(0), col(1), col(2));
    // Rotation matrix r[row][col].
    let r = [[x[0], y[0], z[0]], [x[1], y[1], z[1]], [x[2], y[2], z[2]]];
    let trace = r[0][0] + r[1][1] + r[2][2];
    let q = if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        [
            (r[2][1] - r[1][2]) / s,
            (r[0][2] - r[2][0]) / s,
            (r[1][0] - r[0][1]) / s,
            0.25 * s,
        ]
    } else if r[0][0] > r[1][1] && r[0][0] > r[2][2] {
        let s = (1.0 + r[0][0] - r[1][1] - r[2][2]).sqrt() * 2.0;
        [
            0.25 * s,
            (r[0][1] + r[1][0]) / s,
            (r[0][2] + r[2][0]) / s,
            (r[2][1] - r[1][2]) / s,
        ]
    } else if r[1][1] > r[2][2] {
        let s = (1.0 + r[1][1] - r[0][0] - r[2][2]).sqrt() * 2.0;
        [
            (r[0][1] + r[1][0]) / s,
            0.25 * s,
            (r[1][2] + r[2][1]) / s,
            (r[0][2] - r[2][0]) / s,
        ]
    } else {
        let s = (1.0 + r[2][2] - r[0][0] - r[1][1]).sqrt() * 2.0;
        [
            (r[0][2] + r[2][0]) / s,
            (r[1][2] + r[2][1]) / s,
            0.25 * s,
            (r[1][0] - r[0][1]) / s,
        ]
    };
    (qnorm(q), [m[12], m[13], m[14]])
}

// ---------------------------------------------------------------------------
// Reading.

/// Reads the animations of a file: glTF (`.glb`, `.gltf`) here, FBX and
/// USD (`.fbx`, `.usd`, `.usda`, `.usdc`, `.usdz`) through the scene reader.
pub fn read_animations(path: &Path) -> Result<Vec<SourceAnimation>> {
    match ModelFormat::from_path(path) {
        Some(ModelFormat::Glb | ModelFormat::Gltf) => {
            let bytes = std::fs::read(path).map_err(|e| Error::io(path, e))?;
            read_gltf(&bytes, path.parent())
        }
        Some(format) if format.has_skeletons() => read_scene(&crate::scene::read_file(path)?),
        _ => Err(invalid(format!(
            "{}: unknown animation file type (expected .glb, .gltf, .fbx, .usd, .usda, .usdc or .usdz)",
            path.display()
        ))),
    }
}

/// The clips of a scene (FBX, USD, …) as source animations: the joints with
/// their bind pose as the rest pose, samples resampled to [`FRAME_RATE`].
/// A joint keeps its own rest transform for what its track does not animate.
pub fn read_scene(scene: &Scene) -> Result<Vec<SourceAnimation>> {
    crate::scene::retarget::scene_animations(scene, FRAME_RATE)
}

/// One animated property of a node.
struct Channel {
    times: Vec<f32>,
    /// Values (4 or 3 per key; cubic splines: in-tangent, value, out-tangent).
    values: Vec<Vec<f32>>,
    interpolation: gltf::animation::Interpolation,
}

impl Channel {
    fn sample(&self, t: f32, rotation: bool) -> Vec<f32> {
        use gltf::animation::Interpolation::*;
        let cubic = self.interpolation == CubicSpline;
        let value = |k: usize| -> &Vec<f32> { &self.values[if cubic { 3 * k + 1 } else { k }] };
        let n = self.times.len();
        if n == 0 {
            return Vec::new();
        }
        if t <= self.times[0] || n == 1 {
            return value(0).clone();
        }
        if t >= self.times[n - 1] {
            return value(n - 1).clone();
        }
        let k = self.times.partition_point(|&x| x <= t).saturating_sub(1);
        let (t0, t1) = (self.times[k], self.times[k + 1]);
        let dt = (t1 - t0).max(1e-9);
        let u = ((t - t0) / dt).clamp(0.0, 1.0);
        match self.interpolation {
            Step => value(k).clone(),
            Linear if rotation => {
                let q = |v: &Vec<f32>| [v[0], v[1], v[2], v[3]];
                slerp(q(value(k)), q(value(k + 1)), u).to_vec()
            }
            Linear => value(k)
                .iter()
                .zip(value(k + 1))
                .map(|(a, b)| a + (b - a) * u)
                .collect(),
            CubicSpline => {
                let (p0, m0) = (value(k), &self.values[3 * k + 2]);
                let (p1, m1) = (value(k + 1), &self.values[3 * (k + 1)]);
                let (u2, u3) = (u * u, u * u * u);
                let h00 = 2.0 * u3 - 3.0 * u2 + 1.0;
                let h10 = u3 - 2.0 * u2 + u;
                let h01 = -2.0 * u3 + 3.0 * u2;
                let h11 = u3 - u2;
                let v: Vec<f32> = (0..p0.len())
                    .map(|i| h00 * p0[i] + h10 * dt * m0[i] + h01 * p1[i] + h11 * dt * m1[i])
                    .collect();
                if rotation {
                    qnorm([v[0], v[1], v[2], v[3]]).to_vec()
                } else {
                    v
                }
            }
        }
    }
}

/// Reads the animations of a glTF document. Every node is a joint; the rest
/// pose is the node's own transform.
pub fn read_gltf(bytes: &[u8], base: Option<&Path>) -> Result<Vec<SourceAnimation>> {
    let gltf = gltf::Gltf::from_slice(bytes).map_err(|e| invalid(format!("glTF: {e}")))?;
    let buffers = super::gltf::load_buffers(&gltf, base)?;
    let document = &gltf.document;
    let mut parent = vec![None; document.nodes().len()];
    for node in document.nodes() {
        for child in node.children() {
            parent[child.index()] = Some(node.index());
        }
    }
    let joints: Vec<SourceJoint> = document
        .nodes()
        .map(|node| {
            let (t, r, _) = node.transform().decomposed();
            SourceJoint {
                name: node
                    .name()
                    .map_or_else(|| format!("node{}", node.index()), str::to_owned),
                parent: parent[node.index()],
                rest_rotation: qnorm(r),
                rest_translation: t,
                rotations: None,
                translations: None,
            }
        })
        .collect();
    let mut out = Vec::new();
    for animation in document.animations() {
        let mut channels: HashMap<(usize, bool), Channel> = HashMap::new();
        let (mut start, mut end) = (f32::INFINITY, f32::NEG_INFINITY);
        for channel in animation.channels() {
            use gltf::animation::Property;
            let rotation = match channel.target().property() {
                Property::Rotation => true,
                Property::Translation => false,
                _ => continue,
            };
            let reader = channel.reader(|b| buffers.get(b.index()).map(Vec::as_slice));
            let times: Vec<f32> = reader
                .read_inputs()
                .ok_or_else(|| invalid("glTF: animation input missing"))?
                .collect();
            let values: Vec<Vec<f32>> = match reader.read_outputs() {
                Some(gltf::animation::util::ReadOutputs::Rotations(r)) => {
                    r.into_f32().map(|q| q.to_vec()).collect()
                }
                Some(gltf::animation::util::ReadOutputs::Translations(t)) => {
                    t.map(|v| v.to_vec()).collect()
                }
                _ => return Err(invalid("glTF: animation output missing")),
            };
            let interpolation = channel.sampler().interpolation();
            let keys = if interpolation == gltf::animation::Interpolation::CubicSpline {
                values.len() / 3
            } else {
                values.len()
            };
            if keys != times.len() || times.iter().any(|t| !t.is_finite()) {
                return Err(invalid("glTF: animation input and output differ in length"));
            }
            if let (Some(&a), Some(&b)) = (times.first(), times.last()) {
                start = start.min(a);
                end = end.max(b);
            }
            channels.insert(
                (channel.target().node().index(), rotation),
                Channel {
                    times,
                    values,
                    interpolation,
                },
            );
        }
        if channels.is_empty() || !start.is_finite() {
            continue;
        }
        let frames = (((end - start) * FRAME_RATE).round() as usize + 1).min(100_000);
        let mut anim = SourceAnimation {
            name: animation
                .name()
                .map_or_else(|| format!("animation{}", animation.index()), str::to_owned),
            frames,
            joints: joints.clone(),
        };
        for ((node, rotation), channel) in &channels {
            let samples =
                (0..frames).map(|f| channel.sample(start + f as f32 / FRAME_RATE, *rotation));
            let joint = &mut anim.joints[*node];
            if *rotation {
                joint.rotations = Some(samples.map(|v| qnorm([v[0], v[1], v[2], v[3]])).collect());
            } else {
                joint.translations = Some(samples.map(|v| [v[0], v[1], v[2]]).collect());
            }
        }
        out.push(anim);
    }
    if out.is_empty() {
        return Err(invalid("the file has no animation"));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Retargeting: shared with `scene::anim::clips_to_game`.

pub use crate::scene::retarget::{RestPose, Retargeted, match_name};

/// Fits a source animation to `bones` (indices of `skeleton`): a track per
/// bone. `map` names source joints for game bones (game name → joint name)
/// before matching by name; the rest pose is reconciled automatically
/// ([`crate::scene::retarget::retarget`]).
pub fn retarget(
    source: &SourceAnimation,
    skeleton: &Skeleton,
    bones: &[usize],
    map: &HashMap<String, String>,
) -> Result<Retargeted> {
    crate::scene::retarget::retarget(
        source,
        skeleton,
        bones,
        &crate::scene::retarget::RetargetOptions {
            rest: RestPose::Auto,
            map: map.clone(),
        },
    )
}

// ---------------------------------------------------------------------------
// Into a bank.

/// Where an imported animation goes.
#[derive(Debug, Clone, PartialEq)]
pub enum ClipTarget {
    /// Replace the animation of this clip (index in the bank): same name hash,
    /// bone set and flags.
    Replace(usize),
    /// Add a clip named `name` (its hash is SDBM of the lower-case name).
    Add {
        name: String,
        /// Also add a named entry `name` that plays the clip, so the game can
        /// ask for it by name.
        entry: bool,
        /// Priority of the entry's wrapper node.
        priority: u32,
        /// Copy flags (root motion, looping, bone set) from this clip;
        /// default: the first clip of the bank.
        like: Option<usize>,
    },
}

/// Import settings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportOptions {
    pub encode: EncodeOptions,
    /// Override the clip's looping flag.
    pub looping: Option<bool>,
    /// Bone set of an added clip (hash); default: that of the `like` clip.
    pub bone_set: Option<u32>,
    /// Game bone name → source joint name, before matching by name.
    pub map: HashMap<String, String>,
    /// How the file's rest pose is reconciled with the skeleton.
    pub rest: RestPose,
}

/// What an import did.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ImportSummary {
    /// `replace` or `add`.
    pub mode: String,
    pub clip: String,
    pub clip_index: usize,
    pub clip_hash: u32,
    pub frames: u32,
    pub bone_set: u32,
    pub bones: usize,
    pub mapped: Vec<String>,
    pub unmapped: Vec<String>,
    pub unused: Vec<String>,
    pub scale: f32,
    /// How the rest pose was reconciled (`AsIs`, `Axes`, `Retarget`).
    pub rest: String,
    pub looping: bool,
    pub scheme: u32,
    /// Error of the encoded clip against the retargeted tracks.
    pub max_angle: f32,
    pub max_translation: f32,
    pub streams_before: usize,
    pub streams_after: usize,
    pub bank_before: usize,
    pub bank_after: usize,
    pub entry: Option<String>,
    pub warnings: Vec<String>,
}

/// Writes `source` into the bank `data` as told by `target`; returns the new
/// bank and a summary.
pub fn import_clip(
    data: &[u8],
    rigs: &[Rig],
    source: &SourceAnimation,
    target: &ClipTarget,
    options: &ImportOptions,
) -> Result<(Vec<u8>, ImportSummary)> {
    let mut image = BankImage::parse(data)?;
    let bank = AnimBank::parse(data)?;
    let skeleton = find_skeleton(rigs, image.skeleton)
        .ok_or_else(|| invalid(format!("no rig has skeleton {:08x}", image.skeleton)))?;
    let mut summary = ImportSummary {
        bank_before: data.len(),
        scheme: options.encode.scheme,
        ..Default::default()
    };
    let (index, bone_set_hash) = match target {
        ClipTarget::Replace(i) => {
            let clip = image
                .clips
                .get(*i)
                .ok_or_else(|| invalid(format!("the bank has no clip {i}")))?;
            summary.mode = "replace".into();
            summary.streams_before = clip.streams.len();
            summary.clip = bank.clips[*i].display_name();
            (Some(*i), clip.bone_set())
        }
        ClipTarget::Add { name, like, .. } => {
            let like = like.unwrap_or(0);
            let template = image
                .clips
                .get(like)
                .ok_or_else(|| invalid(format!("the bank has no clip {like}")))?;
            summary.mode = "add".into();
            summary.clip = name.clone();
            (None, options.bone_set.unwrap_or(template.bone_set()))
        }
    };
    let bones = bone_set(rigs, bone_set_hash)
        .ok_or_else(|| invalid(format!("bone set {bone_set_hash:08x} is in no rig")))?;
    let fitted = crate::scene::retarget::retarget(
        source,
        skeleton,
        bones,
        &crate::scene::retarget::RetargetOptions {
            rest: options.rest,
            map: options.map.clone(),
        },
    )?;
    let encoded = encode_clip(&fitted.tracks, fitted.frames, &options.encode)?;
    let index = match (index, target) {
        (Some(i), _) => {
            image.replace_clip(i, &encoded)?;
            i
        }
        (
            None,
            ClipTarget::Add {
                name,
                entry,
                priority,
                like,
            },
        ) => {
            let hash = sdbm(name.to_ascii_lowercase().as_bytes());
            if image.clip_index(hash).is_some() {
                return Err(invalid(format!("the bank already has a clip `{name}`")));
            }
            let mut record = image.clips[like.unwrap_or(0)].record_like(hash);
            record[0x14..0x18].copy_from_slice(&bone_set_hash.to_le_bytes());
            let i = image.add_clip(record, &encoded)?;
            if *entry {
                image.add_entry(name, i, *priority)?;
                summary.entry = Some(name.clone());
            }
            i
        }
        (None, ClipTarget::Replace(_)) => unreachable!("replace has an index"),
    };
    if let Some(looping) = options.looping {
        image.clips[index].set_looping(looping);
        // Durations follow the flag.
        let encoded_again = crate::resource::anim_encode::EncodedClip {
            streams: image.clips[index].streams.clone(),
            format: image.clips[index].format(),
            frames: image.clips[index].frames(),
            ..Default::default()
        };
        image.replace_clip(index, &encoded_again)?;
    }
    let clip = &image.clips[index];
    let end_ticks = (clip.frames() as f32 - 1.0) * crate::resource::anim::TICKS_PER_FRAME;
    let late = clip
        .events
        .iter()
        .filter(|e| e.frame > end_ticks + 2.0)
        .count();
    if late > 0 {
        summary.warnings.push(format!(
            "{late} event(s) of the clip lie after its new end ({} frames)",
            clip.frames()
        ));
    }
    if !fitted.unmapped.is_empty() {
        summary.warnings.push(format!(
            "{} bone(s) of the set have no joint in the animation and keep the bind pose",
            fitted.unmapped.len()
        ));
    }
    let out = image.encode()?;
    // The game's reader must decode the new clip.
    let check = AnimBank::parse(&out)?;
    let decoded = check.clips[index].decode(&out, bones)?;
    if decoded.len() != bones.len() {
        return Err(invalid("the written clip does not decode"));
    }
    summary.clip_index = index;
    summary.clip_hash = image.clips[index].hash();
    summary.frames = encoded.frames;
    summary.bone_set = bone_set_hash;
    summary.bones = bones.len();
    summary.mapped = fitted.mapped;
    summary.unmapped = fitted.unmapped;
    summary.unused = fitted.unused;
    summary.scale = fitted.scale;
    summary.rest = format!("{:?}", fitted.rest);
    summary.looping = image.clips[index].looping();
    summary.max_angle = encoded.max_angle;
    summary.max_translation = encoded.max_translation;
    summary.streams_after = encoded.streams.len();
    summary.bank_after = out.len();
    Ok((out, summary))
}

/// The tracks of a bank clip as a [`SourceAnimation`] on its skeleton: for
/// tests and for copying clips between banks.
pub fn source_from_tracks(
    name: &str,
    skeleton: &Skeleton,
    tracks: &[BoneTrack],
) -> SourceAnimation {
    let locals: Vec<(Quat, Vec3)> = skeleton.bind_locals().iter().map(decompose).collect();
    let frames = tracks.iter().map(|t| t.rotations.len()).max().unwrap_or(0);
    let mut joints: Vec<SourceJoint> = skeleton
        .bones
        .iter()
        .zip(&locals)
        .map(|(b, &(r, t))| SourceJoint {
            name: b.display_name(),
            parent: b.parent,
            rest_rotation: r,
            rest_translation: t,
            rotations: None,
            translations: None,
        })
        .collect();
    for t in tracks {
        if let Some(j) = joints.get_mut(t.bone) {
            j.rotations = Some(t.rotations.clone());
            j.translations = Some(t.translations.clone());
        }
    }
    SourceAnimation {
        name: name.to_owned(),
        frames,
        joints,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qmul(a: Quat, b: Quat) -> Quat {
        crate::scene::math::quat_mul(a, b)
    }

    fn qconj(q: Quat) -> Quat {
        crate::scene::math::quat_conjugate(q)
    }

    fn qrot(q: Quat, v: Vec3) -> Vec3 {
        let p = qmul(qmul(q, [v[0], v[1], v[2], 0.0]), qconj(q));
        [p[0], p[1], p[2]]
    }

    #[test]
    fn quaternion_helpers() {
        let q = qnorm([0.0, 0.0, (0.5f32).sin(), (0.5f32).cos()]); // 1 rad about Z
        let v = qrot(q, [1.0, 0.0, 0.0]);
        assert!((v[0] - 1f32.cos()).abs() < 1e-5 && (v[1] - 1f32.sin()).abs() < 1e-5);
        let m: Mat4 = {
            let (c, s) = (1f32.cos(), 1f32.sin());
            [
                c, s, 0.0, 0.0, -s, c, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 2.0, 3.0, 1.0,
            ]
        };
        let (r, t) = decompose(&m);
        assert!(crate::resource::anim_encode::quat_angle(r, q) < 1e-5);
        assert_eq!(t, [1.0, 2.0, 3.0]);
        let h = slerp(IDENTITY, q, 0.5);
        assert!((crate::resource::anim_encode::quat_angle(h, IDENTITY) - 0.5).abs() < 1e-4);
    }

    /// A chain of three bones going up Y, one unit apart.
    fn chain() -> Skeleton {
        use crate::resource::rcb::Bone;
        let bone = |name: &'static str, parent: Option<usize>, y: f32| Bone {
            hash: sdbm(name.as_bytes()),
            name: Some(name),
            parent,
            bind_index: -1,
            bind_type: 1,
            inverse_bind: [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, -y, 0.0, 1.0,
            ],
        };
        Skeleton {
            name_hash: 0,
            name: "chain".into(),
            bones: vec![
                bone("root", None, 0.0),
                bone("mid", Some(0), 1.0),
                bone("tip", Some(1), 2.0),
            ],
        }
    }

    fn axis(axis: Vec3, a: f32) -> Quat {
        let s = (a / 2.0).sin();
        [axis[0] * s, axis[1] * s, axis[2] * s, (a / 2.0).cos()]
    }

    #[test]
    fn retargets_across_bone_axes_and_sizes() {
        let skeleton = chain();
        // The animation in model space: the root turns about Y and rises,
        // the middle bone bends about Z.
        let frames = 5;
        let world = |f: usize| -> Vec<Quat> {
            let a = f as f32 * 0.3;
            let g0 = axis([0.0, 1.0, 0.0], a);
            let g1 = qmul(g0, axis([0.0, 0.0, 1.0], a));
            vec![g0, g1, g1]
        };
        // A source skeleton twice as big whose joints have other axes:
        // rest rotation A_j in model space.
        let rest: Vec<Quat> = vec![
            axis([1.0, 0.0, 0.0], 0.7),
            axis([0.0, 0.0, 1.0], -1.1),
            axis([0.0, 1.0, 0.0], 2.0),
        ];
        let positions: [Vec3; 3] = [[0.0, 0.0, 0.0], [0.0, 2.0, 0.0], [0.0, 4.0, 0.0]];
        let mut joints = Vec::new();
        for j in 0..3usize {
            let parent = j.checked_sub(1);
            let (pr, pp) = parent.map_or((IDENTITY, [0.0; 3]), |p| (rest[p], positions[p]));
            let d = [
                positions[j][0] - pp[0],
                positions[j][1] - pp[1],
                positions[j][2] - pp[2],
            ];
            joints.push(SourceJoint {
                name: ["Rig:root", "Rig:mid", "Rig:tip"][j].into(),
                parent,
                rest_rotation: qmul(qconj(pr), rest[j]),
                rest_translation: qrot(qconj(pr), d),
                rotations: Some(Vec::new()),
                translations: None,
            });
        }
        let mut rise = Vec::new();
        for f in 0..frames {
            // Source globals: the same change from rest as the target's.
            let g: Vec<Quat> = (0..3).map(|j| qmul(world(f)[j], rest[j])).collect();
            for j in 0..3usize {
                let p = j.checked_sub(1).map_or(IDENTITY, |p| g[p]);
                joints[j]
                    .rotations
                    .as_mut()
                    .unwrap()
                    .push(qnorm(qmul(qconj(p), g[j])));
            }
            rise.push([0.0, f as f32 * 0.2, 0.0]);
        }
        joints[0].translations = Some(rise);
        let source = SourceAnimation {
            name: "test".into(),
            frames,
            joints,
        };
        let out = retarget(&source, &skeleton, &[0, 1, 2], &HashMap::new()).unwrap();
        assert!((out.scale - 0.5).abs() < 1e-5, "{}", out.scale);
        assert!(out.unmapped.is_empty());
        for f in 0..frames {
            let w = world(f);
            let expect = [w[0], qmul(qconj(w[0]), w[1]), qmul(qconj(w[1]), w[2])];
            for (b, want) in expect.iter().enumerate() {
                let got = out.tracks[b].rotations[f];
                assert!(
                    crate::resource::anim_encode::quat_angle(got, *want) < 1e-4,
                    "frame {f} bone {b}: {got:?} vs {want:?}"
                );
            }
            // The root rises by half the source's rise.
            assert!((out.tracks[0].translations[f][1] - f as f32 * 0.1).abs() < 1e-5);
            assert!((out.tracks[1].translations[f][1] - 1.0).abs() < 1e-5);
        }
    }

    /// Clips of the game skeleton written as FBX and USD (stage 25) come
    /// back as the same tracks through [`read_animations`] and [`retarget`].
    #[test]
    fn reads_fbx_and_usd_animations() {
        let skeleton = chain();
        let frames = 7;
        let mut tracks = Vec::new();
        for (b, axis_of) in [[0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]]
            .into_iter()
            .enumerate()
        {
            let rest_y = if b == 0 { 0.0 } else { 1.0 };
            tracks.push(BoneTrack {
                bone: b,
                rotations: (0..frames)
                    .map(|f| axis(axis_of, 0.2 * f as f32 + 0.1 * b as f32))
                    .collect(),
                translations: (0..frames)
                    .map(|f| {
                        let rise = if b == 0 { 0.05 * f as f32 } else { 0.0 };
                        [0.0, rest_y + rise, 0.0]
                    })
                    .collect(),
            });
        }
        let scene = crate::scene::anim::clips_scene(
            &skeleton,
            &[("wave".into(), tracks.clone())],
            FRAME_RATE,
        );
        let dir = tempfile::tempdir().unwrap();
        for ext in ["fbx", "usda", "usdc", "usdz"] {
            let path = dir.path().join(format!("wave.{ext}"));
            crate::scene::write_file(&scene, &path, None).unwrap();
            let animations = read_animations(&path).unwrap();
            assert_eq!(animations.len(), 1, "{ext}");
            assert_eq!(animations[0].frames, frames, "{ext}");
            let out = retarget(&animations[0], &skeleton, &[0, 1, 2], &HashMap::new()).unwrap();
            assert!(out.unmapped.is_empty(), "{ext}: {:?}", out.unmapped);
            assert!((out.scale - 1.0).abs() < 1e-4, "{ext}: {}", out.scale);
            for (got, want) in out.tracks.iter().zip(&tracks) {
                for f in 0..frames {
                    let angle = crate::resource::anim_encode::quat_angle(
                        got.rotations[f],
                        want.rotations[f],
                    );
                    assert!(angle < 1e-3, "{ext} bone {} frame {f}: {angle}", got.bone);
                    let (a, b) = (got.translations[f], want.translations[f]);
                    assert!(
                        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-4),
                        "{ext} bone {} frame {f}: {a:?} vs {b:?}",
                        got.bone
                    );
                }
            }
        }
        let obj = dir.path().join("wave.obj");
        assert!(read_animations(&obj).is_err());
    }

    #[test]
    fn matches_names_without_namespaces() {
        assert_eq!(match_name("mixamorig:Hips"), "hips");
        assert_eq!(match_name("Armature|Spine"), "spine");
    }
}

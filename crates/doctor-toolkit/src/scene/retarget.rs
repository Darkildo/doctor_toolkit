//! Fitting an animation to a game skeleton: the one retargeting used by
//! clip import into banks (`import::anim`) and by [`super::anim::clips_to_game`].
//!
//! A [`SourceAnimation`] (joints with their bind pose and local samples per
//! frame) is mapped onto the bones of a [`Skeleton`]:
//!
//! 1. joints are matched to bones: an explicit map (game bone → joint),
//!    then the name without a namespace (`mixamorig:Hips` → `hips`, any
//!    case), then the hash the name stands for (known bone names, the
//!    exporters' 8-digit hashes);
//! 2. the rest poses are reconciled ([`RestPose`]): `AsIs` keeps the
//!    file's local transforms (files made from the game skeleton), `Axes`
//!    takes world orientations turned by quarter turns (Blender's FBX bone
//!    axes), `Retarget` moves each bone by its joint's rotation from the rest
//!    pose in model space and by its translation offset scaled by the ratio
//!    of the skeletons' heights; `Auto` picks one;
//! 3. a track per requested bone, bones without a joint in the bind pose,
//!    quaternions kept on one hemisphere from frame to frame.
//!
//! [`scene_animations`] turns the clips of any scene (glTF, FBX, USD) into
//! source animations; it refuses files without an armature, and FBX
//! armatures without a bind pose (Blender's FBX of an armature alone), whose
//! rest pose would be a frame of the animation.

use std::collections::HashMap;

use super::{Scene, math};
use crate::hash::sdbm;
use crate::resource::anim::{BoneTrack, SourceAnimation, SourceJoint};
use crate::resource::rcb::Skeleton;
use crate::{Error, Result};

type Quat = [f32; 4];
type Vec3 = [f32; 3];
/// A rigid transform: rotation, then translation.
type Xf = (Quat, Vec3);

const IDENTITY: Quat = [0.0, 0.0, 0.0, 1.0];

fn invalid(message: impl Into<String>) -> Error {
    Error::InvalidArgument(message.into())
}

fn qnorm(q: Quat) -> Quat {
    let n = q.iter().map(|v| v * v).sum::<f32>().sqrt();
    if n > 1e-8 { q.map(|v| v / n) } else { IDENTITY }
}

fn qmul(a: Quat, b: Quat) -> Quat {
    math::quat_mul(a, b)
}

fn qconj(q: Quat) -> Quat {
    math::quat_conjugate(q)
}

fn qrot(q: Quat, v: Vec3) -> Vec3 {
    let p = qmul(qmul(q, [v[0], v[1], v[2], 0.0]), qconj(q));
    [p[0], p[1], p[2]]
}

fn xmul(a: Xf, b: Xf) -> Xf {
    let t = qrot(a.0, b.1);
    (
        qnorm(qmul(a.0, b.0)),
        [a.1[0] + t[0], a.1[1] + t[1], a.1[2] + t[2]],
    )
}

fn xinv(a: Xf) -> Xf {
    let r = qconj(a.0);
    let t = qrot(r, a.1);
    (r, [-t[0], -t[1], -t[2]])
}

fn xf_of(m: &crate::resource::rcb::Mat4) -> Xf {
    let (t, r, _) = math::decompose(m);
    (qnorm(r), t)
}

fn globals(parents: &[Option<usize>], locals: &[Xf]) -> Vec<Xf> {
    let mut out: Vec<Xf> = Vec::with_capacity(locals.len());
    for (i, &local) in locals.iter().enumerate() {
        let g = match parents[i].filter(|&p| p < i).map(|p| out[p]) {
            Some(parent) => xmul(parent, local),
            None => local,
        };
        out.push(g);
    }
    out
}

/// How the rest pose of the file is reconciled with the game skeleton.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub enum RestPose {
    /// `AsIs` when every matched joint's bind pose equals the game bone's
    /// (within 1 cm and about 2.5°); else `Axes` when every joint's
    /// orientation differs from the bone's by quarter turns (within 30°);
    /// else `Retarget`.
    #[default]
    Auto,
    /// Local transforms of the file are the game's (files made from the
    /// game skeleton).
    AsIs,
    /// The file's joints are the game's bones with other axes (Blender's FBX
    /// export turns bone axes by quarter turns): world orientations are
    /// turned back by the quarter turns, positions taken as they are.
    Axes,
    /// The joint's rotation from its rest pose in model space is applied to
    /// the bone's rest pose; translations move by the joint's offset from its
    /// rest, turned into the bone's parent space and scaled by the ratio of
    /// the skeletons' heights.
    Retarget,
}

/// Settings of [`retarget`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RetargetOptions {
    pub rest: RestPose,
    /// Game bone name → joint name, tried before the name rules.
    pub map: HashMap<String, String>,
}

/// An animation fitted to a skeleton.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct Retargeted {
    /// One track per requested bone, in the requested order.
    pub tracks: Vec<BoneTrack>,
    pub frames: usize,
    /// Per requested bone: whether a joint animates it.
    pub animated: Vec<bool>,
    /// Requested bones with a joint, by name.
    pub mapped: Vec<String>,
    /// Requested bones without a joint: held in the bind pose.
    pub unmapped: Vec<String>,
    /// Animated joints that drive no bone.
    pub unused: Vec<String>,
    /// Joints with no game bone at all.
    pub unmatched: Vec<String>,
    /// Translation scale (`Retarget`: target height / source height).
    pub scale: f32,
    /// The rest-pose mode used.
    pub rest: RestPose,
}

/// Normalised name for matching: lower case, without a namespace prefix
/// (`mixamorig:Hips` → `hips`, `Armature|bone` → `bone`).
pub fn match_name(name: &str) -> String {
    name.rsplit([':', '|'])
        .next()
        .unwrap_or(name)
        .to_ascii_lowercase()
}

/// The joint of `source` that drives each bone of `skeleton`.
pub fn joints_for_bones(
    source: &SourceAnimation,
    skeleton: &Skeleton,
    map: &HashMap<String, String>,
) -> Vec<Option<usize>> {
    let mut by_name: HashMap<String, usize> = HashMap::new();
    let mut by_hash: HashMap<u32, usize> = HashMap::new();
    for (i, j) in source.joints.iter().enumerate() {
        by_name.entry(match_name(&j.name)).or_insert(i);
        let short = j.name.rsplit([':', '|']).next().unwrap_or(&j.name);
        for name in [j.name.as_str(), short] {
            by_hash
                .entry(crate::import::model::joint_hash(name))
                .or_insert(i);
        }
        by_hash
            .entry(sdbm(match_name(&j.name).as_bytes()))
            .or_insert(i);
    }
    skeleton
        .bones
        .iter()
        .map(|b| {
            let name = b.display_name();
            if let Some(j) = map
                .get(&name)
                .or_else(|| map.get(&name.to_ascii_lowercase()))
            {
                return by_name.get(&match_name(j)).copied();
            }
            by_name
                .get(&match_name(&name))
                .or_else(|| by_hash.get(&b.hash))
                .copied()
        })
        .collect()
}

/// The rotation `r` rounded to the nearest quarter turns about the axes
/// (a signed permutation), and the angle between them in radians.
pub(crate) fn quarter_turns(r: Quat) -> (Quat, f32) {
    let m = math::quat_matrix(r);
    let mut snapped = math::IDENTITY;
    let mut used = [false; 3];
    for c in 0..3 {
        let row = (0..3)
            .max_by(|&x, &y| m[c * 4 + x].abs().total_cmp(&m[c * 4 + y].abs()))
            .unwrap_or(c);
        if used[row] {
            return (r, std::f32::consts::PI);
        }
        used[row] = true;
        for k in 0..3 {
            snapped[c * 4 + k] = 0.0;
        }
        snapped[c * 4 + row] = m[c * 4 + row].signum();
    }
    if math::determinant3(&snapped) < 0.0 {
        return (r, std::f32::consts::PI);
    }
    let q = math::matrix_quat(&snapped);
    let dot = (0..4).map(|i| q[i] * r[i]).sum::<f32>().abs().min(1.0);
    (q, 2.0 * dot.acos())
}

fn same_pose(a: Xf, b: Xf) -> bool {
    let dot = (0..4).map(|i| a.0[i] * b.0[i]).sum::<f32>().abs();
    let dist = (0..3)
        .map(|i| (a.1[i] - b.1[i]).powi(2))
        .sum::<f32>()
        .sqrt();
    dot > 0.9998 && dist < 0.01
}

/// Height of a pose: the extent of the points along Y.
fn height(points: impl Iterator<Item = Vec3>) -> f32 {
    let (lo, hi) = points.fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), p| {
        (lo.min(p[1]), hi.max(p[1]))
    });
    if hi > lo { hi - lo } else { 0.0 }
}

/// Fits `source` to `bones` (indices of `skeleton`): a track per bone.
pub fn retarget(
    source: &SourceAnimation,
    skeleton: &Skeleton,
    bones: &[usize],
    options: &RetargetOptions,
) -> Result<Retargeted> {
    let frames = source.frames.max(1);
    let n = skeleton.bones.len();
    if let Some(&b) = bones.iter().find(|&&b| b >= n) {
        return Err(invalid(format!(
            "bone {b} is not in skeleton `{}`",
            skeleton.name
        )));
    }
    let game_locals: Vec<Xf> = skeleton.bind_locals().iter().map(xf_of).collect();
    let game_parents: Vec<Option<usize>> = skeleton.bones.iter().map(|b| b.parent).collect();
    let game_rest = globals(&game_parents, &game_locals);
    let source_parents: Vec<Option<usize>> = source.joints.iter().map(|j| j.parent).collect();
    let source_bind: Vec<Xf> = source
        .joints
        .iter()
        .map(|j| (qnorm(j.rest_rotation), j.rest_translation))
        .collect();
    let source_rest = globals(&source_parents, &source_bind);
    let joint_of = joints_for_bones(source, skeleton, &options.map);
    // Joint → bone (the first bone a joint drives).
    let mut bone_of: Vec<Option<usize>> = vec![None; source.joints.len()];
    for (b, j) in joint_of.iter().enumerate() {
        if let Some(j) = *j
            && bone_of[j].is_none()
        {
            bone_of[j] = Some(b);
        }
    }
    if joint_of.iter().all(Option::is_none) {
        return Err(invalid(format!(
            "no joint of the animation is a bone of skeleton `{}` (joints: {}…); bone names are in data/gf2/bone_names.txt",
            skeleton.name,
            source
                .joints
                .iter()
                .take(5)
                .map(|j| j.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    let mapped = |b: usize| joint_of[b].map(|j| (b, j));
    let rest = match options.rest {
        RestPose::Auto => {
            let pairs: Vec<(usize, usize)> = (0..n).filter_map(mapped).collect();
            if pairs
                .iter()
                .all(|&(b, j)| same_pose(source_bind[j], game_locals[b]))
            {
                RestPose::AsIs
            } else if pairs.iter().all(|&(b, j)| {
                quarter_turns(qmul(qconj(game_rest[b].0), source_rest[j].0)).1 < 30f32.to_radians()
            }) {
                RestPose::Axes
            } else {
                RestPose::Retarget
            }
        }
        other => other,
    };
    let turns: Vec<Quat> = (0..n)
        .map(|b| match joint_of[b] {
            Some(j) => quarter_turns(qmul(qconj(game_rest[b].0), source_rest[j].0)).0,
            None => IDENTITY,
        })
        .collect();
    let scale = if rest == RestPose::Retarget {
        let s = height((0..n).filter_map(mapped).map(|(_, j)| source_rest[j].1));
        let t = height((0..n).filter_map(mapped).map(|(b, _)| game_rest[b].1));
        if s > 1e-6 && t > 1e-6 { t / s } else { 1.0 }
    } else {
        1.0
    };
    let mut tracks: Vec<BoneTrack> = bones
        .iter()
        .map(|&bone| BoneTrack {
            bone,
            rotations: Vec::with_capacity(frames),
            translations: Vec::with_capacity(frames),
        })
        .collect();
    let sample = |j: &SourceJoint, f: usize| -> Xf {
        let r = j
            .rotations
            .as_ref()
            .and_then(|v| v.get(f).or(v.last()))
            .copied()
            .unwrap_or(j.rest_rotation);
        let t = j
            .translations
            .as_ref()
            .and_then(|v| v.get(f).or(v.last()))
            .copied()
            .unwrap_or(j.rest_translation);
        (qnorm(r), t)
    };
    for f in 0..frames {
        let locals: Vec<Xf> = source.joints.iter().map(|j| sample(j, f)).collect();
        let game: Vec<Xf> = match rest {
            RestPose::AsIs | RestPose::Auto => (0..n)
                .map(|b| match joint_of[b] {
                    Some(j) => {
                        local_on_game(&locals, &source_parents, j, game_parents[b], &bone_of)
                    }
                    None => game_locals[b],
                })
                .collect(),
            RestPose::Axes => {
                let world_src = globals(&source_parents, &locals);
                let mut world: Vec<Xf> = Vec::with_capacity(n);
                for b in 0..n {
                    let w = match joint_of[b] {
                        Some(j) => (qnorm(qmul(world_src[j].0, qconj(turns[b]))), world_src[j].1),
                        None => match game_parents[b].filter(|&p| p < b) {
                            Some(p) => xmul(world[p], game_locals[b]),
                            None => game_locals[b],
                        },
                    };
                    world.push(w);
                }
                (0..n)
                    .map(|b| match game_parents[b].filter(|&p| p < b) {
                        Some(p) => xmul(xinv(world[p]), world[b]),
                        None => world[b],
                    })
                    .collect()
            }
            RestPose::Retarget => {
                let src = globals(&source_parents, &locals);
                let mut global = vec![IDENTITY; n];
                let mut out = vec![(IDENTITY, [0.0f32; 3]); n];
                for b in 0..n {
                    let (rest_r, rest_t) = game_locals[b];
                    let parent_global = game_parents[b]
                        .filter(|&p| p < b)
                        .map_or(IDENTITY, |p| global[p]);
                    let (r, t) = match joint_of[b] {
                        Some(j) => {
                            let delta = qmul(src[j].0, qconj(source_rest[j].0));
                            let g = qnorm(qmul(delta, game_rest[b].0));
                            let r = qnorm(qmul(qconj(parent_global), g));
                            let joint = &source.joints[j];
                            let t = if joint.translations.is_some() {
                                let now = locals[j].1;
                                let was = joint.rest_translation;
                                let offset = [now[0] - was[0], now[1] - was[1], now[2] - was[2]];
                                let source_parent =
                                    joint.parent.map_or(IDENTITY, |p| source_rest[p].0);
                                let target_parent =
                                    game_parents[b].map_or(IDENTITY, |p| game_rest[p].0);
                                let d = qrot(qmul(qconj(target_parent), source_parent), offset);
                                [
                                    rest_t[0] + d[0] * scale,
                                    rest_t[1] + d[1] * scale,
                                    rest_t[2] + d[2] * scale,
                                ]
                            } else {
                                rest_t
                            };
                            (r, t)
                        }
                        None => (rest_r, rest_t),
                    };
                    global[b] = qnorm(qmul(parent_global, r));
                    out[b] = (r, t);
                }
                out
            }
        };
        for track in &mut tracks {
            let (r, t) = game[track.bone];
            let r = match track.rotations.last() {
                Some(prev) if (0..4).map(|i| prev[i] * r[i]).sum::<f32>() < 0.0 => r.map(|x| -x),
                _ => r,
            };
            track.rotations.push(r);
            track.translations.push(t);
        }
    }
    let name = |b: usize| skeleton.bones[b].display_name();
    let moves = |j: &SourceJoint| j.rotations.is_some() || j.translations.is_some();
    Ok(Retargeted {
        tracks,
        frames,
        animated: bones
            .iter()
            .map(|&b| joint_of[b].is_some_and(|j| moves(&source.joints[j])))
            .collect(),
        mapped: bones
            .iter()
            .filter(|&&b| joint_of[b].is_some())
            .map(|&b| name(b))
            .collect(),
        unmapped: bones
            .iter()
            .filter(|&&b| joint_of[b].is_none())
            .map(|&b| name(b))
            .collect(),
        unused: source
            .joints
            .iter()
            .enumerate()
            .filter(|(i, j)| bone_of[*i].is_none() && moves(j))
            .map(|(_, j)| j.name.clone())
            .collect(),
        unmatched: source
            .joints
            .iter()
            .enumerate()
            .filter(|(i, _)| bone_of[*i].is_none())
            .map(|(_, j)| j.name.clone())
            .collect(),
        scale,
        rest,
    })
}

/// The local transform of the bone of joint `j`: the joint's local composed
/// with the unmatched joints between it and the joint of the game parent.
fn local_on_game(
    locals: &[Xf],
    parents: &[Option<usize>],
    j: usize,
    game_parent: Option<usize>,
    bone_of: &[Option<usize>],
) -> Xf {
    let mut m = locals[j];
    let mut p = parents[j];
    let mut guard = 0;
    while let Some(pj) = p {
        if bone_of[pj].is_some() && bone_of[pj] == game_parent {
            break;
        }
        if bone_of[pj].is_some() || guard > 256 {
            // A different hierarchy: keep the joint's own local.
            return locals[j];
        }
        m = xmul(locals[pj], m);
        p = parents[pj];
        guard += 1;
    }
    if p.is_none() && game_parent.is_some() {
        return locals[j];
    }
    m
}

/// The clips of a scene (glTF, FBX, USD) as source animations at
/// `frame_rate`: the joints with their bind pose as the rest pose; a joint
/// keeps its own rest transform for what its track does not animate.
///
/// Refuses a scene without joints, and one whose joints all lack a bind
/// pose (FBX of an armature alone, as Blender writes it).
pub fn scene_animations(scene: &Scene, frame_rate: f32) -> Result<Vec<SourceAnimation>> {
    if scene.joints.is_empty() {
        return Err(invalid(
            "the file has no armature: animations are read from the joints of a skeleton \
             (in Blender, export the Armature object)",
        ));
    }
    if !scene.joints_without_bind.is_empty()
        && scene.joints_without_bind.len() == scene.joints.len()
    {
        return Err(invalid(
            "the armature of this FBX file has no bind pose (Blender writes none when the \
             armature is exported without its mesh), so its rest pose would be a frame of \
             the animation: export the armature together with a mesh skinned to it, or \
             export glTF or USD",
        ));
    }
    let frame_rate = frame_rate.max(1.0);
    let bind: Vec<Xf> = scene.bind_locals().iter().map(xf_of).collect();
    let rest: Vec<Xf> = scene.joints.iter().map(|j| xf_of(&j.local)).collect();
    let joints: Vec<SourceJoint> = scene
        .joints
        .iter()
        .zip(&bind)
        .map(|(joint, &(r, t))| SourceJoint {
            name: joint.name.clone(),
            parent: joint.parent,
            rest_rotation: r,
            rest_translation: t,
            rotations: None,
            translations: None,
        })
        .collect();
    let mut out = Vec::new();
    for clip in &scene.clips {
        let frames_in = clip.frames();
        if frames_in == 0 || clip.tracks.is_empty() {
            continue;
        }
        let rate_in = if clip.frame_rate > 0.0 {
            clip.frame_rate
        } else {
            frame_rate
        };
        let frames =
            (((frames_in - 1) as f32 * frame_rate / rate_in).round() as usize + 1).min(1 << 20);
        let at = |f: usize| f as f32 * rate_in / frame_rate;
        let mut anim = SourceAnimation {
            name: clip.name.clone(),
            frames,
            joints: joints.clone(),
        };
        for track in &clip.tracks {
            let Some(joint) = anim.joints.get_mut(track.joint) else {
                continue;
            };
            let (rest_r, rest_t) = rest[track.joint];
            joint.rotations = Some(
                (0..frames)
                    .map(|f| qnorm(math::sample_quat(&track.rotations, at(f)).unwrap_or(rest_r)))
                    .collect(),
            );
            joint.translations = Some(
                (0..frames)
                    .map(|f| math::sample_vec3(&track.translations, at(f)).unwrap_or(rest_t))
                    .collect(),
            );
        }
        // Joints without a track hold their rest transform; when it is not
        // the bind pose, it is a constant sample.
        for (i, joint) in anim.joints.iter_mut().enumerate() {
            if joint.rotations.is_some() {
                continue;
            }
            let ((r, t), (br, bt)) = (rest[i], bind[i]);
            let dot: f32 = (0..4).map(|k| r[k] * br[k]).sum::<f32>().abs();
            let dist = (0..3).map(|k| (t[k] - bt[k]).powi(2)).sum::<f32>().sqrt();
            if dot < 0.99999 || dist > 1e-4 {
                joint.rotations = Some(vec![r; frames]);
                joint.translations = Some(vec![t; frames]);
            }
        }
        out.push(anim);
    }
    if out.is_empty() {
        return Err(invalid("the file has no animation"));
    }
    Ok(out)
}

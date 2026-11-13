//! Animation clips between scenes and the game skeleton.
//!
//! The game stores, per frame and per bone, a rotation and a translation in
//! the bone's parent space ([`BoneTrack`], `resource/anim.rs`), at 29.97
//! frames per second ([`FRAME_RATE`]). [`clips_to_game`] turns the clips of
//! any [`Scene`] (glTF, FBX, USD) into such tracks on a game skeleton:
//!
//! 1. file joints are matched to game bones by name (the same rule as the
//!    model importer: known bone names, the exporter's 8-digit hashes, or
//!    an explicit map), unmatched joints are reported;
//! 2. rest poses are reconciled ([`RestPose`]): files made from the game
//!    skeleton (exported by this toolkit and edited) keep their local
//!    transforms as they are; files from other rigs are retargeted by
//!    world-space rotation deltas from the rest pose;
//! 3. clips are resampled to the game frame rate;
//! 4. every skeleton bone gets a track (bones the file does not animate
//!    hold their bind pose), quaternions on one hemisphere frame to frame.
//!
//! [`clips_scene`] does the reverse for writing clips to any format.

use std::collections::HashMap;

use super::math::{self, IDENTITY};
use super::{Scene, SceneClip};
use crate::export::gltf::SceneAnimation;
use crate::resource::anim::{BoneTrack, FRAME_RATE};
use crate::resource::rcb::{Mat4, Skeleton};
use crate::{Error, Result};

/// Keyframes of one animated property with arbitrary key times.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Keys {
    /// Seconds, ascending.
    pub times: Vec<f32>,
    /// One value per key (3 or 4 components).
    pub values: Vec<Vec<f32>>,
    /// Hold each value until the next key.
    pub step: bool,
}

/// The value of keys at `time` (clamped to the keys); quaternions are
/// interpolated spherically.
pub fn sample_keys(keys: &Keys, time: f32, quaternion: bool) -> Vec<f32> {
    let n = keys.times.len().min(keys.values.len());
    if n == 0 {
        return Vec::new();
    }
    if n == 1 || time <= keys.times[0] {
        return keys.values[0].clone();
    }
    if time >= keys.times[n - 1] {
        return keys.values[n - 1].clone();
    }
    let i = keys.times[..n].partition_point(|&t| t <= time).max(1) - 1;
    let (t0, t1) = (keys.times[i], keys.times[i + 1]);
    let (a, b) = (&keys.values[i], &keys.values[i + 1]);
    if keys.step || t1 <= t0 || a.len() != b.len() {
        return a.clone();
    }
    let t = (time - t0) / (t1 - t0);
    if quaternion && a.len() == 4 {
        return math::slerp([a[0], a[1], a[2], a[3]], [b[0], b[1], b[2], b[3]], t).to_vec();
    }
    a.iter().zip(b).map(|(x, y)| x + (y - x) * t).collect()
}

/// How the rest pose of the file is reconciled with the game skeleton.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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
    /// export turns bone axes by quarter turns, and its rest pose may be a
    /// frame of the clip): world orientations are taken as they are, turned
    /// by the quarter turns, and positions as they are.
    Axes,
    /// World-space rotation deltas from the file's rest pose are applied to
    /// the game's bind pose; bone lengths stay the game's, the root moves by
    /// the file root's displacement.
    Retarget,
}

/// Options of [`clips_to_game`].
#[derive(Debug, Clone)]
pub struct ClipOptions {
    /// Output frame rate (the game's: 30).
    pub frame_rate: f32,
    pub rest: RestPose,
    /// Only clips with these names (any case); empty: all.
    pub clips: Vec<String>,
    /// Explicit joint → game bone names, tried before the name rules.
    pub bone_map: HashMap<String, String>,
}

impl Default for ClipOptions {
    fn default() -> Self {
        Self {
            frame_rate: FRAME_RATE,
            rest: RestPose::Auto,
            clips: Vec::new(),
            bone_map: HashMap::new(),
        }
    }
}

/// A clip on a game skeleton, ready to be encoded into a bank.
#[derive(Debug, Clone, PartialEq)]
pub struct GameClip {
    pub name: String,
    pub frames: usize,
    pub frame_rate: f32,
    /// One track per skeleton bone, in skeleton order, `frames` keys each.
    pub tracks: Vec<BoneTrack>,
    /// Per skeleton bone: whether the file animates it.
    pub animated: Vec<bool>,
    /// The rest-pose mode used.
    pub rest: RestPose,
    /// File joints with no game bone.
    pub unmapped: Vec<String>,
}

/// Index of the game bone for each scene joint.
pub fn map_joints(
    scene: &Scene,
    skeleton: &Skeleton,
    bone_map: &HashMap<String, String>,
) -> Vec<Option<usize>> {
    let by_hash: HashMap<u32, usize> = skeleton
        .bones
        .iter()
        .enumerate()
        .map(|(i, b)| (b.hash, i))
        .collect();
    let lookup = |name: &str| -> Option<usize> {
        let hash = crate::import::model::joint_hash(name);
        by_hash.get(&hash).copied()
    };
    scene
        .joints
        .iter()
        .map(|joint| {
            if let Some(target) = bone_map.get(&joint.name) {
                return lookup(target);
            }
            lookup(&joint.name).or_else(|| {
                // `namespace:name` (Maya, Mixamo) or `armature|name`.
                let short = joint.name.rsplit([':', '|']).next()?;
                (short != joint.name).then(|| lookup(short)).flatten()
            })
        })
        .collect()
}

/// The rotation `r` rounded to the nearest quarter turns about the axes
/// (a signed permutation), and the angle between them in radians.
fn quarter_turns(r: [f32; 4]) -> ([f32; 4], f32) {
    let m = math::quat_matrix(r);
    let mut snapped = IDENTITY;
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

fn rest_matches(a: &Mat4, b: &Mat4) -> bool {
    let (ta, ra, _) = math::decompose(a);
    let (tb, rb, _) = math::decompose(b);
    let dot = (0..4).map(|i| ra[i] * rb[i]).sum::<f32>().abs();
    let dist = (0..3).map(|i| (ta[i] - tb[i]).powi(2)).sum::<f32>().sqrt();
    dot > 0.9998 && dist < 0.01
}

/// Local transform of every scene joint at frame `f` of a clip (rest pose
/// for joints without a track).
fn clip_locals(scene: &Scene, clip: &SceneClip, frame: f32, rest: &[Mat4]) -> Vec<Mat4> {
    let mut locals = rest.to_vec();
    for track in &clip.tracks {
        let Some(local) = locals.get_mut(track.joint) else {
            continue;
        };
        let (t0, r0, s0) = math::decompose(&rest[track.joint]);
        let t = math::sample_vec3(&track.translations, frame).unwrap_or(t0);
        let r = math::sample_quat(&track.rotations, frame).unwrap_or(r0);
        let s = math::sample_vec3(&track.scales, frame).unwrap_or(if track.scales.is_empty() {
            [1.0; 3]
        } else {
            s0
        });
        *local = math::compose(t, r, s);
    }
    let _ = scene;
    locals
}

fn globals(parents: &[Option<usize>], locals: &[Mat4]) -> Vec<Mat4> {
    let mut out: Vec<Mat4> = Vec::with_capacity(locals.len());
    for (i, local) in locals.iter().enumerate() {
        let g = match parents[i].and_then(|p| out.get(p)) {
            Some(parent) => math::multiply(parent, local),
            None => *local,
        };
        out.push(g);
    }
    out
}

/// Converts the clips of `scene` to tracks on a game skeleton.
pub fn clips_to_game(
    scene: &Scene,
    skeleton: &Skeleton,
    options: &ClipOptions,
) -> Result<Vec<GameClip>> {
    if scene.clips.is_empty() {
        return Err(Error::InvalidArgument("the file has no animation".into()));
    }
    let mapping = map_joints(scene, skeleton, &options.bone_map);
    if mapping.iter().all(Option::is_none) {
        return Err(Error::InvalidArgument(format!(
            "no joint of the file is a bone of skeleton `{}` (joints: {}…); bone names are in data/gf2/bone_names.txt",
            skeleton.name,
            scene
                .joints
                .iter()
                .take(5)
                .map(|j| j.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    let unmapped: Vec<String> = scene
        .joints
        .iter()
        .zip(&mapping)
        .filter(|(_, m)| m.is_none())
        .map(|(j, _)| j.name.clone())
        .collect();
    // Bind poses: the game's, and the file's (from its inverse bind matrices).
    let game_locals = skeleton.bind_locals();
    let game_parents: Vec<Option<usize>> = skeleton.bones.iter().map(|b| b.parent).collect();
    let game_globals = globals(&game_parents, &game_locals);
    let file_parents: Vec<Option<usize>> = scene.joints.iter().map(|j| j.parent).collect();
    let file_bind = scene.bind_locals();
    let file_bind_globals = globals(&file_parents, &file_bind);
    let file_rest: Vec<Mat4> = scene.joints.iter().map(|j| j.local).collect();
    let rest = match options.rest {
        RestPose::Auto => {
            let same = mapping
                .iter()
                .enumerate()
                .all(|(j, m)| m.is_none_or(|b| rest_matches(&file_bind[j], &game_locals[b])));
            let turned = mapping.iter().enumerate().all(|(j, m)| {
                m.is_none_or(|b| {
                    let (_, r, _) = math::decompose(&math::multiply(
                        &math::invert(&game_globals[b]),
                        &file_bind_globals[j],
                    ));
                    quarter_turns(r).1 < 30f32.to_radians()
                })
            });
            if same {
                RestPose::AsIs
            } else if turned {
                RestPose::Axes
            } else {
                RestPose::Retarget
            }
        }
        other => other,
    };
    // Quarter turns from each game bone's frame to its file joint's.
    let turns: Vec<[f32; 4]> = (0..skeleton.bones.len())
        .map(|b| {
            let Some(j) = mapping.iter().position(|m| *m == Some(b)) else {
                return [0.0, 0.0, 0.0, 1.0];
            };
            let (_, r, _) = math::decompose(&math::multiply(
                &math::invert(&game_globals[b]),
                &file_bind_globals[j],
            ));
            quarter_turns(r).0
        })
        .collect();
    // Game bone → file joint.
    let mut joint_of: Vec<Option<usize>> = vec![None; skeleton.bones.len()];
    for (j, m) in mapping.iter().enumerate() {
        if let Some(b) = *m
            && joint_of[b].is_none()
        {
            joint_of[b] = Some(j);
        }
    }
    let mut out = Vec::new();
    for clip in &scene.clips {
        if !options.clips.is_empty()
            && !options
                .clips
                .iter()
                .any(|c| c.eq_ignore_ascii_case(&clip.name))
        {
            continue;
        }
        let frames_in = clip.frames();
        if frames_in == 0 {
            continue;
        }
        let rate_in = if clip.frame_rate > 0.0 {
            clip.frame_rate
        } else {
            options.frame_rate
        };
        let rate_out = options.frame_rate.max(1.0);
        let frames =
            (((frames_in - 1) as f32 * rate_out / rate_in).round() as usize + 1).min(1 << 20);
        let mut animated = vec![false; skeleton.bones.len()];
        for track in &clip.tracks {
            if let Some(Some(b)) = mapping.get(track.joint) {
                animated[*b] = true;
            }
        }
        let mut tracks: Vec<BoneTrack> = (0..skeleton.bones.len())
            .map(|bone| BoneTrack {
                bone,
                rotations: Vec::with_capacity(frames),
                translations: Vec::with_capacity(frames),
            })
            .collect();
        for f in 0..frames {
            let frame_in = f as f32 * rate_in / rate_out;
            let locals = clip_locals(scene, clip, frame_in, &file_rest);
            let game: Vec<Mat4> = match rest {
                RestPose::AsIs | RestPose::Auto => (0..skeleton.bones.len())
                    .map(|b| match joint_of[b] {
                        Some(j) => file_local_on_game(
                            &locals,
                            &file_parents,
                            j,
                            &game_parents,
                            b,
                            &mapping,
                        ),
                        None => game_locals[b],
                    })
                    .collect(),
                RestPose::Axes => {
                    let file_globals = globals(&file_parents, &locals);
                    let mut world: Vec<Mat4> = Vec::with_capacity(skeleton.bones.len());
                    for b in 0..skeleton.bones.len() {
                        let w = match joint_of[b] {
                            Some(j) => {
                                let (t, r, _) = math::decompose(&file_globals[j]);
                                let r = math::quat_mul(r, math::quat_conjugate(turns[b]));
                                math::compose(t, math::quat_normalize(r), [1.0; 3])
                            }
                            None => match game_parents[b] {
                                Some(p) => math::multiply(&world[p], &game_locals[b]),
                                None => game_locals[b],
                            },
                        };
                        world.push(w);
                    }
                    (0..skeleton.bones.len())
                        .map(|b| match game_parents[b] {
                            Some(p) => math::multiply(&math::invert(&world[p]), &world[b]),
                            None => world[b],
                        })
                        .collect()
                }
                RestPose::Retarget => {
                    let file_globals = globals(&file_parents, &locals);
                    retarget(
                        &file_globals,
                        &file_bind_globals,
                        &game_globals,
                        &game_locals,
                        &game_parents,
                        &joint_of,
                    )
                }
            };
            for (b, m) in game.iter().enumerate() {
                let (t, r, _) = math::decompose(m);
                let track = &mut tracks[b];
                let r = match track.rotations.last() {
                    Some(prev) if (0..4).map(|i| prev[i] * r[i]).sum::<f32>() < 0.0 => {
                        r.map(|x| -x)
                    }
                    _ => r,
                };
                track.rotations.push(r);
                track.translations.push(t);
            }
        }
        out.push(GameClip {
            name: clip.name.clone(),
            frames,
            frame_rate: rate_out,
            tracks,
            animated,
            rest,
            unmapped: unmapped.clone(),
        });
    }
    if out.is_empty() {
        return Err(Error::InvalidArgument(format!(
            "no clip named {}",
            options.clips.join(", ")
        )));
    }
    Ok(out)
}

/// The local transform of game bone `b` from the file: the file joint's
/// local, composed with any file joints between it and the joint of the
/// game parent.
fn file_local_on_game(
    locals: &[Mat4],
    file_parents: &[Option<usize>],
    j: usize,
    game_parents: &[Option<usize>],
    b: usize,
    mapping: &[Option<usize>],
) -> Mat4 {
    let game_parent = game_parents[b];
    let mut m = locals[j];
    let mut p = file_parents[j];
    let mut guard = 0;
    while let Some(pj) = p {
        if mapping[pj].is_some() && mapping[pj] == game_parent {
            break;
        }
        if mapping[pj].is_some() || guard > 256 {
            // A different hierarchy: keep the joint's own local.
            return locals[j];
        }
        m = math::multiply(&locals[pj], &m);
        p = file_parents[pj];
        guard += 1;
    }
    if p.is_none() && game_parent.is_some() {
        return locals[j];
    }
    m
}

fn retarget(
    file_globals: &[Mat4],
    file_rest_globals: &[Mat4],
    game_rest_globals: &[Mat4],
    game_locals: &[Mat4],
    game_parents: &[Option<usize>],
    joint_of: &[Option<usize>],
) -> Vec<Mat4> {
    let n = game_locals.len();
    let mut world: Vec<Mat4> = Vec::with_capacity(n);
    for b in 0..n {
        let parent_world = game_parents[b].map_or(IDENTITY, |p| world[p]);
        let w = match joint_of[b] {
            Some(j) => {
                // Rotation delta in model space.
                let (_, ra, _) = math::decompose(&file_globals[j]);
                let (_, rr, _) = math::decompose(&file_rest_globals[j]);
                let delta = math::quat_mul(ra, math::quat_conjugate(rr));
                let (gt, gr, _) = math::decompose(&game_rest_globals[b]);
                let r = math::quat_normalize(math::quat_mul(delta, gr));
                let t = match game_parents[b] {
                    Some(_) => math::point(&parent_world, math::translation_of(&game_locals[b])),
                    None => {
                        let a = math::translation_of(&file_globals[j]);
                        let rest = math::translation_of(&file_rest_globals[j]);
                        [
                            gt[0] + a[0] - rest[0],
                            gt[1] + a[1] - rest[1],
                            gt[2] + a[2] - rest[2],
                        ]
                    }
                };
                math::compose(t, r, [1.0; 3])
            }
            None => math::multiply(&parent_world, &game_locals[b]),
        };
        world.push(w);
    }
    (0..n)
        .map(|b| match game_parents[b] {
            Some(p) => math::multiply(&math::invert(&world[p]), &world[b]),
            None => world[b],
        })
        .collect()
}

/// Reads the clips of a model file onto a game skeleton.
pub fn read_clips(
    path: &std::path::Path,
    skeleton: &Skeleton,
    options: &ClipOptions,
) -> Result<Vec<GameClip>> {
    let scene = super::read_file(path)?;
    clips_to_game(&scene, skeleton, options)
}

/// A scene with the game skeleton and clips of bone tracks, for writing
/// clips in any format ([`super::write`]).
pub fn clips_scene(
    skeleton: &Skeleton,
    clips: &[(String, Vec<BoneTrack>)],
    frame_rate: f32,
) -> Scene {
    let animations: Vec<SceneAnimation<'_>> = clips
        .iter()
        .map(|(name, tracks)| SceneAnimation {
            name: name.clone(),
            skeleton,
            tracks: tracks.clone(),
            frame_rate,
        })
        .collect();
    Scene {
        name: skeleton.name.clone(),
        joints: super::game::skeleton_joints(skeleton),
        clips: super::game::clips(skeleton, &animations),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::sdbm;
    use crate::resource::rcb::Bone;
    use crate::scene::{JointTrack, SceneJoint};

    fn skeleton() -> Skeleton {
        let bone = |name: &'static str, parent, t: [f32; 3]| Bone {
            hash: sdbm(name.as_bytes()),
            name: Some(name),
            parent,
            bind_index: -1,
            bind_type: 1,
            inverse_bind: math::invert(&math::translation(t)),
        };
        Skeleton {
            name_hash: 0,
            name: "s".into(),
            bones: vec![
                bone("m_root", None, [0.0, 0.0, 0.0]),
                bone("m_pelvis", Some(0), [0.0, 1.0, 0.0]),
                bone("m_spine", Some(1), [0.0, 1.5, 0.0]),
            ],
        }
    }

    fn tracks(frames: usize) -> Vec<BoneTrack> {
        (0..3)
            .map(|bone| BoneTrack {
                bone,
                rotations: (0..frames)
                    .map(|f| math::axis_quat(1, f as f32 * 0.1 * bone as f32))
                    .collect(),
                translations: vec![
                    [[0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.5, 0.0]][bone];
                    frames
                ],
            })
            .collect()
    }

    #[test]
    fn game_clips_round_trip_through_a_scene() {
        let skeleton = skeleton();
        let original = tracks(10);
        let scene = clips_scene(&skeleton, &[("walk".into(), original.clone())], 30.0);
        let back = clips_to_game(&scene, &skeleton, &ClipOptions::default()).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].rest, RestPose::AsIs);
        assert_eq!(back[0].frames, 10);
        for (a, b) in original.iter().zip(&back[0].tracks) {
            for (x, y) in a.rotations.iter().zip(&b.rotations) {
                let dot: f32 = (0..4).map(|i| x[i] * y[i]).sum();
                assert!(dot.abs() > 0.99999, "{x:?} {y:?}");
            }
            for (x, y) in a.translations.iter().zip(&b.translations) {
                assert!((0..3).all(|i| (x[i] - y[i]).abs() < 1e-5), "{x:?} {y:?}");
            }
        }
    }

    #[test]
    fn resamples_to_the_game_rate() {
        let skeleton = skeleton();
        let mut scene = clips_scene(&skeleton, &[("a".into(), tracks(11))], 30.0);
        scene.clips[0].frame_rate = 60.0;
        let back = clips_to_game(&scene, &skeleton, &ClipOptions::default()).unwrap();
        assert_eq!(back[0].frames, 6);
        // Frame 1 at 30 fps is frame 2 at 60.
        let expected = math::axis_quat(1, 0.2 * 2.0);
        let got = back[0].tracks[2].rotations[1];
        let dot: f32 = (0..4).map(|i| expected[i] * got[i]).sum();
        assert!(dot.abs() > 0.99999);
    }

    /// The game chain with every joint's frame turned by `angle` about X;
    /// the pelvis turns 90° about the model's Y axis. Returns the rest mode
    /// used, the pelvis rotation and the spine translation on the game skeleton.
    fn other_rest(angle: f32) -> (RestPose, [f32; 4], [f32; 3]) {
        let skeleton = skeleton();
        // The same chain, but every joint's rest frame rotated 90° about X.
        let rx = math::quat_matrix(math::axis_quat(0, angle));
        let mut scene = Scene::default();
        let worlds = [
            math::multiply(&math::translation([0.0, 0.0, 0.0]), &rx),
            math::multiply(&math::translation([0.0, 1.0, 0.0]), &rx),
            math::multiply(&math::translation([0.0, 1.5, 0.0]), &rx),
        ];
        for (i, name) in ["m_root", "m_pelvis", "m_spine"].iter().enumerate() {
            let parent = i.checked_sub(1);
            let local = match parent {
                Some(p) => math::multiply(&math::invert(&worlds[p]), &worlds[i]),
                None => worlds[i],
            };
            scene.joints.push(SceneJoint {
                name: (*name).into(),
                parent,
                local,
                inverse_bind: math::invert(&worlds[i]),
            });
        }
        // Pelvis turns 90° about the model's Y axis.
        let turn = math::axis_quat(1, std::f32::consts::FRAC_PI_2);
        let rxq = math::matrix_quat(&rx);
        let global = math::compose([0.0, 1.0, 0.0], math::quat_mul(turn, rxq), [1.0; 3]);
        let (t, r, _) = math::decompose(&math::multiply(&math::invert(&worlds[0]), &global));
        scene.clips.push(SceneClip {
            name: "turn".into(),
            frame_rate: 30.0,
            tracks: vec![JointTrack {
                joint: 1,
                rotations: vec![r],
                translations: vec![t],
                scales: vec![],
            }],
        });
        let back = clips_to_game(&scene, &skeleton, &ClipOptions::default()).unwrap();
        (
            back[0].rest,
            back[0].tracks[1].rotations[0],
            back[0].tracks[2].translations[0],
        )
    }

    #[test]
    fn retargets_rigs_with_other_rest_orientations() {
        let turn = math::axis_quat(1, std::f32::consts::FRAC_PI_2);
        // Quarter turns: other bone axes; 50°: another rest pose.
        for (angle, mode) in [(90f32, RestPose::Axes), (50.0, RestPose::Retarget)] {
            let (rest, got, spine) = other_rest(angle.to_radians());
            assert_eq!(rest, mode);
            let dot: f32 = (0..4).map(|i| turn[i] * got[i]).sum();
            assert!(dot.abs() > 0.9999, "{angle}: {got:?} vs {turn:?}");
            // Bone lengths stay the game's.
            assert!((spine[1] - 0.5).abs() < 1e-5, "{angle}: {spine:?}");
        }
    }

    #[test]
    fn keys_interpolate_and_clamp() {
        let keys = Keys {
            times: vec![0.0, 1.0],
            values: vec![vec![0.0, 0.0, 0.0], vec![2.0, 4.0, 6.0]],
            step: false,
        };
        assert_eq!(sample_keys(&keys, 0.5, false), vec![1.0, 2.0, 3.0]);
        assert_eq!(sample_keys(&keys, 5.0, false), vec![2.0, 4.0, 6.0]);
        let step = Keys { step: true, ..keys };
        assert_eq!(sample_keys(&step, 0.5, false), vec![0.0; 3]);
    }
}

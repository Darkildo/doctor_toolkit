//! Animation clips between scenes and the game skeleton.
//!
//! The game stores, per frame and per bone, a rotation and a translation in
//! the bone's parent space ([`BoneTrack`], `resource/anim.rs`), at 29.97
//! frames per second ([`FRAME_RATE`]). [`clips_to_game`] turns the clips of
//! any [`Scene`] (glTF, FBX, USD) into such tracks on a game skeleton:
//!
//! the clips are resampled to the game frame rate
//! ([`super::retarget::scene_animations`]) and fitted to every bone of the
//! skeleton by the retargeting shared with clip import
//! ([`super::retarget::retarget`]: joints matched by name, rest poses
//! reconciled by [`RestPose`], bones the file does not animate in their bind
//! pose, quaternions on one hemisphere frame to frame).
//!
//! [`clips_scene`] does the reverse for writing clips to any format.

use std::collections::HashMap;

use super::Scene;
use super::math;
use super::retarget::{self, RetargetOptions};
use crate::export::gltf::SceneAnimation;
use crate::resource::anim::{BoneTrack, FRAME_RATE};
use crate::resource::rcb::Skeleton;
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

pub use super::retarget::RestPose;

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

/// Converts the clips of `scene` to tracks on a game skeleton, every bone
/// of it, through the shared retargeting ([`super::retarget::retarget`]).
pub fn clips_to_game(
    scene: &Scene,
    skeleton: &Skeleton,
    options: &ClipOptions,
) -> Result<Vec<GameClip>> {
    if scene.clips.is_empty() {
        return Err(Error::InvalidArgument("the file has no animation".into()));
    }
    let animations = retarget::scene_animations(scene, options.frame_rate)?;
    // Joint → bone names here, bone → joint there.
    let fit = RetargetOptions {
        rest: options.rest,
        map: options
            .bone_map
            .iter()
            .map(|(joint, bone)| (bone.clone(), joint.clone()))
            .collect(),
    };
    let bones: Vec<usize> = (0..skeleton.bones.len()).collect();
    let mut out = Vec::new();
    for anim in &animations {
        if !options.clips.is_empty()
            && !options
                .clips
                .iter()
                .any(|c| c.eq_ignore_ascii_case(&anim.name))
        {
            continue;
        }
        let fitted = retarget::retarget(anim, skeleton, &bones, &fit)?;
        out.push(GameClip {
            name: anim.name.clone(),
            frames: fitted.frames,
            frame_rate: options.frame_rate.max(1.0),
            tracks: fitted.tracks,
            animated: fitted.animated,
            rest: fitted.rest,
            unmapped: fitted.unmatched,
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
    use crate::scene::{JointTrack, SceneClip, SceneJoint};

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

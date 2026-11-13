//! The shared retargeting ([`super::retarget`]) against the two paths it
//! replaced: `import::anim::retarget` (stage 26, clips into banks) and
//! `scene::anim::clips_to_game` (stage 25). Their code is kept here, as it
//! was, to compare results.

use std::collections::HashMap;

use super::retarget::*;
use crate::resource::anim::{BoneTrack, SourceAnimation, SourceJoint};
use crate::resource::rcb::{Bone, Skeleton};
use crate::scene::{JointTrack, Scene, SceneClip, SceneJoint, math};

#[allow(dead_code, clippy::all)]
mod legacy_import {
    use crate::hash::sdbm;
    use crate::resource::anim::{BoneTrack, SourceAnimation};
    use crate::resource::rcb::{Mat4, Skeleton};
    use crate::{Error, Result};
    use serde::Serialize;
    use std::collections::HashMap;
    fn invalid(message: impl Into<String>) -> Error {
        Error::InvalidArgument(message.into())
    }

    type Quat = [f32; 4];
    type Vec3 = [f32; 3];

    const IDENTITY: Quat = [0.0, 0.0, 0.0, 1.0];

    fn qmul(a: Quat, b: Quat) -> Quat {
        [
            a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
            a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
            a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
            a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
        ]
    }

    fn qconj(q: Quat) -> Quat {
        [-q[0], -q[1], -q[2], q[3]]
    }

    fn qnorm(q: Quat) -> Quat {
        let n = q.iter().map(|v| v * v).sum::<f32>().sqrt();
        if n > 1e-8 { q.map(|v| v / n) } else { IDENTITY }
    }

    fn qrot(q: Quat, v: Vec3) -> Vec3 {
        let p = qmul(qmul(q, [v[0], v[1], v[2], 0.0]), qconj(q));
        [p[0], p[1], p[2]]
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

    /// How the joints of a source animation map to a game skeleton.
    #[derive(Debug, Clone, Default, PartialEq, Serialize)]
    pub struct Retargeted {
        /// One track per requested bone, in the requested order.
        pub tracks: Vec<BoneTrack>,
        pub frames: usize,
        /// Game bones (of the requested set) with a source joint, by name.
        pub mapped: Vec<String>,
        /// Requested bones without a source joint: held in the bind pose.
        pub unmapped: Vec<String>,
        /// Source joints that drive no bone.
        pub unused: Vec<String>,
        /// Translation scale (target height / source height).
        pub scale: f32,
    }

    /// Normalised name for matching: lower case, without a namespace prefix
    /// (`mixamorig:Hips` → `hips`).
    fn match_name(name: &str) -> String {
        name.rsplit([':', '|'])
            .next()
            .unwrap_or(name)
            .to_ascii_lowercase()
    }

    /// Global rest rotations and translations from local ones.
    fn globals(parents: &[Option<usize>], locals: &[(Quat, Vec3)]) -> Vec<(Quat, Vec3)> {
        let mut out: Vec<Option<(Quat, Vec3)>> = vec![None; locals.len()];
        fn get(
            i: usize,
            parents: &[Option<usize>],
            locals: &[(Quat, Vec3)],
            out: &mut Vec<Option<(Quat, Vec3)>>,
            depth: usize,
        ) -> (Quat, Vec3) {
            if let Some(g) = out[i] {
                return g;
            }
            let (r, t) = locals[i];
            let g = match parents[i].filter(|&p| p != i && depth < 1000) {
                Some(p) => {
                    let (pr, pt) = get(p, parents, locals, out, depth + 1);
                    let tt = qrot(pr, t);
                    (
                        qnorm(qmul(pr, r)),
                        [pt[0] + tt[0], pt[1] + tt[1], pt[2] + tt[2]],
                    )
                }
                None => (r, t),
            };
            out[i] = Some(g);
            g
        }
        for i in 0..locals.len() {
            get(i, parents, locals, &mut out, 0);
        }
        out.into_iter()
            .map(|g| g.unwrap_or((IDENTITY, [0.0; 3])))
            .collect()
    }

    /// Height of a rest pose: the extent of the joints along Y.
    fn height(globals: &[(Quat, Vec3)]) -> f32 {
        let (lo, hi) = globals
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), g| {
                (lo.min(g.1[1]), hi.max(g.1[1]))
            });
        if hi > lo { hi - lo } else { 0.0 }
    }

    /// Fits a source animation to `bones` (indices of `skeleton`): a track per
    /// bone. `map` names source joints for game bones (game name → joint name)
    /// before matching by name.
    pub fn retarget(
        source: &SourceAnimation,
        skeleton: &Skeleton,
        bones: &[usize],
        map: &HashMap<String, String>,
    ) -> Result<Retargeted> {
        let frames = source.frames.max(1);
        let n = skeleton.bones.len();
        // Target rest pose.
        let target_locals: Vec<(Quat, Vec3)> =
            skeleton.bind_locals().iter().map(decompose).collect();
        let target_parents: Vec<Option<usize>> = skeleton.bones.iter().map(|b| b.parent).collect();
        let target_rest = globals(&target_parents, &target_locals);
        // Source rest pose.
        let source_parents: Vec<Option<usize>> = source.joints.iter().map(|j| j.parent).collect();
        let source_locals: Vec<(Quat, Vec3)> = source
            .joints
            .iter()
            .map(|j| (qnorm(j.rest_rotation), j.rest_translation))
            .collect();
        let source_rest = globals(&source_parents, &source_locals);
        // Bone → joint.
        let by_name: HashMap<String, usize> = source
            .joints
            .iter()
            .enumerate()
            .map(|(i, j)| (match_name(&j.name), i))
            .collect();
        let joint_of: Vec<Option<usize>> = skeleton
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
                by_name.get(&match_name(&name)).copied().or_else(|| {
                    source
                        .joints
                        .iter()
                        .position(|j| sdbm(match_name(&j.name).as_bytes()) == b.hash)
                })
            })
            .collect();
        let mapped_joints: Vec<usize> = joint_of.iter().flatten().copied().collect();
        if mapped_joints.is_empty() {
            return Err(invalid(format!(
                "no joint of the animation matches a bone of skeleton `{}` by name",
                skeleton.name
            )));
        }
        let source_height = height(
            &mapped_joints
                .iter()
                .map(|&j| source_rest[j])
                .collect::<Vec<_>>(),
        );
        let target_height = height(
            &(0..n)
                .filter(|&b| joint_of[b].is_some())
                .map(|b| target_rest[b])
                .collect::<Vec<_>>(),
        );
        let scale = if source_height > 1e-6 && target_height > 1e-6 {
            target_height / source_height
        } else {
            1.0
        };
        // Source global rotations per frame.
        let source_frames: Vec<Vec<(Quat, Vec3)>> = (0..frames)
            .map(|f| {
                let locals: Vec<(Quat, Vec3)> = source
                    .joints
                    .iter()
                    .map(|j| {
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
                    })
                    .collect();
                globals(&source_parents, &locals)
            })
            .collect();
        // Target global rotations per frame: the source's change from its rest
        // pose, applied to the bone's rest pose; unmapped bones follow their parent.
        let mut tracks: Vec<BoneTrack> = bones
            .iter()
            .map(|&b| BoneTrack {
                bone: b,
                rotations: Vec::with_capacity(frames),
                translations: Vec::with_capacity(frames),
            })
            .collect();
        for (f, src) in source_frames.iter().enumerate() {
            let mut global = vec![IDENTITY; n];
            let mut local = vec![(IDENTITY, [0.0f32; 3]); n];
            // Bones are stored parents first.
            for b in 0..n {
                let (rest_local_r, rest_local_t) = target_locals[b];
                let parent_global = target_parents[b]
                    .filter(|&p| p < b)
                    .map_or(IDENTITY, |p| global[p]);
                let (r, t) = match joint_of[b] {
                    Some(j) => {
                        let delta = qmul(src[j].0, qconj(source_rest[j].0));
                        let g = qnorm(qmul(delta, target_rest[b].0));
                        let r = qnorm(qmul(qconj(parent_global), g));
                        // Translation: rest plus the source's offset from its rest,
                        // moved into the bone's parent space and scaled.
                        let joint = &source.joints[j];
                        let t = match &joint.translations {
                            Some(v) => {
                                let now = v
                                    .get(f)
                                    .or(v.last())
                                    .copied()
                                    .unwrap_or(joint.rest_translation);
                                let rest = joint.rest_translation;
                                let offset = [now[0] - rest[0], now[1] - rest[1], now[2] - rest[2]];
                                // Source parent space → model space (rest) →
                                // target parent space.
                                let source_parent =
                                    joint.parent.map_or(IDENTITY, |p| source_rest[p].0);
                                let target_parent =
                                    target_parents[b].map_or(IDENTITY, |p| target_rest[p].0);
                                let d = qrot(qmul(qconj(target_parent), source_parent), offset);
                                [
                                    rest_local_t[0] + d[0] * scale,
                                    rest_local_t[1] + d[1] * scale,
                                    rest_local_t[2] + d[2] * scale,
                                ]
                            }
                            None => rest_local_t,
                        };
                        (r, t)
                    }
                    None => (rest_local_r, rest_local_t),
                };
                global[b] = qnorm(qmul(parent_global, r));
                local[b] = (r, t);
            }
            for track in &mut tracks {
                let (r, t) = local[track.bone];
                track.rotations.push(r);
                track.translations.push(t);
            }
        }
        let name = |b: usize| skeleton.bones[b].display_name();
        let used: std::collections::HashSet<usize> = mapped_joints.iter().copied().collect();
        Ok(Retargeted {
            tracks,
            frames,
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
                .filter(|(i, j)| {
                    !used.contains(i) && (j.rotations.is_some() || j.translations.is_some())
                })
                .map(|(_, j)| j.name.clone())
                .collect(),
            scale,
        })
    }
}

#[allow(dead_code, clippy::all)]
mod legacy_scene {
    use crate::resource::anim::BoneTrack;
    use crate::resource::rcb::{Mat4, Skeleton};
    use crate::scene::anim::{ClipOptions, GameClip, RestPose};
    use crate::scene::math::{self, IDENTITY};
    use crate::scene::{Scene, SceneClip};
    use crate::{Error, Result};
    use std::collections::HashMap;
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
                        Some(_) => {
                            math::point(&parent_world, math::translation_of(&game_locals[b]))
                        }
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
}

type Quat = [f32; 4];
type Vec3 = [f32; 3];

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

fn axis(axis: Vec3, a: f32) -> Quat {
    let s = (a / 2.0).sin();
    [axis[0] * s, axis[1] * s, axis[2] * s, (a / 2.0).cos()]
}

/// A chain `m_root` → `m_pelvis` → `m_spine` along Y.
fn skeleton() -> Skeleton {
    let bone = |name: &'static str, parent, y: f32| Bone {
        hash: crate::hash::sdbm(name.as_bytes()),
        name: Some(name),
        parent,
        bind_index: -1,
        bind_type: 1,
        inverse_bind: math::invert(&math::translation([0.0, y, 0.0])),
    };
    Skeleton {
        name_hash: 0,
        name: "s".into(),
        bones: vec![
            bone("m_root", None, 0.0),
            bone("m_pelvis", Some(0), 1.0),
            bone("m_spine", Some(1), 1.5),
        ],
    }
}

/// The same motion on a source chain `size` times as big whose joints have
/// rest rotations `rest` (model space): the root turns about Y and rises,
/// the pelvis bends about Z.
fn source(rest: [Quat; 3], size: f32, frames: usize) -> SourceAnimation {
    let world = |f: usize| -> [Quat; 3] {
        let a = f as f32 * 0.3;
        let g0 = axis([0.0, 1.0, 0.0], a);
        let g1 = qmul(g0, axis([0.0, 0.0, 1.0], a));
        [g0, g1, g1]
    };
    let positions: [Vec3; 3] = [[0.0; 3], [0.0, size, 0.0], [0.0, 1.5 * size, 0.0]];
    let mut joints = Vec::new();
    for j in 0..3usize {
        let parent = j.checked_sub(1);
        let (pr, pp) = parent.map_or(([0.0, 0.0, 0.0, 1.0], [0.0; 3]), |p| {
            (rest[p], positions[p])
        });
        let d = [
            positions[j][0] - pp[0],
            positions[j][1] - pp[1],
            positions[j][2] - pp[2],
        ];
        joints.push(SourceJoint {
            name: ["Rig:m_root", "Rig:m_pelvis", "Rig:m_spine"][j].into(),
            parent,
            rest_rotation: qmul(qconj(pr), rest[j]),
            rest_translation: qrot(qconj(pr), d),
            rotations: Some(Vec::new()),
            translations: None,
        });
    }
    let mut rise = Vec::new();
    for f in 0..frames {
        let g: Vec<Quat> = (0..3).map(|j| qmul(world(f)[j], rest[j])).collect();
        for j in 0..3usize {
            let p = j.checked_sub(1).map_or([0.0, 0.0, 0.0, 1.0], |p| g[p]);
            joints[j]
                .rotations
                .as_mut()
                .unwrap()
                .push(math::quat_normalize(qmul(qconj(p), g[j])));
        }
        rise.push([0.0, f as f32 * 0.2, 0.0]);
    }
    joints[0].translations = Some(rise);
    SourceAnimation {
        name: "test".into(),
        frames,
        joints,
    }
}

fn same_tracks(a: &[BoneTrack], b: &[BoneTrack], what: &str) {
    assert_eq!(a.len(), b.len(), "{what}");
    for (x, y) in a.iter().zip(b) {
        assert_eq!(x.bone, y.bone, "{what}");
        assert_eq!(x.rotations.len(), y.rotations.len(), "{what}");
        for (f, (p, q)) in x.rotations.iter().zip(&y.rotations).enumerate() {
            let dot: f32 = (0..4).map(|i| p[i] * q[i]).sum();
            assert!(
                dot.abs() > 0.99999,
                "{what}: bone {} frame {f}: {p:?} vs {q:?}",
                x.bone
            );
        }
        for (f, (p, q)) in x.translations.iter().zip(&y.translations).enumerate() {
            assert!(
                (0..3).all(|i| (p[i] - q[i]).abs() < 1e-4),
                "{what}: bone {} frame {f}: {p:?} vs {q:?}",
                x.bone
            );
        }
    }
}

/// Half a square root of two: quarter turns.
const H: f32 = std::f32::consts::FRAC_1_SQRT_2;

const REST_SETS: [[Quat; 3]; 3] = [
    // The game's axes.
    [[0.0, 0.0, 0.0, 1.0]; 3],
    // Other axes on every joint.
    [
        [0.329_191, 0.0, 0.0, 0.944_26],
        [0.0, 0.0, -0.522_687, 0.852_524],
        [0.0, 0.841_471, 0.0, 0.540_302],
    ],
    // Quarter turns (Blender's FBX bone axes).
    [[H, 0.0, 0.0, H], [H, 0.0, 0.0, H], [0.0, H, 0.0, H]],
];

/// The clip import path (stage 26) gave the same tracks: its mode is
/// `Retarget`, and `Auto` agrees with it on rotations wherever it picks
/// another mode, and on translations for skeletons of the same size.
#[test]
fn matches_the_old_import_path() {
    let skeleton = skeleton();
    for (k, rest) in REST_SETS.iter().enumerate() {
        for size in [1.0, 2.0] {
            let src = source(*rest, size, 6);
            let old =
                legacy_import::retarget(&src, &skeleton, &[0, 1, 2], &HashMap::new()).unwrap();
            let options = RetargetOptions {
                rest: RestPose::Retarget,
                map: HashMap::new(),
            };
            let new = retarget(&src, &skeleton, &[0, 1, 2], &options).unwrap();
            let what = format!("rest set {k}, size {size}");
            same_tracks(&old.tracks, &new.tracks, &what);
            assert_eq!(old.scale, new.scale, "{what}");
            assert_eq!(old.mapped, new.mapped);
            assert_eq!(old.unmapped, new.unmapped);
            assert_eq!(old.unused, new.unused);
            if size == 1.0 {
                let auto =
                    retarget(&src, &skeleton, &[0, 1, 2], &RetargetOptions::default()).unwrap();
                assert_eq!(
                    auto.rest,
                    [RestPose::AsIs, RestPose::Retarget, RestPose::Axes][k],
                    "{what}"
                );
                same_tracks(&old.tracks, &auto.tracks, &format!("{what}, auto"));
            }
        }
    }
}

/// A scene with the chain of [`source`] (rest frames turned by `rest`) and
/// one clip of it.
fn scene_of(rest: [Quat; 3], frames: usize) -> Scene {
    let src = source(rest, 1.0, frames);
    let mut scene = Scene::default();
    let mut worlds: Vec<crate::resource::rcb::Mat4> = Vec::new();
    for (j, joint) in src.joints.iter().enumerate() {
        let local = math::compose(joint.rest_translation, joint.rest_rotation, [1.0; 3]);
        let world = match joint.parent {
            Some(p) => math::multiply(&worlds[p], &local),
            None => local,
        };
        worlds.push(world);
        scene.joints.push(SceneJoint {
            name: joint.name.clone(),
            parent: joint.parent,
            local,
            inverse_bind: math::invert(&world),
        });
        let _ = j;
    }
    scene.clips.push(SceneClip {
        name: "test".into(),
        frame_rate: crate::resource::anim::FRAME_RATE,
        tracks: src
            .joints
            .iter()
            .enumerate()
            .map(|(j, joint)| JointTrack {
                joint: j,
                rotations: joint.rotations.clone().unwrap(),
                translations: joint
                    .translations
                    .clone()
                    .unwrap_or_else(|| vec![joint.rest_translation; frames]),
                scales: vec![],
            })
            .collect(),
    });
    scene
}

/// The scene path (stage 25, `clips_to_game`) gave the same tracks in its
/// `AsIs` and `Axes` modes; in `Retarget` it kept the game's bone lengths
/// and moved the root by the file's displacement, which is what the shared
/// mode gives for skeletons of the same size.
#[test]
fn matches_the_old_scene_path() {
    use crate::scene::anim::{ClipOptions, clips_to_game};
    let skeleton = skeleton();
    for (k, rest) in REST_SETS.iter().enumerate() {
        let scene = scene_of(*rest, 6);
        let options = ClipOptions::default();
        let old = legacy_scene::clips_to_game(&scene, &skeleton, &options).unwrap();
        let new = clips_to_game(&scene, &skeleton, &options).unwrap();
        let what = format!("rest set {k}");
        assert_eq!(old[0].rest, new[0].rest, "{what}");
        assert_eq!(old[0].frames, new[0].frames, "{what}");
        assert_eq!(old[0].animated, new[0].animated, "{what}");
        assert_eq!(old[0].unmapped, new[0].unmapped, "{what}");
        same_tracks(&old[0].tracks, &new[0].tracks, &what);
        for mode in [RestPose::AsIs, RestPose::Axes, RestPose::Retarget] {
            if (k == 1) != (mode == RestPose::Retarget) && k != 0 {
                continue;
            }
            let options = ClipOptions {
                rest: mode,
                ..ClipOptions::default()
            };
            let old = legacy_scene::clips_to_game(&scene, &skeleton, &options).unwrap();
            let new = clips_to_game(&scene, &skeleton, &options).unwrap();
            same_tracks(&old[0].tracks, &new[0].tracks, &format!("{what}, {mode:?}"));
        }
    }
}

/// Joint names: namespaces, any case, explicit maps, hashes.
#[test]
fn matches_joints_like_both_old_paths() {
    let skeleton = skeleton();
    let mut src = source(REST_SETS[0], 1.0, 2);
    src.joints[0].name = "mixamorig:M_ROOT".into();
    src.joints[1].name = "Armature|m_pelvis".into();
    src.joints[2].name = format!("_{:08x}", crate::hash::sdbm(b"m_spine"));
    assert_eq!(
        joints_for_bones(&src, &skeleton, &HashMap::new()),
        [Some(0), Some(1), Some(2)]
    );
    src.joints[2].name = "chest".into();
    let map = HashMap::from([("m_spine".to_owned(), "chest".to_owned())]);
    assert_eq!(joints_for_bones(&src, &skeleton, &map)[2], Some(2));
    let none = HashMap::new();
    let fitted = retarget(&src, &skeleton, &[0, 1, 2], &RetargetOptions::default()).unwrap();
    assert_eq!(fitted.unmapped, ["m_spine"]);
    assert_eq!(fitted.unused, ["chest"]);
    assert_eq!(joints_for_bones(&src, &skeleton, &none)[2], None);
}

/// FBX of an armature alone (no `BindPose`, no skin: Blender's export of
/// the armature without its mesh) is refused with a clear message; the same
/// file with its bind pose is read; a file without joints is refused.
#[test]
fn refuses_fbx_armatures_without_a_bind_pose() {
    use crate::scene::SideFiles;
    use crate::scene::anim::{ClipOptions, clips_to_game};
    use crate::scene::fbx::{self, node};
    let skeleton = skeleton();
    let scene = scene_of(REST_SETS[2], 4);
    let bytes = fbx::write(&scene).unwrap();
    let with = fbx::read(&bytes, &SideFiles::new(None)).unwrap();
    assert!(with.joints_without_bind.is_empty());
    assert!(clips_to_game(&with, &skeleton, &ClipOptions::default()).is_ok());
    // Drop the bind pose.
    let mut doc = node::parse(&bytes).unwrap();
    for top in &mut doc.nodes {
        if top.name == "Objects" {
            top.children.retain(|o| o.name != "Pose");
        }
    }
    let stripped = node::write_binary(&doc.nodes, doc.version);
    let without = fbx::read(&stripped, &SideFiles::new(None)).unwrap();
    assert_eq!(without.joints_without_bind.len(), 3);
    let err = clips_to_game(&without, &skeleton, &ClipOptions::default())
        .unwrap_err()
        .to_string();
    assert!(err.contains("no bind pose"), "{err}");
    let err = crate::import::anim::read_scene(&without)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("export the armature together with a mesh"),
        "{err}"
    );
    // No armature at all.
    let mut empty = scene.clone();
    empty.joints.clear();
    let err = scene_animations(&empty, 30.0).unwrap_err().to_string();
    assert!(err.contains("no armature"), "{err}");
}

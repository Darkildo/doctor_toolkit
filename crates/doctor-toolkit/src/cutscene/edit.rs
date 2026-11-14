//! Keys of a shot edited at a moment of the cut-scene (stage 62): the
//! camera's path (`0x304`) and focal length (`0x307`), and an actor's
//! position and rotation (`0x206`).
//!
//! An edit at time `t` (seconds from the start of the shot) changes the key
//! at `t` if there is one within [`KEY_TOLERANCE`], and inserts one
//! otherwise. A new camera key gets the slope the curve has at `t` as its
//! tangents, so the path keeps its shape around it; a new actor key is a
//! point of the linear path. Angles of the camera stay close to the curve's
//! (no jumps of 360°), quaternions of the actor keep the sign of the
//! previous key.

use super::{HALF_APERTURE_MM, Shot};
use crate::resource::trinity::{
    CURVE_FRAME_RATE, Command, CurveKey, Guid, PositionKey, RotationKey, SeqBlock, SeqMaster,
    eval_curve, eval_curves, eval_rotations,
};
use crate::{Error, Result};

/// Keys this close to the time edited are the key edited (seconds): half a
/// frame at 30 frames per second.
pub const KEY_TOLERANCE: f32 = 1.0 / 60.0;

/// What a key edit did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEdit {
    /// Changed the key at the time.
    Replaced,
    /// Added a key at the time.
    Inserted,
}

/// What is keyed: the camera of the shot or an actor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    Camera,
    Actor(Guid),
}

/// The command of the block that holds the keys of `target` at local time
/// `t`: the last such command started by `t` (or the first one, if `t` is
/// before all of them). `fov` picks the focal length (`0x307`) of the camera.
pub fn command_at(block: &SeqBlock, target: Target, fov: bool, t: f32) -> Option<usize> {
    let times = block.times();
    let wanted = |c: &Command| match (target, c) {
        (Target::Camera, Command::CameraCurves { .. }) => !fov,
        (Target::Camera, Command::CameraFov { .. }) => fov,
        (Target::Actor(g), Command::ActorTransform { actor, .. }) => *actor == g,
        _ => false,
    };
    let mut found = None;
    for (i, c) in block.commands.iter().enumerate() {
        if !wanted(c) {
            continue;
        }
        if times[i] <= t + KEY_TOLERANCE || found.is_none() {
            found = Some(i);
        }
        if times[i] > t {
            break;
        }
    }
    found
}

/// The time keys of `target` are at, in seconds from the start of the shot,
/// in order (the camera: keys of any of its six curves and of its focal
/// length).
pub fn key_times(block: &SeqBlock, target: Target) -> Vec<f32> {
    let times = block.times();
    let mut out: Vec<f32> = Vec::new();
    for (c, &at) in block.commands.iter().zip(&times) {
        match (target, c) {
            (Target::Camera, Command::CameraCurves { curves, .. }) => {
                out.extend(curves.iter().flatten().map(|k| at + k.time));
            }
            (Target::Camera, Command::CameraFov { keys, .. }) => {
                out.extend(keys.iter().map(|k| at + k.time));
            }
            (
                Target::Actor(g),
                Command::ActorTransform {
                    actor,
                    positions,
                    rotations,
                },
            ) if *actor == g => {
                out.extend(positions.iter().map(|k| at + k.time));
                out.extend(rotations.iter().map(|k| at + k.time));
            }
            _ => {}
        }
    }
    out.sort_by(f32::total_cmp);
    out.dedup_by(|a, b| (*a - *b).abs() < KEY_TOLERANCE);
    out
}

/// The key of `target` at `t` (seconds from the start of the shot), if one
/// is within [`KEY_TOLERANCE`].
pub fn key_at(block: &SeqBlock, target: Target, t: f32) -> Option<f32> {
    key_times(block, target)
        .into_iter()
        .find(|k| (k - t).abs() < KEY_TOLERANCE)
}

/// Sets `value` on a curve at `t`: the key there, or a new key with the
/// curve's slope at `t`.
fn set_curve(keys: &mut Vec<CurveKey>, t: f32, value: f32) -> KeyEdit {
    if let Some(k) = keys.iter_mut().find(|k| (k.time - t).abs() < KEY_TOLERANCE) {
        k.value = value;
        return KeyEdit::Replaced;
    }
    // Slope in value per second, as the tangent angle of the format (value
    // per frame at 30 frames per second).
    let h = 1.0 / 120.0;
    let slope = if keys.len() > 1 {
        (eval_curve(keys, t + h) - eval_curve(keys, t - h)) / (2.0 * h)
    } else {
        0.0
    };
    let tangent = (slope / CURVE_FRAME_RATE).atan();
    let mut key = CurveKey::flat(t, value);
    key.in_tangent = tangent;
    key.out_tangent = tangent;
    if let Some(w) = keys.first() {
        key.in_weight = w.in_weight;
        key.out_weight = w.out_weight;
    }
    let at = keys.iter().position(|k| k.time > t).unwrap_or(keys.len());
    keys.insert(at, key);
    KeyEdit::Inserted
}

/// `angle` (degrees) moved by whole turns to be nearest to `near`.
pub fn unwrap_degrees(angle: f32, near: f32) -> f32 {
    angle + ((near - angle) / 360.0).round() * 360.0
}

/// Euler angles `x, y, z` in degrees of a camera matrix (`R = Rz·Ry·Rx`,
/// column-major as [`crate::resource::trinity::camera_matrix`] makes it):
/// of the two solutions, the one nearest to `near`, each angle moved by
/// whole turns towards it.
pub fn euler_of(m: &[f32; 16], near: [f32; 3]) -> [f32; 3] {
    // r[row][col] = m[col * 4 + row].
    let r = |row: usize, col: usize| m[col * 4 + row];
    let sy = (-r(2, 0)).clamp(-1.0, 1.0);
    let candidates: Vec<[f32; 3]> = if sy.abs() > 0.99999 {
        // Gimbal lock: x and z turn about the same axis; keep z. With
        // cos y = 0: sin y = 1 gives r01 = sin(x − z), r11 = cos(x − z);
        // sin y = −1 gives r01 = −sin(x + z), r11 = cos(x + z).
        let ry = sy.asin();
        let rz = near[2].to_radians();
        let rx = if sy > 0.0 {
            rz + r(0, 1).atan2(r(1, 1))
        } else {
            -rz + (-r(0, 1)).atan2(r(1, 1))
        };
        vec![[rx, ry, rz]]
    } else {
        let ry = sy.asin();
        let a = {
            let cy = ry.cos();
            [
                (r(2, 1) / cy).atan2(r(2, 2) / cy),
                ry,
                (r(1, 0) / cy).atan2(r(0, 0) / cy),
            ]
        };
        let ry2 = std::f32::consts::PI - ry;
        let b = {
            let cy = ry2.cos();
            [
                (r(2, 1) / cy).atan2(r(2, 2) / cy),
                ry2,
                (r(1, 0) / cy).atan2(r(0, 0) / cy),
            ]
        };
        vec![a, b]
    };
    candidates
        .into_iter()
        .map(|c| {
            let d = c.map(f32::to_degrees);
            [
                unwrap_degrees(d[0], near[0]),
                unwrap_degrees(d[1], near[1]),
                unwrap_degrees(d[2], near[2]),
            ]
        })
        .min_by(|a, b| {
            let dist = |v: &[f32; 3]| (0..3).map(|i| (v[i] - near[i]).abs()).sum::<f32>();
            dist(a).total_cmp(&dist(b))
        })
        .expect("a solution")
}

/// The rotation of a column-major matrix as a unit quaternion `(x, y, z, w)`
/// (the inverse of [`super::ActorState::matrix`]).
pub fn quaternion_of(m: &[f32; 16]) -> [f32; 4] {
    let r = |row: usize, col: usize| m[col * 4 + row];
    // Normalise the axes first: the matrix may carry a scale.
    let len = |c: usize| {
        (0..3)
            .map(|i| r(i, c).powi(2))
            .sum::<f32>()
            .sqrt()
            .max(1e-9)
    };
    let (l0, l1, l2) = (len(0), len(1), len(2));
    let a = |row: usize, col: usize| r(row, col) / [l0, l1, l2][col];
    let trace = a(0, 0) + a(1, 1) + a(2, 2);
    let q = if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        [
            (a(2, 1) - a(1, 2)) / s,
            (a(0, 2) - a(2, 0)) / s,
            (a(1, 0) - a(0, 1)) / s,
            0.25 * s,
        ]
    } else if a(0, 0) > a(1, 1) && a(0, 0) > a(2, 2) {
        let s = (1.0 + a(0, 0) - a(1, 1) - a(2, 2)).sqrt() * 2.0;
        [
            0.25 * s,
            (a(0, 1) + a(1, 0)) / s,
            (a(0, 2) + a(2, 0)) / s,
            (a(2, 1) - a(1, 2)) / s,
        ]
    } else if a(1, 1) > a(2, 2) {
        let s = (1.0 + a(1, 1) - a(0, 0) - a(2, 2)).sqrt() * 2.0;
        [
            (a(0, 1) + a(1, 0)) / s,
            0.25 * s,
            (a(1, 2) + a(2, 1)) / s,
            (a(0, 2) - a(2, 0)) / s,
        ]
    } else {
        let s = (1.0 + a(2, 2) - a(0, 0) - a(1, 1)).sqrt() * 2.0;
        [
            (a(0, 2) + a(2, 0)) / s,
            (a(1, 2) + a(2, 1)) / s,
            0.25 * s,
            (a(1, 0) - a(0, 1)) / s,
        ]
    };
    let n = q.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-12);
    q.map(|v| v / n)
}

fn block_mut(seq: &mut SeqMaster) -> Result<&mut SeqBlock> {
    seq.block_mut()
        .ok_or_else(|| Error::InvalidArgument("the shot has no command block".into()))
}

/// Sets the camera at `t` (seconds from the start of the shot) to `values`
/// (x, y, z and the angles x, y, z in degrees) on the camera path playing
/// then: the key at `t` of each curve, or new keys.
pub fn set_camera(seq: &mut SeqMaster, t: f32, values: [f32; 6]) -> Result<KeyEdit> {
    let block = block_mut(seq)?;
    let index = command_at(block, Target::Camera, false, t)
        .ok_or_else(|| Error::InvalidArgument("the shot has no camera path".into()))?;
    let at = block.times()[index];
    let Command::CameraCurves { curves, .. } = &mut block.commands[index] else {
        unreachable!("a camera path");
    };
    let local = (t - at).max(0.0);
    // Angles stay near the curve's (no turn of 360° between keys).
    let now = eval_curves(curves, local);
    let mut values = values;
    for i in 3..6 {
        values[i] = unwrap_degrees(values[i], now[i]);
    }
    let mut edit = KeyEdit::Replaced;
    for (keys, value) in curves.iter_mut().zip(values) {
        if set_curve(keys, local, value) == KeyEdit::Inserted {
            edit = KeyEdit::Inserted;
        }
    }
    Ok(edit)
}

/// The focal length in millimetres for a vertical field of view (degrees)
/// of the 16:9 picture: the inverse of [`super::CameraState::vertical_fov`].
pub fn focal_length(vertical_fov: f32) -> f32 {
    HALF_APERTURE_MM / (vertical_fov.clamp(1.0, 170.0).to_radians() / 2.0).tan()
}

/// Sets the focal length (millimetres) of the camera at `t`.
pub fn set_focal_length(seq: &mut SeqMaster, t: f32, focal: f32) -> Result<KeyEdit> {
    let block = block_mut(seq)?;
    let index = command_at(block, Target::Camera, true, t)
        .ok_or_else(|| Error::InvalidArgument("the shot has no focal length curve".into()))?;
    let at = block.times()[index];
    let Command::CameraFov { keys, .. } = &mut block.commands[index] else {
        unreachable!("a focal length curve");
    };
    Ok(set_curve(keys, (t - at).max(0.0), focal.max(1.0)))
}

/// Places actor `actor` at `t`: its position and (or) rotation key at `t`,
/// or new keys. An actor without position keys in the shot gets a
/// placement command where it is created.
pub fn set_actor(
    seq: &mut SeqMaster,
    actor: Guid,
    t: f32,
    position: Option<[f32; 3]>,
    rotation: Option<[f32; 4]>,
) -> Result<KeyEdit> {
    let block = block_mut(seq)?;
    let index = match command_at(block, Target::Actor(actor), false, t) {
        Some(i) => i,
        None => {
            // A placement from where the actor is created.
            let times = block.times();
            let (created, start) = block
                .commands
                .iter()
                .zip(&times)
                .find_map(|(c, &at)| match c {
                    Command::ActorCreate {
                        actor: a, position, ..
                    } if *a == actor => Some((*position, at)),
                    _ => None,
                })
                .ok_or_else(|| Error::InvalidArgument("the actor is not in the shot".into()))?;
            let local = (t - start).max(0.0);
            block.insert_at(
                start,
                Command::ActorTransform {
                    actor,
                    positions: vec![PositionKey {
                        time: local,
                        position: position.unwrap_or(created),
                    }],
                    rotations: vec![RotationKey {
                        time: local,
                        rotation: rotation.unwrap_or([0.0, 0.0, 0.0, 1.0]),
                    }],
                },
            );
            return Ok(KeyEdit::Inserted);
        }
    };
    let at = block.times()[index];
    let Command::ActorTransform {
        positions,
        rotations,
        ..
    } = &mut block.commands[index]
    else {
        unreachable!("a placement");
    };
    let local = (t - at).max(0.0);
    let mut edit = KeyEdit::Replaced;
    if let Some(p) = position {
        match positions
            .iter_mut()
            .find(|k| (k.time - local).abs() < KEY_TOLERANCE)
        {
            Some(k) => k.position = p,
            None => {
                let i = positions
                    .iter()
                    .position(|k| k.time > local)
                    .unwrap_or(positions.len());
                positions.insert(
                    i,
                    PositionKey {
                        time: local,
                        position: p,
                    },
                );
                edit = KeyEdit::Inserted;
            }
        }
    }
    if let Some(mut q) = rotation {
        // The sign of the rotation now: the blend between keys stays short.
        if let Some(now) = eval_rotations(rotations, local)
            && (0..4).map(|i| now[i] * q[i]).sum::<f32>() < 0.0
        {
            q = q.map(|v| -v);
        }
        match rotations
            .iter_mut()
            .find(|k| (k.time - local).abs() < KEY_TOLERANCE)
        {
            Some(k) => k.rotation = q,
            None => {
                let i = rotations
                    .iter()
                    .position(|k| k.time > local)
                    .unwrap_or(rotations.len());
                rotations.insert(
                    i,
                    RotationKey {
                        time: local,
                        rotation: q,
                    },
                );
                edit = KeyEdit::Inserted;
            }
        }
    } else if position.is_some()
        && !rotations
            .iter()
            .any(|k| (k.time - local).abs() < KEY_TOLERANCE)
        && let Some(now) = eval_rotations(rotations, local)
    {
        // A new position key keeps the rotation it had (a key of both at
        // the same time, as the game's shots have them).
        let i = rotations
            .iter()
            .position(|k| k.time > local)
            .unwrap_or(rotations.len());
        rotations.insert(
            i,
            RotationKey {
                time: local,
                rotation: now,
            },
        );
    }
    Ok(edit)
}

/// Removes the keys of `target` at `t` (seconds from the start of the
/// shot): of the camera, every curve's key there and the focal length's;
/// of an actor, its position and rotation keys. A curve keeps its last key.
/// Returns how many keys went.
pub fn delete_key(seq: &mut SeqMaster, target: Target, t: f32) -> Result<usize> {
    let block = block_mut(seq)?;
    let times = block.times();
    let mut removed = 0;
    let mut kept_last = false;
    let mut take = |len: usize, at: Option<usize>| -> Option<usize> {
        let i = at?;
        if len <= 1 {
            kept_last = true;
            return None;
        }
        removed += 1;
        Some(i)
    };
    for (c, &at) in block.commands.iter_mut().zip(&times) {
        let local = t - at;
        let near = |time: f32| (time - local).abs() < KEY_TOLERANCE;
        match (target, c) {
            (Target::Camera, Command::CameraCurves { curves, .. }) => {
                for keys in curves.iter_mut() {
                    let at = keys.iter().position(|k| near(k.time));
                    if let Some(i) = take(keys.len(), at) {
                        keys.remove(i);
                    }
                }
            }
            (Target::Camera, Command::CameraFov { keys, .. }) => {
                let at = keys.iter().position(|k| near(k.time));
                if let Some(i) = take(keys.len(), at) {
                    keys.remove(i);
                }
            }
            (
                Target::Actor(g),
                Command::ActorTransform {
                    actor,
                    positions,
                    rotations,
                },
            ) if *actor == g => {
                let at = positions.iter().position(|k| near(k.time));
                if let Some(i) = take(positions.len(), at) {
                    positions.remove(i);
                }
                let at = rotations.iter().position(|k| near(k.time));
                if let Some(i) = take(rotations.len(), at) {
                    rotations.remove(i);
                }
            }
            _ => {}
        }
    }
    if removed == 0 {
        return Err(Error::InvalidArgument(if kept_last {
            "the only key of a curve stays".into()
        } else {
            format!("no key at {t:.3} s")
        }));
    }
    Ok(removed)
}

/// The time of the key of `target` before (`forward` false) or after `t`
/// in shot `shot`, in seconds from the start of the shot.
pub fn next_key(shot: &Shot, target: Target, t: f32, forward: bool) -> Option<f32> {
    let keys = key_times(shot.block(), target);
    if forward {
        keys.into_iter().find(|&k| k > t + KEY_TOLERANCE)
    } else {
        keys.into_iter().rev().find(|&k| k < t - KEY_TOLERANCE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::trinity::{FixedText, camera_matrix};

    fn guid() -> Guid {
        Guid(*b"\x01\x02\x03\x04\x05\x06\x07\x08DOCTORTK")
    }

    fn seq() -> SeqMaster {
        let actor = guid();
        SeqMaster {
            path: FixedText::new("x:/assets/igcs/igc99/source/igc99_s01.xml"),
            blocks: vec![SeqBlock {
                version: 4,
                hash: 0,
                transform: [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
                name: FixedText::new("trSeq_igc99_s01"),
                duration: 2.0,
                commands: vec![
                    Command::ActorCreate {
                        actor,
                        name: FixedText::new("igc99_hero"),
                        flags: [0; 3],
                        position: [1.0, 0.0, 0.0],
                        reserved: 0,
                    },
                    Command::Wait { seconds: 0.5 },
                    Command::CameraCurves {
                        camera: 0,
                        curves: std::array::from_fn(|i| match i {
                            0 => vec![CurveKey::flat(0.0, 0.0), CurveKey::flat(1.0, 10.0)],
                            4 => vec![CurveKey::flat(0.0, 170.0), CurveKey::flat(1.0, 179.0)],
                            _ => vec![CurveKey::flat(0.0, 0.0)],
                        }),
                    },
                    Command::CameraFov {
                        camera: 0,
                        keys: vec![CurveKey::flat(0.0, 50.0)],
                    },
                    Command::Wait { seconds: 1.5 },
                ],
            }],
        }
    }

    #[test]
    fn a_camera_key_is_replaced_on_a_key_and_inserted_between() {
        let mut s = seq();
        // On the key at 1.5 s (0.5 + 1.0).
        let edit = set_camera(&mut s, 1.5, [12.0, 1.0, 2.0, 0.0, 180.0, 0.0]).unwrap();
        assert_eq!(edit, KeyEdit::Inserted, "y and z had one key only");
        let Command::CameraCurves { curves, .. } = &s.blocks[0].commands[2] else {
            panic!()
        };
        assert_eq!(curves[0].len(), 2);
        assert_eq!(curves[0][1].value, 12.0);
        assert_eq!(curves[1].len(), 2);
        // Between keys: a key in every curve with the slope of the curve.
        let mut s = seq();
        set_camera(&mut s, 1.0, [5.0, 0.0, 0.0, 0.0, 175.0, 0.0]).unwrap();
        let Command::CameraCurves { curves, .. } = &s.blocks[0].commands[2] else {
            panic!()
        };
        assert_eq!(curves[0].len(), 3);
        assert!((curves[0][1].time - 0.5).abs() < 1e-6);
        assert!(curves[0][1].in_tangent.abs() > 0.0, "slope kept");
        // Values at the key are what was set.
        let state = super::super::camera_state(
            &Shot {
                resource: 0,
                id: 0,
                seq: s.clone(),
                start: 0.0,
                stream: None,
                stream_name: String::new(),
                actors: Vec::new(),
                clips: Vec::new(),
            },
            1.0,
        )
        .unwrap();
        assert!((state.values[0] - 5.0).abs() < 1e-4);
        assert!((state.values[4] - 175.0).abs() < 1e-4);
        // A turn of 360° is taken off.
        let mut s = seq();
        set_camera(&mut s, 1.5, [10.0, 0.0, 0.0, 0.0, -178.0, 0.0]).unwrap();
        let Command::CameraCurves { curves, .. } = &s.blocks[0].commands[2] else {
            panic!()
        };
        assert!((curves[4][1].value - 182.0).abs() < 1e-3);
    }

    #[test]
    fn euler_angles_come_back_from_the_matrix() {
        for v in [
            [10.0f32, 20.0, 30.0],
            [-80.0, 45.0, 170.0],
            [0.0, 0.0, 0.0],
            [5.0, -60.0, -120.0],
        ] {
            let m = camera_matrix([0.0, 0.0, 0.0, v[0], v[1], v[2]]);
            let back = euler_of(&m, v);
            for i in 0..3 {
                assert!((back[i] - v[i]).abs() < 1e-2, "{v:?} → {back:?}");
            }
            // Near another solution the same matrix comes back.
            let other = euler_of(&m, [v[0] + 180.0, 180.0 - v[1], v[2] + 180.0]);
            let m2 = camera_matrix([0.0, 0.0, 0.0, other[0], other[1], other[2]]);
            for i in 0..16 {
                assert!((m[i] - m2[i]).abs() < 1e-4);
            }
        }
    }

    #[test]
    fn quaternions_come_back_from_the_matrix() {
        let state = super::super::ActorState {
            position: [1.0, 2.0, 3.0],
            rotation: crate::resource::trinity::euler_quaternion(10.0, 70.0, -30.0),
            clip: None,
            speed: 1.0,
        };
        let q = quaternion_of(&state.matrix());
        let dot: f32 = (0..4).map(|i| q[i] * state.rotation[i]).sum();
        assert!(dot.abs() > 0.9999, "{q:?} {:?}", state.rotation);
    }

    #[test]
    fn focal_length_and_field_of_view_agree() {
        let state = super::super::CameraState {
            shot: 0,
            camera: 0,
            values: [0.0; 6],
            fov: 35.0,
            aspect: 16.0 / 9.0,
        };
        assert!((focal_length(state.vertical_fov()) - 35.0).abs() < 1e-3);
        let mut s = seq();
        assert_eq!(
            set_focal_length(&mut s, 0.5, 70.0).unwrap(),
            KeyEdit::Replaced
        );
        assert_eq!(
            set_focal_length(&mut s, 1.0, 35.0).unwrap(),
            KeyEdit::Inserted
        );
    }

    #[test]
    fn an_actor_gets_a_placement_then_keys() {
        let mut s = seq();
        let g = guid();
        assert_eq!(
            set_actor(&mut s, g, 1.0, Some([2.0, 0.0, 0.0]), None).unwrap(),
            KeyEdit::Inserted
        );
        let shot = Shot {
            resource: 0,
            id: 0,
            seq: s.clone(),
            start: 0.0,
            stream: None,
            stream_name: String::new(),
            actors: Vec::new(),
            clips: Vec::new(),
        };
        let st = super::super::actor_state(&shot, &g, 1.0).unwrap();
        assert_eq!(st.position, [2.0, 0.0, 0.0]);
        // The same time: replaced; another time: inserted with the rotation kept.
        assert_eq!(
            set_actor(&mut s, g, 1.0, Some([3.0, 0.0, 0.0]), None).unwrap(),
            KeyEdit::Replaced
        );
        assert_eq!(
            set_actor(&mut s, g, 1.8, Some([4.0, 0.0, 0.0]), None).unwrap(),
            KeyEdit::Inserted
        );
        let q = crate::resource::trinity::euler_quaternion(0.0, 90.0, 0.0);
        set_actor(&mut s, g, 1.8, None, Some(q)).unwrap();
        assert_eq!(key_times(&s.blocks[0], Target::Actor(g)), vec![1.0, 1.8]);
        assert_eq!(delete_key(&mut s, Target::Actor(g), 1.8).unwrap(), 2);
        assert!(
            delete_key(&mut s, Target::Actor(g), 1.0).is_err(),
            "last key"
        );
    }

    #[test]
    fn camera_keys_are_listed_found_and_deleted() {
        let mut s = seq();
        assert_eq!(key_times(&s.blocks[0], Target::Camera), vec![0.5, 1.5]);
        assert_eq!(key_at(&s.blocks[0], Target::Camera, 1.505), Some(1.5));
        assert_eq!(key_at(&s.blocks[0], Target::Camera, 1.2), None);
        set_camera(&mut s, 1.0, [5.0, 0.0, 0.0, 0.0, 175.0, 0.0]).unwrap();
        assert_eq!(key_times(&s.blocks[0], Target::Camera), vec![0.5, 1.0, 1.5]);
        assert_eq!(delete_key(&mut s, Target::Camera, 1.0).unwrap(), 6);
        assert!(delete_key(&mut s, Target::Camera, 1.2).is_err());
    }
}

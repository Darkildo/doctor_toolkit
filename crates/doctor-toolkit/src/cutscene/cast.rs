//! Actors of a cut-scene as characters and animation clips: the parts
//! database set a `PartedAnimated` names, the clip of its bank, and the pose
//! of the skeleton at some moment.

use std::collections::HashMap;

use crate::character::{Character, MeshLocator, parse_selection};
use crate::game::{GameInstance, ResourceIndex};
use crate::hash::sdbm;
use crate::resource::anim::{AnimBank, BoneTrack, FRAME_RATE, bone_set, find_skeleton};
use crate::resource::partsdb::PartsDb;
use crate::resource::rcb::{Mat4, Rig, Skeleton, multiply};
use crate::{Error, Result};

use super::{ActorInfo, Shot, Source};

const TYPE_PARTSDB: u32 = sdbm(b"partsdb");

/// A decoded clip with the skeleton it animates.
#[derive(Debug, Clone)]
pub struct Animation {
    pub name: String,
    pub skeleton: Skeleton,
    pub tracks: Vec<BoneTrack>,
    pub frames: usize,
}

impl Animation {
    /// Bone → model transforms (column-major) at `seconds` into the clip
    /// (held at the last frame); bones without a track keep the bind pose.
    pub fn pose(&self, seconds: f32) -> Vec<Mat4> {
        let frame = (seconds.max(0.0) * FRAME_RATE).min(self.frames.saturating_sub(1) as f32);
        let (f0, s) = (frame.floor() as usize, frame.fract());
        let mut locals = self.skeleton.bind_locals();
        for track in &self.tracks {
            let Some(local) = locals.get_mut(track.bone) else {
                continue;
            };
            let n = track.rotations.len();
            if n == 0 {
                continue;
            }
            let a = f0.min(n - 1);
            let b = (f0 + 1).min(n - 1);
            let q = nlerp(track.rotations[a], track.rotations[b], s);
            let ta = track.translations[a.min(track.translations.len() - 1)];
            let tb = track.translations[b.min(track.translations.len() - 1)];
            let t: [f32; 3] = std::array::from_fn(|k| ta[k] + (tb[k] - ta[k]) * s);
            *local = trs(t, q);
        }
        let mut globals: Vec<Mat4> = Vec::with_capacity(locals.len());
        for (i, bone) in self.skeleton.bones.iter().enumerate() {
            let g = match bone.parent {
                Some(p) if p < i => multiply(&globals[p], &locals[i]),
                _ => locals[i],
            };
            globals.push(g);
        }
        globals
    }

    /// Skinning matrices (pose × inverse bind) at `seconds`.
    pub fn skin(&self, seconds: f32) -> Vec<Mat4> {
        self.pose(seconds)
            .iter()
            .zip(&self.skeleton.bones)
            .map(|(g, b)| multiply(g, &b.inverse_bind))
            .collect()
    }

    /// Length in seconds.
    pub fn duration(&self) -> f32 {
        self.frames as f32 / FRAME_RATE
    }
}

fn nlerp(a: [f32; 4], b: [f32; 4], s: f32) -> [f32; 4] {
    let dot: f32 = (0..4).map(|k| a[k] * b[k]).sum();
    let sign = if dot < 0.0 { -1.0 } else { 1.0 };
    let q: [f32; 4] = std::array::from_fn(|k| a[k] * (1.0 - s) + b[k] * sign * s);
    let len = q.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-12);
    q.map(|v| v / len)
}

/// Column-major matrix from a translation and a quaternion `(x, y, z, w)`.
pub fn trs(t: [f32; 3], q: [f32; 4]) -> Mat4 {
    let [x, y, z, w] = q;
    [
        1.0 - 2.0 * (y * y + z * z),
        2.0 * (x * y + w * z),
        2.0 * (x * z - w * y),
        0.0,
        2.0 * (x * y - w * z),
        1.0 - 2.0 * (x * x + z * z),
        2.0 * (y * z + w * x),
        0.0,
        2.0 * (x * z + w * y),
        2.0 * (y * z - w * x),
        1.0 - 2.0 * (x * x + y * y),
        0.0,
        t[0],
        t[1],
        t[2],
        1.0,
    ]
}

/// Finds the characters and clips of actors; caches parsed databases and banks.
pub struct Cast {
    meshes: MeshLocator,
    /// Parts databases by lower-case name: `(stream, resource)`.
    databases: HashMap<String, (usize, usize)>,
    parsed: HashMap<(usize, usize), PartsDb>,
    rigs: Vec<Rig>,
    banks: HashMap<(usize, usize), (Vec<u8>, AnimBank)>,
}

impl Cast {
    /// `rigs`: every rig of the game (`GameAssets::rigs`).
    pub fn new(game: &GameInstance, index: &ResourceIndex, rigs: Vec<Rig>) -> Self {
        let mut databases = HashMap::new();
        let mut opened: HashMap<usize, crate::game::StreamFile> = HashMap::new();
        for e in &index.entries {
            if e.header.type_id != TYPE_PARTSDB {
                continue;
            }
            let stream = e.stream as usize;
            let file = match opened.entry(stream) {
                std::collections::hash_map::Entry::Occupied(o) => o.into_mut(),
                std::collections::hash_map::Entry::Vacant(v) => match game.open_stream(stream) {
                    Ok(f) => v.insert(f),
                    Err(_) => continue,
                },
            };
            // The name is the first string of the database; parse only that.
            if let Ok(data) = file.resource_data(e.resource as usize)
                && let Ok(db) = PartsDb::parse(&data)
            {
                databases
                    .entry(db.name.to_ascii_lowercase())
                    .or_insert((stream, e.resource as usize));
            }
        }
        Self {
            meshes: MeshLocator::new(index),
            databases,
            parsed: HashMap::new(),
            rigs,
            banks: HashMap::new(),
        }
    }

    /// `(stream, resource)` of the parts database `name`, parsed.
    fn database(&mut self, game: &GameInstance, name: &str) -> Result<(usize, usize)> {
        let location = *self
            .databases
            .get(&name.to_ascii_lowercase())
            .ok_or_else(|| Error::InvalidArgument(format!("no parts database `{name}`")))?;
        if let std::collections::hash_map::Entry::Vacant(e) = self.parsed.entry(location) {
            let data = game.open_stream(location.0)?.resource_data(location.1)?;
            e.insert(PartsDb::parse(&data)?);
        }
        Ok(location)
    }

    /// The character an actor shows: the set (and preset item) of its parts
    /// database. `None` for actors without a database (props).
    pub fn character(
        &mut self,
        game: &GameInstance,
        actor: &ActorInfo,
    ) -> Option<Result<Character>> {
        let assembly = actor.assembly.clone()?;
        let location = match self.database(game, &assembly) {
            Ok(l) => l,
            Err(e) => return Some(Err(e)),
        };
        let db = &self.parsed[&location];
        let set = actor
            .set
            .as_deref()
            .and_then(|s| db.set_index(s))
            .or_else(|| db.set_index("_all"))
            .unwrap_or(0);
        let choices: Vec<String> = actor
            .preset
            .iter()
            .map(|(p, i)| format!("{p}={i}"))
            .collect();
        let selection =
            match parse_selection(db, set, &choices).or_else(|_| parse_selection(db, set, &[])) {
                Ok(s) => s,
                Err(e) => return Some(Err(e)),
            };
        Some(Character::load(
            game,
            &self.meshes,
            location.0,
            db,
            set,
            &selection,
            0,
        ))
    }

    /// The clip `name` of the shot's banks, decoded.
    pub fn animation(&mut self, source: &Source<'_>, shot: &Shot, name: &str) -> Result<Animation> {
        let info = shot
            .clip(name)
            .ok_or_else(|| {
                Error::InvalidArgument(format!("no clip `{name}` in {}", shot.stream_name))
            })?
            .clone();
        let stream = shot
            .stream
            .ok_or_else(|| Error::InvalidArgument(format!("no stream {}", shot.stream_name)))?;
        let key = (stream, info.bank);
        if let std::collections::hash_map::Entry::Vacant(e) = self.banks.entry(key) {
            let file = source.game.open_stream(stream)?;
            let data = source.resource(stream, &file, info.bank)?;
            let bank = AnimBank::parse(&data)?;
            e.insert((data, bank));
        }
        let (data, bank) = &self.banks[&key];
        let clip = &bank.clips[info.clip];
        let skeleton = find_skeleton(&self.rigs, bank.skeleton)
            .ok_or_else(|| {
                Error::InvalidArgument(format!("no rig has skeleton {:08x}", bank.skeleton))
            })?
            .clone();
        let bones = bone_set(&self.rigs, clip.bone_set).ok_or_else(|| {
            Error::InvalidArgument(format!("bone set {:08x} not found", clip.bone_set))
        })?;
        let tracks = clip.decode(data, bones)?;
        let frames = tracks.iter().map(|t| t.rotations.len()).max().unwrap_or(0);
        Ok(Animation {
            name: clip.display_name(),
            skeleton,
            tracks,
            frames,
        })
    }

    pub fn rigs(&self) -> &[Rig] {
        &self.rigs
    }
}

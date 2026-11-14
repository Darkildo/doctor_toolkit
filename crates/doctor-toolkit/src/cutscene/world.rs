//! Where a cut-scene plays: the placement of its keys in the world, the
//! points its camera and actors pass, the zones of the level around them and
//! the time of day it sets (`docs/formats/cutscenes.md` §9).
//!
//! The keys of a shot (actor positions `0x206`, camera curves `0x304`) are in
//! the space of the shot's anchor: the `TrinityGameSequence` object's matrix
//! combined with the transform of the shot's `seqb` block. Neither is set in
//! the game (no sequence object has `CMD_LoadMatrix`, every block transform
//! is the identity), so the keys are world coordinates; [`anchor`] still
//! applies both, as the sequencer does when the object asks for it.

use std::collections::HashMap;
use std::sync::Arc;

use rayon::prelude::*;
use serde::Serialize;

use super::{Cutscene, actor_state, camera_state};
use crate::Result;
use crate::game::{GameInstance, ResourceIndex};
use crate::hash::sdbm;
use crate::level::{
    Catalog, IDENTITY, Level, LevelSource, Lookup, Overrides, RowMatrix, Scope, compose,
};
use crate::resource::simgroup::{SimGroup, Value};
use crate::resource::trinity::{Command, SeqBlock};

/// Class hash of `SetTimeOfDay` SimGroup objects.
pub const CLASS_SET_TIME_OF_DAY: u32 = 0x6185_7316;
const TYPE_SIMGROUP: u32 = sdbm(b"simgroup");

/// The transform of a `seqb` block as a row-major matrix (row vectors).
///
/// The sequencer (0x00599340) builds it from floats 4..8 of the block's
/// transform (a quaternion x, y, z, w) and floats 0..3 with w = 1 (a
/// position); float 3 and 8 are not read. In the game the floats are
/// (−0, −0, −0, 1, 0, 0, 0, 1, 0): the identity.
pub fn block_matrix(block: &SeqBlock) -> RowMatrix {
    let t = &block.transform;
    let [x, y, z, w] = [t[4], t[5], t[6], t[7]];
    let n = (x * x + y * y + z * z + w * w).sqrt();
    let [x, y, z, w] = if n > 1e-6 {
        [x / n, y / n, z / n, w / n]
    } else {
        [0.0, 0.0, 0.0, 1.0]
    };
    [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y + w * z),
            2.0 * (x * z - w * y),
            0.0,
        ],
        [
            2.0 * (x * y - w * z),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z + w * x),
            0.0,
        ],
        [
            2.0 * (x * z + w * y),
            2.0 * (y * z - w * x),
            1.0 - 2.0 * (x * x + y * y),
            0.0,
        ],
        [t[0], t[1], t[2], 1.0],
    ]
}

/// The space of a shot's keys: the block transform placed by the sequence
/// object's matrix (`p · block · object`).
pub fn anchor(block: &SeqBlock, object: Option<&RowMatrix>) -> RowMatrix {
    compose(&block_matrix(block), object.unwrap_or(&IDENTITY))
}

/// Whether a matrix is the identity (within rounding).
pub fn is_identity(m: &RowMatrix) -> bool {
    m.iter()
        .flatten()
        .zip(IDENTITY.iter().flatten())
        .all(|(a, b)| (a - b).abs() < 1e-5)
}

/// A point in the anchor's space moved into the world.
pub fn to_world(anchor: &RowMatrix, p: [f32; 3]) -> [f32; 3] {
    let mut out = [0.0; 3];
    for (j, o) in out.iter_mut().enumerate() {
        *o = p[0] * anchor[0][j] + p[1] * anchor[1][j] + p[2] * anchor[2][j] + anchor[3][j];
    }
    out
}

/// A column-major matrix (camera or actor to anchor space) moved into the
/// world: `anchor · m` for column vectors.
pub fn matrix_to_world(anchor: &RowMatrix, m: &[f32; 16]) -> [f32; 16] {
    if is_identity(anchor) {
        return *m;
    }
    // A row-major `p · A` with row vectors is the column-major matrix with
    // the same 16 floats.
    let a: [f32; 16] = *anchor
        .as_flattened()
        .first_chunk::<16>()
        .expect("16 floats");
    let mut out = [0.0; 16];
    for c in 0..4 {
        for r in 0..4 {
            out[c * 4 + r] = (0..4).map(|k| a[k * 4 + r] * m[c * 4 + k]).sum();
        }
    }
    out
}

impl Cutscene {
    /// The anchor of shot `s` (see [`anchor`]).
    pub fn anchor(&self, s: usize) -> RowMatrix {
        anchor(
            self.shots[s].block(),
            self.sequence.as_ref().and_then(|q| q.matrix.as_ref()),
        )
    }
}

/// What passes a point of [`path_points`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum PointKind {
    Camera,
    Actor,
}

/// A world point the camera or an actor passes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct PathPoint {
    pub position: [f32; 3],
    pub kind: PointKind,
    /// Seconds from the start of the cut-scene.
    pub time: f32,
}

/// Actors farther than this from the shot's camera are left out of
/// [`path_points`].
pub const ACTOR_REACH: f32 = 150.0;

/// The camera and the actors of every shot, sampled every `step` seconds
/// (and at the end of each shot), in world coordinates; actors farther than
/// [`ACTOR_REACH`] from the camera are left out.
pub fn path_points(cutscene: &Cutscene, step: f32) -> Vec<PathPoint> {
    let step = step.max(0.01);
    let mut out = Vec::new();
    for (s, shot) in cutscene.shots.iter().enumerate() {
        let anchor = cutscene.anchor(s);
        let duration = shot.duration();
        let samples = (duration / step).ceil().max(1.0) as usize;
        for i in 0..=samples {
            let local = (i as f32 * step).min(duration);
            let time = shot.start + local;
            let camera = camera_state(shot, local)
                .map(|c| to_world(&anchor, [c.values[0], c.values[1], c.values[2]]));
            if let Some(position) = camera {
                out.push(PathPoint {
                    position,
                    kind: PointKind::Camera,
                    time,
                });
            }
            for actor in &shot.actors {
                let Some(a) = actor_state(shot, &actor.guid, local) else {
                    continue;
                };
                let position = to_world(&anchor, a.position);
                // Actors parked out of the picture (igc02 keeps them 100 m
                // above the origin) do not say where the scene is.
                let far = camera.is_some_and(|c| {
                    let d2: f32 = (0..3).map(|i| (c[i] - position[i]).powi(2)).sum();
                    d2 > ACTOR_REACH * ACTOR_REACH
                });
                if !far {
                    out.push(PathPoint {
                        position,
                        kind: PointKind::Actor,
                        time,
                    });
                }
            }
        }
    }
    out
}

/// What a zone is, by its path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub enum ZoneKind {
    /// `…/high/…`: full detail of a district.
    High,
    /// `…/medium/…`: the district's streets and distant buildings.
    Medium,
    /// `…/low/…`: far LOD of a district.
    Low,
    /// `…/interior/…` and `…/interiors_*/…`.
    Interior,
    /// `…/local/sky`.
    Sky,
    /// `…/local/ocean/…`.
    Ocean,
    /// A stream of the cut-scene's mission (`…/objectives/<mission>/…`).
    Mission,
    /// The cut-scene's own folder stream.
    Cutscene,
    Other,
}

impl ZoneKind {
    /// The kind of a zone named `name` (lower case, forward slashes).
    pub fn of(name: &str) -> Self {
        let parts: Vec<&str> = name.split('/').collect();
        let has = |p: &str| parts.contains(&p);
        if parts.get(2) == Some(&"global") {
            return Self::Mission;
        }
        if has("sky") {
            Self::Sky
        } else if has("ocean") {
            Self::Ocean
        } else if parts.iter().any(|p| p.starts_with("interior")) {
            Self::Interior
        } else if has("high") {
            Self::High
        } else if has("medium") {
            Self::Medium
        } else if has("low") {
            Self::Low
        } else {
            Self::Other
        }
    }

    /// How far (metres) a zone of this kind reaches past the box of its
    /// objects when choosing it for a point of `point` kind; `None`: the kind
    /// is not chosen by points.
    pub fn reach(self, point: PointKind) -> Option<f32> {
        match (self, point) {
            (Self::High | Self::Other, _) => Some(60.0),
            (Self::Medium, _) => Some(200.0),
            (Self::Mission, _) => Some(40.0),
            (Self::Interior, PointKind::Actor) => Some(2.0),
            (Self::Interior, PointKind::Camera) => Some(0.5),
            (Self::Low | Self::Sky | Self::Ocean | Self::Cutscene, _) => None,
        }
    }

    /// Whether the kind is loaded whatever the points (sky, ocean, the
    /// cut-scene's own objects).
    pub fn always(self) -> bool {
        matches!(self, Self::Sky | Self::Ocean | Self::Cutscene)
    }
}

/// The extent of a zone's placed objects.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ZoneBox {
    pub stream: usize,
    pub name: String,
    pub kind: ZoneKind,
    pub min: [f32; 3],
    pub max: [f32; 3],
    /// Objects that gave the box.
    pub objects: usize,
}

impl ZoneBox {
    pub fn is_empty(&self) -> bool {
        self.objects == 0
    }

    /// Distance from `p` to the box (0 inside).
    pub fn distance(&self, p: [f32; 3]) -> f32 {
        let d2: f32 = (0..3)
            .map(|i| {
                (self.min[i] - p[i])
                    .max(p[i] - self.max[i])
                    .max(0.0)
                    .powi(2)
            })
            .sum();
        d2.sqrt()
    }

    fn add(&mut self, p: [f32; 3]) {
        if !p.iter().all(|v| v.is_finite()) {
            return;
        }
        self.min = std::array::from_fn(|i| self.min[i].min(p[i]));
        self.max = std::array::from_fn(|i| self.max[i].max(p[i]));
        self.objects += 1;
    }
}

/// Class of the geometry of a zone: terrain, streets and rooms.
const CLASS_ZONE_RENDER_LOD: &str = "ZoneRenderLOD";

/// Whether an object is the geometry of its zone (terrain, rooms): its
/// meshes, not its position, say where the zone is.
fn is_room(object: &crate::level::LevelObject) -> bool {
    object.class_name == Some(CLASS_ZONE_RENDER_LOD)
}

/// The box of every zone of a level: positions of the objects placed with a
/// matrix, and for the zone's geometry (`ZoneRenderLOD`: terrain at the
/// origin with meshes in world coordinates, rooms of interiors) the corners
/// of its meshes' boxes, which `mesh_box` gives by mesh id.
pub fn zone_boxes(
    game: &GameInstance,
    level: &Level,
    mesh_box: &dyn Fn(u32) -> Option<([f32; 3], [f32; 3])>,
) -> Vec<ZoneBox> {
    let mut boxes: HashMap<usize, ZoneBox> = HashMap::new();
    for group in &level.groups {
        let stream = group.location.stream;
        let b = boxes.entry(stream).or_insert_with(|| {
            let name = game.streams()[stream].stream_name();
            ZoneBox {
                stream,
                kind: ZoneKind::of(&name),
                name,
                min: [f32::INFINITY; 3],
                max: [f32::NEG_INFINITY; 3],
                objects: 0,
            }
        });
        for object in &group.objects {
            let Some(m) = object.transform else { continue };
            if !is_room(object) {
                // Objects at the origin without a matrix are not placed.
                if object.has_matrix {
                    b.add([m[3][0], m[3][1], m[3][2]]);
                }
                continue;
            }
            let Some(model) = object.model.and_then(|id| level.models.get(&id)) else {
                continue;
            };
            for piece in &model.desc.pieces {
                let Some((lo, hi)) = mesh_box(piece.mesh) else {
                    continue;
                };
                let placed = compose(&piece.matrix, &m);
                for corner in 0..8 {
                    let c = [
                        if corner & 1 == 0 { lo[0] } else { hi[0] },
                        if corner & 2 == 0 { lo[1] } else { hi[1] },
                        if corner & 4 == 0 { lo[2] } else { hi[2] },
                    ];
                    b.add(to_world(&placed, c));
                }
            }
        }
    }
    let mut out: Vec<ZoneBox> = boxes.into_values().collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The boxes of every candidate zone of a cut-scene whose folder stream is
/// `folder` ([`is_candidate`]): the city's local zones, the mission's
/// streams and the folder stream. Reads their SimGroups and, for terrain,
/// the boxes of its meshes (about a second for a city, in parallel).
pub fn candidate_boxes(
    game: &Arc<GameInstance>,
    index: &ResourceIndex,
    folder: &str,
) -> Result<Vec<ZoneBox>> {
    let catalog = Catalog::new(game, index);
    let streams: Vec<usize> = catalog
        .zones
        .iter()
        .filter(|z| z.simgroups > 0 && is_candidate(&z.name, folder))
        .map(|z| z.stream)
        .collect();
    let parents = game.stream_parents()?;
    let scope = Scope::new(game, &parents, &streams);
    let source = Arc::new(LevelSource::new(
        game.clone(),
        Arc::new(Lookup::new(index)),
        Overrides::default(),
        scope,
    ));
    let level = Level::load(source.clone(), &streams)?;
    // Meshes of objects at the origin without a matrix (terrain).
    let mut wanted: Vec<u32> = level
        .groups
        .iter()
        .flat_map(|g| &g.objects)
        .filter(|o| o.transform.is_some() && is_room(o))
        .filter_map(|o| level.models.get(&o.model?))
        .flat_map(|m| m.desc.pieces.iter().map(|p| p.mesh))
        .collect();
    wanted.sort_unstable();
    wanted.dedup();
    let boxes: HashMap<u32, ([f32; 3], [f32; 3])> = wanted
        .par_iter()
        .filter_map(|&id| {
            let m = source.mesh(id).ok()?;
            let ok = m.aabb_min.iter().chain(&m.aabb_max).all(|v| v.is_finite())
                && (0..3).all(|i| m.aabb_min[i] <= m.aabb_max[i]);
            ok.then_some((id, (m.aabb_min, m.aabb_max)))
        })
        .collect();
    source.close_streams();
    let mut out = zone_boxes(game, &level, &|id| boxes.get(&id).copied());
    for b in &mut out {
        if b.name == folder {
            b.kind = ZoneKind::Cutscene;
        }
    }
    Ok(out)
}

/// Why a zone was chosen.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Choice {
    pub stream: usize,
    /// Distance from the nearest point to the zone's box (0 for zones
    /// loaded whatever the points).
    pub distance: f32,
}

/// The zones to show for the points of a cut-scene: zones loaded always
/// (sky, ocean, the cut-scene's stream) and zones whose box, grown by the
/// reach of their kind, holds a point; sorted by stream.
pub fn choose_zones(boxes: &[ZoneBox], points: &[PathPoint]) -> Vec<Choice> {
    let mut out = Vec::new();
    for b in boxes {
        if b.kind.always() {
            out.push(Choice {
                stream: b.stream,
                distance: 0.0,
            });
            continue;
        }
        if b.is_empty() {
            continue;
        }
        let best = points
            .iter()
            .filter_map(|p| {
                let reach = b.kind.reach(p.kind)?;
                let d = b.distance(p.position);
                (d <= reach).then_some(d)
            })
            .fold(None, |best: Option<f32>, d| {
                Some(best.map_or(d, |b| b.min(d)))
            });
        if let Some(distance) = best {
            out.push(Choice {
                stream: b.stream,
                distance,
            });
        }
    }
    // Variants of one interior (`venue_055`, `venue_055b`, `venue_055c`)
    // fill the same rooms in different states: keep one, the base.
    let name = |s: usize| {
        boxes
            .iter()
            .find(|b| b.stream == s)
            .map(|b| b.name.as_str())
    };
    let interiors: Vec<(usize, &str)> = out
        .iter()
        .filter_map(|c| {
            let b = boxes.iter().find(|b| b.stream == c.stream)?;
            (b.kind == ZoneKind::Interior).then_some((c.stream, b.name.as_str()))
        })
        .collect();
    out.retain(|c| {
        let Some(n) = name(c.stream) else { return true };
        let base = variant_base(n);
        // A shorter name of the same base chosen too: this one is a variant.
        !interiors.iter().any(|&(s, other)| {
            s != c.stream && variant_base(other) == base && other.len() < n.len()
        }) || !interiors.iter().any(|&(s, _)| s == c.stream)
    });
    out.sort_by_key(|c| c.stream);
    out
}

/// The zone name without a variant letter (`…/venue_055b` and
/// `…/venue_131_b` → `…/venue_055`, `…/venue_131`).
pub fn variant_base(name: &str) -> &str {
    let bytes = name.as_bytes();
    let n = bytes.len();
    if n >= 3 && bytes[n - 1].is_ascii_lowercase() {
        if bytes[n - 2].is_ascii_digit() {
            return &name[..n - 1];
        }
        if bytes[n - 2] == b'_' && bytes[n - 3].is_ascii_digit() {
            return &name[..n - 2];
        }
    }
    name
}

/// The city of a stream (`gf2/cuba/…` → `cuba`).
pub fn city_of(stream_name: &str) -> Option<&str> {
    let mut parts = stream_name.split('/');
    (parts.next()? == "gf2").then_some(())?;
    let city = parts.next()?;
    matches!(city, "cuba" | "miami" | "queens").then_some(city)
}

/// The mission directory of a cut-scene's folder stream: the path up to
/// the component after `objectives` or `checkpoints`
/// (`gf2/cuba/global/objectives/chapter60-mission08-killingcastro`).
pub fn mission_dir(stream_name: &str) -> Option<&str> {
    let mut at = 0;
    let mut parts = stream_name.split('/').peekable();
    while let Some(p) = parts.next() {
        at += p.len() + 1;
        if p == "objectives" || p == "checkpoints" {
            let next = parts.next()?;
            return Some(&stream_name[..at + next.len()]);
        }
    }
    None
}

/// Whether `name` is a candidate zone of a cut-scene whose folder stream is
/// `folder`: the city's local zones, the zones of the mission, the folder
/// stream itself; not the shot streams (the preview draws the actors).
pub fn is_candidate(name: &str, folder: &str) -> bool {
    if name == folder {
        return true;
    }
    if name.starts_with(folder) && name[folder.len()..].starts_with('/') {
        return false;
    }
    if let Some(city) = city_of(folder)
        && name.starts_with(&format!("gf2/{city}/local/"))
    {
        return true;
    }
    mission_dir(folder).is_some_and(|m| name.starts_with(m) && name[m.len()..].starts_with('/'))
}

/// A time of day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct TimeOfDay {
    pub hour: u8,
    pub minute: u8,
}

impl TimeOfDay {
    pub fn new(hour: u8, minute: u8) -> Self {
        Self {
            hour: hour % 24,
            minute: minute.min(59),
        }
    }

    /// Hours from midnight with the minutes as a fraction.
    pub fn hours(self) -> f32 {
        self.hour as f32 + self.minute as f32 / 60.0
    }

    /// `HH:MM`.
    pub fn parse(text: &str) -> Option<Self> {
        let (h, m) = text.trim().split_once(':')?;
        let (h, m): (u8, u8) = (h.parse().ok()?, m.get(..2).unwrap_or(m).parse().ok()?);
        (h < 24 && m < 60).then(|| Self::new(h, m))
    }
}

impl std::fmt::Display for TimeOfDay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:02}:{:02}", self.hour, self.minute)
    }
}

/// A time command of a shot (text command of kind 0, `docs/formats/cutscenes.md` §2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeCommand {
    /// `changeTime HH:MM`: the game clock.
    Change(TimeOfDay),
    /// `changeApparentTime HH:MM`: sky and light only.
    Apparent(TimeOfDay),
    /// `queueChangeTime HH:MM`: the clock after the cut-scene.
    Queue(TimeOfDay),
    /// `restoreTimeState`: back to the clock before the cut-scene.
    Restore,
}

impl TimeCommand {
    pub fn parse(text: &str) -> Option<Self> {
        let mut words = text.split_whitespace();
        let verb = words.next()?;
        let mut time = || words.next().and_then(TimeOfDay::parse);
        Some(match verb {
            "changeTime" => Self::Change(time()?),
            "changeApparentTime" => Self::Apparent(time()?),
            "queueChangeTime" => Self::Queue(time()?),
            "restoreTimeState" => Self::Restore,
            _ => return None,
        })
    }
}

/// Shadow ranges a shot sets (`setShadowRanges a b c d`, kind 2).
pub fn shadow_ranges(text: &str) -> Option<[f32; 4]> {
    let mut words = text.split_whitespace();
    (words.next()? == "setShadowRanges").then_some(())?;
    let v: Vec<f32> = words.filter_map(|w| w.parse().ok()).collect();
    (v.len() == 4).then(|| [v[0], v[1], v[2], v[3]])
}

/// Where the time of day of a moment comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeSource {
    /// A time command of the cut-scene: shot and command index.
    Command { shot: usize, command: usize },
    /// The time before the cut-scene (mission or game clock).
    Before,
}

/// The time the cut-scene sets at `t` seconds: the latest `changeTime` or
/// `changeApparentTime` before `t`, `None` before any or after
/// `restoreTimeState`.
pub fn time_at(cutscene: &Cutscene, t: f32) -> (Option<TimeOfDay>, TimeSource) {
    let mut out = (None, TimeSource::Before);
    for (s, shot) in cutscene.shots.iter().enumerate() {
        if shot.start > t {
            break;
        }
        let block = shot.block();
        for (c, (command, at)) in block.commands.iter().zip(block.times()).enumerate() {
            if shot.start + at > t {
                break;
            }
            let Command::Script { kind: 0, text, .. } = command else {
                continue;
            };
            match TimeCommand::parse(&text.text) {
                Some(TimeCommand::Change(time) | TimeCommand::Apparent(time)) => {
                    out = (
                        Some(time),
                        TimeSource::Command {
                            shot: s,
                            command: c,
                        },
                    );
                }
                Some(TimeCommand::Restore) => out = (None, TimeSource::Before),
                _ => {}
            }
        }
    }
    out
}

/// The hour a `SetTimeOfDay` object sets: attribute 3 (hour) and 4 (minute,
/// 255 keeps it); `None` without an hour.
pub fn set_time_of_day(group: &SimGroup, object: usize) -> Option<TimeOfDay> {
    let o = &group.objects[object];
    if o.class != CLASS_SET_TIME_OF_DAY {
        return None;
    }
    let int = |i| match group.attr(o, CLASS_SET_TIME_OF_DAY, i) {
        Some(Value::Int(v)) => Some(*v),
        _ => None,
    };
    let hour = int(3).filter(|h| (0..24).contains(h))?;
    let minute = int(4).filter(|m| (0..60).contains(m)).unwrap_or(0);
    Some(TimeOfDay::new(hour as u8, minute as u8))
}

/// The time a mission sets before its cut-scene: the first `SetTimeOfDay`
/// with an hour in the streams of the mission directory of `folder`
/// (`docs/formats/cutscenes.md` §9.3), skipping objects whose message
/// resets or ends something.
pub fn mission_time(game: &GameInstance, folder: &str) -> Result<Option<TimeOfDay>> {
    let Some(dir) = mission_dir(folder) else {
        return Ok(None);
    };
    let prefix = format!("{dir}/");
    let mut found: Vec<(bool, String, TimeOfDay)> = Vec::new();
    for (stream, file) in game.streams().iter().enumerate() {
        let name = file.stream_name();
        if !name.starts_with(&prefix) {
            continue;
        }
        let opened = game.open_stream(stream)?;
        for (index, r) in opened.layout().resources.iter().enumerate() {
            if r.header.type_id != TYPE_SIMGROUP {
                continue;
            }
            let data = opened.resource_data(index)?;
            let group = SimGroup::parse(&data)?;
            for o in 0..group.objects.len() {
                let Some(time) = set_time_of_day(&group, o) else {
                    continue;
                };
                let message = match group.attr(&group.objects[o], CLASS_SET_TIME_OF_DAY, 0) {
                    Some(Value::String(s)) => s.to_lowercase(),
                    _ => String::new(),
                };
                let late = message.contains("reset") || message.starts_with("end");
                found.push((late, name.clone(), time));
            }
        }
    }
    found.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    Ok(found.first().map(|f| f.2))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::trinity::FixedText;

    fn block(transform: [f32; 9]) -> SeqBlock {
        SeqBlock {
            version: 4,
            hash: 0,
            transform,
            name: FixedText::new("trSeq_test"),
            duration: 1.0,
            commands: Vec::new(),
        }
    }

    const GAME: [f32; 9] = [-0.0, -0.0, -0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];

    #[test]
    fn the_game_anchor_is_the_identity() {
        let b = block(GAME);
        assert!(is_identity(&anchor(&b, None)));
        assert!(is_identity(&anchor(&b, Some(&IDENTITY))));
        assert_eq!(
            to_world(&anchor(&b, None), [1.0, 2.0, 3.0]),
            [1.0, 2.0, 3.0]
        );
    }

    #[test]
    fn an_anchor_moves_and_turns_the_keys() {
        // Block: 10 m along X; object: turned 90° about Y, then 100 m up.
        let b = block([10.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        let mut object = [
            [0.0, 0.0, -1.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 100.0, 0.0, 1.0],
        ];
        let a = anchor(&b, Some(&object));
        let p = to_world(&a, [0.0, 0.0, 0.0]);
        // (10, 0, 0) turned: X goes to −Z.
        assert!((p[0]).abs() < 1e-5 && (p[1] - 100.0).abs() < 1e-5 && (p[2] + 10.0).abs() < 1e-5);
        // A quaternion in the block turns too: 180° about Y.
        let b = block([0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0]);
        object = IDENTITY;
        let p = to_world(&anchor(&b, Some(&object)), [1.0, 0.0, 2.0]);
        assert!(
            (p[0] + 1.0).abs() < 1e-5 && (p[2] + 2.0).abs() < 1e-5,
            "{p:?}"
        );
        // A column-major camera matrix follows the anchor.
        let cam = crate::resource::trinity::camera_matrix([1.0, 2.0, 3.0, 0.0, 0.0, 0.0]);
        let moved = matrix_to_world(&a, &cam);
        let at = [moved[12], moved[13], moved[14]];
        assert_eq!(
            at.map(|v| (v * 1e4).round() / 1e4),
            to_world(&a, [1.0, 2.0, 3.0]).map(|v| (v * 1e4).round() / 1e4)
        );
    }

    #[test]
    fn zone_kinds_follow_the_path() {
        assert_eq!(
            ZoneKind::of("gf2/cuba/local/virgil/high/terrain3"),
            ZoneKind::High
        );
        assert_eq!(
            ZoneKind::of("gf2/cuba/local/vern/medium/zone3"),
            ZoneKind::Medium
        );
        assert_eq!(
            ZoneKind::of("gf2/cuba/local/virgil/low/zone1"),
            ZoneKind::Low
        );
        assert_eq!(
            ZoneKind::of("gf2/cuba/local/virgil/interior/venue_055"),
            ZoneKind::Interior
        );
        assert_eq!(
            ZoneKind::of("gf2/miami/local/andie/interiors_andie/manganocompound"),
            ZoneKind::Interior
        );
        assert_eq!(ZoneKind::of("gf2/cuba/local/sky/sky"), ZoneKind::Sky);
        assert_eq!(
            ZoneKind::of("gf2/cuba/local/ocean/high/terrain1"),
            ZoneKind::Ocean
        );
        assert_eq!(
            ZoneKind::of("gf2/cuba/global/objectives/x/cp_735_zonestream"),
            ZoneKind::Mission
        );
    }

    #[test]
    fn missions_cities_and_candidates() {
        let folder =
            "gf2/cuba/global/objectives/chapter60-mission08-killingcastro/cp_734/_734_igc_content";
        assert_eq!(city_of(folder), Some("cuba"));
        assert_eq!(city_of("mp_mall/mp_mall"), None);
        assert_eq!(
            mission_dir(folder),
            Some("gf2/cuba/global/objectives/chapter60-mission08-killingcastro")
        );
        assert_eq!(
            mission_dir("gf2/queens/global/checkpoints/c04-r01-rendezvous01/igcs/cp_40"),
            Some("gf2/queens/global/checkpoints/c04-r01-rendezvous01")
        );
        assert_eq!(
            mission_dir("gf2/cuba/local/yaeger/high/compoundstream"),
            None
        );
        assert!(is_candidate(folder, folder));
        assert!(!is_candidate(&format!("{folder}/igc16_s01"), folder));
        assert!(is_candidate("gf2/cuba/local/virgil/high/terrain3", folder));
        assert!(!is_candidate(
            "gf2/miami/local/andie/high/terrain10",
            folder
        ));
        assert!(is_candidate(
            "gf2/cuba/global/objectives/chapter60-mission08-killingcastro/cp_735/cp_735_zonestream",
            folder
        ));
        assert!(!is_candidate(
            "gf2/cuba/global/objectives/chapter61-mission09-almeidaspursuit/cp_762/x",
            folder
        ));
    }

    fn zone(stream: usize, kind: ZoneKind, min: [f32; 3], max: [f32; 3]) -> ZoneBox {
        ZoneBox {
            stream,
            name: format!("z{stream}"),
            kind,
            min,
            max,
            objects: 1,
        }
    }

    fn point(x: f32, z: f32, kind: PointKind) -> PathPoint {
        PathPoint {
            position: [x, 0.0, z],
            kind,
            time: 0.0,
        }
    }

    #[test]
    fn zones_are_chosen_along_the_path() {
        let boxes = vec![
            // A street block next to the path.
            zone(0, ZoneKind::High, [0.0, 0.0, 0.0], [50.0, 10.0, 50.0]),
            // A block 100 m away: out of reach of high zones.
            zone(1, ZoneKind::High, [150.0, 0.0, 0.0], [200.0, 10.0, 50.0]),
            // The district's streets: medium zones reach farther.
            zone(2, ZoneKind::Medium, [150.0, 0.0, 0.0], [400.0, 10.0, 400.0]),
            // An interior the actors stand in, and one the camera passes near.
            zone(
                3,
                ZoneKind::Interior,
                [-20.0, -1.0, -20.0],
                [-10.0, 5.0, -10.0],
            ),
            zone(4, ZoneKind::Interior, [60.0, -1.0, 60.0], [70.0, 5.0, 70.0]),
            // Far LOD: never by points; the sky: always.
            zone(5, ZoneKind::Low, [-1e4, -1e4, -1e4], [1e4, 1e4, 1e4]),
            zone(6, ZoneKind::Sky, [0.0; 3], [0.0; 3]),
        ];
        let points = vec![
            point(-15.0, -15.0, PointKind::Actor),
            point(55.0, 55.0, PointKind::Camera),
        ];
        let chosen: Vec<usize> = choose_zones(&boxes, &points)
            .iter()
            .map(|c| c.stream)
            .collect();
        assert_eq!(chosen, vec![0, 2, 3, 6]);
        // Without points only the zones loaded always.
        let chosen: Vec<usize> = choose_zones(&boxes, &[]).iter().map(|c| c.stream).collect();
        assert_eq!(chosen, vec![6]);
        // Empty boxes are never chosen by points.
        let mut empty = zone(7, ZoneKind::High, [0.0; 3], [0.0; 3]);
        empty.objects = 0;
        assert!(choose_zones(&[empty], &points).is_empty());
    }

    #[test]
    fn one_variant_of_an_interior_is_chosen() {
        assert_eq!(
            variant_base("a/interior/venue_055b"),
            "a/interior/venue_055"
        );
        assert_eq!(
            variant_base("a/interior/venue_131_b"),
            "a/interior/venue_131"
        );
        assert_eq!(variant_base("a/interior/venue_055"), "a/interior/venue_055");
        assert_eq!(
            variant_base("a/interior/business_16"),
            "a/interior/business_16"
        );
        let room = |stream, name: &str| ZoneBox {
            name: name.into(),
            ..zone(stream, ZoneKind::Interior, [0.0; 3], [10.0; 3])
        };
        let boxes = vec![
            room(0, "c/interior/venue_055"),
            room(1, "c/interior/venue_055b"),
            room(2, "c/interior/venue_055c"),
            room(3, "c/interior/venue_054b"),
        ];
        let points = [point(5.0, 5.0, PointKind::Actor)];
        let chosen: Vec<usize> = choose_zones(&boxes, &points)
            .iter()
            .map(|c| c.stream)
            .collect();
        assert_eq!(chosen, vec![0, 3]);
    }

    #[test]
    fn time_commands_parse() {
        assert_eq!(
            TimeCommand::parse("changeApparentTime 17:00"),
            Some(TimeCommand::Apparent(TimeOfDay::new(17, 0)))
        );
        assert_eq!(
            TimeCommand::parse("changeTime 23:30"),
            Some(TimeCommand::Change(TimeOfDay::new(23, 30)))
        );
        assert_eq!(
            TimeCommand::parse("queueChangeTime 23:00"),
            Some(TimeCommand::Queue(TimeOfDay::new(23, 0)))
        );
        assert_eq!(
            TimeCommand::parse("restoreTimeState"),
            Some(TimeCommand::Restore)
        );
        assert_eq!(TimeCommand::parse("changeTime 25:00"), None);
        assert_eq!(TimeCommand::parse("music_igc_16"), None);
        assert_eq!(
            shadow_ranges("setShadowRanges 2.0 28.0 75.0 0.0"),
            Some([2.0, 28.0, 75.0, 0.0])
        );
        assert_eq!(shadow_ranges("restoreShadowRanges"), None);
        assert_eq!(TimeOfDay::new(17, 30).hours(), 17.5);
        assert_eq!(TimeOfDay::new(7, 5).to_string(), "07:05");
    }
}

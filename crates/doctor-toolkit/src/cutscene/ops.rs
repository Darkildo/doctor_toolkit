//! Operations on cut-scenes the surfaces share (stage 104): the command
//! line, the MCP server and the editor name a shot the same way, get the
//! same views of it and record the same edits in the workspace.
//!
//! A shot is named by its folder stream and its resource index or source
//! name (`igc16_s02`). Times are seconds from the start of the shot;
//! positions and angles are those of the shot's keys ([`super::world`]: in
//! every shot of the game they are world coordinates). Every edit is one
//! operation of the workspace.

use serde::{Deserialize, Serialize};

use super::edit::{self, KeyEdit, Target};
use super::{
    ActorInfo, AudioTrack, Cutscene, Shot, Source, actor_state, audio_track, camera_state,
    command_span, list_shots, sequences_in,
};
use crate::game::GameInstance;
use crate::resource::tob::Tob;
use crate::resource::trinity::{
    Command, Guid, SeqMaster, TYPE_TRINITY_SEQ_MASTER, euler_quaternion,
};
use crate::workspace::{FileKey, Workspace};
use crate::{Error, Result};

fn bad<T>(why: impl Into<String>) -> Result<T> {
    Err(Error::InvalidArgument(why.into()))
}

// ------------------------------------------------------------ addressing

/// The folder stream `stream` and, with `shot`, the resource index of that
/// shot: its index in the stream or its source name (`igc01_s03`).
pub fn locate(
    game: &GameInstance,
    stream: &str,
    shot: Option<&str>,
) -> Result<(usize, Option<usize>)> {
    let stream_name = stream.trim().trim_end_matches(".str");
    let stream = game
        .find_stream(stream_name)
        .ok_or_else(|| Error::InvalidArgument(format!("stream `{stream_name}` not found")))?;
    let Some(shot) = shot.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok((stream, None));
    };
    let file = game.open_stream(stream)?;
    let resources = &file.layout().resources;
    if let Ok(index) = shot.parse::<usize>() {
        return match resources.get(index) {
            Some(r) if r.header.type_id == TYPE_TRINITY_SEQ_MASTER => Ok((stream, Some(index))),
            _ => bad(format!(
                "resource {index} of {stream_name} is not a cut-scene shot"
            )),
        };
    }
    let mut names = Vec::new();
    for (index, r) in resources.iter().enumerate() {
        if r.header.type_id == TYPE_TRINITY_SEQ_MASTER {
            let seq = SeqMaster::parse(&file.resource_data(index)?)?;
            if seq.source_name().eq_ignore_ascii_case(shot) {
                return Ok((stream, Some(index)));
            }
            names.push(seq.source_name().to_owned());
        }
    }
    bad(format!(
        "no shot `{shot}` in {stream_name} (shots: {})",
        names.join(", ")
    ))
}

/// The one shot an edit works on.
fn one_shot(game: &GameInstance, stream: &str, shot: &str) -> Result<(usize, usize)> {
    match locate(game, stream, Some(shot))? {
        (stream, Some(shot)) => Ok((stream, shot)),
        _ => bad("give the shot: its resource index or its name"),
    }
}

// ------------------------------------------------------------ views

/// The shots of one folder stream and the sequences that play them.
#[derive(Debug, Clone, Serialize)]
pub struct StreamView {
    pub stream: String,
    pub seconds: f32,
    pub sequences: Vec<SequenceView>,
    pub shots: Vec<ShotLine>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SequenceView {
    pub guid: String,
    pub start_message: Option<String>,
    pub end_message: Option<String>,
    pub shots: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShotLine {
    /// Resource index in the folder stream.
    pub resource: usize,
    pub name: String,
    pub seconds: f32,
}

/// Every cut-scene of the game: its shots by folder stream.
pub fn overview(source: &Source<'_>) -> Result<Vec<StreamView>> {
    let shots = list_shots(source.game)?;
    let mut out: Vec<StreamView> = Vec::new();
    let mut last = None;
    for s in shots {
        if last != Some(s.stream) {
            last = Some(s.stream);
            out.push(StreamView {
                stream: s.stream_name.clone(),
                seconds: 0.0,
                sequences: sequences_in(source, s.stream)?
                    .into_iter()
                    .map(|q| SequenceView {
                        guid: q.guid,
                        start_message: q.start_message,
                        end_message: q.end_message,
                        shots: q.shots.len(),
                    })
                    .collect(),
                shots: Vec::new(),
            });
        }
        let view = out.last_mut().expect("pushed above");
        view.seconds += s.duration;
        view.shots.push(ShotLine {
            resource: s.resource,
            name: s.name,
            seconds: s.duration,
        });
    }
    Ok(out)
}

/// A command of a shot on its time line.
#[derive(Debug, Clone, Serialize)]
pub struct CommandLine {
    pub index: usize,
    /// Seconds from the start of the shot.
    pub time: f32,
    /// How long it lasts (paths and clips), seconds.
    pub length: f32,
    pub op: String,
    pub label: String,
    /// An animation whose clip is in no bank of the shot stream.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub clip_missing: bool,
}

/// One shot: where it is, who plays in it, its commands and keys.
#[derive(Debug, Clone, Serialize)]
pub struct ShotView {
    /// Place in the cut-scene.
    pub index: usize,
    pub name: String,
    pub resource: usize,
    /// Seconds from the start of the cut-scene.
    pub start: f32,
    pub end: f32,
    /// The shot's own stream, when the game has it.
    pub stream: Option<String>,
    pub actors: Vec<ActorInfo>,
    pub clips: Vec<String>,
    pub commands: Vec<CommandLine>,
    /// Times of the camera keys, seconds from the start of the shot.
    pub camera_keys: Vec<f32>,
}

/// The view of shot `s` of a cut-scene.
pub fn shot_view(cutscene: &Cutscene, s: usize) -> ShotView {
    let shot = &cutscene.shots[s];
    let block = shot.block();
    let commands = block
        .commands
        .iter()
        .zip(block.times())
        .enumerate()
        .filter(|(_, (c, _))| !matches!(c, Command::Wait { .. }))
        .map(|(index, (c, time))| CommandLine {
            index,
            time,
            length: command_span(shot, index, time).1,
            op: op_name(c),
            label: c.label(),
            clip_missing: matches!(c, Command::ActorAnim { clip, .. } if shot.clip(&clip.text).is_none()),
        })
        .collect();
    ShotView {
        index: s,
        name: shot.seq.source_name().to_owned(),
        resource: shot.resource,
        start: shot.start,
        end: shot.end(),
        stream: shot.stream.map(|_| shot.stream_name.clone()),
        actors: shot.actors.clone(),
        clips: shot
            .clips
            .iter()
            .map(|c| c.name.clone().unwrap_or_else(|| format!("{:08x}", c.hash)))
            .collect(),
        commands,
        camera_keys: edit::key_times(block, Target::Camera),
    }
}

/// `op` of the JSON form of a command (`camera_curves`).
fn op_name(command: &Command) -> String {
    serde_json::to_value(command)
        .ok()
        .and_then(|v| v.get("op").and_then(|o| o.as_str().map(str::to_owned)))
        .unwrap_or_default()
}

/// The camera of a shot at some moment.
#[derive(Debug, Clone, Serialize)]
pub struct CameraView {
    pub position: [f32; 3],
    /// Angles about x, y, z in degrees.
    pub rotation: [f32; 3],
    /// Millimetres.
    pub focal_length: f32,
    /// Degrees, of the 16:9 picture.
    pub vertical_fov: f32,
    /// A key is at this time.
    pub on_key: bool,
}

/// An actor of a shot at some moment.
#[derive(Debug, Clone, Serialize)]
pub struct ActorView {
    pub name: String,
    pub guid: Guid,
    pub position: [f32; 3],
    /// Quaternion (x, y, z, w).
    pub rotation: [f32; 4],
    pub clip: Option<String>,
    /// Times of its position and rotation keys.
    pub keys: Vec<f32>,
}

/// Where the camera and the actors of a shot are at `time`.
#[derive(Debug, Clone, Serialize)]
pub struct Moment {
    /// Seconds from the start of the shot.
    pub time: f32,
    pub camera: Option<CameraView>,
    pub actors: Vec<ActorView>,
}

/// The shot `time` seconds after its start.
pub fn moment(shot: &Shot, time: f32) -> Moment {
    let block = shot.block();
    let camera = camera_state(shot, time).map(|c| CameraView {
        position: [c.values[0], c.values[1], c.values[2]],
        rotation: [c.values[3], c.values[4], c.values[5]],
        focal_length: c.fov,
        vertical_fov: c.vertical_fov(),
        on_key: edit::key_at(block, Target::Camera, time).is_some(),
    });
    let actors = shot
        .actors
        .iter()
        .filter_map(|a| {
            let state = actor_state(shot, &a.guid, time)?;
            Some(ActorView {
                name: a.name.clone(),
                guid: a.guid,
                position: state.position,
                rotation: state.rotation,
                clip: state.clip.map(|(name, _)| name),
                keys: edit::key_times(block, Target::Actor(a.guid)),
            })
        })
        .collect();
    Moment {
        time,
        camera,
        actors,
    }
}

// ------------------------------------------------------------ edits

/// What an edit of a shot recorded.
#[derive(Debug, Clone, Serialize)]
pub struct ShotEdit {
    /// The label of the operation.
    pub label: String,
    /// False when the workspace already had this.
    pub changed: bool,
    /// The folder stream (its index in the game) and the shot's resource.
    #[serde(skip)]
    pub stream: usize,
    pub resource: usize,
    pub shot: String,
    /// What was done, in words.
    pub done: Vec<String>,
}

/// Reads shot `shot` of `stream` as the workspace has it, lets `change`
/// edit it and records the result as one operation.
fn edit_shot(
    game: &GameInstance,
    ws: &mut Workspace,
    stream: &str,
    shot: &str,
    change: impl FnOnce(&mut SeqMaster, &mut Vec<String>) -> Result<String>,
) -> Result<ShotEdit> {
    let (stream, resource) = one_shot(game, stream, shot)?;
    let mut seq = SeqMaster::parse(&ws.current_data(game, stream, resource)?)?;
    let name = seq.source_name().to_owned();
    let mut done = Vec::new();
    let label = change(&mut seq, &mut done)?;
    let changed = ws.set_resource(game, stream, resource, seq.to_bytes()?, &label)?;
    Ok(ShotEdit {
        label,
        changed,
        stream,
        resource,
        shot: name,
        done,
    })
}

fn key_word(edit: KeyEdit) -> &'static str {
    match edit {
        KeyEdit::Replaced => "changed the key",
        KeyEdit::Inserted => "added a key",
    }
}

/// An edit of the camera of a shot.
///
/// With `time`: the key there (changed, or added on the path) takes
/// `position`, `rotation` and the lens; what is not given stays as the path
/// has it at that time. `delete_key` removes the keys at `time`. Without
/// `time` the whole shot changes: `offset` and `turn` move every key of the
/// path, the lens is set on every key of the focal length.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CameraEdit {
    /// Seconds from the start of the shot.
    #[serde(default)]
    pub time: Option<f32>,
    #[serde(default)]
    pub position: Option<[f32; 3]>,
    /// Angles about x, y, z in degrees.
    #[serde(default)]
    pub rotation: Option<[f32; 3]>,
    /// The lens in millimetres (70, 50, 35 …).
    #[serde(default)]
    pub focal_length: Option<f32>,
    /// The lens as the vertical field of view of the 16:9 picture, degrees.
    #[serde(default)]
    pub vertical_fov: Option<f32>,
    /// Added to the position (of the key at `time`, or of every key).
    #[serde(default)]
    pub offset: Option<[f32; 3]>,
    /// Added to the angles (of the key at `time`, or of every key).
    #[serde(default)]
    pub turn: Option<[f32; 3]>,
    #[serde(default)]
    pub delete_key: bool,
}

impl CameraEdit {
    fn lens(&self) -> Result<Option<f32>> {
        match (self.focal_length, self.vertical_fov) {
            (Some(_), Some(_)) => bad("give `focal_length` or `vertical_fov`, not both"),
            (Some(f), None) if f > 0.0 => Ok(Some(f)),
            (Some(_), None) => bad("the focal length is a positive number of millimetres"),
            (None, Some(v)) => Ok(Some(edit::focal_length(v))),
            (None, None) => Ok(None),
        }
    }

    /// Applies the edit to a shot; says what it did in `done` and returns
    /// the label of the operation.
    pub fn apply(&self, seq: &mut SeqMaster, done: &mut Vec<String>) -> Result<String> {
        let name = seq.source_name().to_owned();
        let lens = self.lens()?;
        let Some(t) = self.time else {
            if self.delete_key || self.position.is_some() || self.rotation.is_some() {
                return bad("give `time`: the key to set or delete");
            }
            let o = self.offset.unwrap_or_default();
            let r = self.turn.unwrap_or_default();
            let delta = [o[0], o[1], o[2], r[0], r[1], r[2]];
            if delta.iter().any(|v| *v != 0.0) {
                let keys = edit::offset_camera(seq, delta)?;
                done.push(format!("moved {keys} key value(s) of the camera path"));
            }
            if let Some(focal) = lens {
                let keys = edit::set_lens(seq, focal)?;
                done.push(format!("lens {focal:.1} mm on {keys} key(s)"));
            }
            if done.is_empty() {
                return bad("nothing to change: give `offset`, `turn` or a lens, or a `time`");
            }
            return Ok(format!("Camera of {name}"));
        };
        if t < 0.0 {
            return bad("`time` is seconds from the start of the shot");
        }
        if self.delete_key {
            let removed = edit::delete_key(seq, Target::Camera, t)?;
            done.push(format!("removed {removed} key(s) at {t:.3} s"));
            return Ok(format!("Delete the camera key at {t:.2} s ({name})"));
        }
        if self.position.is_some()
            || self.rotation.is_some()
            || self.offset.is_some()
            || self.turn.is_some()
        {
            let block = seq
                .block()
                .ok_or_else(|| Error::InvalidArgument("the shot has no command block".into()))?;
            let mut values = edit::camera_values(block, t)
                .ok_or_else(|| Error::InvalidArgument("the shot has no camera path".into()))?;
            if let Some(p) = self.position {
                values[..3].copy_from_slice(&p);
            }
            if let Some(r) = self.rotation {
                values[3..].copy_from_slice(&r);
            }
            for i in 0..3 {
                values[i] += self.offset.map_or(0.0, |o| o[i]);
                values[3 + i] += self.turn.map_or(0.0, |o| o[i]);
            }
            let what = edit::set_camera(seq, t, values)?;
            done.push(format!("{} of the path at {t:.3} s", key_word(what)));
        }
        if let Some(focal) = lens {
            let what = edit::set_focal_length(seq, t, focal)?;
            done.push(format!(
                "{} of the focal length at {t:.3} s: {focal:.1} mm",
                key_word(what)
            ));
        }
        if done.is_empty() {
            return bad(
                "nothing to change: give `position`, `rotation`, `offset`, `turn` or a lens",
            );
        }
        Ok(format!("Key of the camera at {t:.2} s ({name})"))
    }
}

/// Edits the camera of a shot ([`CameraEdit`]).
pub fn camera(
    game: &GameInstance,
    ws: &mut Workspace,
    stream: &str,
    shot: &str,
    p: &CameraEdit,
) -> Result<ShotEdit> {
    edit_shot(game, ws, stream, shot, |seq, done| p.apply(seq, done))
}

/// An edit of an actor of a shot.
///
/// With `time`: its key there takes `position` and the rotation;
/// `delete_key` removes the keys at `time`. Without `time`: `offset` moves
/// the actor for the whole shot. `clip` lets it play another clip of the
/// shot's banks.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ActorEdit {
    /// The actor: its name in the shot (`igc16_player_herc`) or its GUID.
    pub actor: String,
    #[serde(default)]
    pub time: Option<f32>,
    #[serde(default)]
    pub position: Option<[f32; 3]>,
    /// Angles about x, y, z in degrees.
    #[serde(default)]
    pub rotation: Option<[f32; 3]>,
    /// The rotation as a quaternion (x, y, z, w) instead.
    #[serde(default)]
    pub quaternion: Option<[f32; 4]>,
    /// Added to the position (of the key at `time`, or for the whole shot).
    #[serde(default)]
    pub offset: Option<[f32; 3]>,
    /// Another clip of the shot's banks for its animation.
    #[serde(default)]
    pub clip: Option<String>,
    #[serde(default)]
    pub delete_key: bool,
}

/// The actor of a shot named by `text`: its name or its GUID.
pub fn find_actor(seq: &SeqMaster, text: &str) -> Result<(Guid, String)> {
    let text = text.trim();
    let actors: Vec<(Guid, &str)> = seq
        .block()
        .into_iter()
        .flat_map(|b| &b.commands)
        .filter_map(|c| match c {
            Command::ActorCreate { actor, name, .. } => Some((*actor, name.text.as_str())),
            _ => None,
        })
        .collect();
    let guid = text.parse::<Guid>().ok();
    actors
        .iter()
        .find(|(g, name)| name.eq_ignore_ascii_case(text) || Some(*g) == guid)
        .map(|(g, name)| (*g, (*name).to_owned()))
        .ok_or_else(|| {
            let names: Vec<&str> = actors.iter().map(|(_, n)| *n).collect();
            Error::InvalidArgument(format!(
                "no actor `{text}` in {} (actors: {})",
                seq.source_name(),
                names.join(", ")
            ))
        })
}

impl ActorEdit {
    fn turn(&self) -> Result<Option<[f32; 4]>> {
        match (self.rotation, self.quaternion) {
            (Some(_), Some(_)) => bad("give `rotation` or `quaternion`, not both"),
            (Some(r), None) => Ok(Some(euler_quaternion(r[0], r[1], r[2]))),
            (None, Some(q)) => {
                let n = q.iter().map(|v| v * v).sum::<f32>().sqrt();
                if n < 1e-6 {
                    return bad("the quaternion is zero");
                }
                Ok(Some(q.map(|v| v / n)))
            }
            (None, None) => Ok(None),
        }
    }

    /// Applies the edit to a shot (the clip is not checked against the
    /// shot's banks here: [`actor`] does).
    pub fn apply(&self, seq: &mut SeqMaster, done: &mut Vec<String>) -> Result<String> {
        let shot = seq.source_name().to_owned();
        let (guid, name) = find_actor(seq, &self.actor)?;
        let rotation = self.turn()?;
        if let Some(clip) = &self.clip {
            let old = edit::set_clip(seq, guid, clip)?;
            done.push(format!("clip {old} → {clip}"));
        }
        match self.time {
            None => {
                if self.delete_key || self.position.is_some() || rotation.is_some() {
                    return bad("give `time`: the key to set or delete");
                }
                if let Some(o) = self.offset {
                    let places = edit::offset_actor(seq, guid, o)?;
                    done.push(format!("moved {places} place(s) of {name}"));
                }
            }
            Some(t) if t < 0.0 => return bad("`time` is seconds from the start of the shot"),
            Some(t) if self.delete_key => {
                let removed = edit::delete_key(seq, Target::Actor(guid), t)?;
                done.push(format!("removed {removed} key(s) at {t:.3} s"));
            }
            Some(t) => {
                let mut position = self.position;
                if let Some(o) = self.offset {
                    let now = match position {
                        Some(p) => p,
                        None => actor_position(seq, guid, t)?,
                    };
                    position = Some([now[0] + o[0], now[1] + o[1], now[2] + o[2]]);
                }
                if position.is_some() || rotation.is_some() {
                    let what = edit::set_actor(seq, guid, t, position, rotation)?;
                    done.push(format!("{} of {name} at {t:.3} s", key_word(what)));
                }
            }
        }
        if done.is_empty() {
            return bad(
                "nothing to change: give `position`, `rotation`, `offset`, `clip` or `delete_key`",
            );
        }
        Ok(match self.time {
            Some(t) => format!("Key of {name} at {t:.2} s ({shot})"),
            None => format!("Actor {name} of {shot}"),
        })
    }
}

/// Where `actor` is `t` seconds into the shot `seq`.
fn actor_position(seq: &SeqMaster, actor: Guid, t: f32) -> Result<[f32; 3]> {
    let shot = Shot {
        resource: 0,
        id: 0,
        seq: seq.clone(),
        start: 0.0,
        stream: None,
        stream_name: String::new(),
        actors: Vec::new(),
        clips: Vec::new(),
    };
    actor_state(&shot, &actor, t)
        .map(|s| s.position)
        .ok_or_else(|| Error::InvalidArgument(format!("the actor is not there yet at {t:.3} s")))
}

/// Edits an actor of a shot ([`ActorEdit`]); a new clip must be in the
/// banks of the shot's stream.
pub fn actor(
    game: &GameInstance,
    ws: &mut Workspace,
    stream: &str,
    shot: &str,
    p: &ActorEdit,
) -> Result<ShotEdit> {
    if let Some(clip) = &p.clip {
        let (s, resource) = one_shot(game, stream, shot)?;
        let cutscene = Cutscene::load(&Source::new(game, Some(ws)), s, Some(resource))?;
        let loaded = cutscene
            .shots
            .iter()
            .find(|x| x.resource == resource)
            .ok_or_else(|| Error::InvalidArgument("the shot is not in its sequence".into()))?;
        if loaded.clip(clip).is_none() {
            let names: Vec<String> = loaded.clips.iter().filter_map(|c| c.name.clone()).collect();
            return bad(format!(
                "no clip `{clip}` in {} (clips: {})",
                loaded.stream_name,
                names.join(", ")
            ));
        }
    }
    edit_shot(game, ws, stream, shot, |seq, done| p.apply(seq, done))
}

/// An edit of a command of a shot: moved to `time` and (or) with `fields`
/// of its JSON form changed (a text on the screen, a message, the seconds
/// of a fade, keys of a curve).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CommandEdit {
    /// Index of the command in the shot.
    pub command: usize,
    /// Seconds from the start of the shot.
    #[serde(default)]
    pub time: Option<f32>,
    #[serde(default)]
    pub fields: Option<serde_json::Value>,
}

impl CommandEdit {
    pub fn apply(&self, seq: &mut SeqMaster, done: &mut Vec<String>) -> Result<String> {
        let shot = seq.source_name().to_owned();
        let old = seq
            .block()
            .and_then(|b| b.commands.get(self.command))
            .cloned()
            .ok_or_else(|| Error::InvalidArgument(format!("no command {}", self.command)))?;
        let label = old.label();
        let new = match &self.fields {
            Some(fields) => edit::patch_command(&old, fields)?,
            None => old.clone(),
        };
        if new != old {
            done.push(format!("`{label}` is now `{}`", new.label()));
        }
        if let Some(t) = self.time {
            if t < 0.0 {
                return bad("`time` is seconds from the start of the shot");
            }
            done.push(format!("moved to {t:.3} s"));
        }
        if done.is_empty() {
            return bad("nothing to change: give `time` or `fields`");
        }
        edit::replace_command(seq, self.command, new, self.time)?;
        Ok(match (self.time, self.fields.is_some()) {
            (Some(t), false) => format!("Move `{label}` to {t:.3} s"),
            _ => format!("Edit `{label}` of {shot}"),
        })
    }
}

/// Edits a command of a shot ([`CommandEdit`]).
pub fn command(
    game: &GameInstance,
    ws: &mut Workspace,
    stream: &str,
    shot: &str,
    p: &CommandEdit,
) -> Result<ShotEdit> {
    edit_shot(game, ws, stream, shot, |seq, done| p.apply(seq, done))
}

/// Replaces a shot by its JSON form (what the export of a shot writes).
pub fn import_json(
    game: &GameInstance,
    ws: &mut Workspace,
    stream: &str,
    shot: &str,
    json: &str,
) -> Result<ShotEdit> {
    let new: SeqMaster = serde_json::from_str(json)
        .map_err(|e| Error::InvalidArgument(format!("not the JSON of a shot: {e}")))?;
    edit_shot(game, ws, stream, shot, |seq, done| {
        let commands: usize = new.blocks.iter().map(|b| b.commands.len()).sum();
        done.push(format!("{commands} command(s)"));
        *seq = new;
        Ok(format!("Import cut-scene shot {}", seq.source_name()))
    })
}

// ------------------------------------------------------------ subtitles

/// A new start and (or) length of a line of a subtitle table.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubtitleEdit {
    /// The line, as the views list them.
    pub line: usize,
    /// Milliseconds from the start of the sound.
    #[serde(default)]
    pub start_ms: Option<u32>,
    #[serde(default)]
    pub duration_ms: Option<u32>,
}

/// What a subtitle edit recorded.
#[derive(Debug, Clone, Serialize)]
pub struct SubtitleDone {
    pub label: String,
    /// The sound file of the archives that carries the table.
    pub entry: String,
    pub line: usize,
    pub start_ms: u32,
    pub duration_ms: u32,
    /// Size of the sound file: all of it is the change (see [`retime`]).
    pub file_bytes: usize,
    pub note: String,
}

/// The sound `snu` with line `p.line` of `track`'s subtitle table retimed;
/// the lines of the group stay sorted by their start. Returns the new file
/// and the times the line has now.
pub fn retimed(track: &AudioTrack, snu: &[u8], p: &SubtitleEdit) -> Result<(Vec<u8>, u32, u32)> {
    let sub = track
        .subtitles
        .get(p.line)
        .ok_or_else(|| Error::InvalidArgument(format!("no line {}", p.line)))?;
    let mut tob: Tob = track
        .tob
        .clone()
        .ok_or_else(|| Error::InvalidArgument("the sound has no subtitle table".into()))?;
    let group = &mut tob.groups[sub.group_index];
    let entry = &mut group.entries[sub.entry];
    if let Some(s) = p.start_ms {
        entry.start_ms = s;
    }
    if let Some(d) = p.duration_ms {
        entry.duration_ms = d;
    }
    let (start, duration) = (entry.start_ms, entry.duration_ms);
    group.entries.sort_by_key(|e| e.start_ms);
    Ok((tob.replace_in_snu(snu)?, start, duration))
}

/// Why a retimed subtitle is a change of megabytes.
pub const WHOLE_SOUND_NOTE: &str = "the times of subtitles are in the header of the sound file, \
     and a file of the archives is replaced as a whole: a mod with this edit carries the whole \
     sound";

/// Retimes a line of the subtitles of the streamed sound `entry` of the
/// archives (`audiostreams\igc16\aud_igc16….exa.snu`): one operation of the
/// workspace that replaces the sound file.
pub fn retime(
    game: &GameInstance,
    ws: &mut Workspace,
    entry: &str,
    what: &str,
    p: &SubtitleEdit,
) -> Result<SubtitleDone> {
    if p.start_ms.is_none() && p.duration_ms.is_none() {
        return bad("give the new start or length of the line");
    }
    let key = FileKey::archive_entry(entry.trim());
    let snu = ws.current_file(game, &key)?;
    let track = audio_track(key.name(), &snu, None);
    if track.tob.is_none() {
        return bad(format!("{} has no subtitle table", key.name()));
    }
    let (data, start_ms, duration_ms) = retimed(&track, &snu, p)?;
    let label = format!(
        "Subtitle {} of {what} at {:.3} s for {:.3} s",
        p.line,
        start_ms as f32 / 1000.0,
        duration_ms as f32 / 1000.0
    );
    let file_bytes = data.len();
    ws.set_file(game, &key, data, &label)?;
    Ok(SubtitleDone {
        label,
        entry: key.name().to_owned(),
        line: p.line,
        start_ms,
        duration_ms,
        file_bytes,
        note: WHOLE_SOUND_NOTE.to_owned(),
    })
}

/// Retimes a line of the subtitles of the cut-scene of `stream`: of sound
/// track `track`, or of the first one with subtitles.
pub fn subtitle(
    game: &GameInstance,
    ws: &mut Workspace,
    stream: &str,
    shot: Option<&str>,
    track: Option<&str>,
    p: &SubtitleEdit,
) -> Result<SubtitleDone> {
    let (stream, shot) = locate(game, stream, shot)?;
    let cutscene = Cutscene::load(&Source::new(game, Some(ws)), stream, shot)?;
    let audio = match track {
        Some(name) => cutscene
            .audio
            .iter()
            .find(|a| a.entry.eq_ignore_ascii_case(name.trim()))
            .ok_or_else(|| Error::InvalidArgument(format!("no sound track `{name}`")))?,
        None => cutscene
            .audio
            .iter()
            .find(|a| a.tob.is_some())
            .ok_or_else(|| {
                Error::InvalidArgument("the cut-scene's sound has no subtitles".into())
            })?,
    };
    let entry = audio.entry.clone();
    retime(game, ws, &entry, &cutscene.name, p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;

    fn game() -> (tempfile::TempDir, GameInstance) {
        let dir = tempfile::tempdir().unwrap();
        testing::write_game(dir.path()).unwrap();
        testing::write_cutscene(dir.path()).unwrap();
        testing::write_cutscene_sound(dir.path()).unwrap();
        let game = GameInstance::open(dir.path()).unwrap();
        (dir, game)
    }

    fn shot_of(game: &GameInstance, ws: &Workspace) -> Shot {
        let (stream, _) = locate(game, testing::CUTSCENE, None).unwrap();
        Cutscene::load(&Source::new(game, Some(ws)), stream, None)
            .unwrap()
            .shots
            .remove(0)
    }

    #[test]
    fn a_shot_is_found_by_index_and_by_name() {
        let (_dir, game) = game();
        let (stream, none) = locate(&game, testing::CUTSCENE, None).unwrap();
        assert_eq!(none, None);
        assert_eq!(
            locate(&game, testing::CUTSCENE, Some("IGC99_S01")).unwrap(),
            (stream, Some(0))
        );
        assert_eq!(
            locate(&game, testing::CUTSCENE, Some("0")).unwrap(),
            (stream, Some(0))
        );
        let e = locate(&game, testing::CUTSCENE, Some("1")).unwrap_err();
        assert!(e.to_string().contains("not a cut-scene shot"), "{e}");
        let e = locate(&game, testing::CUTSCENE, Some("igc99_s02")).unwrap_err();
        assert!(e.to_string().contains("igc99_s01"), "{e}");
        let all = overview(&Source::new(&game, None)).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].stream, testing::CUTSCENE);
        assert_eq!(all[0].shots[0].name, "igc99_s01");
    }

    #[test]
    fn the_camera_takes_a_key_a_lens_and_a_move_of_the_whole_path() {
        let (dir, game) = game();
        let mut ws = Workspace::create(dir.path().join("ws"), &game).unwrap();
        let p = CameraEdit {
            time: Some(1.0),
            position: Some([5.0, 3.0, 8.0]),
            focal_length: Some(35.0),
            ..Default::default()
        };
        let done = camera(&game, &mut ws, testing::CUTSCENE, "igc99_s01", &p).unwrap();
        assert!(done.changed);
        assert_eq!(ws.operations().len(), 1, "one operation");
        let shot = shot_of(&game, &ws);
        let at = moment(&shot, 1.0).camera.unwrap();
        assert_eq!(at.position, [5.0, 3.0, 8.0]);
        assert!((at.focal_length - 35.0).abs() < 1e-4);
        assert!(at.on_key);
        assert_eq!(
            shot_view(
                &Cutscene::load(&Source::new(&game, Some(&ws)), done.stream, None).unwrap(),
                0
            )
            .camera_keys,
            vec![0.0, 1.0, 2.0]
        );
        // The whole path 1.5 m up, a wider lens on every key.
        let p = CameraEdit {
            offset: Some([0.0, 1.5, 0.0]),
            focal_length: Some(28.0),
            ..Default::default()
        };
        camera(&game, &mut ws, testing::CUTSCENE, "0", &p).unwrap();
        let shot = shot_of(&game, &ws);
        assert!((moment(&shot, 1.0).camera.unwrap().position[1] - 4.5).abs() < 1e-4);
        assert!((moment(&shot, 0.0).camera.unwrap().position[1] - 3.2).abs() < 1e-4);
        assert!((moment(&shot, 2.0).camera.unwrap().focal_length - 28.0).abs() < 1e-4);
        // The key goes; a key that is not there is an error.
        let p = CameraEdit {
            time: Some(1.0),
            delete_key: true,
            ..Default::default()
        };
        camera(&game, &mut ws, testing::CUTSCENE, "0", &p).unwrap();
        assert!(camera(&game, &mut ws, testing::CUTSCENE, "0", &p).is_err());
        let empty = CameraEdit::default();
        assert!(camera(&game, &mut ws, testing::CUTSCENE, "0", &empty).is_err());
        assert_eq!(ws.operations().len(), 3);
    }

    #[test]
    fn an_actor_is_moved_for_the_shot_and_keyed() {
        let (dir, game) = game();
        let mut ws = Workspace::create(dir.path().join("ws"), &game).unwrap();
        let p = ActorEdit {
            actor: "igc99_hero".into(),
            offset: Some([-1.0, 0.0, 0.5]),
            ..Default::default()
        };
        actor(&game, &mut ws, testing::CUTSCENE, "0", &p).unwrap();
        let shot = shot_of(&game, &ws);
        let m = moment(&shot, 0.5);
        assert_eq!(m.actors[0].position, [0.0, 0.0, 0.5], "created at x = 1");
        assert_eq!(moment(&shot, 2.0).actors[0].position, [3.0, 0.0, 0.5]);
        let p = ActorEdit {
            actor: shot.actors[0].guid.to_string(),
            time: Some(1.5),
            position: Some([9.0, 0.0, 0.0]),
            rotation: Some([0.0, 90.0, 0.0]),
            ..Default::default()
        };
        actor(&game, &mut ws, testing::CUTSCENE, "0", &p).unwrap();
        let shot = shot_of(&game, &ws);
        let a = &moment(&shot, 1.5).actors[0];
        assert_eq!(a.position, [9.0, 0.0, 0.0]);
        assert!(
            (a.rotation[1].abs() - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3,
            "{:?}",
            a.rotation
        );
        assert_eq!(a.keys, vec![1.0, 1.5, 2.0]);
        let nobody = ActorEdit {
            actor: "nobody".into(),
            offset: Some([1.0, 0.0, 0.0]),
            ..Default::default()
        };
        let e = actor(&game, &mut ws, testing::CUTSCENE, "0", &nobody).unwrap_err();
        assert!(e.to_string().contains("igc99_hero"), "{e}");
        // The shot has no banks: no clip to swap to.
        let clip = ActorEdit {
            actor: "igc99_hero".into(),
            clip: Some("igc99_walk".into()),
            ..Default::default()
        };
        assert!(actor(&game, &mut ws, testing::CUTSCENE, "0", &clip).is_err());
    }

    #[test]
    fn a_command_is_moved_and_its_fields_changed() {
        let (dir, game) = game();
        let mut ws = Workspace::create(dir.path().join("ws"), &game).unwrap();
        // Command 1 creates the camera: a new aspect, and half a second later.
        let p = CommandEdit {
            command: 1,
            time: Some(0.5),
            fields: Some(serde_json::json!({ "aspect": 1.5 })),
        };
        let done = command(&game, &mut ws, testing::CUTSCENE, "0", &p).unwrap();
        assert!(done.changed);
        let shot = shot_of(&game, &ws);
        let block = shot.block();
        let (index, time) = block
            .commands
            .iter()
            .zip(block.times())
            .enumerate()
            .find_map(|(i, (c, t))| match c {
                Command::CameraCreate { aspect, .. } => {
                    assert_eq!(*aspect, 1.5);
                    Some((i, t))
                }
                _ => None,
            })
            .unwrap();
        assert!((time - 0.5).abs() < 1e-6);
        assert!(index > 1);
        assert!(
            (shot.duration() - 2.0).abs() < 1e-5,
            "the others keep their time"
        );
        let wrong = CommandEdit {
            command: index,
            time: None,
            fields: Some(serde_json::json!({ "colour": 1 })),
        };
        let e = command(&game, &mut ws, testing::CUTSCENE, "0", &wrong).unwrap_err();
        assert!(e.to_string().contains("aspect"), "{e}");
        let kind = CommandEdit {
            command: index,
            time: None,
            fields: Some(serde_json::json!({ "op": "fade" })),
        };
        assert!(command(&game, &mut ws, testing::CUTSCENE, "0", &kind).is_err());
    }

    #[test]
    fn a_text_command_takes_a_plain_string() {
        use crate::resource::trinity::FixedText;
        let old = Command::Script {
            kind: 1,
            capacity: 64,
            text: FixedText {
                text: "old_message".into(),
                slack: Some(vec![1, 2, 3]),
            },
            trailer: 0,
        };
        let new = edit::patch_command(&old, &serde_json::json!({ "text": "new_message" })).unwrap();
        let Command::Script { text, kind, .. } = new else {
            panic!()
        };
        assert_eq!(text, FixedText::new("new_message"));
        assert_eq!(kind, 1);
    }

    #[test]
    fn a_subtitle_is_retimed_in_its_sound_file() {
        let (dir, game) = game();
        let mut ws = Workspace::create(dir.path().join("ws"), &game).unwrap();
        let (stream, _) = locate(&game, testing::CUTSCENE, None).unwrap();
        let before = Cutscene::load(&Source::new(&game, Some(&ws)), stream, None).unwrap();
        assert_eq!(before.audio.len(), 1);
        assert_eq!(before.audio[0].subtitles[0].start_ms, 500);
        let p = SubtitleEdit {
            line: 0,
            start_ms: Some(1500),
            duration_ms: None,
        };
        let done = subtitle(&game, &mut ws, testing::CUTSCENE, None, None, &p).unwrap();
        assert_eq!(done.entry, testing::CUTSCENE_SOUND);
        assert_eq!((done.start_ms, done.duration_ms), (1500, 400));
        assert_eq!(ws.operations().len(), 1);
        // Line 0 started before line 1 (at 1000 ms): now it is the second.
        let after = Cutscene::load(&Source::new(&game, Some(&ws)), stream, None).unwrap();
        let starts: Vec<u32> = after.audio[0]
            .subtitles
            .iter()
            .map(|s| s.start_ms)
            .collect();
        assert_eq!(starts, vec![1000, 1500]);
        // The same by the entry of the archives; the sound data is untouched.
        let p = SubtitleEdit {
            line: 0,
            start_ms: None,
            duration_ms: Some(900),
        };
        let done = retime(&game, &mut ws, testing::CUTSCENE_SOUND, "igc99", &p).unwrap();
        assert_eq!((done.start_ms, done.duration_ms), (1000, 900));
        let key = FileKey::archive_entry(testing::CUTSCENE_SOUND);
        assert!(
            ws.current_file(&game, &key)
                .unwrap()
                .ends_with(b"SOUND DATA")
        );
        assert!(
            retime(
                &game,
                &mut ws,
                testing::CUTSCENE_SOUND,
                "x",
                &SubtitleEdit::default()
            )
            .is_err()
        );
    }

    /// The finding of the test mods: a mod that only retimes a subtitle
    /// carries the whole sound, and building it says so.
    #[test]
    fn a_mod_with_a_retimed_subtitle_says_that_it_carries_the_whole_sound() {
        use crate::mods::{ModInfo, Relations, build_from_workspace};
        let dir = tempfile::tempdir().unwrap();
        testing::write_game(dir.path()).unwrap();
        testing::write_cutscene(dir.path()).unwrap();
        testing::write_cutscene_sound_with(dir.path(), &vec![7u8; 2 << 20]).unwrap();
        let game = GameInstance::open(dir.path()).unwrap();
        let mut ws = Workspace::create(dir.path().join("ws"), &game).unwrap();
        let p = SubtitleEdit {
            line: 1,
            start_ms: Some(1200),
            duration_ms: None,
        };
        let done = subtitle(&game, &mut ws, testing::CUTSCENE, None, None, &p).unwrap();
        assert!(done.file_bytes > 2 << 20, "the whole file is the change");
        assert!(done.note.contains("whole"));
        let info = ModInfo {
            id: "late-line".into(),
            name: "Late line".into(),
            version: "1.0.0".into(),
            author: String::new(),
            description: String::new(),
            url: None,
            relations: Relations::default(),
            whole_resources: false,
        };
        let out = dir.path().join("late-line.zip");
        let report = build_from_workspace(&ws, &info, None, None, &out).unwrap();
        assert_eq!(report.notes.len(), 1, "{:?}", report.notes);
        assert!(report.notes[0].contains(testing::CUTSCENE_SOUND));
        assert!(report.notes[0].contains("whole"));
        assert!(report.payload_bytes > 2 << 20);
        // A mod of a file alone installs: the mod manager plans the file.
        assert_eq!(report.resources, 0);
        let mut mods = crate::mods::ModManager::open(&game).unwrap();
        mods.install_file(&game, &out, true).unwrap();
        assert_eq!(mods.plan_files().len(), 1);
        // A small sound: no note.
        let small = tempfile::tempdir().unwrap();
        testing::write_game(small.path()).unwrap();
        testing::write_cutscene(small.path()).unwrap();
        testing::write_cutscene_sound(small.path()).unwrap();
        let game = GameInstance::open(small.path()).unwrap();
        let mut ws = Workspace::create(small.path().join("ws"), &game).unwrap();
        subtitle(&game, &mut ws, testing::CUTSCENE, None, None, &p).unwrap();
        let out = small.path().join("late-line.zip");
        let report = build_from_workspace(&ws, &info, None, None, &out).unwrap();
        assert!(report.notes.is_empty(), "{:?}", report.notes);
    }
}

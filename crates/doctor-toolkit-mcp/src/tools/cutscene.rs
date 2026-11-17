//! In-engine cut-scenes (stage 104): the shots of the game, a shot's time
//! line, camera and actors, and the edits the editor makes in its level
//! view and time line — keys of the camera and of actors, fields and times
//! of commands. Every edit is an operation of
//! `doctor_toolkit::cutscene::ops`, the same the command line records.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use doctor_toolkit::cutscene::cast::Cast;
use doctor_toolkit::cutscene::gltf::{ExportOptions, export as export_gltf};
use doctor_toolkit::cutscene::ops::{self, ActorEdit, CameraEdit, CommandEdit};
use doctor_toolkit::cutscene::{Cutscene, Source};
use doctor_toolkit::game::GameAssets;
use doctor_toolkit::workspace::Workspace;

use super::Output;
use super::edit::finish;
use crate::state::{ToolResult, Toolkit, fail};

// ------------------------------------------------------------ parameters

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct CutscenesParams {
    /// A folder stream with shots
    /// (`gf2/cuba/global/objectives/…/_734_igc_content`): the cut-scene of
    /// it — shots in playing order, sound tracks, subtitles. Without it:
    /// every stream with shots.
    #[serde(default)]
    pub stream: Option<String>,
    /// With `stream`: one shot, by its resource index or source name
    /// (`igc16_s02`) — its actors, commands with times, camera keys.
    #[serde(default)]
    pub shot: Option<String>,
    /// With `shot`: where the camera and the actors are at this time
    /// (seconds from the start of the shot).
    #[serde(default)]
    pub time: Option<f32>,
    /// Without `stream`: only streams whose name contains this text.
    #[serde(default)]
    pub filter: Option<String>,
    /// Read the game files only, without the edits of the workspace.
    #[serde(default)]
    pub original: Option<bool>,
    /// Skip this many streams of the list (paging).
    #[serde(default)]
    pub offset: Option<usize>,
    /// At most this many streams (default 100, at most 500); each with the
    /// names of its shots and the messages that start its sequences.
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct CutsceneCameraParams {
    /// The folder stream of the cut-scene (`cutscenes` lists them).
    pub stream: String,
    /// The shot: resource index or source name (`igc16_s02`).
    pub shot: String,
    /// Seconds from the start of the shot: the key to set (the key there
    /// is changed, else one is added on the path). Without it `offset`,
    /// `turn` and the lens change every key of the shot.
    #[serde(default)]
    pub time: Option<f32>,
    /// Position `[x, y, z]` of the key (Y is up; world coordinates in the
    /// game's shots).
    #[serde(default)]
    pub position: Option<[f32; 3]>,
    /// Angles about x, y, z in degrees of the key (the camera looks down
    /// −Z; x tilts it, y turns it).
    #[serde(default)]
    pub rotation: Option<[f32; 3]>,
    /// The lens: focal length in millimetres (70, 50, 35; smaller is wider).
    #[serde(default)]
    pub focal_length: Option<f32>,
    /// The lens as the vertical field of view of the 16:9 picture in
    /// degrees, instead of `focal_length`.
    #[serde(default)]
    pub vertical_fov: Option<f32>,
    /// `[x, y, z]` added to the position: of the key at `time`, or of
    /// every key of the path.
    #[serde(default)]
    pub offset: Option<[f32; 3]>,
    /// Degrees added to the angles: of the key at `time`, or of every key.
    #[serde(default)]
    pub turn: Option<[f32; 3]>,
    /// Remove the camera's keys at `time`.
    #[serde(default)]
    pub delete_key: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct CutsceneActorParams {
    /// The folder stream of the cut-scene.
    pub stream: String,
    /// The shot: resource index or source name.
    pub shot: String,
    /// The actor: its name in the shot (`igc16_player_herc`) or its GUID.
    pub actor: String,
    /// Seconds from the start of the shot: the key to set. Without it
    /// `offset` moves the actor for the whole shot.
    #[serde(default)]
    pub time: Option<f32>,
    /// Position `[x, y, z]` of the key.
    #[serde(default)]
    pub position: Option<[f32; 3]>,
    /// Angles about x, y, z in degrees of the key.
    #[serde(default)]
    pub rotation: Option<[f32; 3]>,
    /// The rotation as a quaternion `[x, y, z, w]` instead.
    #[serde(default)]
    pub quaternion: Option<[f32; 4]>,
    /// `[x, y, z]` added to the position: of the key at `time`, or where
    /// the actor is created and of every key.
    #[serde(default)]
    pub offset: Option<[f32; 3]>,
    /// Another clip of the shot's banks for the actor's animation
    /// (`cutscenes` with `shot` lists the clips).
    #[serde(default)]
    pub clip: Option<String>,
    /// Remove the actor's keys at `time`.
    #[serde(default)]
    pub delete_key: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct CutsceneCommandParams {
    /// The folder stream of the cut-scene.
    pub stream: String,
    /// The shot: resource index or source name.
    pub shot: String,
    /// Index of the command (`cutscenes` with `shot` lists them).
    pub command: usize,
    /// Move it to this time (seconds from the start of the shot); the
    /// other commands keep theirs.
    #[serde(default)]
    pub time: Option<f32>,
    /// Fields of the command to change, as `resource_json` of the shot
    /// shows them: `{"text": "start_igc_17"}` of a text command (time of
    /// day, message, shadows, time scale; `kind` 0–3), `{"seconds": 2,
    /// "color": [0, 0, 0, 1]}` of a fade, `{"speed": 0.5}` of an
    /// animation, `{"state": 2}` of an effect.
    #[serde(default)]
    pub fields: Option<Value>,
}

// ------------------------------------------------------------ reading

fn shot_json(view: &ops::ShotView) -> ToolResult<Value> {
    Ok(serde_json::to_value(view)?)
}

pub fn cutscenes(kit: &Toolkit, p: CutscenesParams) -> ToolResult<Output> {
    let ws = kit.workspace_unless(p.original.unwrap_or(false))?;
    let source = Source::new(&kit.game, ws.as_ref());
    let Some(stream) = p.stream.as_deref() else {
        if p.shot.is_some() || p.time.is_some() {
            return fail("`shot` and `time` need `stream`");
        }
        let filter = p.filter.as_deref().map(str::to_lowercase);
        let all: Vec<ops::StreamView> = ops::overview(&source)?
            .into_iter()
            .filter(|s| filter.as_deref().is_none_or(|f| s.stream.contains(f)))
            .collect();
        let offset = p.offset.unwrap_or(0);
        let limit = super::clamp(p.limit, 100, 500);
        // Compact: the game has hundreds of shots, and the answer has a limit.
        let page: Vec<Value> = all
            .iter()
            .skip(offset)
            .take(limit)
            .map(|s| {
                json!({
                    "stream": s.stream,
                    "seconds": s.seconds,
                    "starts_on": s.sequences.iter()
                        .filter_map(|q| q.start_message.as_deref())
                        .collect::<Vec<_>>(),
                    "shots": s.shots.iter().map(|x| x.name.as_str()).collect::<Vec<_>>().join(" "),
                })
            })
            .collect();
        return Ok(Output::Json(json!({
            "streams": page,
            "total": all.len(),
            "offset": offset,
            "shots": all.iter().map(|s| s.shots.len()).sum::<usize>(),
            "note": "give `stream` for a cut-scene, `stream` and `shot` for a shot",
        })));
    };
    let (stream, shot) = ops::locate(&kit.game, stream, p.shot.as_deref())?;
    let cutscene = Cutscene::load(&source, stream, shot)?;
    if let Some(resource) = shot {
        let s = cutscene
            .shots
            .iter()
            .position(|s| s.resource == resource)
            .ok_or_else(|| crate::state::ToolError("the shot is not in its sequence".into()))?;
        let mut out = json!({
            "cutscene": cutscene.name,
            "stream": cutscene.stream_name,
            "shot": shot_json(&ops::shot_view(&cutscene, s))?,
        });
        if let Some(t) = p.time {
            out["moment"] = serde_json::to_value(ops::moment(&cutscene.shots[s], t))?;
        }
        return Ok(Output::Json(out));
    }
    if p.time.is_some() {
        return fail("`time` is seconds from the start of a shot: give `shot`");
    }
    let shots: Vec<Value> = cutscene
        .shots
        .iter()
        .enumerate()
        .map(|(i, s)| {
            json!({
                "index": i,
                "name": s.seq.source_name(),
                "resource": s.resource,
                "start": s.start,
                "end": s.end(),
                "actors": s.actors.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
                "commands": s.block().commands.len(),
            })
        })
        .collect();
    Ok(Output::Json(json!({
        "cutscene": cutscene.name,
        "stream": cutscene.stream_name,
        "seconds": cutscene.duration(),
        "sequence": serde_json::to_value(&cutscene.sequence)?,
        "shots": shots,
        "audio": serde_json::to_value(&cutscene.audio)?,
        "note": "times of subtitles are milliseconds from the start of their sound; \
                 edit_subtitle with `entry` and `line` changes them",
    })))
}

// ------------------------------------------------------------ edits

/// Records a shot edit and answers with the operation and its diff.
fn record(
    kit: &Toolkit,
    what: String,
    edit: impl FnOnce(&mut Workspace) -> doctor_toolkit::Result<ops::ShotEdit>,
) -> ToolResult<Output> {
    let mut access = kit.write_workspace(&what)?;
    let ws = &mut access.ws;
    let since = ws.head();
    let done = edit(ws)?;
    let label = done.label.clone();
    finish(ws, since, &label, serde_json::to_value(&done)?)
}

pub fn cutscene_camera(kit: &Toolkit, p: CutsceneCameraParams) -> ToolResult<Output> {
    let edit = CameraEdit {
        time: p.time,
        position: p.position,
        rotation: p.rotation,
        focal_length: p.focal_length,
        vertical_fov: p.vertical_fov,
        offset: p.offset,
        turn: p.turn,
        delete_key: p.delete_key.unwrap_or(false),
    };
    record(kit, format!("cutscene_camera: {}", p.shot), |ws| {
        ops::camera(&kit.game, ws, &p.stream, &p.shot, &edit)
    })
}

pub fn cutscene_actor(kit: &Toolkit, p: CutsceneActorParams) -> ToolResult<Output> {
    let edit = ActorEdit {
        actor: p.actor,
        time: p.time,
        position: p.position,
        rotation: p.rotation,
        quaternion: p.quaternion,
        offset: p.offset,
        clip: p.clip,
        delete_key: p.delete_key.unwrap_or(false),
    };
    record(kit, format!("cutscene_actor: {}", p.shot), |ws| {
        ops::actor(&kit.game, ws, &p.stream, &p.shot, &edit)
    })
}

pub fn cutscene_command(kit: &Toolkit, p: CutsceneCommandParams) -> ToolResult<Output> {
    let edit = CommandEdit {
        command: p.command,
        time: p.time,
        fields: p.fields,
    };
    record(kit, format!("cutscene_command: {}", p.shot), |ws| {
        ops::command(&kit.game, ws, &p.stream, &p.shot, &edit)
    })
}

// ------------------------------------------------------------ export

/// The shot `resource` of `stream` as one glTF animation: the camera and
/// the actors with their clips (what `cutscene gltf` of the command line
/// writes). Returns the file and what is in it.
pub fn shot_glb(
    kit: &Toolkit,
    ws: Option<&Workspace>,
    stream: usize,
    resource: usize,
) -> ToolResult<(Vec<u8>, Value)> {
    let source = Source::new(&kit.game, ws);
    let cutscene = Cutscene::load(&source, stream, Some(resource))?;
    let shots: Vec<usize> = cutscene
        .shots
        .iter()
        .position(|s| s.resource == resource)
        .into_iter()
        .collect();
    let index = kit.resources();
    let mut assets = GameAssets::new(kit.game.clone(), index);
    let rigs = assets.rigs().to_vec();
    let mut cast = Cast::new(&kit.game, index, rigs);
    let options = ExportOptions {
        shots,
        skeletons_only: false,
    };
    let (glb, report) = export_gltf(&cutscene, &source, &mut cast, &mut assets, &options)?;
    let info = json!({
        "cutscene": cutscene.name,
        "actors": report.actors,
        "characters": report.characters,
        "animations": report.animations,
        "problems": report.problems,
    });
    Ok((glb, info))
}

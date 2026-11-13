//! The live link with the running game (stage 34): what the module in the
//! game answers. Reading (`live_status`, `player`, `objects_near`) is always
//! there; writing into the running game (`teleport`, `send_message`, `lua`)
//! only with `--allow-game`, and the module itself must allow it (the
//! launches of `play` do).

use std::time::Duration;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use doctor_toolkit::live::{Client, Endpoint, LinkError, proto};
use doctor_toolkit::play::SessionState;

use super::Output;
use crate::state::{ToolError, ToolResult, Toolkit, fail};

/// How long a request may wait for the game (1–2 frames a second without
/// a graphics card under Wine make answers slow).
const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct LiveStatusParams {}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct PlayerParams {}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ObjectsNearParams {
    /// Centre `[x, y, z]` (Y is up) [default: the player].
    #[serde(default)]
    pub center: Option<[f32; 3]>,
    /// Radius in metres (default 30).
    #[serde(default)]
    pub radius: Option<f32>,
    /// Only this class: its name (`VariableSwitch`) or `0x` hash.
    #[serde(default)]
    pub class: Option<String>,
    /// At most this many objects (default 50, at most 500), nearest first.
    #[serde(default)]
    pub limit: Option<u16>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct TeleportParams {
    /// Where to put the player `[x, y, z]` (Y is up).
    pub position: [f32; 3],
    /// Turn him to this heading in degrees (0 faces +Z); he keeps his heading when not given.
    #[serde(default)]
    pub yaw_degrees: Option<f32>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct SendMessageParams {
    /// The message: its name (`boundary_fade_out`) or `0x` hash.
    pub message: String,
    /// Only to this object: its GUID (32 hexadecimal digits, as
    /// `objects_near` prints it); to every listener when not given.
    #[serde(default)]
    pub target: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct LuaParams {
    /// Lua 5.0 code run in the game's Lua state; `return` values come back
    /// as text, e.g. `return GetSimTime()`.
    pub code: String,
}

/// Where the live link is: the module `play` installed, else the one the
/// server was given, else the one installed in the game folder (by the
/// editor's Play or `doctor-toolkit play --live`).
pub fn endpoint(kit: &Toolkit) -> (Endpoint, &'static str) {
    let session = kit.session.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(e) = session.as_ref().and_then(|s| s.endpoint()) {
        return (e, "the module installed by play");
    }
    drop(session);
    if let Some(e) = &kit.live {
        return (e.clone(), "the server's configuration");
    }
    (
        Endpoint::of_game(kit.game.root()),
        "the module in the game folder",
    )
}

fn connect(kit: &Toolkit) -> ToolResult<Client> {
    let (endpoint, from) = endpoint(kit);
    Client::connect(&endpoint, "doctor-toolkit mcp", TIMEOUT).map_err(|e| match e {
        LinkError::Unreachable(_) => ToolError(format!(
            "{e} (port {} from {from}). The game must run with the live module: `play` starts \
             it so (with --allow-game), or the editor's Play with the live link on",
            endpoint.port
        )),
        other => ToolError(other.to_string()),
    })
}

/// A write into the running game: only when the module allows writes.
fn connect_for_write(kit: &Toolkit) -> ToolResult<Client> {
    let client = connect(kit)?;
    if !client.hello().writes {
        return fail(
            "the module in the game does not allow writes (it was installed with writes off); \
             start the game with `play`, or turn writes on in the editor's live link options",
        );
    }
    Ok(client)
}

fn pose(p: &proto::Pose) -> Value {
    json!({
        "position": p.position,
        "forward": p.forward,
        "up": p.up,
        "right": p.right,
        "yaw_degrees": p.yaw().to_degrees(),
    })
}

fn class_hash(text: &str) -> u32 {
    match proto::MessageId::parse(text) {
        proto::MessageId::Hash(h) => h,
        proto::MessageId::Name(n) => proto::name_hash(&n),
    }
}

/// The game started by `play`, when there is one.
pub fn session_info(kit: &Toolkit) -> Value {
    let mut session = kit.session.lock().unwrap_or_else(|e| e.into_inner());
    let Some(s) = session.as_mut() else {
        return json!({ "started_by_this_server": false });
    };
    let state = match s.state() {
        SessionState::Starting => "starting".to_owned(),
        SessionState::Running(_) => "running".to_owned(),
        SessionState::Exited => "exited".to_owned(),
        SessionState::NotStarted(why) => format!("not started: {why}"),
    };
    json!({
        "started_by_this_server": true,
        "state": state,
        "seconds": s.started.elapsed().as_secs(),
        "display": s.display.as_ref().map(|d| d.name()),
        "screen": s.display.as_ref().map(|d| d.size()),
        "start": s.start,
        "live_module": s.live.is_some(),
        "workspace_applied_for_this_launch": s.revert_after,
        "notes": s.notes,
    })
}

pub fn live_status(kit: &Toolkit, _: LiveStatusParams) -> ToolResult<Output> {
    let game = session_info(kit);
    let mut client = match connect(kit) {
        Ok(c) => c,
        Err(e) => {
            return Ok(Output::Json(json!({
                "game": game,
                "link": false,
                "why": e.0,
            })));
        }
    };
    let hello = client.hello().clone();
    let status = client.status()?;
    let streams = client.streams().unwrap_or_default();
    Ok(Output::Json(json!({
        "game": game,
        "link": true,
        "module": hello.module,
        "edition_ok": hello.edition_ok,
        "edition": hello.edition,
        "writes_allowed_by_module": hello.writes,
        "writes_allowed_by_server": kit.allow_game,
        "frame": status.frame,
        "loop": status.state.name(),
        "level_loaded": status.player,
        "sim_time_ms": status.sim_time_ms,
        "time_of_day": status.time_of_day,
        "streams": streams.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
    })))
}

pub fn player(kit: &Toolkit, _: PlayerParams) -> ToolResult<Output> {
    let mut client = connect(kit)?;
    let player = client.player()?;
    let camera = client.camera().ok().flatten();
    Ok(Output::Json(json!({
        "player": player.as_ref().map(pose),
        "note": player.is_none().then_some("no player: no level is loaded"),
        "camera": camera.map(|c| json!({ "pose": pose(&c.pose), "fov": c.fov })),
    })))
}

pub fn objects_near(kit: &Toolkit, p: ObjectsNearParams) -> ToolResult<Output> {
    let mut client = connect(kit)?;
    let radius = p.radius.unwrap_or(30.0).clamp(0.1, 10_000.0);
    let max = p.limit.unwrap_or(50).clamp(1, 500);
    let objects = client.objects(p.center, radius, p.class.as_deref().map(class_hash), max)?;
    let items: Vec<Value> = objects
        .iter()
        .map(|o| {
            json!({
                "guid": proto::guid_hex(&o.guid),
                "class": o.class_name,
                "class_hash": format!("{:#010x}", o.class),
                "position": o.position,
                "distance": o.distance,
            })
        })
        .collect();
    Ok(Output::Json(json!({
        "center": p.center.map_or_else(|| json!("the player"), |c| json!(c)),
        "radius": radius,
        "count": items.len(),
        "objects": items,
    })))
}

pub fn teleport(kit: &Toolkit, p: TeleportParams) -> ToolResult<Output> {
    if p.position.iter().any(|v| !v.is_finite()) {
        return fail("the position must be three finite numbers");
    }
    let mut client = connect_for_write(kit)?;
    let before = client.player().ok().flatten();
    let done = client.teleport(p.position, p.yaw_degrees.map(f32::to_radians))?;
    let after = client.player().ok().flatten();
    Ok(Output::Json(json!({
        "before": before.as_ref().map(pose),
        "after": after.as_ref().map(pose),
        "note": done.note,
    })))
}

pub fn send_message(kit: &Toolkit, p: SendMessageParams) -> ToolResult<Output> {
    let target = p
        .target
        .as_deref()
        .map(|t| {
            proto::parse_guid(t)
                .ok_or_else(|| ToolError(format!("`{t}` is not a GUID (32 hexadecimal digits)")))
        })
        .transpose()?;
    let mut client = connect_for_write(kit)?;
    let done = client.send(proto::MessageId::parse(&p.message), target)?;
    Ok(Output::Json(json!({
        "message": p.message,
        "received_by": done.count,
        "note": done.note,
    })))
}

pub fn lua(kit: &Toolkit, p: LuaParams) -> ToolResult<Output> {
    if p.code.trim().is_empty() {
        return fail("give Lua code");
    }
    let mut client = connect_for_write(kit)?;
    let reply = client.lua(&p.code)?;
    Ok(Output::Json(json!({
        "ok": reply.ok,
        "values": reply.values,
    })))
}

//! Debugging the running game over the live link and what goes with it
//! (stage 104): the game's journal (`game_log`), pause, step and
//! breakpoints (`pause`, `resume`, `breakpoints`), the free camera
//! (`camera`), edited textures sent into the running game
//! (`push_textures`), the crash reports of the game (`crash_reports`) and
//! the copies of the game on this computer (`detect_games`).
//!
//! **Who lets a stopped game go.** A breakpoint, `pause` and a step hold
//! the game's main thread. The module never holds it for good: a stop ends
//! by itself after `hold_s` (120 s unless set, at most 900). Before that
//! `resume` lets it go, and `stop` ends the game whatever it does. While
//! the game stands the reading tools answer; writes and Lua are answered
//! only when it stands in a frame (a pause or a step), and the writing
//! tools refuse with the place of the stop otherwise.

use std::time::{Duration, Instant};

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use doctor_toolkit::crash;
use doctor_toolkit::edition::Edition;
use doctor_toolkit::install::{self, SearchRoots};
use doctor_toolkit::live::names::LiveNames;
use doctor_toolkit::live::proto::journal::mask;
use doctor_toolkit::live::proto::{self, MessageId};
use doctor_toolkit::live::view::{debug_json, event_json, free_camera};
use doctor_toolkit::live::{self, Client};
use doctor_toolkit::play::start;

use super::Output;
use super::live::{connect, connect_for_write};
use crate::state::{ToolError, ToolResult, Toolkit, fail};

// ------------------------------------------------------------ parameters

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct GameLogParams {
    /// Start after this event number (`next` of the previous call); all the
    /// module kept when not given.
    #[serde(default)]
    pub after: Option<u64>,
    /// Also messages of the bus with their receivers (the module records
    /// them while someone asks; many).
    #[serde(default)]
    pub messages: Option<bool>,
    /// Only Lua errors (with their call stack).
    #[serde(default)]
    pub errors: Option<bool>,
    /// Only events that contain this text (case does not matter): a part of
    /// printed text, a message name, a script.
    #[serde(default)]
    pub filter: Option<String>,
    /// At most this many events (default 100, at most 500).
    #[serde(default)]
    pub limit: Option<u16>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct PauseParams {}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ResumeParams {
    /// Go on for one frame and stop again.
    #[serde(default)]
    pub step: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct BreakpointsParams {
    /// Stop when one of these messages goes through the bus, before it
    /// reaches its listeners (names or `0x` hashes).
    #[serde(default)]
    pub messages: Vec<String>,
    /// With `messages`: only when this object (GUID) gets the message.
    #[serde(default)]
    pub to: Option<String>,
    /// Stop when the game prints text that contains one of these
    /// (`dt:SCRIPT:NODE;` is a traced node of a visual script).
    #[serde(default)]
    pub prints: Vec<String>,
    /// Stop on any Lua error.
    #[serde(default)]
    pub lua_error: Option<bool>,
    /// Each rule goes away after its first hit.
    #[serde(default)]
    pub once: Option<bool>,
    /// Keep the breakpoints set before and add these (they are replaced
    /// when not given).
    #[serde(default)]
    pub add: Option<bool>,
    /// Remove every breakpoint.
    #[serde(default)]
    pub clear: Option<bool>,
    /// Seconds a stop lasts at most before the game goes on by itself
    /// (default 120, at most 900).
    #[serde(default)]
    pub hold_s: Option<u16>,
    /// Wait up to this many seconds for the game to stop (at most 120) and
    /// answer with where it stands.
    #[serde(default)]
    pub wait_s: Option<u64>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct CameraParams {
    /// Where the game draws from, `[x, y, z]` (Y is up) [default: where its
    /// camera is].
    #[serde(default)]
    pub position: Option<[f32; 3]>,
    /// Look at this point `[x, y, z]`, instead of `yaw_degrees` and `pitch_degrees`.
    #[serde(default)]
    pub look_at: Option<[f32; 3]>,
    /// Heading in degrees (0 faces +Z) [default: the camera's].
    #[serde(default)]
    pub yaw_degrees: Option<f32>,
    /// Pitch in degrees, up is positive [default: the camera's].
    #[serde(default)]
    pub pitch_degrees: Option<f32>,
    /// Give the game its own camera back.
    #[serde(default)]
    pub release: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct PushTexturesParams {
    /// Only this texture: its name hash (`0x…`, the last id of its
    /// tg4h/tg4d) [default: every texture the workspace edits].
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct CrashReportsParams {
    /// One report: its number in the list (1 is the newest) or its name,
    /// with its text; the list when not given.
    #[serde(default)]
    pub report: Option<String>,
    /// With `report`: start the text at this character (paging).
    #[serde(default)]
    pub offset: Option<usize>,
    /// With `report`: at most this many characters (default 12000).
    #[serde(default)]
    pub max_chars: Option<usize>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct DetectGamesParams {}

// ------------------------------------------------------------ helpers

/// Names of messages and objects of the game files, for events (built once).
fn names(kit: &Toolkit) -> &LiveNames {
    kit.live_names()
}

fn need_debug(client: &Client) -> ToolResult<()> {
    if !client.has_debug() {
        return fail(format!(
            "the module in the game speaks protocol {}: breakpoints, pause, the free camera \
             and textures need the module of this toolkit (`play` installs it)",
            client.version()
        ));
    }
    Ok(())
}

/// The debug state with the rules of who lets the game go.
fn state_json(kit: &Toolkit, client: &mut Client) -> ToolResult<Value> {
    let d = client.debug_state()?;
    let mut v = debug_json(&d, Some(names(kit)));
    if let Some(stop) = &d.stop {
        v["held_s"] = json!(f64::from(stop.held_ms) / 1000.0);
        v["goes_on_by_itself_in_s"] = json!(f64::from(stop.resume_in_ms) / 1000.0);
        v["note"] = json!(
            "the game stands: `resume` lets it go (`step: true` for one frame), `stop` ends it; \
             reading tools answer, writes and lua only while it stands in a frame"
        );
    }
    Ok(v)
}

/// Lets a game go that `stop` is about to end: no breakpoints, no pause, the
/// game's own camera. Returns whether it stood. Best effort: a game that
/// does not answer is ended anyway.
pub(crate) fn release(endpoint: &live::Endpoint) -> bool {
    let Ok(mut client) = Client::connect(endpoint, "doctor-toolkit mcp", Duration::from_secs(2))
    else {
        return false;
    };
    if !client.has_debug() || !client.hello().writes {
        return false;
    }
    let Ok(state) = client.debug_state() else {
        return false;
    };
    let stood = state.stop.is_some() || state.pending;
    if !state.rules.is_empty() {
        let _ = client.set_breaks(Vec::new(), 0);
    }
    if state.camera.is_some() {
        let _ = client.set_camera(None);
    }
    if stood {
        let _ = client.control(proto::Control::Continue);
        settle(&mut client, |d| d.stop.is_none());
    }
    stood
}

/// Why a write cannot go into a game that stands outside a frame; the
/// writing tools of the live link ask this before they write.
pub(crate) fn refuse_while_stopped(client: &mut Client) -> ToolResult<()> {
    if !client.has_debug() {
        return Ok(());
    }
    let Ok(d) = client.debug_state() else {
        return Ok(());
    };
    match d.stop {
        Some(stop) if stop.place != proto::StopPlace::Frame => fail(format!(
            "the game stands at a breakpoint in the {} and takes writes only when it stands in \
             a frame: `resume` first (it goes on by itself in {:.0} s)",
            stop.place.name(),
            f64::from(stop.resume_in_ms) / 1000.0
        )),
        _ => Ok(()),
    }
}

// ------------------------------------------------------------ the journal

pub fn game_log(kit: &Toolkit, p: GameLogParams) -> ToolResult<Output> {
    let errors = p.errors.unwrap_or(false);
    let messages = p.messages.unwrap_or(false);
    if errors && messages {
        return fail("give `errors` or `messages`, not both");
    }
    let kinds = if errors {
        mask::LUA_ERROR
    } else if messages {
        mask::ALL_V8
    } else {
        mask::OUTPUT | mask::LUA_ERROR
    };
    let mut client = connect(kit)?;
    if !client.has_journal() {
        return fail(format!(
            "the module in the game speaks protocol {} and has no journal: `play` installs the \
             module of this toolkit",
            client.version()
        ));
    }
    let limit = p.limit.unwrap_or(100).clamp(1, 500);
    let want = p.filter.as_deref().map(str::to_ascii_lowercase);
    let names = messages.then(|| names(kit));
    let mut next = p.after.unwrap_or(0);
    let mut lost = 0;
    let mut events = Vec::new();
    let mut recording = 0;
    // With a filter: read on until enough events match or the journal ends.
    for _ in 0..20 {
        let batch = client.events(next, kinds, limit)?;
        recording = batch.recording;
        lost += batch.lost;
        let done = batch.events.is_empty();
        for e in &batch.events {
            let v = event_json(e, names);
            if want
                .as_ref()
                .is_none_or(|w| v.to_string().to_ascii_lowercase().contains(w))
                && events.len() < usize::from(limit)
            {
                events.push(v);
            }
        }
        next = batch.next.saturating_sub(1);
        if done || events.len() >= usize::from(limit) || want.is_none() {
            break;
        }
    }
    let asked = mask::for_version(kinds, client.version());
    Ok(Output::Json(json!({
        "events": events,
        "next": next,
        "lost": lost,
        "note": (recording & asked != asked).then_some(
            "the module records only a part of this (journal=0 in doctor_live.cfg, or a hook \
             could not be installed)"
        ),
        "more": "call again with `after: next` for what happens later",
    })))
}

// ------------------------------------------------------------ pause, step, breakpoints

/// Waits (at most two seconds) until the game has done what a control
/// asked: the module answers at once and the game's main thread follows in
/// its next frame, so the state read right after the answer is the old one.
fn settle(client: &mut Client, done: impl Fn(&proto::DebugState) -> bool) {
    let until = Instant::now() + Duration::from_secs(2);
    while Instant::now() < until {
        match client.debug_state() {
            Ok(d) if done(&d) => return,
            Ok(_) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => return,
        }
    }
}

pub fn pause(kit: &Toolkit, _: PauseParams) -> ToolResult<Output> {
    let mut client = connect_for_write(kit)?;
    need_debug(&client)?;
    let done = client.control(proto::Control::Pause)?;
    // In a menu or while loading there is no level frame to stop in: the
    // pause stays pending, and the state says so.
    settle(&mut client, |d| d.stop.is_some());
    Ok(Output::Json(json!({
        "done": done.note,
        "state": state_json(kit, &mut client)?,
        "note": "the game stops in its next level frame and stays stopped until `resume`, \
                 or by itself after the hold limit",
    })))
}

pub fn resume(kit: &Toolkit, p: ResumeParams) -> ToolResult<Output> {
    let mut client = connect_for_write(kit)?;
    need_debug(&client)?;
    let control = if p.step.unwrap_or(false) {
        proto::Control::Step
    } else {
        proto::Control::Continue
    };
    let before = client.debug_state()?.stop.map(|s| (s.frame, s.seq));
    let done = client.control(control)?;
    if p.step.unwrap_or(false) {
        // Stopped again, one frame on.
        settle(&mut client, |d| {
            d.stop.as_ref().map(|s| (s.frame, s.seq)) != before && d.stop.is_some()
        });
    } else {
        settle(&mut client, |d| d.stop.is_none());
    }
    Ok(Output::Json(json!({
        "done": done.note,
        "state": state_json(kit, &mut client)?,
    })))
}

pub fn breakpoints(kit: &Toolkit, p: BreakpointsParams) -> ToolResult<Output> {
    let mut client = connect_for_write(kit)?;
    need_debug(&client)?;
    let once = p.once.unwrap_or(false);
    let clear = p.clear.unwrap_or(false);
    let target =
        p.to.as_deref()
            .map(|t| proto::parse_guid(t).ok_or_else(|| ToolError(format!("`{t}` is not a GUID"))))
            .transpose()?;
    if target.is_some() && p.messages.is_empty() {
        return fail("`to` narrows `messages`: give the message too");
    }
    let mut rules: Vec<proto::BreakRule> = p
        .messages
        .iter()
        .map(|m| proto::BreakRule {
            on: proto::BreakOn::Message {
                hash: MessageId::parse(m).hash(),
                receiver: target,
            },
            once,
        })
        .chain(p.prints.iter().map(|t| proto::BreakRule {
            on: proto::BreakOn::Output { text: t.clone() },
            once,
        }))
        .collect();
    if p.lua_error.unwrap_or(false) {
        rules.push(proto::BreakRule {
            on: proto::BreakOn::LuaError,
            once,
        });
    }
    if clear && !rules.is_empty() {
        return fail("`clear` removes every breakpoint: give it alone");
    }
    let hold = p.hold_s.unwrap_or(0).min(proto::debug::HOLD_MAX_S);
    let mut changed = false;
    if !rules.is_empty() || clear || hold != 0 {
        if p.add.unwrap_or(false) || (rules.is_empty() && !clear) {
            let mut old = client.debug_state()?.rules;
            old.extend(rules);
            rules = old;
        }
        if rules.len() > proto::debug::MAX_RULES {
            return fail(format!(
                "{} breakpoints; the module keeps {}",
                rules.len(),
                proto::debug::MAX_RULES
            ));
        }
        client.set_breaks(rules, hold)?;
        changed = true;
    }
    let mut waited = None;
    if let Some(wait) = p.wait_s {
        let until = Instant::now() + Duration::from_secs(wait.min(120));
        let mut stopped = client.debug_state()?.stop.is_some();
        while !stopped && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(200));
            stopped = client.debug_state()?.stop.is_some();
        }
        waited = Some(stopped);
    }
    Ok(Output::Json(json!({
        "changed": changed,
        "stopped_while_waiting": waited,
        "state": state_json(kit, &mut client)?,
    })))
}

// ------------------------------------------------------------ the free camera

pub fn camera(kit: &Toolkit, p: CameraParams) -> ToolResult<Output> {
    let mut client = connect_for_write(kit)?;
    need_debug(&client)?;
    if p.release.unwrap_or(false) {
        if p.position.is_some() || p.look_at.is_some() || p.yaw_degrees.is_some() {
            return fail("`release` gives the camera back: give it alone");
        }
        let done = client.set_camera(None)?;
        return Ok(Output::Json(
            json!({ "done": done.note, "free_camera": false }),
        ));
    }
    if p.look_at.is_some() && (p.yaw_degrees.is_some() || p.pitch_degrees.is_some()) {
        return fail("give `look_at` or `yaw_degrees`/`pitch_degrees`, not both");
    }
    let now = client
        .camera()?
        .ok_or_else(|| ToolError("the game has no camera now (not in a level)".into()))?;
    let at = p.position.unwrap_or(now.pose.position);
    let (yaw, pitch) = match p.look_at {
        Some(target) => {
            let d = [target[0] - at[0], target[1] - at[1], target[2] - at[2]];
            let flat = (d[0] * d[0] + d[2] * d[2]).sqrt();
            if flat < 1e-6 && d[1].abs() < 1e-6 {
                return fail("`look_at` is where the camera stands");
            }
            (
                Some(d[0].atan2(d[2]).to_degrees()),
                Some(d[1].atan2(flat).to_degrees()),
            )
        }
        None => (p.yaw_degrees, p.pitch_degrees),
    };
    let view = free_camera(now, Some(at), yaw, pitch);
    let done = client.set_camera(Some(view))?;
    Ok(Output::Json(json!({
        "done": done.note,
        "free_camera": true,
        "position": view.pose.position,
        "forward": view.pose.forward,
        "fov_degrees": view.fov,
        "note": "the game draws from here until `camera` with `release: true`, with the field \
                 of view of its own camera; the player stays where he is. `screenshot` shows \
                 the view",
    })))
}

// ------------------------------------------------------------ textures

pub fn push_textures(kit: &Toolkit, p: PushTexturesParams) -> ToolResult<Output> {
    let want = p.name.as_deref().map(super::parse_hash).transpose()?;
    let Some(ws) = kit.workspace()? else {
        return fail("there is no workspace: no edited texture to send");
    };
    let textures = live::hot_textures(&ws, want)?;
    if textures.is_empty() {
        return fail(if want.is_some() {
            "the workspace edits no texture with this name"
        } else {
            "the workspace edits no texture (import_texture records one)"
        });
    }
    let mut client = connect_for_write(kit)?;
    need_debug(&client)?;
    let mut failed = 0;
    let sent: Vec<Value> = textures
        .into_iter()
        .map(|t| match client.texture(t.name, t.data) {
            Ok(done) => json!({
                "texture": format!("{:#010x}", t.name),
                "stream": t.stream,
                "done": done.note,
            }),
            Err(e) => {
                failed += 1;
                json!({
                    "texture": format!("{:#010x}", t.name),
                    "stream": t.stream,
                    "error": e.to_string(),
                })
            }
        })
        .collect();
    Ok(Output::Json(json!({
        "textures": sent,
        "failed": failed,
        "note": "a texture is replaced when the game has it loaded and the new one has its \
                 size, format and levels; the game files are not changed",
    })))
}

// ------------------------------------------------------------ crash reports

pub fn crash_reports(kit: &Toolkit, p: CrashReportsParams) -> ToolResult<Output> {
    let root = kit.game.root();
    let reports = crash::reports(root);
    let Some(which) = p.report.as_deref() else {
        let module = match crash::installed(root) {
            crash::Installed::Ours => "installed by the toolkit",
            crash::Installed::Foreign => "a file the toolkit did not install (left alone)",
            crash::Installed::No => "not installed",
        };
        return Ok(Output::Json(json!({
            "reports": reports.iter().take(100).map(crash::CrashReport::json).collect::<Vec<_>>(),
            "total": reports.len(),
            "folder": crash::dir(root),
            "module": module,
            "switched_off_by_the_player": crash::disabled(root),
            "note": "give `report` (1 is the newest) for the text; kb_search with an address \
                     tells what the knowledge base knows about it. Whether the module is in \
                     the game is the player's choice, made in the mod manager",
        })));
    };
    let count = reports.len();
    let report = crash::pick(reports, Some(which)).ok_or_else(|| {
        ToolError(format!(
            "no report `{which}` ({count} report(s); call without `report` for the list)"
        ))
    })?;
    let text = std::fs::read_to_string(&report.path)
        .map_err(|e| ToolError(format!("cannot read {}: {e}", report.path.display())))?;
    let text = crash::annotate(&text);
    let chars: Vec<char> = text.chars().collect();
    let offset = p.offset.unwrap_or(0).min(chars.len());
    let max = p.max_chars.unwrap_or(12_000).clamp(200, 60_000);
    let end = (offset + max).min(chars.len());
    Ok(Output::Json(json!({
        "summary": report.json(),
        "text": chars[offset..end].iter().collect::<String>(),
        "next_offset": (end < chars.len()).then_some(end),
        "total_chars": chars.len(),
    })))
}

// ------------------------------------------------------------ copies of the game

pub fn detect_games(kit: &Toolkit, _: DetectGamesParams) -> ToolResult<Output> {
    let installs = install::detect(&SearchRoots::from_env());
    let served = std::fs::canonicalize(kit.game.root()).ok();
    let found: Vec<Value> = installs
        .iter()
        .map(|i| {
            let edition = Edition::identify(&i.path);
            let here = std::fs::canonicalize(&i.path).ok();
            json!({
                "install": i,
                "edition": edition,
                "summary": edition.summary(),
                "served_by_this_server": here.is_some() && here == served,
            })
        })
        .collect();
    let this = Edition::identify(kit.game.root());
    Ok(Output::Json(json!({
        "games": found,
        "this_game": {
            "path": kit.game.root(),
            "edition": this.summary(),
        },
        "note": "searched Steam libraries, Wine and Proton prefixes, Lutris, Heroic and \
                 Bottles; the server serves the one game it was started with",
    })))
}

// ------------------------------------------------------------ play: a point here

/// A new `PlayerSpawn` at `at` in level stream `stream`, recorded in the
/// workspace (what the editor's "Make a point here and play" does).
/// Returns its GUID for `-playerspawn`.
pub fn spawn_here(kit: &Toolkit, stream: &str, at: [f32; 3]) -> ToolResult<(String, Value)> {
    if !kit.allow_edits {
        return fail(
            "`position` records a new spawn point in the workspace: the server needs \
             --allow-edits for it (or give `spawn`, an existing point)",
        );
    }
    let points = start::spawn_points(&kit.game, stream)?;
    let Some(template) = start::spawn_template(&points) else {
        return fail(format!(
            "{stream} has no PlayerSpawn without a message to copy; give `spawn` instead"
        ));
    };
    let taken = kit.object_guids();
    let mut access = kit.write_workspace(&format!("play: a spawn point in {stream}"))?;
    let ws = &mut access.ws;
    let (object, guid) = start::copy_spawn(&kit.game, ws, template, at, &|g| taken.contains(g))?;
    let info = json!({
        "guid": guid.to_string(),
        "stream": template.stream,
        "object": format!("{}#{}:{object}", template.stream, template.resource),
        "copied_from": template.guid.to_string(),
        "position": at,
        "note": "recorded in the workspace (undo takes it back); the game gets it with the \
                 workspace of this launch",
    });
    Ok((guid.to_string(), info))
}

//! Writing into the game and running it (`--allow-game`): `apply` and
//! `unapply` (the workspace into the game files and back, with the backups
//! of the workspace), `play` and `stop` (the game on an X server of its
//! own, see [`crate::game`]), `screenshot` (a frame of that X server).

use std::time::{Duration, Instant};

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use doctor_toolkit::install::{self, SearchRoots};
use doctor_toolkit::live::{self, Client, ModuleInstall};
use doctor_toolkit::play::dxvk::{self, DxvkInstall};
use doctor_toolkit::play::overlay::{self, Choice, Overlay, OverlayMode, Source};
use doctor_toolkit::play::start::{self, CmdFile, StartOptions};
use doctor_toolkit::play::{self, LaunchMethod, PlayConfig, SessionState};
use doctor_toolkit::workspace::{ApplyOptions, ApplyReport, StreamAction, Workspace};

use super::Output;
use crate::game::{Display, GameSession, XServer};
use crate::state::{ToolError, ToolResult, Toolkit, fail};

/// Size of the game's screen when not given (as the editor's game tab).
pub const DEFAULT_SIZE: [u32; 2] = [1280, 720];

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ApplyParams {
    /// Only these streams (as resource keys name them); all when not given.
    #[serde(default)]
    pub streams: Vec<String>,
    /// Overwrite streams changed outside the workspace (their foreign changes are lost).
    #[serde(default)]
    pub force: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct UnapplyParams {
    /// Put back streams changed outside the workspace too.
    #[serde(default)]
    pub force: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct PlayParams {
    /// Start right in this level stream, without menus, profile or save:
    /// `gf2/cuba`, `gf2/miami`, `gf2/queens`, `mp_mall/mp_mall` (one
    /// top-level stream). The usual start (menus) when not given.
    #[serde(default)]
    pub stream: Option<String>,
    /// With `stream`: appear at this PlayerSpawn object (its GUID, 32
    /// hexadecimal digits; `level_objects` with filter `PlayerSpawn` lists them).
    #[serde(default)]
    pub spawn: Option<String>,
    /// Draw with the graphics card (DXVK) under Wine: about 30 frames a
    /// second instead of 1–2 [default: as the editor's Play options say].
    #[serde(default)]
    pub dxvk: Option<bool>,
    /// Give the game the workspace for this launch (default true): through
    /// the live module without writing into the game folder when it can,
    /// else written into the game and taken out again when the game ends
    /// (see `write_game`).
    #[serde(default)]
    pub apply: Option<bool>,
    /// Give the game the enabled mods of the mod manager instead of the
    /// workspace, through the live module (never written into the game).
    #[serde(default)]
    pub mods: Option<bool>,
    /// How the workspace and `cmd.txt` reach the game: `false` — only
    /// through the live module from staged files, the game folder is not
    /// written (fails when the module cannot be used); `true` — written into
    /// the game folder and put back afterwards; not given — as the editor's
    /// Play options say (by default through the module when it is built,
    /// the game has the ASI loader and the edition fits).
    #[serde(default)]
    pub write_game: Option<bool>,
    /// Wait until a level is loaded (the live link sees the player), at
    /// most `timeout_s` (default true).
    #[serde(default)]
    pub wait_loaded: Option<bool>,
    /// Longest wait in seconds (default 240, at most 900).
    #[serde(default)]
    pub timeout_s: Option<u64>,
    /// Screen size `[width, height]` (default 1280×720).
    #[serde(default)]
    pub size: Option<[u32; 2]>,
    /// The graphics module for this launch (stage 59): time of day without
    /// lost seconds, vsync 60 frames a second instead of 30, the game's
    /// limiter sleeping [default: as the editor's Play options say]. The
    /// options below imply it.
    #[serde(default)]
    pub render: Option<bool>,
    /// Frame limit of the graphics module, frames a second (10–1000).
    #[serde(default)]
    pub fps: Option<u32>,
    /// Horizontal field of view of the player's camera at 16:9, degrees
    /// (40–120; the game's is 60).
    #[serde(default)]
    pub fov: Option<u32>,
    /// FXAA over the finished frame.
    #[serde(default)]
    pub fxaa: Option<bool>,
    /// Sharpening over the finished frame, 0–100.
    #[serde(default)]
    pub sharpen: Option<u32>,
    /// Start a copy of the executable with Large Address Aware from a
    /// mirror of the game folder (the game's executable is not written).
    #[serde(default)]
    pub laa: Option<bool>,
    /// With DXVK: the Vulkan driver the game loads (`auto`, `nvidia`,
    /// `radeon`, `intel`; empty — all).
    #[serde(default)]
    pub vulkan_driver: Option<String>,
}

impl PlayParams {
    /// The graphics of this launch over those of the editor.
    fn graphics(&self, editor: &play::render::GraphicsOptions) -> play::render::GraphicsOptions {
        let mut g = editor.clone();
        let implied = self.fps.is_some()
            || self.fov.is_some()
            || self.fxaa == Some(true)
            || self.sharpen.is_some_and(|s| s > 0);
        g.module = self.render.unwrap_or(g.module) || implied;
        if let Some(v) = self.fps {
            g.fps = v;
        }
        if let Some(v) = self.fov {
            g.fov = v;
        }
        if let Some(v) = self.fxaa {
            g.fxaa = v;
        }
        if let Some(v) = self.sharpen {
            g.sharpen = v;
        }
        if let Some(v) = self.laa {
            g.laa = v;
        }
        if let Some(v) = &self.vulkan_driver {
            g.vulkan_driver = v.clone();
        }
        g
    }
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct StopParams {}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ScreenshotParams {
    /// Widest picture in pixels (default 960; the frame is halved until it fits).
    #[serde(default)]
    pub max_width: Option<u32>,
}

// ------------------------------------------------------------ apply

fn report_json(report: &ApplyReport) -> Value {
    let streams: Vec<Value> = report
        .streams
        .iter()
        .map(|(s, a)| match a {
            StreamAction::Rebuilt(n) => json!({ "stream": s, "action": "rebuilt", "replaced": n }),
            StreamAction::Restored => json!({ "stream": s, "action": "restored" }),
        })
        .collect();
    json!({
        "streams": streams,
        "written": report.written,
        "backed_up": report.backed_up,
        "warnings": report.warnings,
    })
}

fn refuse_while_running(kit: &Toolkit) -> ToolResult<()> {
    // A stand-in launch (tests) does not look for the real game on the machine.
    let running = if kit.launch.is_some() {
        Vec::new()
    } else {
        play::game_processes()
    };
    if !running.is_empty() {
        return fail(format!(
            "the game is running (process {}): its files cannot be written now; `stop` it \
             (when this server started it) or close it",
            running
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let mut session = kit.session.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(s) = session.as_mut()
        && !matches!(
            s.state(),
            SessionState::Exited | SessionState::NotStarted(_)
        )
    {
        return fail("the game started by `play` is running: `stop` it first");
    }
    Ok(())
}

pub fn apply(kit: &Toolkit, p: ApplyParams) -> ToolResult<Output> {
    refuse_while_running(kit)?;
    if !Workspace::exists(&kit.workspace_dir) {
        return fail("there is no workspace: nothing to write into the game");
    }
    let mut access = kit.write_workspace("apply: write the workspace into the game files")?;
    let report = access.ws.apply(
        &kit.game,
        ApplyOptions {
            force: p.force.unwrap_or(false),
            streams: (!p.streams.is_empty()).then_some(p.streams),
        },
    )?;
    let mut out = report_json(&report);
    out["note"] = json!(
        "the original files are kept as backups: `unapply` puts them back, the edits stay in the workspace"
    );
    Ok(Output::Json(out))
}

pub fn unapply(kit: &Toolkit, p: UnapplyParams) -> ToolResult<Output> {
    refuse_while_running(kit)?;
    if !Workspace::exists(&kit.workspace_dir) {
        return fail("there is no workspace: nothing was written into the game by it");
    }
    let mut access = kit.write_workspace("unapply: put the original game files back")?;
    let report = access.ws.unapply(&kit.game, p.force.unwrap_or(false))?;
    let mut out = report_json(&report);
    out["changes_kept_in_workspace"] = json!(access.ws.len());
    Ok(Output::Json(out))
}

// ------------------------------------------------------------ play

/// The editor's Play options of this game (`settings.toml`), when it has any.
fn editor_play_config(kit: &Toolkit) -> Option<PlayConfig> {
    let path = config_dir()?.join("settings.toml");
    let text = std::fs::read_to_string(path).ok()?;
    let table: toml::Table = text.parse().ok()?;
    let games = table.get("games")?.as_table()?;
    let root = kit.game.root();
    let canonical = std::fs::canonicalize(root).ok();
    let (_, game) = games.iter().find(|(key, _)| {
        std::path::Path::new(key.as_str()) == root
            || canonical.is_some() && std::fs::canonicalize(key.as_str()).ok() == canonical
    })?;
    game.get("play")?.clone().try_into().ok()
}

/// The settings folder of the editor, as the editor finds it.
fn config_dir() -> Option<std::path::PathBuf> {
    let var = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(std::path::PathBuf::from)
    };
    if let Some(dir) = var("DOCTOR_TOOLKIT_CONFIG_DIR") {
        return Some(dir);
    }
    let base = if cfg!(windows) {
        var("APPDATA")
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|h| h.join("Library").join("Application Support"))
    } else {
        var("XDG_CONFIG_HOME").or_else(|| var("HOME").map(|h| h.join(".config")))
    };
    base.map(|b| b.join("doctor-toolkit"))
}

/// Why the game cannot start in `stream` (stage 32: one top-level stream).
fn start_problem(stream: &str) -> Option<String> {
    let depth = stream
        .replace('\\', "/")
        .split('/')
        .filter(|p| !p.is_empty())
        .count();
    (depth > 2).then(|| {
        format!(
            "`{stream}` is a child stream: the game starts only in a top-level one (`gf2/cuba`, \
             `gf2/miami`, `gf2/queens`, `mp_mall/mp_mall`); choose the spawn point there"
        )
    })
}

pub fn play(kit: &Toolkit, p: PlayParams) -> ToolResult<Output> {
    {
        let mut session = kit.session.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = session.as_mut() {
            if !matches!(
                s.state(),
                SessionState::Exited | SessionState::NotStarted(_)
            ) {
                return fail("the game started by `play` is running: `stop` it first");
            }
            // An ended launch: clean up after it before the next.
            let ended = session.take().expect("checked");
            drop(session);
            finish(kit, ended)?;
        }
    }
    if kit.launch.is_none() {
        refuse_while_running(kit)?;
    }
    let root = kit.game.root().to_path_buf();
    let size = p.size.unwrap_or(DEFAULT_SIZE);
    if size[0] < 320 || size[1] < 200 || size[0] > 3840 || size[1] > 2160 {
        return fail("`size` is between 320×200 and 3840×2160");
    }
    let stream = p.stream.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if let Some(problem) = stream.and_then(start_problem) {
        return fail(problem);
    }
    if p.spawn.is_some() && stream.is_none() {
        return fail("`spawn` needs `stream`");
    }
    if let Some(spawn) = &p.spawn {
        start::spawn_arg(spawn).map_err(|e| ToolError(format!("`spawn`: {e}")))?;
    }
    let options = StartOptions {
        streams: stream.map(str::to_owned).into_iter().collect(),
        spawn: p.spawn.clone(),
        windowed: None,
        resolution: Some((size[0], size[1])),
    };
    let editor = editor_play_config(kit).unwrap_or_default();
    let mut notes = Vec::new();

    // How to start it.
    let (mut plan, method) = match &kit.launch {
        Some(plan) => (plan.clone(), None),
        None => {
            let method = match &editor.method {
                LaunchMethod::Auto => {
                    play::auto_method(&root, &install::detect(&SearchRoots::from_env()))
                }
                m => m.clone(),
            };
            (play::plan(&method, &root)?, Some(method))
        }
    };
    if plan.detached {
        return fail(
            "the launch method of this game starts it through another program (Steam, a \
             launcher), which would show it on the desktop; choose `wine` (or `proton`, \
             `direct`) in the editor's Play options",
        );
    }
    if let Some(method) = &method {
        notes.push(format!("method: {}", method.describe()));
        if cfg!(unix) {
            play::in_virtual_desktop(&mut plan, size);
        }
    }
    let dxvk_found = if p.dxvk.unwrap_or(editor.dxvk) {
        match &method {
            Some(m) => match dxvk::check(m, &root) {
                Ok(found) => {
                    dxvk::apply_env(&mut plan, &root);
                    notes.push(format!("dxvk: {}", found.source));
                    Some(found)
                }
                Err(problem) => {
                    notes.push(format!("dxvk not used: {problem}"));
                    None
                }
            },
            None => None,
        }
    } else {
        None
    };

    // The X server first: when it cannot start, nothing was written.
    #[cfg(unix)]
    let display = {
        let server = kit.x_server.clone().or_else(XServer::find).ok_or_else(|| {
            ToolError(
                "Xvfb is needed: the game runs on an X server of its own so that nothing \
                 shows on the desktop (install the package xvfb / xorg-server-xvfb)"
                    .into(),
            )
        })?;
        Some(Display::start(&server, size).map_err(ToolError)?)
    };
    #[cfg(not(unix))]
    let display: Option<Display> = None;

    // Files of earlier launches that ended without cleaning up.
    if start::restore_stale(&root)? {
        notes.push(format!(
            "took away the {} of an earlier launch",
            start::CMD_FILE
        ));
    }
    if live::restore_stale(&root)? {
        notes.push("took away the live module of an earlier launch".into());
    }
    if dxvk::restore_stale(&root)? {
        notes.push(format!("took away the {} of an earlier launch", dxvk::DLL));
    }

    // Through the live module, without writing into the game (stage 46).
    let mods = p.mods.unwrap_or(false);
    let apply = p.apply.unwrap_or(true) && !mods && Workspace::exists(&kit.workspace_dir);
    let mode = match p.write_game {
        Some(true) => OverlayMode::Off,
        Some(false) => OverlayMode::On,
        None if mods => OverlayMode::On,
        None => editor.overlay,
    };
    if mods && mode == OverlayMode::Off {
        return fail(
            "`mods` gives the game the mods through the live module; `write_game` cannot be true with it (`mods apply` writes them)",
        );
    }
    let wants = apply || mods || !options.is_empty();
    let choice = if wants {
        overlay::choose(&root, mode).map_err(|e| ToolError(e.to_string()))?
    } else {
        Choice::Write(None)
    };
    if let Choice::Write(Some(why)) = &choice {
        notes.push(format!("the game folder is written for this launch: {why}"));
    }
    let mut staged = None;
    if choice == Choice::Overlay {
        if overlay::remove_stale(&root)? {
            notes.push("removed the staged files of an earlier launch".into());
        }
        let source = if mods {
            Some(Source::Mods)
        } else if apply {
            Some(Source::Workspace(kit.workspace_dir.clone()))
        } else {
            None
        };
        let dir = overlay::default_dir(&root)
            .ok_or_else(|| ToolError("no folder for the staged files (no HOME)".into()))?;
        let cmd_txt = if options.is_empty() {
            None
        } else {
            Some(options.cmd_txt()?)
        };
        let built =
            Overlay::build(&kit.game, source.as_ref(), cmd_txt.as_deref(), &dir).map_err(|e| {
                ToolError(format!(
                    "cannot stage the changes; the game was not started: {e}"
                ))
            })?;
        notes.push(format!(
            "{} file(s) given to the game by the live module from {}, nothing written into the game folder",
            built.redirects().len(),
            dir.display()
        ));
        staged = Some(built);
    }

    // The workspace into the game.
    let mut applied = None;
    if staged.is_none() && apply {
        let mut access =
            kit.write_workspace("play: write the workspace into the game before starting it")?;
        let report = access
            .ws
            .apply(&kit.game, ApplyOptions::default())
            .map_err(|e| {
                ToolError(format!(
                    "cannot write the workspace into the game; the game was not started: {e}"
                ))
            })?;
        applied = Some(report_json(&report));
    }
    let revert_after = applied.is_some();
    // From here on a failure takes back what was written.
    let redirects = staged
        .as_ref()
        .map(|o| o.redirects().to_vec())
        .unwrap_or_default();
    let mut cleanup = Cleanup {
        kit,
        cmd_file: None,
        live: None,
        dxvk: None,
        overlay: staged,
        revert: revert_after,
    };
    if !options.is_empty() && cleanup.overlay.is_none() {
        cleanup.cmd_file = Some(CmdFile::write(&root, &options)?);
    }
    cleanup.live = install_live(&root, kit.allow_game, redirects, &mut notes)?;
    if cleanup.overlay.is_some() && cleanup.live.is_none() {
        return fail(
            "the live module could not be installed, and without it the game would not get the staged files",
        );
    }
    if let Some(found) = &dxvk_found {
        cleanup.dxvk = Some(DxvkInstall::write(&root, found)?);
    }
    // Graphics last: with LAA the mirror links everything put above.
    let graphics = p.graphics(&editor.graphics);
    let graphics_launch = match &method {
        Some(method) if graphics.module || graphics.laa || !graphics.vulkan_driver.is_empty() => {
            let vulkan = dxvk_found.as_ref().map(|_| dxvk::vulkan32());
            let g =
                play::render::prepare(&kit.game, method, &mut plan, &graphics, vulkan.as_ref())?;
            notes.extend(g.notes.iter().cloned());
            Some(g)
        }
        _ => None,
    };
    let mut session = GameSession::spawn(&plan, display)?;
    session.graphics = graphics_launch;
    session.own_processes_only = kit.launch.is_some();
    session.start = options;
    session.cmd_file = cleanup.cmd_file.take();
    session.live = cleanup.live.take();
    session.dxvk = cleanup.dxvk.take();
    session.overlay = cleanup.overlay.take();
    session.revert_after = revert_after;
    cleanup.revert = false;
    session.notes = notes;
    let endpoint = session.endpoint();
    *kit.session.lock().unwrap_or_else(|e| e.into_inner()) = Some(session);

    let mut waited = json!(null);
    if p.wait_loaded.unwrap_or(true) {
        let timeout = Duration::from_secs(p.timeout_s.unwrap_or(240).clamp(1, 900));
        waited = wait_loaded(kit, endpoint.as_ref(), timeout);
    }
    Ok(Output::Json(json!({
        "started": true,
        "game": super::live::session_info(kit),
        "workspace_applied": applied,
        "through_live_module": session_overlay(kit),
        "wait": waited,
        "next": "screenshot shows the screen; live_status, player and objects_near read the game; \
                 teleport, send_message and lua act in it; stop ends it and takes back what play wrote",
    })))
}

/// What `play` wrote, taken back when it fails before the game starts.
struct Cleanup<'a> {
    kit: &'a Toolkit,
    cmd_file: Option<CmdFile>,
    live: Option<ModuleInstall>,
    dxvk: Option<DxvkInstall>,
    overlay: Option<Overlay>,
    revert: bool,
}

impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        if let Some(f) = self.cmd_file.take() {
            let _ = f.restore();
        }
        if let Some(m) = self.live.take() {
            let _ = m.restore();
        }
        if let Some(d) = self.dxvk.take() {
            let _ = d.restore();
        }
        if self.revert
            && let Ok(mut access) = self
                .kit
                .write_workspace("play: take the workspace out of the game again")
        {
            let _ = access.ws.unapply(&self.kit.game, false);
        }
    }
}

/// The live module for this launch (writes on: the server runs with
/// `--allow-game`; it also lets the player move after a start straight in
/// a level), or nothing when the game has no ASI loader.
/// Whether the running launch gets its files through the live module, and
/// how many.
fn session_overlay(kit: &Toolkit) -> Value {
    let session = kit.session.lock().unwrap_or_else(|e| e.into_inner());
    match session.as_ref().and_then(|s| s.overlay.as_ref()) {
        Some(o) => json!({ "files": o.redirects().len(), "staged_in": o.dir() }),
        None => json!(null),
    }
}

fn install_live(
    root: &std::path::Path,
    writes: bool,
    redirects: Vec<doctor_toolkit::live::proto::files::Redirect>,
    notes: &mut Vec<String>,
) -> ToolResult<Option<ModuleInstall>> {
    let Some(path) = live::module_binary() else {
        notes
            .push("the live module is not built (tools/build-live-module.sh): no live link".into());
        return Ok(None);
    };
    if live::loader::find(root).is_none() {
        notes.push(
            "no ASI loader (dinput8.dll) in the game folder: no live link, and after a start \
             straight in a level the player cannot move"
                .into(),
        );
        return Ok(None);
    }
    let bytes = std::fs::read(&path).map_err(|e| ToolError(format!("{}: {e}", path.display())))?;
    let config = live::Config {
        port: live::free_port(),
        token: live::new_token(),
        writes,
        fix_input: true,
        // The game's output, Lua errors and messages for the assistant.
        journal: true,
        files_log: false,
        redirects,
    };
    let install = ModuleInstall::write(root, &bytes, &config)?;
    notes.push(format!(
        "live module installed for this launch, port {}, writes {}",
        config.port,
        if writes { "on" } else { "off" }
    ));
    Ok(Some(install))
}

/// Waits until the live link sees the player (a level is loaded), the
/// game ends, or `timeout` passes.
fn wait_loaded(kit: &Toolkit, endpoint: Option<&live::Endpoint>, timeout: Duration) -> Value {
    let started = Instant::now();
    let endpoint = endpoint.cloned().or_else(|| kit.live.clone());
    let mut last = "the game is starting".to_owned();
    while started.elapsed() < timeout {
        {
            let mut session = kit.session.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(s) = session.as_mut() {
                match s.state() {
                    SessionState::Exited => {
                        return json!({ "loaded": false, "why": "the game exited", "seconds": started.elapsed().as_secs() });
                    }
                    SessionState::NotStarted(why) => {
                        return json!({ "loaded": false, "why": why, "seconds": started.elapsed().as_secs() });
                    }
                    _ => {}
                }
            }
        }
        let Some(endpoint) = &endpoint else {
            // No live link: the start can only be waited for.
            std::thread::sleep(Duration::from_secs(2).min(timeout));
            return json!({
                "loaded": null,
                "why": "no live link to tell when the level is loaded; take a screenshot to see",
                "seconds": started.elapsed().as_secs(),
            });
        };
        if let Ok(mut client) =
            Client::connect(endpoint, "doctor-toolkit mcp", Duration::from_secs(3))
            && let Ok(status) = client.status()
        {
            last = format!("main loop {}", status.state.name());
            if status.player {
                return json!({
                    "loaded": true,
                    "loop": status.state.name(),
                    "seconds": started.elapsed().as_secs(),
                });
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    json!({
        "loaded": false,
        "why": format!("not loaded within {} s ({last}); the game keeps running", timeout.as_secs()),
        "seconds": started.elapsed().as_secs(),
    })
}

/// Ends a launch: the game and its X server, then what `play` wrote into
/// the game folder, then the workspace taken out of the game.
pub fn finish(kit: &Toolkit, mut session: GameSession) -> ToolResult<Value> {
    let killed = session.kill();
    let mut done = Vec::new();
    if killed {
        done.push("the game did not end after SIGTERM and was killed".to_owned());
    }
    if let Some(f) = session.cmd_file.take()
        && f.restore()?
    {
        done.push(format!("took away {}", start::CMD_FILE));
    }
    // Before the live module: the LAA mirror hands its logs back first.
    if let Some(g) = session.graphics.take() {
        done.extend(g.finish(kit.game.root())?);
    }
    if let Some(m) = session.live.take()
        && m.restore()?
    {
        done.push("took away the live module".into());
    }
    if let Some(d) = session.dxvk.take()
        && d.restore()?
    {
        done.push(format!("took away {}", dxvk::DLL));
    }
    if let Some(o) = session.overlay.take() {
        o.remove()?;
        done.push(
            "removed the files staged for the live module; the game folder was not written".into(),
        );
    }
    let mut reverted = json!(null);
    if session.revert_after {
        session.revert_after = false;
        let mut access = kit.write_workspace("stop: take the workspace out of the game")?;
        let report = access.ws.unapply(&kit.game, false)?;
        reverted = report_json(&report);
    }
    Ok(json!({
        "stopped": true,
        "ran_seconds": session.started.elapsed().as_secs(),
        "done": done,
        "workspace_taken_out_of_the_game": reverted,
    }))
}

pub fn stop(kit: &Toolkit, _: StopParams) -> ToolResult<Output> {
    let session = kit.session.lock().unwrap_or_else(|e| e.into_inner()).take();
    let Some(session) = session else {
        return fail("no game was started by `play` (a game started elsewhere is closed there)");
    };
    Ok(Output::Json(finish(kit, session)?))
}

pub fn screenshot(kit: &Toolkit, p: ScreenshotParams) -> ToolResult<Output> {
    let display = {
        let mut session = kit.session.lock().unwrap_or_else(|e| e.into_inner());
        let Some(s) = session.as_mut() else {
            return fail(
                "no game was started by `play`: screenshots are taken of the X server it runs \
                 the game on",
            );
        };
        if matches!(
            s.state(),
            SessionState::Exited | SessionState::NotStarted(_)
        ) {
            return fail("the game has ended; `play` starts it again");
        }
        s.display.as_ref().map(Display::name).ok_or_else(|| {
            ToolError("screenshots need the game on an X server of its own (Linux)".into())
        })?
    };
    #[cfg(unix)]
    {
        let max_width = p.max_width.unwrap_or(960).clamp(64, 3840);
        let (png, size, mean) = crate::game::screenshot(&display, max_width).map_err(ToolError)?;
        Ok(Output::Image {
            png,
            info: json!({
                "display": display,
                "width": size[0],
                "height": size[1],
                "mean_brightness": (mean * 10.0).round() / 10.0,
                "note": if mean < 2.0 { "the frame is (almost) black: the game may still be loading" } else { "" },
            }),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = (display, p);
        fail("screenshots are only taken on Linux")
    }
}

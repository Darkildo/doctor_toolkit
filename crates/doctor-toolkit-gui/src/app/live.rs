//! The live link in the game tab (stage 34): the "Link" panel — state,
//! player and camera of the running game, objects nearby, teleport,
//! messages and Lua — and the marks of the player and the camera in level
//! tabs. Stage 49: the game's journal (prints, Lua errors with their call
//! stack, messages of the bus) in the panel and in the editor's Log, every
//! object of the game (logic too) and a search by GUID or name — names come
//! from the SimGroups of the game and its workspace.
//!
//! Stage 35: breakpoints, pause and step, the free camera and textures
//! without a restart (`super::live_debug`); messages of the game for the
//! "Live" layer of logic tabs, the camera of the game for level tabs.
//!
//! A background thread talks to the module in the game
//! (`doctor_toolkit::live::Client`): it reconnects every 2 s while the
//! game starts, polls state, player and camera four times a second and
//! runs the actions the panel asks for. Writes go out only after the
//! confirmation window.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::RichText;

use doctor_toolkit::live::names::{self, LiveNames};
use doctor_toolkit::live::proto::journal::{EventBody, mask};
use doctor_toolkit::live::proto::{
    self, CameraView, Event, Guid, Hello, MessageId, ObjectInfo, Pose, Status, World, WorldFilter,
};
use doctor_toolkit::live::{self, Client, Endpoint, LiveOptions, ModuleInstall, loader::Loader};
use doctor_toolkit_mods::i18n::{tr, trf};

use super::{BrowserApp, Level};

/// Player and camera of the game for the level tabs: position and the
/// direction they face.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LiveMarks {
    pub player: Option<([f32; 3], [f32; 3])>,
    pub camera: Option<([f32; 3], [f32; 3])>,
    /// The game's camera as it is (stage 35: the level view follows it).
    pub game_camera: Option<CameraView>,
    /// The module has the free camera (protocol 3) and writes are on.
    pub can_free_camera: bool,
    /// The free camera is on.
    pub free_camera: bool,
}

/// What the thread last learnt.
#[derive(Default)]
pub(super) struct Shared {
    connected: bool,
    hello: Option<Hello>,
    status: Option<Status>,
    player: Option<Pose>,
    camera: Option<CameraView>,
    error: Option<String>,
    /// Result of the last action: success and text.
    result: Option<(bool, String)>,
    objects: Vec<ObjectInfo>,
    streams: Vec<String>,
    lua: Vec<String>,
    busy: bool,
    /// The module has the journal and every object (protocol 2).
    journal_ok: bool,
    /// Kinds the module records.
    recording: u8,
    /// The latest events of the journal, oldest first.
    journal: VecDeque<Event>,
    /// Events not yet copied into the editor's Log.
    fresh: Vec<Event>,
    /// Events lost since the link came up.
    lost: u64,
    /// The last answer of "All objects" or a search.
    world: Option<World>,
    /// The module has breakpoints and the rest of stage 35 (protocol 3).
    pub(super) debug_ok: bool,
    /// Breakpoints and whether the game stands.
    pub(super) debug: Option<proto::DebugState>,
    /// Message events not yet given to the logic tabs.
    pub(super) trace: Vec<Event>,
    /// Prints and Lua errors not yet given to the script editor.
    pub(super) script_events: Vec<Event>,
}

/// Events the panel keeps.
const JOURNAL_KEEP: usize = 2000;

/// An action for the thread.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Cmd {
    Teleport([f32; 3], Option<f32>),
    Send(MessageId, Option<Guid>),
    Lua(String),
    Objects(f32),
    Streams,
    World(WorldFilter),
    /// Stage 35: every breakpoint and the hold limit.
    Break(Vec<proto::BreakRule>, u16),
    Control(proto::Control),
    /// The free camera (a write).
    Camera(Option<CameraView>),
    /// Edited textures of the workspace into the game (a write).
    Textures(Vec<live::HotTexture>),
}

impl Cmd {
    pub(super) fn writes(&self) -> bool {
        matches!(
            self,
            Self::Teleport(..)
                | Self::Send(..)
                | Self::Lua(_)
                | Self::Camera(Some(_))
                | Self::Textures(_)
        )
    }
}

/// The live link of the editor.
pub(super) struct LiveState {
    shared: Arc<Mutex<Shared>>,
    tx: Option<Sender<Cmd>>,
    stop: Arc<AtomicBool>,
    endpoint: Option<Endpoint>,
    /// The module installed for the running game.
    pub(super) module: Option<ModuleInstall>,
    teleport: [String; 3],
    yaw: String,
    message: String,
    target: String,
    lua: String,
    radius: f32,
    pub(super) confirm: Option<Cmd>,
    /// The ASI loader of the open game, looked up once.
    loader: Option<(PathBuf, Option<Loader>)>,
    /// Kinds of events the panel asks for ([`mask`]).
    kinds: Arc<AtomicU8>,
    /// Filter of the journal and of the list of objects.
    journal_filter: String,
    /// "All objects": the list of every object instead of those nearby.
    all_objects: bool,
    /// Only objects without a position (logic).
    logic_only: bool,
    /// Search by GUID or name.
    find: String,
    /// Names of objects and messages for the open game (loaded in the
    /// background once the link is up).
    names: Arc<Mutex<NamesState>>,
    /// A Lua error was shown as a warning (once per link).
    warned_lua: bool,
    /// Kinds of events asked for besides those of the panel: messages while
    /// a logic tab follows the game (stage 35).
    pub(super) extra_kinds: Arc<AtomicU8>,
    /// Breakpoints being written in the panel.
    pub(super) break_message: String,
    pub(super) break_target: String,
    pub(super) break_print: String,
    pub(super) hold_s: u16,
}

/// The index of names for the live link.
#[derive(Default)]
enum NamesState {
    #[default]
    None,
    Loading(PathBuf),
    Ready(PathBuf, Arc<LiveNames>),
    Failed(PathBuf, String),
}

impl Default for LiveState {
    fn default() -> Self {
        Self {
            shared: Arc::default(),
            tx: None,
            stop: Arc::new(AtomicBool::new(false)),
            endpoint: None,
            module: None,
            teleport: Default::default(),
            yaw: String::new(),
            message: String::new(),
            target: String::new(),
            lua: "return GetSimTime()".to_owned(),
            radius: 30.0,
            confirm: None,
            loader: None,
            kinds: Arc::new(AtomicU8::new(mask::OUTPUT | mask::LUA_ERROR)),
            journal_filter: String::new(),
            all_objects: false,
            logic_only: false,
            find: String::new(),
            names: Arc::default(),
            warned_lua: false,
            extra_kinds: Arc::new(AtomicU8::new(0)),
            break_message: String::new(),
            break_target: String::new(),
            break_print: String::new(),
            hold_s: proto::debug::HOLD_DEFAULT_S,
        }
    }
}

fn shared(s: &Mutex<Shared>) -> std::sync::MutexGuard<'_, Shared> {
    s.lock().unwrap_or_else(|e| e.into_inner())
}

/// What the thread learnt, for the debugging glue (stage 35).
pub(super) fn shared_debug(state: &LiveState) -> std::sync::MutexGuard<'_, Shared> {
    shared(&state.shared)
}

impl Shared {
    /// Events of the journal lost since the link came up.
    pub(super) fn lost_total(&self) -> u64 {
        self.lost
    }
}

fn run(client: &mut Client, cmd: Cmd, s: &Mutex<Shared>) -> Result<String, live::LinkError> {
    Ok(match cmd {
        Cmd::Teleport(p, yaw) => client.teleport(p, yaw)?.note,
        Cmd::Send(m, target) => {
            let d = client.send(m, target)?;
            trf("live.sent", &[("count", &d.count), ("note", &d.note)])
        }
        Cmd::Lua(code) => {
            let r = client.lua(&code)?;
            let text = if r.ok {
                tr("live.lua_ok").to_owned()
            } else {
                tr("live.lua_failed").to_owned()
            };
            shared(s).lua = r.values;
            if !r.ok {
                return Err(live::LinkError::Refused(proto::Failure {
                    code: proto::ErrorCode::Failed,
                    message: text,
                }));
            }
            text
        }
        Cmd::Objects(radius) => {
            let list = client.objects(None, radius, None, 200)?;
            let n = list.len();
            shared(s).objects = list;
            trf("live.objects_found", &[("count", &n)])
        }
        Cmd::Streams => {
            let list = client.streams()?;
            let n = list.len();
            shared(s).streams = list.into_iter().map(|s| s.name).collect();
            trf("live.streams_found", &[("count", &n)])
        }
        Cmd::World(filter) => {
            let world = client.world(filter, proto::world::MAX_WORLD_OBJECTS)?;
            let text = trf(
                "live.world_found",
                &[("count", &world.total), ("walked", &world.walked)],
            );
            shared(s).world = Some(world);
            text
        }
        Cmd::Break(rules, hold) => client.set_breaks(rules, hold)?.note,
        Cmd::Control(c) => client.control(c)?.note,
        Cmd::Camera(c) => client.set_camera(c)?.note,
        Cmd::Textures(list) => {
            let mut done = 0;
            let mut errors = Vec::new();
            for t in list {
                match client.texture(t.name, t.data) {
                    Ok(_) => done += 1,
                    Err(e) => errors.push(format!("{:#010x}: {e}", t.name)),
                }
            }
            if !errors.is_empty() {
                return Err(live::LinkError::Refused(proto::Failure {
                    code: proto::ErrorCode::Failed,
                    message: trf(
                        "debug.textures_failed",
                        &[("done", &done), ("errors", &errors.join("; "))],
                    ),
                }));
            }
            trf("debug.textures_done", &[("count", &done)])
        }
    })
}

/// Reads new events of the journal into `s`.
fn poll_journal(
    client: &mut Client,
    next: &mut u64,
    kinds: u8,
    s: &Mutex<Shared>,
) -> Result<bool, live::LinkError> {
    let b = client.events(*next, kinds, 500)?;
    *next = b.next.saturating_sub(1);
    let mut sh = shared(s);
    sh.recording = b.recording;
    sh.lost += b.lost;
    let any = !b.events.is_empty();
    for e in b.events {
        match e.body {
            EventBody::Message { .. } | EventBody::MessageCounts { .. } => {
                sh.trace.push(e.clone());
            }
            _ => sh.script_events.push(e.clone()),
        }
        sh.fresh.push(e.clone());
        sh.journal.push_back(e);
    }
    let excess = sh.trace.len().saturating_sub(4 * JOURNAL_KEEP);
    sh.trace.drain(..excess);
    let excess = sh.script_events.len().saturating_sub(4 * JOURNAL_KEEP);
    sh.script_events.drain(..excess);
    let excess = sh.journal.len().saturating_sub(JOURNAL_KEEP);
    sh.journal.drain(..excess);
    let fresh_excess = sh.fresh.len().saturating_sub(JOURNAL_KEEP);
    sh.fresh.drain(..fresh_excess);
    Ok(any)
}

fn worker(
    endpoint: Endpoint,
    s: Arc<Mutex<Shared>>,
    rx: Receiver<Cmd>,
    stop: Arc<AtomicBool>,
    kinds: Arc<AtomicU8>,
    extra_kinds: Arc<AtomicU8>,
    ctx: egui::Context,
) {
    let mut client: Option<Client> = None;
    // Number of the last event read; a new connection reads all the module
    // kept.
    let mut next = 0u64;
    let mut last_try: Option<Instant> = None;
    while !stop.load(Ordering::Relaxed) {
        if client.is_none() && last_try.is_none_or(|t| t.elapsed() > Duration::from_secs(2)) {
            last_try = Some(Instant::now());
            match Client::connect(
                &endpoint,
                concat!("doctor-toolkit editor ", env!("CARGO_PKG_VERSION")),
                Duration::from_secs(3),
            ) {
                Ok(c) => {
                    let mut sh = shared(&s);
                    sh.connected = true;
                    sh.hello = Some(c.hello().clone());
                    sh.journal_ok = c.has_journal();
                    sh.debug_ok = c.has_debug();
                    sh.error = None;
                    client = Some(c);
                    next = 0;
                }
                Err(e) => {
                    let mut sh = shared(&s);
                    sh.connected = false;
                    sh.error = Some(e.to_string());
                }
            }
            ctx.request_repaint();
        }
        let mut broken = false;
        while let Ok(cmd) = rx.try_recv() {
            let Some(c) = client.as_mut() else {
                shared(&s).result = Some((false, tr("live.not_connected").to_owned()));
                continue;
            };
            shared(&s).busy = true;
            ctx.request_repaint();
            c.set_timeout(Duration::from_secs(40));
            let r = run(c, cmd, &s);
            c.set_timeout(Duration::from_secs(3));
            let mut sh = shared(&s);
            sh.busy = false;
            match r {
                Ok(text) => sh.result = Some((true, text)),
                Err(e) => {
                    broken |= matches!(e, live::LinkError::Io(_));
                    sh.result = Some((false, e.to_string()));
                }
            }
            ctx.request_repaint();
        }
        if let Some(c) = client.as_mut().filter(|_| !broken) {
            let polled =
                (|| -> Result<_, live::LinkError> { Ok((c.status()?, c.player()?, c.camera()?)) })(
                );
            {
                let mut sh = shared(&s);
                match polled {
                    Ok((status, player, camera)) => {
                        sh.status = Some(status);
                        sh.player = player;
                        sh.camera = camera;
                    }
                    Err(e) => {
                        broken = !matches!(e, live::LinkError::Refused(_));
                        sh.error = Some(e.to_string());
                    }
                }
            }
            if !broken && c.has_journal() {
                let asked = kinds.load(Ordering::Relaxed) | extra_kinds.load(Ordering::Relaxed);
                match poll_journal(c, &mut next, asked, &s) {
                    Ok(_) => {}
                    Err(e) => broken = !matches!(e, live::LinkError::Refused(_)),
                }
            }
            if !broken && c.has_debug() {
                match c.debug_state() {
                    Ok(d) => shared(&s).debug = Some(d),
                    Err(e) => broken = !matches!(e, live::LinkError::Refused(_)),
                }
            }
            ctx.request_repaint();
        }
        if broken {
            client = None;
            shared(&s).connected = false;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    shared(&s).connected = false;
}

impl LiveState {
    /// Starts following the module at `endpoint`.
    pub(super) fn connect(&mut self, endpoint: Endpoint, ctx: &egui::Context) {
        self.disconnect();
        let (tx, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        self.shared = Arc::default();
        {
            let (s, stop, ctx, endpoint, kinds, extra) = (
                self.shared.clone(),
                stop.clone(),
                ctx.clone(),
                endpoint.clone(),
                self.kinds.clone(),
                self.extra_kinds.clone(),
            );
            std::thread::spawn(move || worker(endpoint, s, rx, stop, kinds, extra, ctx));
        }
        self.tx = Some(tx);
        self.stop = stop;
        self.endpoint = Some(endpoint);
        self.warned_lua = false;
    }

    /// Starts reading the names of `root` in the background, once per game.
    fn load_names(&self, root: &Path) {
        let mut state = self.names.lock().unwrap_or_else(|e| e.into_inner());
        let same = match &*state {
            NamesState::Loading(r) | NamesState::Ready(r, _) | NamesState::Failed(r, _) => {
                r == root
            }
            NamesState::None => false,
        };
        if same {
            return;
        }
        *state = NamesState::Loading(root.to_path_buf());
        let (names, root) = (self.names.clone(), root.to_path_buf());
        std::thread::spawn(move || {
            let result = LiveNames::open(&root, false);
            let mut state = names.lock().unwrap_or_else(|e| e.into_inner());
            *state = match result {
                Ok(n) => NamesState::Ready(root, Arc::new(n)),
                Err(e) => NamesState::Failed(root, e.to_string()),
            };
        });
    }

    /// The names, when loaded (for the debugging glue).
    pub(super) fn names_now(&self) -> Option<Arc<LiveNames>> {
        self.names()
    }

    /// The names, when loaded.
    fn names(&self) -> Option<Arc<LiveNames>> {
        match &*self.names.lock().unwrap_or_else(|e| e.into_inner()) {
            NamesState::Ready(_, n) => Some(n.clone()),
            _ => None,
        }
    }

    /// Events not yet copied into the editor's Log.
    fn take_fresh(&self) -> Vec<Event> {
        std::mem::take(&mut shared(&self.shared).fresh)
    }

    pub(super) fn disconnect(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.tx = None;
        self.endpoint = None;
        shared(&self.shared).connected = false;
    }

    pub(super) fn connected(&self) -> bool {
        self.tx.is_some() && shared(&self.shared).connected
    }

    /// Marks for the level tabs while the link is up.
    pub(super) fn marks(&self) -> Option<LiveMarks> {
        if !self.connected() {
            return None;
        }
        let s = shared(&self.shared);
        let writes = s.hello.as_ref().is_some_and(|h| h.writes && h.edition_ok);
        Some(LiveMarks {
            player: s.player.map(|p| (p.position, p.forward)),
            camera: s.camera.map(|c| (c.pose.position, c.pose.forward)),
            game_camera: s.camera,
            can_free_camera: s.debug_ok && writes,
            free_camera: s.debug.as_ref().is_some_and(|d| d.camera.is_some()),
        })
    }

    pub(super) fn send(&self, cmd: Cmd) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(cmd);
        }
    }

    fn loader(&mut self, root: &Path) -> Option<Loader> {
        if self.loader.as_ref().is_none_or(|(r, _)| r != root) {
            self.loader = Some((root.to_path_buf(), live::loader::find(root)));
        }
        self.loader.as_ref().and_then(|(_, l)| l.clone())
    }
}

fn vec3_text(v: [f32; 3]) -> String {
    format!("{:.2}, {:.2}, {:.2}", v[0], v[1], v[2])
}

fn parse_f32(text: &str) -> Option<f32> {
    text.trim()
        .replace(',', ".")
        .parse::<f32>()
        .ok()
        .filter(|v| v.is_finite())
}

impl BrowserApp {
    /// Play with the live link: the module for this launch. A missing loader
    /// or module only warns; the game starts without the link.
    pub(super) fn install_live(
        &mut self,
        root: &Path,
        options: &LiveOptions,
    ) -> Option<ModuleInstall> {
        if !options.enabled {
            return None;
        }
        if live::loader::find(root).is_none() {
            self.notify(Level::Warning, tr("live.no_loader").to_owned());
            return None;
        }
        let Some(path) = live::module_binary() else {
            self.notify(Level::Warning, tr("live.no_module").to_owned());
            return None;
        };
        let result = std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| {
                ModuleInstall::write(root, &bytes, &options.config()).map_err(|e| e.to_string())
            });
        match result {
            Ok(install) => Some(install),
            Err(error) => {
                self.notify(
                    Level::Warning,
                    trf("live.install_failed", &[("error", &error)]),
                );
                None
            }
        }
    }

    /// A start straight into a level with the live link off: the module only
    /// so that the player can move ([`live::install_for_input`]).
    pub(super) fn install_for_input(&mut self, root: &Path) -> Option<ModuleInstall> {
        match live::install_for_input(root) {
            Ok(live::InputFix::Installed(install)) => {
                self.notify(Level::Info, tr("live.input_fix_installed").to_owned());
                Some(install)
            }
            Ok(live::InputFix::NoLoader) => {
                self.notify(Level::Warning, tr("live.input_fix_no_loader").to_owned());
                None
            }
            Ok(live::InputFix::NoModule) => {
                self.notify(Level::Warning, tr("live.input_fix_no_module").to_owned());
                None
            }
            Err(error) => {
                self.notify(
                    Level::Warning,
                    trf("live.install_failed", &[("error", &error.to_string())]),
                );
                None
            }
        }
    }

    /// Copies new events of the game's journal into the Log panel: prints
    /// and Lua errors (messages stay in the Link panel). The first Lua error
    /// of a link also pops up.
    pub(super) fn live_journal_to_log(&mut self) {
        let fresh = self.play.live.take_fresh();
        if fresh.is_empty() {
            return;
        }
        let names = self.play.live.names();
        for e in &fresh {
            if matches!(
                e.body,
                EventBody::Message { .. } | EventBody::MessageCounts { .. }
            ) {
                continue;
            }
            let text = trf(
                "live.journal_line",
                &[("text", &names::describe(e, names.as_deref()))],
            );
            self.notes.push(Level::Info, &text);
            if matches!(e.body, EventBody::LuaError { .. }) && !self.play.live.warned_lua {
                self.play.live.warned_lua = true;
                self.notify(
                    Level::Warning,
                    trf("live.lua_error_seen", &[("text", &text)]),
                );
            }
        }
    }

    /// After the game exited: the module goes away, the link stops.
    pub(super) fn live_after_exit(&mut self) {
        self.play.live.disconnect();
        if let Some(module) = self.play.live.module.take() {
            match module.restore() {
                Ok(_) => {}
                Err(error) => {
                    doctor_toolkit::report::log(&format!("live module not taken away: {error}"))
                }
            }
        }
    }

    /// The "Link" panel of the game tab.
    pub(super) fn live_panel(&mut self, ui: &mut egui::Ui) {
        let Some(game) = self.game.clone() else {
            ui.label(tr("gametab.no_game"));
            return;
        };
        ui.heading(tr("live.title"));
        // Settings of Play.
        let mut options = self.settings.game(game.root()).play.live;
        ui.checkbox(&mut options.enabled, tr("live.enabled"))
            .on_hover_text(tr("live.enabled_hint"));
        ui.add_enabled_ui(options.enabled, |ui| {
            ui.checkbox(&mut options.writes, tr("live.writes"))
                .on_hover_text(tr("live.writes_hint"));
        });
        if self.settings.game(game.root()).play.live != options {
            self.settings.game_mut(game.root()).play.live = options;
            self.save_settings();
        }
        match self.play.live.loader(game.root()) {
            None => {
                ui.colored_label(doctor_toolkit_mods::WARN, tr("live.no_loader"));
            }
            Some(l) => {
                let mut text = trf(
                    "live.loader",
                    &[(
                        "file",
                        &l.file.file_name().unwrap_or_default().to_string_lossy(),
                    )],
                );
                if !l.modules.is_empty() {
                    text += &trf("live.other_modules", &[("list", &l.modules.join(", "))]);
                }
                ui.label(RichText::new(text).weak().small());
            }
        }
        if live::module_binary().is_none() {
            ui.colored_label(doctor_toolkit_mods::WARN, tr("live.no_module"));
        }
        ui.separator();

        // Connection.
        let connected = self.play.live.connected();
        ui.horizontal(|ui| {
            if self.play.live.tx.is_some() {
                if ui.button(tr("live.disconnect")).clicked() {
                    self.play.live.disconnect();
                }
            } else if ui
                .button(tr("live.connect"))
                .on_hover_text(tr("live.connect_hint"))
                .clicked()
            {
                let endpoint = Endpoint::of_game(game.root());
                self.play.live.connect(endpoint, ui.ctx());
            }
            if let Some(e) = &self.play.live.endpoint {
                ui.label(
                    RichText::new(format!("127.0.0.1:{}", e.port))
                        .weak()
                        .small(),
                );
            }
        });
        let snapshot = {
            let s = shared(&self.play.live.shared);
            (
                s.hello.clone(),
                s.status.clone(),
                s.player,
                s.camera,
                s.error.clone(),
                s.result.clone(),
                s.busy,
            )
        };
        let (hello, status, player, camera, error, result, busy) = snapshot;
        if self.play.live.tx.is_none() {
            ui.label(RichText::new(tr("live.off")).weak());
            return;
        }
        if !connected {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tr("live.waiting"));
            });
            if let Some(e) = error {
                ui.label(RichText::new(e).weak().small());
            }
            return;
        }
        let writes = hello.as_ref().is_some_and(|h| h.writes && h.edition_ok);
        if let Some(h) = &hello {
            if h.edition_ok {
                ui.label(trf(
                    "live.connected",
                    &[("module", &h.module), ("edition", &h.edition)],
                ));
            } else {
                ui.colored_label(
                    doctor_toolkit_mods::BAD,
                    trf("live.wrong_edition", &[("why", &h.edition)]),
                );
                return;
            }
        }
        if let Some(s) = &status {
            let mut line = trf(
                "live.state",
                &[("state", &s.state.name()), ("frame", &s.frame)],
            );
            if let Some(t) = s.sim_time_ms {
                line += &trf(
                    "live.sim_time",
                    &[("seconds", &format!("{:.1}", f64::from(t) / 1000.0))],
                );
            }
            if let Some([h, m, sec]) = s.time_of_day {
                line += &trf(
                    "live.time_of_day",
                    &[("time", &format!("{h:02}:{m:02}:{sec:02}"))],
                );
            }
            ui.label(line);
        }
        egui::Grid::new("live-pose").num_columns(2).show(ui, |ui| {
            ui.label(tr("live.player"));
            match player {
                Some(p) => {
                    ui.label(
                        RichText::new(trf(
                            "live.pose",
                            &[
                                ("position", &vec3_text(p.position)),
                                ("yaw", &format!("{:.1}", p.yaw().to_degrees())),
                            ],
                        ))
                        .monospace(),
                    )
                    .on_hover_text(tr("live.copy_hint"))
                    .clicked()
                    .then(|| ui.ctx().copy_text(vec3_text(p.position)));
                }
                None => {
                    ui.label(RichText::new(tr("live.no_player")).weak());
                }
            }
            ui.end_row();
            ui.label(tr("live.camera"));
            match camera {
                Some(c) => {
                    let mut text = trf(
                        "live.camera_pose",
                        &[
                            ("position", &vec3_text(c.pose.position)),
                            ("direction", &vec3_text(c.pose.forward)),
                        ],
                    );
                    if let Some(f) = c.fov {
                        text += &trf("live.fov", &[("fov", &format!("{f:.1}"))]);
                    }
                    ui.label(RichText::new(text).monospace());
                }
                None => {
                    ui.label(RichText::new(tr("live.no_camera")).weak());
                }
            }
            ui.end_row();
        });
        ui.separator();

        // Reads.
        let journal_ok = shared(&self.play.live.shared).journal_ok;
        if journal_ok {
            self.play.live.load_names(game.root());
        }
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled(
                journal_ok,
                egui::Checkbox::new(&mut self.play.live.all_objects, tr("live.all_objects")),
            )
            .on_hover_text(tr("live.all_objects_hint"))
            .on_disabled_hover_text(tr("live.old_module"));
            if self.play.live.all_objects {
                ui.checkbox(&mut self.play.live.logic_only, tr("live.logic_only"))
                    .on_hover_text(tr("live.logic_only_hint"));
                if ui
                    .add_enabled(!busy, egui::Button::new(tr("live.list_all")))
                    .clicked()
                {
                    let filter = WorldFilter {
                        positioned: self.play.live.logic_only.then_some(false),
                        ..WorldFilter::default()
                    };
                    self.play.live.send(Cmd::World(filter));
                }
            } else {
                ui.add(
                    egui::DragValue::new(&mut self.play.live.radius)
                        .range(1.0..=500.0)
                        .suffix(" m"),
                );
                if ui
                    .add_enabled(!busy, egui::Button::new(tr("live.objects")))
                    .on_hover_text(tr("live.objects_hint"))
                    .clicked()
                {
                    self.play.live.send(Cmd::Objects(self.play.live.radius));
                }
            }
            if ui
                .add_enabled(!busy, egui::Button::new(tr("live.streams")))
                .clicked()
            {
                self.play.live.send(Cmd::Streams);
            }
        });
        if journal_ok {
            self.live_find(ui, busy);
        }
        let (objects, streams, lua_out) = {
            let s = shared(&self.play.live.shared);
            (s.objects.clone(), s.streams.clone(), s.lua.clone())
        };
        if !objects.is_empty() {
            egui::CollapsingHeader::new(trf("live.objects_list", &[("count", &objects.len())]))
                .default_open(true)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("live-objects")
                        .max_height(180.0)
                        .show(ui, |ui| {
                            for o in &objects {
                                let line = format!(
                                    "{:<20} {:6.1} m  {}",
                                    if o.class_name.is_empty() {
                                        "?"
                                    } else {
                                        &o.class_name
                                    },
                                    o.distance,
                                    proto::braced_guid(&o.guid)
                                );
                                let r = ui
                                    .add(
                                        egui::Label::new(RichText::new(line).monospace().small())
                                            .sense(egui::Sense::click()),
                                    )
                                    .on_hover_text(tr("live.object_hint"));
                                if r.clicked() {
                                    self.play.live.target = proto::braced_guid(&o.guid);
                                    self.play.live.teleport = o.position.map(|v| format!("{v:.2}"));
                                }
                            }
                        });
                });
        }
        self.live_world_list(ui);
        if !streams.is_empty() {
            egui::CollapsingHeader::new(trf("live.streams_list", &[("count", &streams.len())]))
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("live-streams")
                        .max_height(140.0)
                        .show(ui, |ui| {
                            for s in &streams {
                                ui.label(RichText::new(s).monospace().small());
                            }
                        });
                });
        }
        ui.separator();
        if journal_ok {
            self.live_journal(ui);
            ui.separator();
        }
        self.live_debug_panel(ui);

        // Writes.
        ui.label(RichText::new(tr("live.writes_title")).strong());
        if !writes {
            ui.label(RichText::new(tr("live.writes_off")).weak());
        }
        ui.add_enabled_ui(writes && !busy, |ui| {
            ui.horizontal(|ui| {
                ui.label(tr("live.teleport_to"));
                for v in &mut self.play.live.teleport {
                    ui.add(egui::TextEdit::singleline(v).desired_width(56.0));
                }
                ui.label(tr("live.yaw"));
                ui.add(egui::TextEdit::singleline(&mut self.play.live.yaw).desired_width(40.0));
            });
            ui.horizontal(|ui| {
                if ui
                    .button(tr("live.here"))
                    .on_hover_text(tr("live.here_hint"))
                    .clicked()
                    && let Some(p) = player
                {
                    self.play.live.teleport = p.position.map(|v| format!("{v:.2}"));
                }
                let pos: Option<Vec<f32>> = self
                    .play
                    .live
                    .teleport
                    .iter()
                    .map(|t| parse_f32(t))
                    .collect();
                if ui
                    .add_enabled(pos.is_some(), egui::Button::new(tr("live.teleport")))
                    .clicked()
                    && let Some(p) = pos
                {
                    let yaw = parse_f32(&self.play.live.yaw).map(f32::to_radians);
                    self.play.live.confirm = Some(Cmd::Teleport([p[0], p[1], p[2]], yaw));
                }
            });
            ui.horizontal(|ui| {
                ui.label(tr("live.message"));
                ui.add(
                    egui::TextEdit::singleline(&mut self.play.live.message).desired_width(150.0),
                );
                ui.label(tr("live.target"));
                ui.add(
                    egui::TextEdit::singleline(&mut self.play.live.target)
                        .hint_text(tr("live.target_all"))
                        .desired_width(150.0),
                );
            });
            let target_text = self.play.live.target.trim().to_owned();
            let target = (!target_text.is_empty()).then(|| proto::parse_guid(&target_text));
            let target_ok = !matches!(target, Some(None));
            if ui
                .add_enabled(
                    !self.play.live.message.trim().is_empty() && target_ok,
                    egui::Button::new(tr("live.send")),
                )
                .on_hover_text(tr("live.send_hint"))
                .clicked()
            {
                self.play.live.confirm = Some(Cmd::Send(
                    MessageId::parse(&self.play.live.message),
                    target.flatten(),
                ));
            }
            ui.label(tr("live.lua"));
            ui.add(
                egui::TextEdit::multiline(&mut self.play.live.lua)
                    .code_editor()
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            );
            if ui.button(tr("live.run_lua")).clicked() {
                self.play.live.confirm = Some(Cmd::Lua(self.play.live.lua.clone()));
            }
        });
        self.live_textures_button(ui, writes && !busy);
        if !lua_out.is_empty() {
            ui.label(RichText::new(lua_out.join("\n")).monospace().small());
        }
        if busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tr("live.busy"));
            });
        } else if let Some((ok, text)) = result {
            if ok {
                ui.label(text);
            } else {
                ui.colored_label(doctor_toolkit_mods::BAD, text);
            }
        }
    }

    /// Asks before a write goes to the game.
    pub(super) fn live_confirm(&mut self, ctx: &egui::Context) {
        let Some(cmd) = self.play.live.confirm.clone() else {
            return;
        };
        debug_assert!(cmd.writes());
        let text = match &cmd {
            Cmd::Teleport(p, yaw) => trf(
                "live.confirm_teleport",
                &[
                    ("position", &vec3_text(*p)),
                    (
                        "yaw",
                        &yaw.map(|y| format!("{:.1}°", y.to_degrees()))
                            .unwrap_or_else(|| tr("live.keep_yaw").to_owned()),
                    ),
                ],
            ),
            Cmd::Send(m, target) => trf(
                "live.confirm_send",
                &[
                    ("message", m),
                    (
                        "target",
                        &target
                            .map(|g| proto::braced_guid(&g))
                            .unwrap_or_else(|| tr("live.target_all").to_owned()),
                    ),
                ],
            ),
            Cmd::Lua(code) => trf("live.confirm_lua", &[("lines", &code.lines().count())]),
            Cmd::Camera(Some(c)) => trf(
                "debug.confirm_camera",
                &[("position", &vec3_text(c.pose.position))],
            ),
            Cmd::Textures(list) => trf("debug.confirm_textures", &[("count", &list.len())]),
            Cmd::Objects(_)
            | Cmd::Streams
            | Cmd::World(_)
            | Cmd::Break(..)
            | Cmd::Control(_)
            | Cmd::Camera(None) => String::new(),
        };
        let mut answer = None;
        egui::Modal::new(egui::Id::unique("live-confirm")).show(ctx, |ui| {
            ui.set_max_width(460.0);
            ui.heading(tr("live.confirm_title"));
            ui.label(text);
            ui.label(RichText::new(tr("live.confirm_note")).weak().small());
            ui.horizontal(|ui| {
                if ui.button(tr("live.confirm_yes")).clicked() {
                    answer = Some(true);
                }
                if ui.button(tr("dialog.cancel")).clicked() {
                    answer = Some(false);
                }
            });
        });
        if let Some(yes) = answer {
            self.play.live.confirm = None;
            if yes {
                self.play.live.send(cmd);
            }
        }
    }
}

impl BrowserApp {
    /// Search of the running game by GUID or by name (names from the
    /// SimGroups of the game and its workspace).
    fn live_find(&mut self, ui: &mut egui::Ui, busy: bool) {
        let names = self.play.live.names();
        ui.horizontal(|ui| {
            let edit = ui.add(
                egui::TextEdit::singleline(&mut self.play.live.find)
                    .hint_text(tr("live.find_hint"))
                    .desired_width(220.0),
            );
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let text = self.play.live.find.trim().to_owned();
            if (ui
                .add_enabled(
                    !busy && !text.is_empty(),
                    egui::Button::new(tr("live.find")),
                )
                .clicked()
                || enter)
                && !text.is_empty()
            {
                let guids = match proto::parse_guid(&text) {
                    Some(g) => vec![g],
                    None => names
                        .as_ref()
                        .map(|n| {
                            n.search(&text)
                                .into_iter()
                                .map(|(g, _)| g)
                                .take(4096)
                                .collect()
                        })
                        .unwrap_or_default(),
                };
                if guids.is_empty() {
                    shared(&self.play.live.shared).result =
                        Some((false, trf("live.find_none", &[("text", &text)])));
                } else {
                    self.play.live.send(Cmd::World(WorldFilter {
                        guids,
                        ..WorldFilter::default()
                    }));
                }
            }
            match &*self
                .play
                .live
                .names
                .lock()
                .unwrap_or_else(|e| e.into_inner())
            {
                NamesState::Loading(_) => {
                    ui.spinner();
                    ui.label(RichText::new(tr("live.names_loading")).weak().small());
                }
                NamesState::Ready(_, n) => {
                    ui.label(
                        RichText::new(trf("live.names_ready", &[("count", &n.objects())]))
                            .weak()
                            .small(),
                    );
                }
                NamesState::Failed(_, e) => {
                    ui.label(RichText::new(e).weak().small());
                }
                NamesState::None => {}
            }
        });
    }

    /// The answer of "All objects" or of a search.
    fn live_world_list(&mut self, ui: &mut egui::Ui) {
        let Some(world) = shared(&self.play.live.shared).world.clone() else {
            return;
        };
        let names = self.play.live.names();
        let filter = self.play.live.journal_filter.to_lowercase();
        let rows: Vec<(String, &proto::WorldObject)> = world
            .objects
            .iter()
            .map(|o| {
                let known = names.as_ref().and_then(|n| n.object(&o.guid));
                let class = world
                    .class_name(o.class)
                    .or(known.map(|k| k.class.as_str()))
                    .unwrap_or("?");
                let line = format!(
                    "{:<22} {:<3} {:<34} {} {}",
                    class,
                    if o.listening { "on" } else { "off" },
                    world.stream_name(o.group).unwrap_or("?"),
                    proto::guid_hex(&o.guid),
                    known.map(|k| k.name.as_str()).unwrap_or_default(),
                );
                (line, o)
            })
            .filter(|(line, _)| filter.is_empty() || line.to_lowercase().contains(&filter))
            .collect();
        let title = trf(
            "live.world_list",
            &[("count", &rows.len()), ("total", &world.total)],
        );
        egui::CollapsingHeader::new(title)
            .id_salt("live-world")
            .default_open(true)
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.play.live.journal_filter)
                        .hint_text(tr("live.filter"))
                        .desired_width(200.0),
                );
                egui::ScrollArea::vertical()
                    .id_salt("live-world-rows")
                    .max_height(200.0)
                    .show_rows(ui, 16.0, rows.len(), |ui, range| {
                        for (line, o) in &rows[range] {
                            let r = ui
                                .add(
                                    egui::Label::new(RichText::new(line).monospace().small())
                                        .sense(egui::Sense::click()),
                                )
                                .on_hover_text(tr("live.object_hint"));
                            if r.clicked() {
                                self.play.live.target = proto::braced_guid(&o.guid);
                                if let Some(p) = o.position {
                                    self.play.live.teleport = p.map(|v| format!("{v:.2}"));
                                }
                            }
                        }
                    });
            });
    }

    /// The game's journal: prints, Lua errors with their call stack,
    /// messages (when asked for), with a filter.
    fn live_journal(&mut self, ui: &mut egui::Ui) {
        let (events, recording, lost) = {
            let s = shared(&self.play.live.shared);
            (s.journal.clone(), s.recording, s.lost)
        };
        let names = self.play.live.names();
        egui::CollapsingHeader::new(trf("live.journal", &[("count", &events.len())]))
            .id_salt("live-journal")
            .default_open(true)
            .show(ui, |ui| {
                let mut kinds = self.play.live.kinds.load(Ordering::Relaxed);
                ui.horizontal_wrapped(|ui| {
                    for (bit, label, hint) in [
                        (
                            mask::OUTPUT,
                            "live.journal_output",
                            "live.journal_output_hint",
                        ),
                        (
                            mask::LUA_ERROR,
                            "live.journal_errors",
                            "live.journal_errors_hint",
                        ),
                        (
                            mask::MESSAGE,
                            "live.journal_messages",
                            "live.journal_messages_hint",
                        ),
                    ] {
                        let mut on = kinds & bit != 0;
                        if ui
                            .checkbox(&mut on, tr(label))
                            .on_hover_text(tr(hint))
                            .changed()
                        {
                            kinds ^= bit;
                        }
                    }
                    ui.add(
                        egui::TextEdit::singleline(&mut self.play.live.journal_filter)
                            .hint_text(tr("live.filter"))
                            .desired_width(160.0),
                    );
                    if ui.button(tr("log.clear")).clicked() {
                        shared(&self.play.live.shared).journal.clear();
                    }
                });
                self.play.live.kinds.store(kinds, Ordering::Relaxed);
                if recording & kinds != kinds && recording != 0 {
                    ui.colored_label(doctor_toolkit_mods::WARN, tr("live.journal_partial"));
                }
                if lost > 0 {
                    ui.label(
                        RichText::new(trf("live.journal_lost", &[("count", &lost)]))
                            .weak()
                            .small(),
                    );
                }
                let filter = self.play.live.journal_filter.to_lowercase();
                let rows: Vec<(String, Vec<String>, bool)> = events
                    .iter()
                    .rev()
                    .filter(|e| e.body.mask() & kinds != 0)
                    .map(|e| {
                        let text = names::describe(e, names.as_deref());
                        let (stack, error) = match &e.body {
                            EventBody::LuaError { traceback, .. } => (traceback.clone(), true),
                            _ => (Vec::new(), false),
                        };
                        (
                            format!("{:>7.1}s {text}", f64::from(e.sim_time_ms) / 1000.0),
                            stack,
                            error,
                        )
                    })
                    .filter(|(t, stack, _)| {
                        filter.is_empty()
                            || t.to_lowercase().contains(&filter)
                            || stack.iter().any(|l| l.to_lowercase().contains(&filter))
                    })
                    .collect();
                if rows.is_empty() {
                    ui.label(RichText::new(tr("live.journal_empty")).weak());
                    return;
                }
                egui::ScrollArea::vertical()
                    .id_salt("live-journal-rows")
                    .max_height(240.0)
                    .show_rows(ui, 16.0, rows.len(), |ui, range| {
                        for (text, stack, error) in &rows[range] {
                            let rich = RichText::new(text).monospace().small();
                            let rich = if *error {
                                rich.color(doctor_toolkit_mods::BAD)
                            } else {
                                rich
                            };
                            let label = ui.add(egui::Label::new(rich).truncate());
                            if !stack.is_empty() {
                                label.on_hover_text(stack.join("\n"));
                            } else {
                                label.on_hover_text(text.as_str());
                            }
                        }
                    });
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use doctor_toolkit::live::fake::FakeGame;

    fn wait_for(s: &LiveState, what: impl Fn(&Shared) -> bool) -> bool {
        for _ in 0..80 {
            if what(&shared(&s.shared)) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    #[test]
    fn follows_the_fake_game_and_acts() {
        let game = FakeGame::start(live::Config {
            port: 0,
            token: "t".into(),
            writes: true,
            fix_input: true,
            journal: true,
        })
        .unwrap();
        let ctx = egui::Context::default();
        let mut state = LiveState::default();
        state.connect(game.endpoint("t"), &ctx);
        assert!(wait_for(&state, |s| s.connected && s.player.is_some()));
        let marks = state.marks().unwrap();
        assert_eq!(marks.player.unwrap().0, [283.9, -0.07, 371.8]);
        assert!(marks.camera.is_some());
        state.send(Cmd::Teleport([1.0, 2.0, 3.0], None));
        assert!(wait_for(&state, |s| s
            .player
            .is_some_and(|p| p.position == [1.0, 2.0, 3.0])));
        state.send(Cmd::Objects(1000.0));
        assert!(wait_for(&state, |s| s.objects.len() == 3));
        state.send(Cmd::Lua("return GetSimTime()".into()));
        assert!(wait_for(&state, |s| !s.lua.is_empty()));
        // The journal: a print and an error of Lua reach the panel and are
        // handed to the editor's Log once.
        assert!(wait_for(&state, |s| s.journal_ok));
        state.send(Cmd::Lua("print(\"from the game\")".into()));
        state.send(Cmd::Lua("this is bad".into()));
        assert!(wait_for(&state, |s| s.journal.len() == 2));
        let fresh = state.take_fresh();
        assert_eq!(fresh.len(), 2);
        assert!(state.take_fresh().is_empty());
        assert!(matches!(fresh[1].body, EventBody::LuaError { .. }));
        // Messages only when the panel asks for them.
        state.send(Cmd::Send(MessageId::Name("doctor_ping".into()), None));
        std::thread::sleep(Duration::from_millis(600));
        assert!(
            !shared(&state.shared)
                .journal
                .iter()
                .any(|e| matches!(e.body, EventBody::Message { .. }))
        );
        state.kinds.store(mask::ALL, Ordering::Relaxed);
        state.send(Cmd::Send(MessageId::Name("doctor_ping".into()), None));
        assert!(wait_for(&state, |s| s
            .journal
            .iter()
            .any(|e| matches!(e.body, EventBody::Message { .. }))));
        // Every object, logic only, and a search by GUID.
        state.send(Cmd::World(WorldFilter {
            positioned: Some(false),
            ..WorldFilter::default()
        }));
        assert!(wait_for(&state, |s| s
            .world
            .as_ref()
            .is_some_and(|w| w.objects.len() == 2)));
        state.send(Cmd::World(WorldFilter {
            guids: vec![[5; 16]],
            ..WorldFilter::default()
        }));
        assert!(wait_for(&state, |s| s
            .world
            .as_ref()
            .is_some_and(|w| w.objects.len() == 1)));
        // Stage 35: breakpoints, pause, the free camera, messages for the
        // logic tabs.
        assert!(wait_for(&state, |s| s.debug_ok && s.debug.is_some()));
        let ping = proto::name_hash("doctor_ping");
        state.send(Cmd::Break(vec![proto::BreakRule::message(ping, None)], 30));
        assert!(wait_for(&state, |s| s
            .debug
            .as_ref()
            .is_some_and(|d| d.rules.len() == 1)));
        state
            .extra_kinds
            .store(mask::MESSAGE | mask::MESSAGE_COUNTS, Ordering::Relaxed);
        game.world.lock().unwrap().activity(0);
        assert!(wait_for(&state, |s| s
            .debug
            .as_ref()
            .is_some_and(|d| d.stop.is_some())));
        assert!(wait_for(&state, |s| s
            .trace
            .iter()
            .any(|e| matches!(e.body, EventBody::MessageCounts { .. }))));
        state.send(Cmd::Control(proto::Control::Continue));
        assert!(wait_for(&state, |s| s
            .debug
            .as_ref()
            .is_some_and(|d| d.stop.is_none())));
        let marks = state.marks().unwrap();
        assert!(marks.can_free_camera && !marks.free_camera);
        let mut view = marks.game_camera.unwrap();
        view.pose.position = [5.0, 6.0, 7.0];
        state.send(Cmd::Camera(Some(view)));
        assert!(wait_for(&state, |s| s
            .camera
            .is_some_and(|c| c.pose.position == [5.0, 6.0, 7.0])));
        // `wait_for` holds the lock of `Shared` while it asks: read the
        // state it hands over, never `state.marks()` (that locks again).
        assert!(wait_for(&state, |s| s
            .debug
            .as_ref()
            .is_some_and(|d| d.camera.is_some())));
        assert!(state.marks().is_some_and(|m| m.free_camera));
        state.disconnect();
        assert!(state.marks().is_none());
    }
}

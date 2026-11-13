//! The live link in the game tab (stage 34): the "Link" panel — state,
//! player and camera of the running game, objects nearby, teleport,
//! messages and Lua — and the marks of the player and the camera in level
//! tabs.
//!
//! A background thread talks to the module in the game
//! (`doctor_toolkit::live::Client`): it reconnects every 2 s while the
//! game starts, polls state, player and camera four times a second and
//! runs the actions the panel asks for. Writes go out only after the
//! confirmation window.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::RichText;

use doctor_toolkit::live::proto::{
    self, CameraView, Guid, Hello, MessageId, ObjectInfo, Pose, Status,
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
}

/// What the thread last learnt.
#[derive(Default)]
struct Shared {
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
}

/// An action for the thread.
#[derive(Debug, Clone, PartialEq)]
enum Cmd {
    Teleport([f32; 3], Option<f32>),
    Send(MessageId, Option<Guid>),
    Lua(String),
    Objects(f32),
    Streams,
}

impl Cmd {
    fn writes(&self) -> bool {
        matches!(self, Self::Teleport(..) | Self::Send(..) | Self::Lua(_))
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
    confirm: Option<Cmd>,
    /// The ASI loader of the open game, looked up once.
    loader: Option<(PathBuf, Option<Loader>)>,
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
        }
    }
}

fn shared(s: &Mutex<Shared>) -> std::sync::MutexGuard<'_, Shared> {
    s.lock().unwrap_or_else(|e| e.into_inner())
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
    })
}

fn worker(
    endpoint: Endpoint,
    s: Arc<Mutex<Shared>>,
    rx: Receiver<Cmd>,
    stop: Arc<AtomicBool>,
    ctx: egui::Context,
) {
    let mut client: Option<Client> = None;
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
                    sh.error = None;
                    client = Some(c);
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
            let (s, stop, ctx, endpoint) = (
                self.shared.clone(),
                stop.clone(),
                ctx.clone(),
                endpoint.clone(),
            );
            std::thread::spawn(move || worker(endpoint, s, rx, stop, ctx));
        }
        self.tx = Some(tx);
        self.stop = stop;
        self.endpoint = Some(endpoint);
    }

    pub(super) fn disconnect(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.tx = None;
        self.endpoint = None;
        shared(&self.shared).connected = false;
    }

    fn connected(&self) -> bool {
        self.tx.is_some() && shared(&self.shared).connected
    }

    /// Marks for the level tabs while the link is up.
    pub(super) fn marks(&self) -> Option<LiveMarks> {
        if !self.connected() {
            return None;
        }
        let s = shared(&self.shared);
        Some(LiveMarks {
            player: s.player.map(|p| (p.position, p.forward)),
            camera: s.camera.map(|c| (c.pose.position, c.pose.forward)),
        })
    }

    fn send(&self, cmd: Cmd) {
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
        ui.horizontal(|ui| {
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
            if ui
                .add_enabled(!busy, egui::Button::new(tr("live.streams")))
                .clicked()
            {
                self.play.live.send(Cmd::Streams);
            }
        });
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
        self.live_confirm(ui.ctx());
    }

    /// Asks before a write goes to the game.
    fn live_confirm(&mut self, ctx: &egui::Context) {
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
            Cmd::Objects(_) | Cmd::Streams => String::new(),
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
        state.disconnect();
        assert!(state.marks().is_none());
    }
}

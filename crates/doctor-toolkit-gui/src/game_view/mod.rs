//! The game in a tab of the editor (stage 33).
//!
//! - **Linux** (and other X11 systems): the game runs on an X server of its
//!   own ([`nested`]: Xvfb, or Xephyr in a host window that is never shown);
//!   its frames become a texture of the tab and the keys, buttons and mouse
//!   motion of the tab go to it ([`x11`]). A click on the picture gives the
//!   mouse and keyboard to the game, Esc takes them back (Shift+Esc sends
//!   Esc to the game).
//! - **Windows**: the game starts in a window, which is put inside the
//!   editor's window over the tab ([`windows`]); input goes to it directly.
//!
//! The editor's Play (`app/play.rs`) starts and follows the game; this view
//! draws it and turns input into the game's input ([`keys`]).

pub mod keys;
#[cfg(all(unix, not(target_os = "macos")))]
pub mod nested;
#[cfg(all(windows, target_pointer_width = "64"))]
pub mod windows;
#[cfg(all(unix, not(target_os = "macos")))]
pub mod x11;

use std::collections::BTreeSet;
use std::sync::Arc;

use egui::{
    Color32, ColorImage, Event, Key, Order, Rect, RichText, Sense, TextureHandle, Vec2,
    ViewportCommand,
};

use doctor_toolkit::play::{LaunchPlan, SessionState};
use doctor_toolkit_mods::i18n::tr;

/// How long a button of the tab holds its key in the game.
#[cfg(all(unix, not(target_os = "macos")))]
const TAP_HOLD: std::time::Duration = std::time::Duration::from_millis(700);

/// The keysym of Escape.
#[cfg(all(unix, not(target_os = "macos")))]
const ESCAPE: u32 = 0xff1b;

/// Size of the game's screen when the settings give none.
pub const DEFAULT_SIZE: [u32; 2] = [1280, 720];

/// What the buttons of the tab ask the editor to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Play,
    Stop,
    Restart,
    Screenshot,
    Options,
}

/// What the editor tells the tab about the game.
pub struct Info {
    /// One line: starting, running, exited…
    pub status: String,
    /// Where the game starts (`Havana, spawn point 3`).
    pub start: String,
    /// Play can start the game now.
    pub can_play: bool,
}

#[cfg(all(unix, not(target_os = "macos")))]
struct Running {
    game: nested::TabGame,
    link: Option<x11::Link>,
    link_error: Option<String>,
}

#[cfg(all(windows, target_pointer_width = "64"))]
struct Running {
    session: doctor_toolkit::play::Session,
    embedded: Option<windows::Embedded>,
    pids: Vec<u32>,
}

#[cfg(not(any(
    all(unix, not(target_os = "macos")),
    all(windows, target_pointer_width = "64")
)))]
struct Running {
    session: doctor_toolkit::play::Session,
}

/// The game tab.
#[derive(Default)]
pub struct GameView {
    run: Option<Running>,
    size: [u32; 2],
    texture: Option<TextureHandle>,
    /// Number of the frame in the texture.
    shown: u64,
    /// The frame in the texture (for screenshots).
    frame: Option<Arc<ColorImage>>,
    /// Mouse and keyboard go to the game.
    captured: bool,
    /// Raw mouse motion arrives (else the pointer's motion is used).
    raw_motion: bool,
    /// The pointer was put back in the middle: its next motion is not the user's.
    skip_delta: bool,
    held_keys: BTreeSet<u32>,
    held_buttons: BTreeSet<u8>,
    /// A key pressed by a button of the tab, released at that time.
    tap: Option<(u32, std::time::Instant)>,
    rest: Vec2,
    /// Input taken from egui at the start of the frame, for the game.
    queued: Vec<Event>,
    /// Where the picture was drawn last.
    image: Option<Rect>,
    /// Frame (pass) the tab was drawn in last.
    drawn_pass: u64,
    /// The X server to use instead of the one found (tests).
    #[cfg(all(unix, not(target_os = "macos")))]
    pub server: Option<nested::Server>,
    /// The editor's own window (Windows: the parent of the game's window).
    #[cfg_attr(not(windows), allow(dead_code))]
    pub parent_window: Option<isize>,
}

impl GameView {
    /// Whether the game can be shown in a tab here; the reason when not.
    pub fn available() -> Result<String, String> {
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            nested::Server::find()
                .map(|s| s.name())
                .ok_or_else(|| tr("gametab.no_server").to_owned())
        }
        #[cfg(all(windows, target_pointer_width = "64"))]
        {
            Ok("SetParent".to_owned())
        }
        #[cfg(not(any(
            all(unix, not(target_os = "macos")),
            all(windows, target_pointer_width = "64")
        )))]
        {
            Err(tr("gametab.unsupported").to_owned())
        }
    }

    /// Starts `plan` for the tab with a screen of `size` pixels.
    pub fn start(
        &mut self,
        plan: &LaunchPlan,
        size: [u32; 2],
        ctx: &egui::Context,
    ) -> Result<(), String> {
        self.release(ctx);
        self.size = size;
        self.frame = None;
        self.texture = None;
        self.shown = 0;
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            let server = self
                .server
                .clone()
                .or_else(nested::Server::find)
                .ok_or_else(|| tr("gametab.no_server").to_owned())?;
            let game = nested::TabGame::start(plan, &server, size)?;
            let display = game.display().map(|d| d.name()).unwrap_or_default();
            let (link, link_error) = match x11::Link::connect(&display, ctx) {
                Ok(link) => (Some(link), None),
                Err(e) => (None, Some(e)),
            };
            self.run = Some(Running {
                game,
                link,
                link_error,
            });
        }
        #[cfg(all(windows, target_pointer_width = "64"))]
        {
            let _ = ctx;
            let session = doctor_toolkit::play::Session::start(plan).map_err(|e| e.to_string())?;
            self.run = Some(Running {
                session,
                embedded: None,
                pids: Vec::new(),
            });
        }
        #[cfg(not(any(
            all(unix, not(target_os = "macos")),
            all(windows, target_pointer_width = "64")
        )))]
        {
            let _ = ctx;
            let session = doctor_toolkit::play::Session::start(plan).map_err(|e| e.to_string())?;
            self.run = Some(Running { session });
        }
        Ok(())
    }

    /// Whether a game of the tab is followed.
    pub fn is_running(&self) -> bool {
        self.run.is_some()
    }

    /// Whether the game was asked to stop and has not yet.
    pub fn is_stopping(&self) -> bool {
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            self.run.as_ref().is_some_and(|r| r.game.stopping())
        }
        #[cfg(not(all(unix, not(target_os = "macos"))))]
        {
            false
        }
    }

    /// The state of the game after a process scan that found `running`;
    /// `None` without a game. Once it exited the tab lets it go.
    pub fn poll(&mut self, running: &[u32], ctx: &egui::Context) -> Option<SessionState> {
        let run = self.run.as_mut()?;
        #[cfg(all(unix, not(target_os = "macos")))]
        let state = {
            if run.link.is_none()
                && let Some(display) = run.game.display()
            {
                match x11::Link::connect(&display.name(), ctx) {
                    Ok(link) => {
                        run.link = Some(link);
                        run.link_error = None;
                    }
                    Err(e) => run.link_error = Some(e),
                }
            }
            run.game.poll(running)
        };
        #[cfg(all(windows, target_pointer_width = "64"))]
        let state = {
            let _ = ctx;
            run.pids = running.to_vec();
            run.pids.extend(run.session.pid());
            run.session.poll(running)
        };
        #[cfg(not(any(
            all(unix, not(target_os = "macos")),
            all(windows, target_pointer_width = "64")
        )))]
        let state = {
            let _ = ctx;
            run.session.poll(running)
        };
        if matches!(state, SessionState::Exited | SessionState::NotStarted(_)) {
            self.release(ctx);
            self.run = None;
        }
        Some(state)
    }

    /// Asks the game to end.
    pub fn stop(&mut self, ctx: &egui::Context) {
        self.release(ctx);
        #[cfg(all(unix, not(target_os = "macos")))]
        if let Some(run) = &mut self.run {
            run.game.stop();
        }
        #[cfg(not(all(unix, not(target_os = "macos"))))]
        if let Some(pid) = self.run.as_ref().and_then(|r| r.session.pid()) {
            // The launched program; the game's window closes with it.
            let _ = std::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T"])
                .status();
        }
    }

    /// The last frame as RGBA bytes, for a screenshot.
    pub fn screenshot(&self) -> Option<([usize; 2], Vec<u8>)> {
        let frame = self.frame.as_ref()?;
        let rgba = frame
            .pixels
            .iter()
            .flat_map(|c| [c.r(), c.g(), c.b(), 255])
            .collect();
        Some((frame.size, rgba))
    }

    /// Takes the input of this frame away from the editor while the game
    /// has it: shortcuts and widgets do not see it. Call before anything
    /// else reads input.
    pub fn take_input(&mut self, ctx: &egui::Context) {
        if !self.captured {
            return;
        }
        let focused = ctx.input(|i| i.viewport().focused) != Some(false);
        if !focused {
            self.release(ctx);
            return;
        }
        ctx.input_mut(|i| {
            let (game, editor): (Vec<Event>, Vec<Event>) =
                std::mem::take(&mut i.events).into_iter().partition(|e| {
                    matches!(
                        e,
                        Event::Key { .. }
                            | Event::Text(_)
                            | Event::PointerButton { .. }
                            | Event::MouseWheel { .. }
                            | Event::MouseMoved(_)
                            | Event::Copy
                            | Event::Cut
                            | Event::Paste(_)
                            | Event::Zoom(_)
                    )
                });
            i.events = editor;
            self.queued.extend(game);
        });
    }

    /// After the frame: a tab that was not drawn (another tab is shown)
    /// gives the input back and hides the game's window.
    pub fn after_frame(&mut self, ctx: &egui::Context) {
        let pass = ctx.cumulative_pass_nr();
        if self.drawn_pass + 1 < pass {
            if self.captured {
                self.release(ctx);
            }
            self.queued.clear();
            #[cfg(all(windows, target_pointer_width = "64"))]
            if let Some(embedded) = self.run.as_ref().and_then(|r| r.embedded.as_ref()) {
                embedded.place(None);
            }
        }
    }

    /// Gives the mouse and keyboard back to the editor; keys and buttons
    /// held in the game are released.
    pub fn release(&mut self, ctx: &egui::Context) {
        self.end_tap(ctx, true);
        #[cfg(all(unix, not(target_os = "macos")))]
        if let Some(input) = self.input() {
            for &key in &self.held_keys {
                input.key(key, false);
            }
            for &button in &self.held_buttons {
                input.button(button, false);
            }
            input.flush();
        }
        #[cfg(all(windows, target_pointer_width = "64"))]
        if let Some(parent) = self.parent_window
            && self.run.as_ref().is_some_and(|r| r.embedded.is_some())
        {
            windows::Embedded::release(parent);
        }
        self.held_keys.clear();
        self.held_buttons.clear();
        self.queued.clear();
        if self.captured {
            self.captured = false;
            ctx.send_viewport_cmd(ViewportCommand::CursorGrab(egui::CursorGrab::None));
            ctx.send_viewport_cmd(ViewportCommand::CursorVisible(true));
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    fn input(&self) -> Option<&x11::Input> {
        self.run.as_ref()?.link.as_ref().map(|l| l.input())
    }

    /// One line about the display, the frame rate and how frames are read.
    fn details(&self) -> Option<String> {
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            let run = self.run.as_ref()?;
            let display = run.game.display()?;
            let mut text = format!(
                "{} {} · {}×{}",
                display.server(),
                display.name(),
                display.size()[0],
                display.size()[1]
            );
            if let Some(link) = &run.link {
                let stats = link.stats();
                text += &doctor_toolkit_mods::i18n::trf(
                    "gametab.stats",
                    &[
                        ("fps", &format!("{:.1}", stats.fps)),
                        ("ms", &format!("{:.1}", stats.read_ms)),
                    ],
                );
                if let Some(error) = link.error() {
                    text += &format!(" · {error}");
                }
            } else if let Some(error) = &run.link_error {
                text += &format!(" · {error}");
            }
            Some(text)
        }
        #[cfg(not(all(unix, not(target_os = "macos"))))]
        {
            let _ = self.run.as_ref()?;
            Some(format!("{}×{}", self.size[0], self.size[1]))
        }
    }

    /// The tab.
    pub fn ui(&mut self, ui: &mut egui::Ui, info: &Info) -> Option<Action> {
        let ctx = ui.ctx().clone();
        self.drawn_pass = ctx.cumulative_pass_nr();
        let mut action = None;
        let running = self.run.is_some();
        ui.horizontal_wrapped(|ui| {
            if running {
                if ui
                    .button(tr("gametab.stop"))
                    .on_hover_text(tr("gametab.stop_hint"))
                    .clicked()
                {
                    action = Some(Action::Stop);
                }
                if ui
                    .button(tr("gametab.restart"))
                    .on_hover_text(tr("gametab.restart_hint"))
                    .clicked()
                {
                    action = Some(Action::Restart);
                }
            } else if ui
                .add_enabled(info.can_play, egui::Button::new(tr("gametab.play")))
                .on_hover_text(tr("gametab.play_hint"))
                .clicked()
            {
                action = Some(Action::Play);
            }
            if ui
                .add_enabled(
                    self.frame.is_some(),
                    egui::Button::new(tr("gametab.screenshot")),
                )
                .on_hover_text(tr("gametab.screenshot_hint"))
                .clicked()
            {
                action = Some(Action::Screenshot);
            }
            if ui
                .add_enabled(
                    self.captured || cfg!(windows),
                    egui::Button::new(tr("gametab.release")),
                )
                .on_hover_text(tr("gametab.release_hint"))
                .clicked()
            {
                self.release(&ctx);
            }
            #[cfg(all(unix, not(target_os = "macos")))]
            if ui
                .add_enabled(running, egui::Button::new(tr("gametab.send_escape")))
                .on_hover_text(tr("gametab.send_escape_hint"))
                .clicked()
                && let Some(input) = self.input()
            {
                // Held like a finger would: the game reads its keys once a
                // frame, and its pause menu misses shorter presses.
                input.key(ESCAPE, true);
                input.flush();
                self.tap = Some((ESCAPE, std::time::Instant::now() + TAP_HOLD));
            }
            if ui.button(tr("gametab.options")).clicked() {
                action = Some(Action::Options);
            }
            ui.separator();
            ui.label(&info.status);
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&info.start).weak());
            if let Some(details) = self.details() {
                ui.label(RichText::new(details).weak().small());
            }
        });
        ui.separator();
        self.update_texture(&ctx);
        self.end_tap(&ctx, false);
        let available = ui.available_rect_before_wrap();
        let size = if self.size[0] > 0 {
            self.size
        } else {
            DEFAULT_SIZE
        };
        let image = keys::fit(available, size, ctx.pixels_per_point());
        let response = ui.allocate_rect(available, Sense::click());
        ui.painter().rect_filled(available, 0.0, Color32::BLACK);
        if let Some(texture) = &self.texture {
            ui.painter().image(
                texture.id(),
                image,
                Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        } else {
            let text = if running {
                tr("gametab.waiting")
            } else {
                tr("gametab.idle")
            };
            ui.painter().text(
                available.center(),
                egui::Align2::CENTER_CENTER,
                text,
                egui::FontId::proportional(16.0),
                ui.visuals().weak_text_color(),
            );
        }
        self.image = Some(image);
        self.embed(&ctx, image);
        if running
            && !self.captured
            && cfg!(not(windows))
            && response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
            && image.contains(pos)
        {
            self.capture(&ctx, pos, image);
        } else if running && !self.captured && response.hovered() && cfg!(not(windows)) {
            response.on_hover_text(tr("gametab.click_to_play"));
        }
        if self.captured {
            self.send_input(&ctx, image);
            self.block_editor(&ctx);
        }
        if running {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        action
    }

    /// Releases the key of a button of the tab once it was held long
    /// enough (or at once with `now`).
    fn end_tap(&mut self, ctx: &egui::Context, now: bool) {
        let Some((key, until)) = self.tap else {
            return;
        };
        if now || std::time::Instant::now() >= until {
            self.tap = None;
            #[cfg(all(unix, not(target_os = "macos")))]
            if let Some(input) = self.input() {
                input.key(key, false);
                input.flush();
            }
            let _ = key;
        } else {
            ctx.request_repaint_after(until - std::time::Instant::now());
        }
    }

    fn update_texture(&mut self, ctx: &egui::Context) {
        #[cfg(all(unix, not(target_os = "macos")))]
        if let Some(link) = self.run.as_ref().and_then(|r| r.link.as_ref()) {
            let (serial, frame) = link.frame();
            if serial != self.shown
                && let Some(frame) = frame
            {
                self.shown = serial;
                let data = egui::ImageData::Color(Arc::clone(&frame));
                match &mut self.texture {
                    Some(texture) if texture.size() == frame.size => {
                        texture.set(data, egui::TextureOptions::LINEAR)
                    }
                    _ => {
                        self.texture =
                            Some(ctx.load_texture("game-frame", data, egui::TextureOptions::LINEAR))
                    }
                }
                self.frame = Some(frame);
            }
        }
        let _ = ctx;
    }

    /// Windows: the game's window over the picture.
    fn embed(&mut self, ctx: &egui::Context, image: Rect) {
        #[cfg(all(windows, target_pointer_width = "64"))]
        if let (Some(run), Some(parent)) = (self.run.as_mut(), self.parent_window) {
            if run.embedded.as_ref().is_some_and(|e| !e.alive()) {
                run.embedded = None;
            }
            if run.embedded.is_none()
                && let Some(window) = windows::find_window(&run.pids)
            {
                run.embedded = Some(windows::Embedded::attach(window, parent));
            }
            if let Some(embedded) = &run.embedded {
                let ppp = ctx.pixels_per_point();
                embedded.place(Some([
                    (image.min.x * ppp).round() as i32,
                    (image.min.y * ppp).round() as i32,
                    (image.width() * ppp).round() as i32,
                    (image.height() * ppp).round() as i32,
                ]));
            }
        }
        let _ = (ctx, image);
    }

    /// The game takes the mouse and keyboard; the pointer goes to `pos`.
    fn capture(&mut self, ctx: &egui::Context, pos: egui::Pos2, image: Rect) {
        self.captured = true;
        self.rest = Vec2::ZERO;
        self.skip_delta = true;
        ctx.send_viewport_cmd(ViewportCommand::CursorGrab(egui::CursorGrab::Confined));
        ctx.send_viewport_cmd(ViewportCommand::CursorVisible(false));
        #[cfg(all(unix, not(target_os = "macos")))]
        if let (Some(input), Some([x, y])) = (self.input(), keys::to_game(pos, image, self.size)) {
            input.warp(x, y);
            input.flush();
        }
        let _ = (pos, image);
    }

    /// The queued input of this frame to the game.
    fn send_input(&mut self, ctx: &egui::Context, image: Rect) {
        let events = std::mem::take(&mut self.queued);
        let mut raw = Vec2::ZERO;
        let mut escape = false;
        #[cfg(all(unix, not(target_os = "macos")))]
        let input = self
            .run
            .as_ref()
            .and_then(|r| r.link.as_ref())
            .map(|l| l.input());
        #[cfg(not(all(unix, not(target_os = "macos"))))]
        let input: Option<&()> = None;
        for event in events {
            match event {
                Event::Key {
                    key,
                    physical_key,
                    pressed,
                    repeat,
                    modifiers,
                } => {
                    let key = physical_key.unwrap_or(key);
                    if key == Key::Escape && pressed && !modifiers.shift {
                        escape = true;
                        continue;
                    }
                    let Some(sym) = keys::keysym(key) else {
                        continue;
                    };
                    if pressed {
                        if !repeat && self.held_keys.insert(sym) {
                            send_key(input, sym, true);
                        }
                    } else if self.held_keys.remove(&sym) {
                        send_key(input, sym, false);
                    }
                }
                Event::PointerButton {
                    button, pressed, ..
                } => {
                    let b = keys::x_button(button);
                    if pressed {
                        if self.held_buttons.insert(b) {
                            send_button(input, b, true);
                        }
                    } else if self.held_buttons.remove(&b) {
                        send_button(input, b, false);
                    }
                }
                Event::MouseWheel { unit, delta, .. } => {
                    let per_line = match unit {
                        egui::MouseWheelUnit::Line => 1.0,
                        egui::MouseWheelUnit::Page => 0.05,
                        egui::MouseWheelUnit::Point => 50.0,
                    };
                    for b in keys::wheel_buttons(delta, per_line) {
                        send_button(input, b, true);
                        send_button(input, b, false);
                    }
                }
                Event::MouseMoved(delta) => {
                    self.raw_motion = true;
                    raw += delta;
                }
                _ => {}
            }
        }
        // Mouse motion: raw device motion when the system gives it, else
        // the pointer's own motion (it stops at the window's edges).
        let delta = if self.raw_motion {
            raw
        } else if std::mem::take(&mut self.skip_delta) {
            Vec2::ZERO
        } else {
            ctx.input(|i| i.pointer.delta())
        };
        let [dx, dy] = keys::motion(delta, 1.0, &mut self.rest);
        if dx != 0 || dy != 0 {
            send_motion(input, dx, dy);
        }
        // Keep the hidden pointer over the picture.
        let centre = image.center();
        if let Some(pos) = ctx.input(|i| i.pointer.latest_pos())
            && (pos - centre).length() > image.width().min(image.height()) / 4.0
        {
            ctx.send_viewport_cmd(ViewportCommand::CursorPosition(centre));
            self.skip_delta = true;
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        if let Some(input) = input {
            input.flush();
        }
        if escape {
            self.release(ctx);
        }
    }

    /// While the game has the mouse nothing else of the editor reacts to
    /// it: a layer over the whole window takes the pointer.
    fn block_editor(&self, ctx: &egui::Context) {
        let screen = ctx.content_rect();
        egui::Area::new(egui::Id::unique("game-tab-capture"))
            .order(Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                ui.allocate_rect(screen, Sense::click_and_drag());
                ui.painter().text(
                    egui::pos2(screen.center().x, screen.max.y - 24.0),
                    egui::Align2::CENTER_CENTER,
                    tr("gametab.captured"),
                    egui::FontId::proportional(13.0),
                    Color32::from_white_alpha(160),
                );
            });
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn send_key(input: Option<&x11::Input>, sym: u32, down: bool) {
    if let Some(input) = input {
        input.key(sym, down);
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn send_button(input: Option<&x11::Input>, button: u8, down: bool) {
    if let Some(input) = input {
        input.button(button, down);
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn send_motion(input: Option<&x11::Input>, dx: i32, dy: i32) {
    if let Some(input) = input {
        input.motion(dx, dy);
    }
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
fn send_key(_: Option<&()>, _: u32, _: bool) {}

#[cfg(not(all(unix, not(target_os = "macos"))))]
fn send_button(_: Option<&()>, _: u8, _: bool) {}

#[cfg(not(all(unix, not(target_os = "macos"))))]
fn send_motion(_: Option<&()>, _: i32, _: i32) {}

impl Drop for GameView {
    fn drop(&mut self) {
        // The game goes with the editor: its X server cannot outlive it.
        #[cfg(all(unix, not(target_os = "macos")))]
        if let Some(run) = &mut self.run {
            run.game.stop();
        }
    }
}

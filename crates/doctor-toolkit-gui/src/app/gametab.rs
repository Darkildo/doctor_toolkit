//! The game tab and the start of the game (stage 33): where Play starts
//! the game (as usual, straight in a city, at a spawn point), "Play from
//! here" of a level tab, and the tab's buttons.
//!
//! The game reads its options from `cmd.txt` (stage 32,
//! `docs/analysis/launch-flags.md`): one top-level stream (`-stream gf2
//! cuba`; several streams or a child stream crash the game) and a
//! `PlayerSpawn` of that stream (`-playerspawn {GUID}`).

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, channel};

use egui::RichText;

use doctor_toolkit::level::{Location, edit};
use doctor_toolkit::play::start::{self, SpawnPoint, StartOptions};
use doctor_toolkit_mods::i18n::{tr, trf};

use super::dock::{DockOp, Pane};
use super::{BrowserApp, Level};
use crate::game_view::{self, Action, GameView};

/// How the game starts, as the Play options show it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StartMode {
    /// Menus, profile, a save.
    Usual,
    /// Straight in a level (`-stream`), at its default point.
    City,
    /// In a level at a chosen `PlayerSpawn` (`-playerspawn`).
    Point,
}

pub(super) fn start_mode(options: &StartOptions) -> StartMode {
    match (options.streams.is_empty(), options.spawn.is_some()) {
        (true, _) => StartMode::Usual,
        (false, false) => StartMode::City,
        (false, true) => StartMode::Point,
    }
}

/// Switches `options` to `mode`; a level needs a stream: the one chosen
/// before, else `fallback`.
pub(super) fn set_start_mode(options: &mut StartOptions, mode: StartMode, fallback: &str) {
    match mode {
        StartMode::Usual => {
            options.streams.clear();
            options.spawn = None;
        }
        StartMode::City | StartMode::Point => {
            let stream = options
                .streams
                .first()
                .cloned()
                .unwrap_or_else(|| fallback.to_owned());
            options.streams = vec![stream];
            if mode == StartMode::City {
                options.spawn = None;
            }
        }
    }
}

/// The options written for one launch: in a tab the game's screen is the
/// tab's (`-resolution`, 1280×720 unless set), on Windows in a window.
pub(super) fn launch_options(options: &StartOptions, in_tab: bool) -> StartOptions {
    let mut out = options.clone();
    if in_tab {
        let [w, h] = tab_size(options);
        out.resolution = Some((w, h));
        if cfg!(windows) {
            out.windowed = Some(true);
        }
    }
    out
}

/// The size of the game's screen in a tab.
pub(super) fn tab_size(options: &StartOptions) -> [u32; 2] {
    options
        .resolution
        .map(|(w, h)| [w, h])
        .unwrap_or(game_view::DEFAULT_SIZE)
}

/// Name of the Wine desktop of the game tab.
const DESKTOP: &str = "doctor_toolkit";

/// Runs the game of a Wine or Proton `plan` in a Wine virtual desktop of
/// the tab's size (`explorer /desktop=doctor_toolkit,WxH godfather2.exe`).
/// On a bare X server of that size the game stops at "Your Graphics Card
/// does not meet the minimum requirements" (checked on the bench); in a
/// virtual desktop it starts, and its mode changes stay inside it. Other
/// plans (a script of one's own, launchers) are left as they are.
pub(super) fn in_virtual_desktop(plan: &mut doctor_toolkit::play::LaunchPlan, size: &[u32; 2]) {
    let program = std::path::Path::new(&plan.program)
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if !(program.starts_with("wine") || program == "proton") {
        return;
    }
    let Some(at) = plan
        .args
        .iter()
        .position(|a| a.to_string_lossy().to_ascii_lowercase().ends_with(".exe"))
    else {
        return;
    };
    if at > 0 && plan.args[at - 1].to_string_lossy().starts_with("/desktop=") {
        return;
    }
    let desktop = format!("/desktop={DESKTOP},{}x{}", size[0], size[1]);
    plan.args
        .splice(at..at, ["explorer".into(), desktop.into()]);
}

/// Why the start would fail, before the game is started: the rules of
/// stage 32.
pub(super) fn start_problem(options: &StartOptions) -> Option<String> {
    if options.streams.len() > 1 {
        return Some(tr("play.start_several").to_owned());
    }
    let stream = options.streams.first()?;
    let depth = stream
        .replace('\\', "/")
        .split('/')
        .filter(|p| !p.is_empty())
        .count();
    (depth > 2).then(|| trf("play.start_child", &[("stream", stream)]))
}

/// `gf2/cuba — Havana`.
pub(super) fn stream_label(stream: &str) -> String {
    let key = match stream {
        "gf2/cuba" => "play.city_cuba",
        "gf2/miami" => "play.city_miami",
        "gf2/queens" => "play.city_queens",
        s if s.starts_with("mp_") => "play.city_mp",
        _ => return stream.to_owned(),
    };
    format!("{stream} — {}", tr(key))
}

/// One line about a spawn point: number, kind, message, position.
pub(super) fn spawn_label(point: &SpawnPoint, number: usize) -> String {
    let mut parts = vec![format!("№{number}")];
    if point.kind == Some(2) {
        parts.push(tr("play.spawn_default").to_owned());
    }
    if let Some(message) = &point.message {
        parts.push(trf("play.spawn_message", &[("message", message)]));
    }
    if let Some(p) = point.position {
        parts.push(format!("({:.1}, {:.1}, {:.1})", p[0], p[1], p[2]));
    }
    parts.join(" · ")
}

/// One line about where Play starts.
pub(super) fn describe_start(options: &StartOptions, points: Option<&[SpawnPoint]>) -> String {
    match start_mode(options) {
        StartMode::Usual => tr("play.start_is_usual").to_owned(),
        StartMode::City => trf(
            "play.start_is_city",
            &[("stream", &stream_label(&options.streams[0]))],
        ),
        StartMode::Point => {
            let spawn = options.spawn.as_deref().unwrap_or_default();
            let wanted = start::spawn_arg(spawn).ok();
            let point = points.and_then(|ps| {
                ps.iter()
                    .enumerate()
                    .find(|(_, p)| Some(p.arg()) == wanted)
                    .map(|(i, p)| spawn_label(p, i + 1))
            });
            trf(
                "play.start_is_point",
                &[
                    ("stream", &stream_label(&options.streams[0])),
                    ("point", &point.unwrap_or_else(|| spawn.to_owned())),
                ],
            )
        }
    }
}

/// The `PlayerSpawn` objects of one stream, read in the background.
pub(super) enum Spawns {
    Loading(Receiver<Result<Vec<SpawnPoint>, String>>),
    Done(Result<Vec<SpawnPoint>, String>),
}

/// "Play from here" of a level tab: the stream to start in and the
/// camera, waiting for the spawn points and the answer.
pub(super) struct PlayHere {
    stream: String,
    eye: [f32; 3],
    /// The level tab it came from.
    tab: u64,
}

/// What the "Play from here" window answered.
enum HereAnswer {
    Nearest(SpawnPoint),
    Create(SpawnPoint),
    Cancel,
}

impl BrowserApp {
    /// The spawn points of `stream`, read once in the background.
    pub(super) fn spawns(&mut self, stream: &str) -> Option<&Result<Vec<SpawnPoint>, String>> {
        let game = self.game.clone()?;
        let cache: &mut HashMap<String, Spawns> = &mut self.play.spawns;
        let entry = cache.entry(stream.to_owned()).or_insert_with(|| {
            let (tx, rx) = channel();
            let name = stream.to_owned();
            std::thread::spawn(move || {
                let _ = tx.send(start::spawn_points(&game, &name).map_err(|e| e.to_string()));
            });
            Spawns::Loading(rx)
        });
        if let Spawns::Loading(rx) = entry
            && let Ok(result) = rx.try_recv()
        {
            *entry = Spawns::Done(result);
        }
        match entry {
            Spawns::Done(result) => Some(result),
            Spawns::Loading(_) => None,
        }
    }

    /// The game tab.
    pub(super) fn game_pane(&mut self, ui: &mut egui::Ui) {
        egui::Panel::right("live-link")
            .resizable(true)
            .default_size(340.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("live-panel")
                    .show(ui, |ui| self.live_panel(ui));
            });
        let info = self.game_tab_info();
        let action = self.play.game_view.ui(ui, &info);
        match action {
            Some(Action::Play) => self.pending.push(super::Command::Play),
            Some(Action::Stop) => {
                self.play.restart = false;
                self.play.game_view.stop(ui.ctx());
            }
            Some(Action::Restart) => {
                self.play.restart = true;
                self.play.game_view.stop(ui.ctx());
            }
            Some(Action::Screenshot) => self.game_screenshot(),
            Some(Action::Options) => self.play.options_open = true,
            None => {}
        }
    }

    fn game_tab_info(&mut self) -> Info {
        let Some(game) = self.game.clone() else {
            return Info {
                status: tr("gametab.no_game").to_owned(),
                start: String::new(),
                can_play: false,
            };
        };
        let settings = self.settings.game(game.root());
        let options = &settings.play.start;
        let points = match options.streams.first().cloned() {
            Some(stream) if options.spawn.is_some() => {
                self.spawns(&stream).and_then(|r| r.as_ref().ok()).cloned()
            }
            _ => None,
        };
        let mut start = describe_start(options, points.as_deref());
        if !settings.in_tab {
            start += &format!(" · {}", tr("gametab.not_in_tab"));
        }
        // Rendering: DXVK or Wine's OpenGL (llvmpipe in the nested server).
        if cfg!(all(unix, not(target_os = "macos"))) {
            let method = self.play_method();
            if settings.play.dxvk {
                start += &match self.dxvk_check(&method, game.root()) {
                    Ok(found) => format!(" · {}", trf("dxvk.tab_on", &[("source", &found.source)])),
                    Err(problem) => format!(
                        " · {}",
                        trf(
                            "dxvk.not_used",
                            &[("reason", &super::play::dxvk_problem(&problem))]
                        )
                    ),
                };
            } else if settings.in_tab && self.dxvk_check(&method, game.root()).is_ok() {
                start += &format!(" · {}", tr("dxvk.tab_off"));
            }
        }
        let view = &self.play.game_view;
        let status = if view.is_stopping() {
            tr("gametab.stopping").to_owned()
        } else if view.is_running() {
            if self.play.running.is_empty() {
                tr("gametab.starting").to_owned()
            } else {
                tr("gametab.running").to_owned()
            }
        } else if let Some(last) = &self.play.last_exit {
            last.clone()
        } else {
            tr("gametab.not_running").to_owned()
        };
        Info {
            status,
            start,
            can_play: self.enabled(super::Command::Play),
        }
    }

    /// Saves the last frame of the game tab as a PNG in the settings folder.
    fn game_screenshot(&mut self) {
        let Some(([w, h], rgba)) = self.play.game_view.screenshot() else {
            return;
        };
        let dir = crate::settings::config_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("screenshots");
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or_default();
        let path = dir.join(format!("game-{stamp}.png"));
        let result = std::fs::create_dir_all(&dir)
            .map_err(|e| e.to_string())
            .and_then(|()| {
                doctor_toolkit::resource::pixels::encode_png(w, h, &rgba).map_err(|e| e.to_string())
            })
            .and_then(|png| std::fs::write(&path, png).map_err(|e| e.to_string()));
        match result {
            Ok(()) => self.notify(
                Level::Info,
                trf("gametab.screenshot_saved", &[("path", &path.display())]),
            ),
            Err(error) => self.notify(
                Level::Error,
                trf("gametab.screenshot_failed", &[("error", &error)]),
            ),
        }
    }

    /// Opens (or shows) the game tab.
    pub(super) fn show_game_tab(&mut self) {
        self.dock_ops.push(DockOp::ShowPanel(Pane::Game));
    }

    // ------------------------------------------------------ play from here

    /// A level tab asked to play from its camera: the top-level stream of
    /// its zones and the nearest spawn point, or a point of one's own.
    pub(super) fn play_here(&mut self, tab: u64, eye: [f32; 3], streams: &[usize]) {
        let Some(game) = self.game.clone() else {
            return;
        };
        let names: Vec<String> = streams
            .iter()
            .filter_map(|&i| game.streams().get(i))
            .filter_map(|s| start::top_level_stream(&s.stream_name()))
            .collect();
        let levels = start::level_streams(&game);
        let Some(stream) = names.iter().find(|n| levels.contains(n)).cloned() else {
            self.notify(Level::Warning, tr("play.here_no_stream").to_owned());
            return;
        };
        // Fresh points: the workspace may have added some.
        self.play.spawns.remove(&stream);
        self.play.play_here = Some(PlayHere { stream, eye, tab });
    }

    /// The "Play from here" window.
    pub(super) fn play_here_window(&mut self, ctx: &egui::Context) {
        let Some(here) = &self.play.play_here else {
            return;
        };
        let (stream, eye, tab) = (here.stream.clone(), here.eye, here.tab);
        let points = self.spawns(&stream).cloned();
        let mut answer = None;
        egui::Modal::new(egui::Id::unique("play-here")).show(ctx, |ui| {
            ui.set_max_width(560.0);
            ui.heading(tr("play.here_title"));
            ui.label(trf(
                "play.here_text",
                &[
                    ("stream", &stream_label(&stream)),
                    ("x", &format!("{:.1}", eye[0])),
                    ("y", &format!("{:.1}", eye[1])),
                    ("z", &format!("{:.1}", eye[2])),
                ],
            ));
            ui.add_space(6.0);
            match &points {
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(tr("play.spawns_loading"));
                    });
                }
                Some(Err(error)) => {
                    ui.colored_label(doctor_toolkit_mods::BAD, error);
                }
                Some(Ok(points)) => {
                    match start::nearest_spawn(points, eye) {
                        Some((point, distance)) => {
                            let number = points.iter().position(|p| p == point).unwrap_or(0) + 1;
                            ui.label(trf(
                                "play.here_nearest",
                                &[
                                    ("point", &spawn_label(point, number)),
                                    ("distance", &format!("{distance:.0}")),
                                ],
                            ));
                            if ui.button(tr("play.here_use_nearest")).clicked() {
                                answer = Some(HereAnswer::Nearest(point.clone()));
                            }
                        }
                        None => {
                            ui.label(tr("play.here_no_points"));
                        }
                    }
                    ui.add_space(6.0);
                    let template = points
                        .iter()
                        .find(|p| p.kind == Some(2) && p.message.is_none())
                        .or_else(|| points.iter().find(|p| p.message.is_none()));
                    ui.label(RichText::new(tr("play.here_create_text")).weak());
                    if ui
                        .add_enabled(
                            template.is_some(),
                            egui::Button::new(tr("play.here_create")),
                        )
                        .clicked()
                        && let Some(t) = template
                    {
                        answer = Some(HereAnswer::Create(t.clone()));
                    }
                }
            }
            ui.add_space(6.0);
            if ui.button(tr("play.here_cancel")).clicked() {
                answer = Some(HereAnswer::Cancel);
            }
        });
        let Some(answer) = answer else { return };
        self.play.play_here = None;
        let spawn = match answer {
            HereAnswer::Cancel => return,
            HereAnswer::Nearest(point) => point.guid.to_string(),
            HereAnswer::Create(template) => match self.create_spawn(tab, &stream, &template, eye) {
                Ok(guid) => guid,
                Err(error) => {
                    self.notify(Level::Error, trf("play.here_failed", &[("error", &error)]));
                    return;
                }
            },
        };
        let Some(game) = self.game.clone() else {
            return;
        };
        let settings = self.settings.game_mut(game.root());
        settings.play.start.streams = vec![stream];
        settings.play.start.spawn = Some(spawn);
        self.save_settings();
        self.pending.push(super::Command::Play);
    }

    /// A copy of `template` at `eye` in the workspace (`level duplicate`);
    /// Play applies it before the start. Returns its GUID.
    fn create_spawn(
        &mut self,
        tab: u64,
        stream: &str,
        template: &SpawnPoint,
        eye: [f32; 3],
    ) -> Result<String, String> {
        let game = self
            .game
            .clone()
            .ok_or_else(|| tr("gametab.no_game").to_owned())?;
        let index = game
            .find_stream(stream)
            .ok_or_else(|| format!("no stream {stream}"))?;
        let at = template.position.unwrap_or(eye);
        let offset = [eye[0] - at[0], eye[1] - at[1], eye[2] - at[2]];
        let location = Location {
            stream: index,
            resource: template.resource,
        };
        let taken = self.levels.get(&tab).map(|l| l.guids()).unwrap_or_default();
        let ws = self.workspace_mut()?;
        let (copy, guid) = edit::duplicate(&game, ws, location, template.object, offset, &|g| {
            taken.contains(g)
        })
        .map_err(|e| e.to_string())?;
        // The point exists once the workspace is in the game: apply first.
        let play = &mut self.settings.game_mut(game.root()).play;
        play.apply_before = true;
        self.notify(
            Level::Info,
            trf(
                "play.here_created",
                &[("guid", &guid), ("object", &copy), ("stream", &stream)],
            ),
        );
        self.after_level_edit(tab, Some((location, copy)));
        Ok(guid.to_string())
    }
}

/// The start of the game in the Play options: as usual, in a city, at a
/// spawn point; the game tab and the window. Returns whether to read the
/// spawn points again.
pub(super) fn start_editor(
    ui: &mut egui::Ui,
    options: &mut StartOptions,
    in_tab: &mut bool,
    levels: &[String],
    points: Option<&Result<Vec<SpawnPoint>, String>>,
    tab: &Result<String, String>,
) -> bool {
    let mut reload = false;
    let fallback = levels.first().map_or("gf2/cuba", String::as_str);
    ui.strong(tr("play.start"));
    let mut mode = start_mode(options);
    ui.horizontal_wrapped(|ui| {
        ui.radio_value(&mut mode, StartMode::Usual, tr("play.start_usual"))
            .on_hover_text(tr("play.start_usual_hint"));
        ui.radio_value(&mut mode, StartMode::City, tr("play.start_city"))
            .on_hover_text(tr("play.start_city_hint"));
        ui.radio_value(&mut mode, StartMode::Point, tr("play.start_point"))
            .on_hover_text(tr("play.start_point_hint"));
    });
    if mode != start_mode(options) {
        set_start_mode(options, mode, fallback);
        if mode == StartMode::Point && options.spawn.is_none() {
            // The default point of the stream until another is chosen.
            if let Some(Ok(ps)) = points
                && ps.first().is_some_and(|p| {
                    Some(p.stream.as_str()) == options.streams.first().map(String::as_str)
                })
                && let Some(p) = ps.iter().find(|p| p.kind == Some(2)).or(ps.first())
            {
                options.spawn = Some(p.guid.to_string());
            } else {
                options.spawn = Some(String::new());
            }
        }
    }
    if mode != StartMode::Usual {
        let current = options.streams.first().cloned().unwrap_or_default();
        let mut chosen = current.clone();
        ui.horizontal(|ui| {
            ui.label(tr("play.stream"));
            egui::ComboBox::from_id_salt("play-stream")
                .selected_text(stream_label(&current))
                .width(300.0)
                .show_ui(ui, |ui| {
                    for level in levels {
                        ui.selectable_value(&mut chosen, level.clone(), stream_label(level));
                    }
                });
        });
        if chosen != current {
            options.streams = vec![chosen];
            if options.spawn.is_some() {
                options.spawn = Some(String::new());
            }
        }
    }
    if mode == StartMode::Point {
        let wanted = options
            .spawn
            .as_deref()
            .and_then(|s| start::spawn_arg(s).ok());
        match points {
            None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(tr("play.spawns_loading"));
                });
            }
            Some(Err(error)) => {
                ui.colored_label(doctor_toolkit_mods::BAD, error);
            }
            Some(Ok(list)) => {
                ui.horizontal(|ui| {
                    ui.label(trf("play.spawns_count", &[("count", &list.len())]));
                    if ui
                        .small_button(tr("play.spawns_reload_button"))
                        .on_hover_text(tr("play.spawns_reload"))
                        .clicked()
                    {
                        reload = true;
                    }
                });
                egui::ScrollArea::vertical()
                    .id_salt("play-spawns")
                    .max_height(160.0)
                    .show(ui, |ui| {
                        for (i, point) in list.iter().enumerate() {
                            let selected = wanted.as_deref() == Some(point.arg().as_str());
                            if ui
                                .selectable_label(selected, spawn_label(point, i + 1))
                                .on_hover_text(point.arg())
                                .clicked()
                            {
                                options.spawn = Some(point.guid.to_string());
                            }
                        }
                    });
                if wanted.is_some() && !list.iter().any(|p| Some(p.arg()) == wanted) {
                    ui.label(RichText::new(tr("play.spawn_own")).weak());
                }
            }
        }
        let mut text = options.spawn.clone().unwrap_or_default();
        ui.horizontal(|ui| {
            ui.label(tr("play.spawn_guid"));
            ui.add(
                egui::TextEdit::singleline(&mut text)
                    .hint_text("{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}")
                    .desired_width(320.0),
            )
            .on_hover_text(tr("play.spawn_guid_hint"));
        });
        if Some(&text) != options.spawn.as_ref() {
            options.spawn = Some(text.trim().to_owned());
        }
        if let Some(spawn) = options.spawn.as_deref()
            && !spawn.is_empty()
            && let Err(error) = start::spawn_arg(spawn)
        {
            ui.colored_label(doctor_toolkit_mods::BAD, error.to_string());
        }
    }
    if mode != StartMode::Usual {
        ui.label(RichText::new(tr("play.start_rules")).weak().small());
    }
    if let Some(problem) = start_problem(options) {
        ui.colored_label(doctor_toolkit_mods::BAD, problem);
    }
    ui.add_space(4.0);
    ui.strong(tr("play.window"));
    ui.checkbox(in_tab, tr("play.in_tab"))
        .on_hover_text(tr("play.in_tab_hint"));
    if *in_tab {
        match tab {
            Ok(how) => ui.label(
                RichText::new(trf("play.in_tab_with", &[("how", how)]))
                    .weak()
                    .small(),
            ),
            Err(why) => ui.colored_label(doctor_toolkit_mods::WARN, why.as_str()),
        };
    } else {
        let mut windowed = options.windowed == Some(true);
        if ui.checkbox(&mut windowed, tr("play.windowed")).changed() {
            options.windowed = windowed.then_some(true);
        }
    }
    let mut own_size = options.resolution.is_some();
    let [mut w, mut h] = tab_size(options);
    ui.horizontal(|ui| {
        ui.checkbox(&mut own_size, tr("play.resolution"))
            .on_hover_text(tr("play.resolution_hint"));
        ui.add_enabled(own_size, egui::DragValue::new(&mut w).range(320..=7680));
        ui.label("×");
        ui.add_enabled(own_size, egui::DragValue::new(&mut h).range(200..=4320));
    });
    options.resolution = own_size.then_some((w, h));
    reload
}

/// What the tab shows about the game; see [`game_view::Info`].
type Info = game_view::Info;

/// Whether the game can run in a tab with `method`.
pub(super) fn tab_possible(method: &doctor_toolkit::play::LaunchMethod) -> Result<String, String> {
    if method.detached() {
        return Err(tr("gametab.detached").to_owned());
    }
    GameView::available()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guid() -> String {
        "dfccdcee363f1a4e444f43544f52544b".to_owned()
    }

    #[test]
    fn start_choice_makes_cmd_txt() {
        let mut options = StartOptions::default();
        assert_eq!(start_mode(&options), StartMode::Usual);
        assert!(options.is_empty(), "a usual start writes no cmd.txt");

        set_start_mode(&mut options, StartMode::City, "gf2/cuba");
        assert_eq!(start_mode(&options), StartMode::City);
        assert_eq!(options.cmd_txt().unwrap(), "-stream gf2 cuba\r\n");

        options.streams = vec!["gf2/miami".into()];
        set_start_mode(&mut options, StartMode::Point, "gf2/cuba");
        assert_eq!(options.streams, ["gf2/miami"], "the chosen city stays");
        assert_eq!(start_mode(&options), StartMode::City, "no point chosen yet");
        options.spawn = Some(guid());
        assert_eq!(start_mode(&options), StartMode::Point);
        assert_eq!(
            options.cmd_txt().unwrap(),
            "-playerspawn {EEDCCCDF-3F36-4E1A-444F-43544F52544B} -stream gf2 miami\r\n"
        );

        // In a tab the screen is the tab's: 1280×720 unless set.
        let tab = launch_options(&options, true);
        let text = tab.cmd_txt().unwrap();
        assert!(
            text.starts_with(if cfg!(windows) {
                "-windowed -resolution 1280 720 "
            } else {
                "-resolution 1280 720 "
            }),
            "{text}"
        );
        assert!(text.ends_with("-stream gf2 miami\r\n"));
        options.resolution = Some((800, 600));
        assert_eq!(tab_size(&options), [800, 600]);
        assert_eq!(launch_options(&options, false), options);

        // Back to the menus: nothing of the level is left.
        set_start_mode(&mut options, StartMode::Usual, "gf2/cuba");
        assert!(options.streams.is_empty() && options.spawn.is_none());
        // A usual start in a tab still sets the screen.
        let tab = launch_options(&StartOptions::default(), true);
        assert_eq!(tab.resolution, Some((1280, 720)));
    }

    #[test]
    fn wine_runs_the_game_in_a_desktop_of_the_tab() {
        use doctor_toolkit::play::LaunchPlan;
        let plan = |program: &str, args: &[&str]| LaunchPlan {
            program: program.into(),
            args: args.iter().map(Into::into).collect(),
            env: Vec::new(),
            dir: "/games/gf2".into(),
            detached: false,
        };
        let args = |p: &LaunchPlan| {
            p.args
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };
        let mut wine = plan("/usr/bin/wine", &["/games/gf2/godfather2.exe"]);
        in_virtual_desktop(&mut wine, &[1280, 720]);
        assert_eq!(
            args(&wine),
            [
                "explorer",
                "/desktop=doctor_toolkit,1280x720",
                "/games/gf2/godfather2.exe"
            ]
        );
        // Once only.
        in_virtual_desktop(&mut wine, &[1280, 720]);
        assert_eq!(args(&wine).len(), 3);
        let mut proton = plan(
            "/steam/Proton/proton",
            &["run", "/games/gf2/godfather2.exe"],
        );
        in_virtual_desktop(&mut proton, &[800, 600]);
        assert_eq!(
            args(&proton),
            [
                "run",
                "explorer",
                "/desktop=doctor_toolkit,800x600",
                "/games/gf2/godfather2.exe"
            ]
        );
        // A script of one's own is the user's to set up.
        let mut script = plan("tools/bench/play-command.sh", &["/games/gf2"]);
        in_virtual_desktop(&mut script, &[1280, 720]);
        assert_eq!(args(&script), ["/games/gf2"]);
        let mut env = plan("env", &["WINEPREFIX=/p", "wine", "godfather2.exe"]);
        in_virtual_desktop(&mut env, &[1280, 720]);
        assert_eq!(args(&env).len(), 3);
    }

    #[test]
    fn stage_32_rules_are_shown() {
        let mut options = StartOptions {
            streams: vec!["gf2/cuba".into()],
            ..StartOptions::default()
        };
        assert_eq!(start_problem(&options), None);
        options.streams.push("gf2/miami".into());
        assert!(
            start_problem(&options).is_some(),
            "several streams crash the game"
        );
        options.streams = vec!["gf2/cuba/local".into()];
        assert!(
            start_problem(&options).is_some(),
            "a child stream crashes the game"
        );
        assert_eq!(start_problem(&StartOptions::default()), None);
    }

    #[test]
    fn labels() {
        assert!(stream_label("gf2/cuba").starts_with("gf2/cuba — "));
        assert_eq!(stream_label("gf2/other"), "gf2/other");
        let point = SpawnPoint {
            stream: "gf2/cuba".into(),
            guid: doctor_toolkit::resource::simgroup::Guid([0; 16]),
            kind: Some(2),
            message: None,
            position: Some([1.0, 2.0, 3.0]),
            facing: None,
            resource: 6,
            object: 1,
        };
        let label = spawn_label(&point, 3);
        assert!(label.starts_with("№3 · "), "{label}");
        assert!(label.ends_with("(1.0, 2.0, 3.0)"), "{label}");
        let options = StartOptions {
            streams: vec!["gf2/cuba".into()],
            spawn: Some(point.guid.to_string()),
            ..StartOptions::default()
        };
        let text = describe_start(&options, Some(std::slice::from_ref(&point)));
        assert!(text.contains("№1"), "{text}");
    }
}

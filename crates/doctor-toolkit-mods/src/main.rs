//! Open Mod Manager, the mod manager for players: install, order and apply
//! mods, or play with them without writing them into the game. It is part
//! of doctor_toolkit ("Powered by Doctor").
//!
//! Usage: `open-mod-manager [GAME_DIR] [MOD_FILE...]`. Without a game folder
//! the one used last is opened, else the game is looked for; mod files
//! (`.zip`, `.vpat`) given, e.g. by a double click, are offered for
//! installation.
//!
//! `DOCTOR_TOOLKIT_MODS_SHOT=<file.png>` saves a screenshot once the mods are
//! loaded and exits; `DOCTOR_TOOLKIT_MODS_SELECT=<id>` selects a mod first,
//! `DOCTOR_TOOLKIT_MODS_INTEGRITY=1` runs the integrity check first,
//! `DOCTOR_TOOLKIT_MODS_ASK=write` opens the question before writing
//! (`later` closes the question about the executable code of a mod, which
//! opens by itself) and `DOCTOR_TOOLKIT_MODS_SIZE=<w>x<h>` sets the size of
//! the window.

use std::path::PathBuf;
use std::sync::Arc;

use doctor_toolkit::game::GameInstance;
use doctor_toolkit_mods::games::{self, GamePicker, Settings};
use doctor_toolkit_mods::guard::Guarded;
use doctor_toolkit_mods::i18n::{tr, trf};
use doctor_toolkit_mods::play::{PlayChoice, Player};
use doctor_toolkit_mods::{ModsPanel, PanelEvent};
use egui::RichText;

fn main() -> eframe::Result {
    if doctor_toolkit_mods::guard::print_version_or_help(
        "open-mod-manager",
        "open-mod-manager [GAME_DIR] [MOD_FILE...]",
    ) {
        return Ok(());
    }
    doctor_toolkit::report::init("open-mod-manager");
    let config = config_dir();
    let settings = config.as_deref().map(Settings::load).unwrap_or_default();
    doctor_toolkit_mods::i18n::set_language(Some(settings.language.as_str()));
    let args = games::arguments(std::env::args_os().skip(1));
    for path in &args.unknown {
        doctor_toolkit::report::log(&format!("ignored argument {}", path.display()));
    }
    let size = std::env::var("DOCTOR_TOOLKIT_MODS_SIZE")
        .ok()
        .and_then(|s| {
            let (w, h) = s.split_once('x')?;
            Some([w.parse().ok()?, h.parse().ok()?])
        })
        .unwrap_or([1180.0, 780.0]);
    let title = tr("mods_app.title");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(title)
            .with_app_id(doctor_toolkit_mods::icon::Variant::Mods.name())
            .with_icon(doctor_toolkit_mods::icon::icon_data(
                doctor_toolkit_mods::icon::Variant::Mods,
            ))
            .with_inner_size(size)
            .with_min_inner_size([640.0, 420.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        title,
        options,
        Box::new(move |cc| {
            doctor_toolkit_mods::theme::install(&cc.egui_ctx);
            doctor_toolkit_mods::skin::install(&cc.egui_ctx);
            let app = ModsApp::new(cc, args, settings, config);
            Ok(Box::new(Guarded::new(app)))
        }),
    )
}

/// The settings folder, unless a screenshot is taken without an explicit
/// `DOCTOR_TOOLKIT_CONFIG_DIR`.
fn config_dir() -> Option<PathBuf> {
    let shot = std::env::var_os("DOCTOR_TOOLKIT_MODS_SHOT").is_some();
    let explicit = std::env::var_os("DOCTOR_TOOLKIT_CONFIG_DIR").is_some();
    if shot && !explicit {
        None
    } else {
        doctor_toolkit_mods::config_dir()
    }
}

/// Saves a screenshot and exits; used for checks without user input.
struct Capture {
    path: PathBuf,
    integrity: bool,
    ask_write: bool,
    frames: usize,
    requested: bool,
}

struct ModsApp {
    panel: ModsPanel,
    /// Locks writing into the game while it runs.
    watch: doctor_toolkit::play::GameWatch,
    error: Option<String>,
    capture: Option<Capture>,
    /// Settings folder for `mods.toml`; `None` keeps the choices in memory.
    config: Option<PathBuf>,
    /// Interface language, empty for the system's.
    language: String,
    /// The first screen: looking for the game.
    picker: Option<GamePicker>,
    /// The edition of the open game, for the top bar.
    edition: String,
    /// Mod files given on the command line, offered once a game is open.
    pending_mods: Vec<PathBuf>,
    player: Player,
    /// Start the game once the mods are written ("Write and play").
    play_after_write: bool,
    /// DXVK and the graphics of the launch, shared with the editor.
    launch: doctor_toolkit_mods::launch::LaunchWindow,
    /// The edition for the tag of the header: (version, the target one).
    edition_tag: Option<(String, bool)>,
    /// Why the mods cannot be played without writing them, looked up now
    /// and then (it reads the game folder): (when, why).
    obstacle: Option<(std::time::Instant, Option<String>)>,
    /// The last word of the player passed on as a note.
    player_note: Option<String>,
}

impl ModsApp {
    fn new(
        cc: &eframe::CreationContext<'_>,
        args: games::Arguments,
        settings: Settings,
        config: Option<PathBuf>,
    ) -> Self {
        let repaint = cc.egui_ctx.clone();
        let mut app = Self {
            // A screenshot does not depend on a game running elsewhere.
            watch: if std::env::var_os("DOCTOR_TOOLKIT_MODS_SHOT").is_some() {
                doctor_toolkit::play::GameWatch::disabled()
            } else {
                doctor_toolkit::play::GameWatch::start(
                    std::time::Duration::from_millis(1500),
                    move || repaint.request_repaint(),
                )
            },
            panel: ModsPanel::standalone(),
            error: None,
            capture: std::env::var_os("DOCTOR_TOOLKIT_MODS_SHOT").map(|path| Capture {
                path: PathBuf::from(path),
                integrity: std::env::var_os("DOCTOR_TOOLKIT_MODS_INTEGRITY").is_some(),
                ask_write: std::env::var("DOCTOR_TOOLKIT_MODS_ASK").as_deref() == Ok("write"),
                frames: 0,
                requested: false,
            }),
            config,
            language: settings.language.clone(),
            picker: None,
            edition: String::new(),
            pending_mods: args.mods,
            player: Player::default(),
            play_after_write: false,
            launch: Default::default(),
            edition_tag: None,
            obstacle: None,
            player_note: None,
        };
        app.player.in_memory = settings.resources_in_memory;
        app.player.config = app.config.clone();
        let start = args
            .game
            .or_else(|| settings.remembered_game().map(PathBuf::from));
        match start {
            Some(dir) => app.open(dir, &cc.egui_ctx),
            None => app.picker = Some(GamePicker::new(&cc.egui_ctx, true)),
        }
        if let Ok(id) = std::env::var("DOCTOR_TOOLKIT_MODS_SELECT") {
            app.panel.select(&id);
        }
        app
    }

    fn open(&mut self, dir: PathBuf, ctx: &egui::Context) {
        let problem = match GameInstance::open(&dir) {
            Ok(game) if game.streams().is_empty() => {
                Some(trf("mods_app.no_streams", &[("dir", &dir.display())]))
            }
            Ok(game) => {
                doctor_toolkit::report::log_game(&game);
                self.error = None;
                let edition = doctor_toolkit::edition::Edition::identify(game.root());
                self.edition = edition.name();
                let version = edition
                    .known_edition()
                    .map(|k| k.version().to_owned())
                    .or_else(|| edition.executable.as_ref().and_then(|e| e.version.clone()))
                    .unwrap_or_else(|| "?".to_owned());
                let target = edition.known.as_deref()
                    == doctor_toolkit::edition::KNOWN.first().map(|k| k.id);
                self.edition_tag = Some((version, target));
                self.obstacle = None;
                self.panel.set_game(Some(Arc::new(game)), ctx);
                self.picker = None;
                if let Some(config) = &self.config
                    && let Err(error) = games::save_setting(config, "game", &dir.to_string_lossy())
                {
                    self.error = Some(error.to_string());
                }
                self.panel
                    .offer_install(std::mem::take(&mut self.pending_mods));
                None
            }
            Err(error) => {
                doctor_toolkit::report::log(&format!("cannot open {}: {error}", dir.display()));
                Some(trf(
                    "mods_app.cannot_open",
                    &[("dir", &dir.display()), ("error", &error)],
                ))
            }
        };
        if let Some(problem) = problem {
            match &mut self.picker {
                Some(picker) => picker.set_problem(problem),
                None => {
                    let mut picker = GamePicker::new(ctx, false);
                    picker.set_problem(problem);
                    self.picker = Some(picker);
                }
            }
        }
    }

    fn drive_capture(&mut self, ctx: &egui::Context) {
        let Some(capture) = &mut self.capture else {
            return;
        };
        ctx.request_repaint();
        let picking = self.picker.as_ref().is_some_and(GamePicker::is_searching);
        if capture.requested || self.panel.is_busy() || picking {
            return;
        }
        capture.frames += 1;
        if capture.integrity {
            capture.integrity = false;
            self.panel.check_integrity(ctx);
            return;
        }
        if capture.ask_write && self.panel.game().is_some() {
            capture.ask_write = false;
            self.panel.ask_apply();
        }
        if std::env::var("DOCTOR_TOOLKIT_MODS_ASK").as_deref() == Ok("later") {
            self.panel.close_code_question();
        }
        if capture.frames < 12 {
            return;
        }
        capture.requested = true;
        let path = capture.path.clone();
        let callback = egui::ScreenshotCallback::new(move |image| {
            let [w, h] = image.size;
            let code = match doctor_toolkit::resource::pixels::encode_png(w, h, image.as_raw())
                .map_err(|e| e.to_string())
                .and_then(|png| std::fs::write(&path, png).map_err(|e| e.to_string()))
            {
                Ok(()) => 0,
                Err(error) => {
                    eprintln!("capture: {error}");
                    1
                }
            };
            std::process::exit(code);
        });
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(callback));
    }

    /// The header over the mods: the game's logo on its menu tile, the
    /// edition, what Play does now, the Play button and the settings.
    fn header(&mut self, ui: &mut egui::Ui) {
        use doctor_toolkit_mods::skin::{self, Glyph};
        let ctx = ui.ctx().clone();
        let art = self.panel.art(&ctx);
        let rect = ui.max_rect();
        skin::paint_backdrop(ui.painter(), rect, &art);
        let Some(game) = self.panel.game().cloned() else {
            return;
        };
        let (on, all) = self.panel.counts();
        let fresh = self
            .obstacle
            .as_ref()
            .is_some_and(|(at, _)| at.elapsed() < std::time::Duration::from_secs(3));
        if !fresh {
            self.obstacle = Some((
                std::time::Instant::now(),
                doctor_toolkit_mods::play::obstacle(game.root()),
            ));
        }
        let blocked = self.obstacle.as_ref().and_then(|(_, o)| o.clone());
        let how = if all == 0 {
            tr("gf2.how_none").to_owned()
        } else if self.panel.up_to_date() && self.panel.has_written() && on > 0 {
            trf("gf2.how_written", &[("on", &on), ("all", &all)])
        } else if blocked.is_some() {
            trf("gf2.how_write", &[("on", &on), ("all", &all)])
        } else {
            trf("gf2.how_staged", &[("on", &on), ("all", &all)])
        };
        ui.horizontal_centered(|ui| {
            ui.add_space(22.0);
            match &art.logo {
                Some(t) => {
                    let size = t.size_vec2();
                    let h = (rect.height() - 22.0).min(88.0);
                    ui.image((t.id(), egui::vec2(size.x * h / size.y, h)));
                }
                None => {
                    ui.label(
                        RichText::new("The Godfather II")
                            .font(skin::display(&ctx, 30.0))
                            .color(skin::CREAM),
                    );
                }
            }
            ui.add_space(20.0);
            ui.vertical(|ui| {
                ui.set_max_width(420.0);
                ui.add_space((rect.height() - 62.0).max(0.0) / 2.0);
                match &self.edition_tag {
                    Some((version, true)) => skin::tag(
                        ui,
                        Some(Glyph::Check),
                        &trf("gf2.edition_tag_ok", &[("version", version)]),
                        skin::OLIVE,
                    ),
                    Some((version, false)) => skin::tag(
                        ui,
                        Some(Glyph::Warn),
                        &trf("gf2.edition_tag_data", &[("version", version)]),
                        skin::MUSTARD,
                    ),
                    None => skin::tag(ui, None, "", skin::BONE_DIM),
                }
                .on_hover_text(&self.edition);
                ui.add_space(6.0);
                let line = ui.add(
                    egui::Label::new(
                        RichText::new(&how)
                            .size(12.5)
                            .color(egui::Color32::from_rgb(0xd9, 0xcf, 0xb6)),
                    )
                    .wrap(),
                );
                if let Some(why) = &blocked {
                    line.on_hover_text(why);
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(16.0);
                // The settings of the program.
                let (r, gear) =
                    ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::click());
                let color = if gear.hovered() {
                    skin::CREAM
                } else {
                    egui::Color32::from_rgb(0xd9, 0xcf, 0xb6)
                };
                skin::paint_glyph(ui.painter(), r.shrink(5.0), Glyph::Gear, color);
                let gear = gear.on_hover_text(tr("gf2.settings"));
                egui::Popup::menu(&gear)
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                    .show(|ui| self.settings_menu(ui));
                ui.add_space(12.0);
                let busy = self.player.busy_label();
                let locked = self.panel.write_lock().is_some();
                let idle = !self.panel.is_busy() && busy.is_none() && !locked;
                let text = busy.clone().unwrap_or_else(|| tr("gf2.play").to_owned());
                let play = skin::play_button(ui, &text, idle, busy.is_some());
                if play.play.on_hover_text(tr("mods.play_hint")).clicked() {
                    self.player.play_with_mods(game.clone(), &ctx);
                }
                egui::Popup::menu(&play.more)
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
                    .show(|ui| {
                        ui.set_min_width(300.0);
                        if menu_item(ui, true, tr("gf2.menu_launch"), tr("gf2.menu_launch_hint")) {
                            let root = game.root().to_path_buf();
                            self.launch.open(self.config.as_deref(), &root);
                        }
                        ui.separator();
                        if menu_item(
                            ui,
                            idle && all > 0,
                            tr("gf2.menu_write"),
                            tr("gf2.menu_write_hint"),
                        ) {
                            if self.panel.up_to_date() {
                                self.player.play_written(game.clone(), &ctx);
                            } else {
                                self.play_after_write = true;
                                self.panel.apply(&ctx);
                            }
                        }
                        if menu_item(
                            ui,
                            idle && self.panel.has_written(),
                            tr("gf2.menu_restore"),
                            "",
                        ) {
                            self.panel.ask_restore();
                        }
                    });
            });
        });
    }

    /// Language, the mods' resources in memory, crash reports, the game
    /// folder.
    fn settings_menu(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(280.0);
        ui.horizontal(|ui| {
            ui.label(tr("gf2.language"));
            if doctor_toolkit_mods::i18n::language_choice(ui, &mut self.language)
                && let Some(dir) = &self.config
                && let Err(error) = games::save_setting(dir, "language", &self.language)
            {
                self.error = Some(error.to_string());
            }
        });
        // Stage 100: the resources of the mods in memory (off by default),
        // kept in `mods.toml`.
        if ui
            .checkbox(&mut self.player.in_memory, tr("play_mods.in_memory"))
            .on_hover_text(tr("play_mods.in_memory_hint"))
            .changed()
            && let Some(dir) = &self.config
            && let Err(error) = games::save_setting(
                dir,
                "resources_in_memory",
                if self.player.in_memory { "1" } else { "0" },
            )
        {
            self.error = Some(error.to_string());
        }
        if let Some(game) = self.panel.game().cloned()
            && let Some(note) = doctor_toolkit_mods::crash::switch(ui, game.root())
        {
            self.panel.note(note, false);
        }
        ui.separator();
        if let Some(game) = self.panel.game() {
            let folder = game.root().display().to_string();
            ui.label(RichText::new(&folder).small().weak());
        }
        if ui
            .add_enabled(
                !self.panel.is_busy(),
                egui::Button::new(tr("mods_app.change_game")),
            )
            .on_hover_text(tr("mods_app.change_game_hint"))
            .clicked()
        {
            self.picker = Some(GamePicker::new(ui.ctx(), false));
            ui.close();
        }
        ui.separator();
        powered_by(ui);
    }

    /// The bar while the game is chosen: the title, the language, Cancel.
    fn picker_bar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(tr("mods_app.title"))
                    .font(doctor_toolkit_mods::skin::display(ui.ctx(), 22.0))
                    .color(doctor_toolkit_mods::skin::BONE),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if doctor_toolkit_mods::i18n::language_choice(ui, &mut self.language)
                    && let Some(dir) = &self.config
                    && let Err(error) = games::save_setting(dir, "language", &self.language)
                {
                    self.error = Some(error.to_string());
                }
                if self.panel.game().is_some()
                    && self.picker.is_some()
                    && ui.button(tr("mods.cancel")).clicked()
                {
                    self.picker = None;
                }
            });
        });
        ui.add_space(6.0);
    }
}

/// A line of a menu with a quieter second line; true when clicked.
fn menu_item(ui: &mut egui::Ui, enabled: bool, text: &str, hint: &str) -> bool {
    let mut job = RichText::new(text).text().to_owned();
    if !hint.is_empty() {
        job.push('\n');
        job.push_str(hint);
    }
    ui.add_enabled(enabled, egui::Button::new(job).wrap())
        .clicked()
}

/// The quiet "Powered by Doctor"; its hint says what the program is part
/// of.
fn powered_by(ui: &mut egui::Ui) {
    ui.add_space(6.0);
    ui.label(RichText::new(tr("mods_app.powered_by")).small().weak())
        .on_hover_text(trf(
            "mods_app.powered_by_hint",
            &[("version", &doctor_toolkit::report::VERSION)],
        ));
}

impl eframe::App for ModsApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let running = self.watch.running();
        self.player.poll(&running);
        self.panel.set_write_lock((!running.is_empty()).then(|| {
            let pids = running
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            trf("play.locked", &[("pids", &pids)])
        }));
        self.panel.set_play_busy(self.player.busy_label());
        // What the player and the program have to say goes to the note of
        // the panel.
        let said = self.player.message().map(|(m, e)| (m.to_owned(), e));
        if said.as_ref().map(|(m, _)| m) != self.player_note.as_ref() {
            self.player_note = said.as_ref().map(|(m, _)| m.clone());
            if let Some((message, error)) = said {
                self.panel.note(message, error);
            }
        }
        if let Some(error) = self.error.take() {
            self.panel.note(error, true);
        }
        let header = self.picker.is_none() && self.panel.game().is_some();
        egui::Panel::top("game")
            .exact_size(if header { 116.0 } else { 44.0 })
            .frame(
                egui::Frame::NONE
                    .fill(doctor_toolkit_mods::skin::NIGHT)
                    .inner_margin(egui::Margin::symmetric(if header { 0 } else { 14 }, 0)),
            )
            .show(ui, |ui| {
                if header {
                    self.header(ui);
                } else {
                    self.picker_bar(ui);
                }
            });
        let mut event = None;
        egui::CentralPanel::default()
            .frame(
                egui::Frame::central_panel(ui.style())
                    .fill(doctor_toolkit_mods::skin::COAL)
                    .inner_margin(egui::Margin::same(if header { 0 } else { 12 })),
            )
            .show(ui, |ui| {
                if let Some(picker) = &mut self.picker {
                    if let Some(dir) = picker.show(ui) {
                        self.open(dir, ui.ctx());
                    }
                } else {
                    event = self.panel.show(ui, None);
                }
            });
        let ctx = ui.ctx().clone();
        match event {
            Some(PanelEvent::PlayWithMods) => {
                if let Some(game) = self.panel.game().cloned() {
                    self.player.play_with_mods(game, &ctx);
                }
            }
            Some(PanelEvent::GameChanged) => {
                if std::mem::take(&mut self.play_after_write)
                    && self.panel.up_to_date()
                    && let Some(game) = self.panel.game().cloned()
                {
                    self.player.play_written(game, &ctx);
                }
            }
            Some(PanelEvent::WillWrite) | None => {}
        }
        if let Some(error) = self.launch.show(&ctx, self.config.as_deref()) {
            self.error = Some(error);
        }
        if self.player.show_window(&ctx) == Some(PlayChoice::WriteAndPlay) {
            self.play_after_write = true;
            self.panel.apply(&ctx);
        }
        self.drive_capture(&ctx);
    }
}

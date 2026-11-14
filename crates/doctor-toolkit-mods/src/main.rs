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
//! `DOCTOR_TOOLKIT_MODS_ASK=write` opens the question before writing and
//! `DOCTOR_TOOLKIT_MODS_SIZE=<w>x<h>` sets the size of the window.

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
        .unwrap_or([1040.0, 760.0]);
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
            cc.egui_ctx.set_theme(egui::ThemePreference::Dark);
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
        };
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
                self.edition = doctor_toolkit::edition::Edition::identify(game.root()).name();
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

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(tr("mods_app.title"))
                    .size(17.0)
                    .strong()
                    .color(ui.visuals().strong_text_color()),
            );
            doctor_toolkit_mods::theme::separator(ui);
            match self.panel.game() {
                Some(game) => {
                    ui.label(RichText::new(game.root().display().to_string()).strong());
                    if !self.edition.is_empty() {
                        ui.label(RichText::new(&self.edition).weak());
                    }
                    if ui
                        .add_enabled(
                            !self.panel.is_busy() && self.picker.is_none(),
                            egui::Button::new(tr("mods_app.change_game")),
                        )
                        .on_hover_text(tr("mods_app.change_game_hint"))
                        .clicked()
                    {
                        self.picker = Some(GamePicker::new(ui.ctx(), false));
                    }
                    if self.picker.is_some() && ui.button(tr("mods.cancel")).clicked() {
                        self.picker = None;
                    }
                }
                None => {
                    ui.label(RichText::new(tr("mods_app.no_game")).weak());
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if doctor_toolkit_mods::i18n::language_choice(ui, &mut self.language)
                    && let Some(dir) = &self.config
                    && let Err(error) = games::save_setting(dir, "language", &self.language)
                {
                    self.error = Some(error.to_string());
                }
                powered_by(ui);
            });
        });
        ui.add_space(2.0);
        if let Some(error) = &self.error {
            ui.colored_label(doctor_toolkit_mods::BAD, error);
        }
        if let Some((message, error)) = self.player.message() {
            let color = if error {
                doctor_toolkit_mods::BAD
            } else {
                doctor_toolkit_mods::GOOD
            };
            ui.colored_label(color, message);
        }
    }
}

/// The quiet "Powered by Doctor" next to the language choice; its hint says
/// what the program is part of.
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
        let bar = doctor_toolkit_mods::theme::bar_fill(ui.visuals());
        egui::Panel::top("game")
            .frame(
                egui::Frame::side_top_panel(ui.style())
                    .fill(bar)
                    .inner_margin(egui::Margin::symmetric(10, 4)),
            )
            .show(ui, |ui| self.top_bar(ui));
        let mut event = None;
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(ui.style()).inner_margin(egui::Margin::same(12)))
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
        if self.player.show_window(&ctx) == Some(PlayChoice::WriteAndPlay) {
            self.play_after_write = true;
            self.panel.apply(&ctx);
        }
        self.drive_capture(&ctx);
    }
}

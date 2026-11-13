//! Mod manager for players: install, order and apply mods.
//!
//! Usage: `doctor-toolkit-mods [GAME_DIR]`.
//!
//! `DOCTOR_TOOLKIT_MODS_SHOT=<file.png>` saves a screenshot once the mods are
//! loaded and exits; `DOCTOR_TOOLKIT_MODS_SELECT=<id>` selects a mod first and
//! `DOCTOR_TOOLKIT_MODS_INTEGRITY=1` runs the integrity check first.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use doctor_toolkit::game::GameInstance;
use doctor_toolkit_mods::ModsPanel;
use doctor_toolkit_mods::guard::Guarded;
use doctor_toolkit_mods::i18n::{tr, trf};
use egui::RichText;

fn main() -> eframe::Result {
    if doctor_toolkit_mods::guard::print_version_or_help(
        "doctor-toolkit-mods",
        "doctor-toolkit-mods [GAME_DIR]",
    ) {
        return Ok(());
    }
    doctor_toolkit::report::init("doctor-toolkit-mods");
    let config = config_dir();
    let language = config.as_deref().map(saved_language).unwrap_or_default();
    doctor_toolkit_mods::i18n::set_language(Some(language.as_str()));
    let game_dir = std::env::args_os().nth(1).map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("doctor_toolkit mods")
            .with_app_id(doctor_toolkit_mods::icon::Variant::Mods.name())
            .with_icon(doctor_toolkit_mods::icon::icon_data(
                doctor_toolkit_mods::icon::Variant::Mods,
            ))
            .with_inner_size([980.0, 700.0]),
        ..Default::default()
    };
    eframe::run_native(
        "doctor_toolkit mods",
        options,
        Box::new(move |cc| {
            let mut app = ModsApp::new(cc, game_dir);
            app.config = config;
            app.language = language;
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

/// The language chosen in `mods.toml`, else the editor's from `settings.toml`;
/// empty for the system's.
fn saved_language(dir: &Path) -> String {
    ["mods.toml", "settings.toml"]
        .iter()
        .find_map(|file| {
            let text = std::fs::read_to_string(dir.join(file)).ok()?;
            let table: toml::Table = text.parse().ok()?;
            table.get("language")?.as_str().map(str::to_owned)
        })
        .unwrap_or_default()
}

/// Writes `language` into `mods.toml`, keeping its other keys.
fn save_language(dir: &Path, language: &str) -> std::io::Result<()> {
    let path = dir.join("mods.toml");
    let mut table: toml::Table = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| t.parse().ok())
        .unwrap_or_default();
    table.insert("language".into(), language.into());
    std::fs::create_dir_all(dir)?;
    std::fs::write(&path, table.to_string())
}

/// Saves a screenshot and exits; used for checks without user input.
struct Capture {
    path: PathBuf,
    integrity: bool,
    frames: usize,
    requested: bool,
}

struct ModsApp {
    panel: ModsPanel,
    /// Locks writing into the game while it runs.
    watch: doctor_toolkit::play::GameWatch,
    error: Option<String>,
    capture: Option<Capture>,
    /// Settings folder for `mods.toml`; `None` keeps the choice in memory.
    config: Option<PathBuf>,
    /// Interface language, empty for the system's.
    language: String,
}

impl ModsApp {
    fn new(cc: &eframe::CreationContext<'_>, game_dir: Option<PathBuf>) -> Self {
        let repaint = cc.egui_ctx.clone();
        let mut app = Self {
            watch: doctor_toolkit::play::GameWatch::start(
                std::time::Duration::from_millis(1500),
                move || repaint.request_repaint(),
            ),
            panel: ModsPanel::new(),
            error: None,
            capture: std::env::var_os("DOCTOR_TOOLKIT_MODS_SHOT").map(|path| Capture {
                path: PathBuf::from(path),
                integrity: std::env::var_os("DOCTOR_TOOLKIT_MODS_INTEGRITY").is_some(),
                frames: 0,
                requested: false,
            }),
            config: None,
            language: String::new(),
        };
        if let Some(dir) = game_dir {
            app.open(dir, &cc.egui_ctx);
        }
        if let Ok(id) = std::env::var("DOCTOR_TOOLKIT_MODS_SELECT") {
            app.panel.select(&id);
        }
        app
    }

    fn open(&mut self, dir: PathBuf, ctx: &egui::Context) {
        match GameInstance::open(&dir) {
            Ok(game) if game.streams().is_empty() => {
                self.error = Some(trf("mods_app.no_streams", &[("dir", &dir.display())]));
            }
            Ok(game) => {
                doctor_toolkit::report::log_game(&game);
                self.error = None;
                self.panel.set_game(Some(Arc::new(game)), ctx);
            }
            Err(error) => {
                doctor_toolkit::report::log(&format!("cannot open {}: {error}", dir.display()));
                self.error = Some(trf(
                    "mods_app.cannot_open",
                    &[("dir", &dir.display()), ("error", &error)],
                ));
            }
        }
    }

    fn drive_capture(&mut self, ctx: &egui::Context) {
        let Some(capture) = &mut self.capture else {
            return;
        };
        ctx.request_repaint();
        if capture.requested || self.panel.is_busy() {
            return;
        }
        capture.frames += 1;
        if capture.integrity {
            capture.integrity = false;
            self.panel.check_integrity(ctx);
            return;
        }
        if capture.frames < 10 {
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
}

impl eframe::App for ModsApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let running = self.watch.running();
        self.panel.set_write_lock((!running.is_empty()).then(|| {
            let pids = running
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            trf("play.locked", &[("pids", &pids)])
        }));
        egui::Panel::top("game").show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        !self.panel.is_busy(),
                        egui::Button::new(tr("mods_app.choose_game")),
                    )
                    .clicked()
                    && let Some(dir) = rfd::FileDialog::new()
                        .set_title(tr("mods_app.choose_game_title"))
                        .pick_folder()
                {
                    self.open(dir, ui.ctx());
                }
                match self.panel.game() {
                    Some(game) => {
                        ui.label(RichText::new(game.root().display().to_string()).strong());
                        ui.label(RichText::new(game.profile().name).weak());
                    }
                    None => {
                        ui.label(RichText::new(tr("mods_app.no_game")).weak());
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if doctor_toolkit_mods::i18n::language_choice(ui, &mut self.language)
                        && let Some(dir) = &self.config
                        && let Err(error) = save_language(dir, &self.language)
                    {
                        self.error = Some(error.to_string());
                    }
                });
            });
            if let Some(error) = &self.error {
                ui.colored_label(doctor_toolkit_mods::BAD, error);
            }
        });
        egui::CentralPanel::default().show(ui, |ui| {
            self.panel.show(ui, None);
        });
        let ctx = ui.ctx().clone();
        self.drive_capture(&ctx);
    }
}

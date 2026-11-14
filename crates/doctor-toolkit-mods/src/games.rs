//! Finding the game for Open Mod Manager: the folder given on the command
//! line, the one used last time (`game` in `mods.toml`), or the installed
//! copies found on this computer ([`doctor_toolkit::install::detect`]): one
//! is opened at once, several are offered to choose from, none gives a
//! hint and "Choose the game folder…".

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

use egui::RichText;

use doctor_toolkit::edition::Edition;
use doctor_toolkit::install::{self, SearchRoots};

use crate::i18n::{tr, trf, trn};
use crate::widgets::{Prominence, action_button};

/// The settings file of Open Mod Manager, next to the editor's.
pub const SETTINGS_FILE: &str = "mods.toml";

/// What Open Mod Manager remembers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    /// Interface language, empty for the system's.
    pub language: String,
    /// The game folder used last.
    pub game: Option<PathBuf>,
}

impl Settings {
    /// Reads `mods.toml` in `dir`; the language falls back to the editor's
    /// (`settings.toml`).
    pub fn load(dir: &Path) -> Self {
        let table = |file: &str| -> Option<toml::Table> {
            std::fs::read_to_string(dir.join(file)).ok()?.parse().ok()
        };
        let own = table(SETTINGS_FILE).unwrap_or_default();
        let language = own
            .get("language")
            .and_then(|v| v.as_str())
            .map(str::to_owned)
            .or_else(|| {
                table("settings.toml")?
                    .get("language")?
                    .as_str()
                    .map(str::to_owned)
            })
            .unwrap_or_default();
        let game = own
            .get("game")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        Self { language, game }
    }

    /// The remembered game folder, if it still holds the game.
    pub fn remembered_game(&self) -> Option<&Path> {
        self.game
            .as_deref()
            .filter(|p| doctor_toolkit::game::looks_like_game(p).is_some())
    }
}

/// Writes `key = value` into `mods.toml` in `dir`, keeping its other keys.
pub fn save_setting(dir: &Path, key: &str, value: &str) -> std::io::Result<()> {
    let path = dir.join(SETTINGS_FILE);
    let mut table: toml::Table = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| t.parse().ok())
        .unwrap_or_default();
    table.insert(key.into(), value.into());
    std::fs::create_dir_all(dir)?;
    std::fs::write(&path, table.to_string())
}

/// The command line: a game folder, mod files to install, and what is
/// neither.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Arguments {
    pub game: Option<PathBuf>,
    pub mods: Vec<PathBuf>,
    pub unknown: Vec<PathBuf>,
}

/// Sorts the arguments: a folder is the game (the first one), a `.zip` or
/// `.vpat` file a mod to install (opened from the system: a double click,
/// "Open with").
pub fn arguments(args: impl IntoIterator<Item = OsString>) -> Arguments {
    let mut out = Arguments::default();
    for arg in args {
        let path = PathBuf::from(arg);
        if path.is_dir() && out.game.is_none() {
            out.game = Some(path);
        } else if crate::is_mod_file(&path) && path.is_file() {
            out.mods.push(std::path::absolute(&path).unwrap_or(path));
        } else {
            out.unknown.push(path);
        }
    }
    out
}

/// The places searched for the game: those of this computer, and the
/// folders of `$DOCTOR_TOOLKIT_MODS_SEARCH` (separated like `PATH`); only
/// those with `DOCTOR_TOOLKIT_MODS_SEARCH_ONLY=1` (screenshots, checks).
pub fn search_roots() -> SearchRoots {
    let mut roots = if std::env::var_os("DOCTOR_TOOLKIT_MODS_SEARCH_ONLY").is_some() {
        SearchRoots::default()
    } else {
        SearchRoots::from_env()
    };
    if let Some(extra) = std::env::var_os("DOCTOR_TOOLKIT_MODS_SEARCH") {
        roots.extra.extend(std::env::split_paths(&extra));
    }
    roots
}

/// An installed copy of the game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub path: PathBuf,
    /// The edition or version of the executable.
    pub edition: String,
    /// How it was found: `Steam, Wine`.
    pub sources: String,
}

/// Every copy of the game in `roots` (slow: reads launcher files and hashes
/// executables; run it on a worker thread).
pub fn search(roots: &SearchRoots) -> Vec<Found> {
    install::detect(roots)
        .into_iter()
        .map(|i| Found {
            edition: Edition::identify(&i.path).name(),
            sources: sources(&i),
            path: i.path,
        })
        .collect()
}

/// How a copy was found, in the interface language: launchers by name.
fn sources(install: &install::Install) -> String {
    use install::OriginKind;
    let mut kinds: Vec<OriginKind> = install.found_by.iter().map(|o| o.kind).collect();
    kinds.sort();
    kinds.dedup();
    kinds
        .iter()
        .map(|k| match k {
            OriginKind::Registry => tr("mods_app.source_registry"),
            OriginKind::Folder => tr("mods_app.source_folder"),
            _ => k.label(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The first screen without a game: looking for it, the copies found, and
/// choosing a folder.
pub struct GamePicker {
    searching: Option<Receiver<Vec<Found>>>,
    found: Vec<Found>,
    /// Open the only copy found at once (the first start).
    auto: bool,
    /// Why the folder chosen last is not the game.
    problem: Option<String>,
}

impl GamePicker {
    /// Starts looking for the game; with `auto`, a single copy found is
    /// taken without asking.
    pub fn new(ctx: &egui::Context, auto: bool) -> Self {
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(search(&search_roots()));
            ctx.request_repaint();
        });
        Self {
            searching: Some(rx),
            found: Vec::new(),
            auto,
            problem: None,
        }
    }

    /// Whether the search still runs.
    pub fn is_searching(&self) -> bool {
        self.searching.is_some()
    }

    /// Tells why a folder could not be opened.
    pub fn set_problem(&mut self, problem: String) {
        self.problem = Some(problem);
    }

    /// Shows the screen; returns the folder chosen.
    pub fn show(&mut self, ui: &mut egui::Ui) -> Option<PathBuf> {
        if let Some(rx) = &self.searching
            && let Ok(found) = rx.try_recv()
        {
            self.searching = None;
            self.found = found;
            if self.auto && self.found.len() == 1 {
                return Some(self.found[0].path.clone());
            }
        }
        let mut chosen = None;
        ui.vertical_centered(|ui| {
            ui.set_max_width(640.0);
            ui.add_space(40.0);
            ui.label(RichText::new(tr("mods_app.welcome")).size(24.0).strong());
            ui.add_space(6.0);
            ui.label(RichText::new(tr("mods_app.welcome_text")).weak());
            ui.add_space(24.0);
        });
        ui.vertical_centered(|ui| {
            ui.set_max_width(640.0);
            if self.searching.is_some() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(tr("mods_app.searching"));
                });
                return;
            }
            if self.found.is_empty() {
                ui.label(RichText::new(tr("mods_app.not_found")).size(16.0).strong());
                ui.add_space(4.0);
                ui.add(egui::Label::new(tr("mods_app.not_found_hint")).wrap());
            } else {
                ui.label(
                    RichText::new(trn("mods_app.found", self.found.len() as u64, &[]))
                        .size(16.0)
                        .strong(),
                );
                ui.add_space(6.0);
                for (i, found) in self.found.iter().enumerate() {
                    egui::Frame::group(ui.style())
                        .inner_margin(egui::Margin::same(10))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.label(
                                        RichText::new(found.path.display().to_string()).strong(),
                                    );
                                    ui.label(
                                        RichText::new(trf(
                                            "mods_app.found_how",
                                            &[
                                                ("edition", &found.edition),
                                                ("sources", &found.sources),
                                            ],
                                        ))
                                        .weak(),
                                    );
                                });
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        let prominence = if i == 0 {
                                            Prominence::Main
                                        } else {
                                            Prominence::Normal
                                        };
                                        if action_button(
                                            ui,
                                            true,
                                            tr("mods_app.use_game"),
                                            prominence,
                                        )
                                        .clicked()
                                        {
                                            chosen = Some(found.path.clone());
                                        }
                                    },
                                );
                            });
                        });
                    ui.add_space(4.0);
                }
            }
            if let Some(problem) = &self.problem {
                ui.add_space(6.0);
                ui.colored_label(crate::BAD, problem);
            }
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                let prominence = if self.found.is_empty() {
                    Prominence::Main
                } else {
                    Prominence::Normal
                };
                if action_button(ui, true, tr("mods_app.choose_game"), prominence).clicked()
                    && let Some(dir) = rfd::FileDialog::new()
                        .set_title(tr("mods_app.choose_game_title"))
                        .pick_folder()
                {
                    chosen = Some(dir);
                }
                if ui.button(tr("mods_app.search_again")).clicked() {
                    *self = Self::new(ui.ctx(), false);
                }
            });
        });
        chosen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_keep_other_keys_and_find_the_game() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Settings::load(dir.path()), Settings::default());
        std::fs::write(dir.path().join("settings.toml"), "language = \"ru\"\n").unwrap();
        assert_eq!(Settings::load(dir.path()).language, "ru");
        save_setting(dir.path(), "language", "en").unwrap();
        let game = dir.path().join("game");
        std::fs::create_dir_all(&game).unwrap();
        doctor_toolkit::testing::write_game(&game).unwrap();
        save_setting(dir.path(), "game", &game.display().to_string()).unwrap();
        let settings = Settings::load(dir.path());
        assert_eq!(settings.language, "en");
        assert_eq!(settings.remembered_game(), Some(game.as_path()));
        std::fs::remove_dir_all(&game).unwrap();
        assert_eq!(Settings::load(dir.path()).remembered_game(), None);
    }

    #[test]
    fn arguments_are_a_game_folder_and_mod_files() {
        let dir = tempfile::tempdir().unwrap();
        let zip = dir.path().join("a.zip");
        let vpat = dir.path().join("b.VPAT");
        let text = dir.path().join("c.txt");
        for file in [&zip, &vpat, &text] {
            std::fs::write(file, b"x").unwrap();
        }
        let args = arguments(
            [dir.path(), &zip, &vpat, &text, &dir.path().join("gone.zip")]
                .iter()
                .map(|p| p.as_os_str().to_owned()),
        );
        assert_eq!(args.game.as_deref(), Some(dir.path()));
        assert_eq!(args.mods, [zip, vpat]);
        assert_eq!(args.unknown.len(), 2);
    }

    #[test]
    fn the_game_in_an_extra_folder_is_found() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("The Godfather II");
        std::fs::create_dir_all(&game).unwrap();
        doctor_toolkit::testing::write_game(&game).unwrap();
        let roots = SearchRoots {
            extra: vec![game.clone(), dir.path().join("nothing")],
            ..SearchRoots::default()
        };
        let found = search(&roots);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, game);
    }
}

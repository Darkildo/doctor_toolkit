//! Settings of the editor, kept between runs in
//! `$XDG_CONFIG_HOME/doctor-toolkit/settings.toml` (`~/.config/doctor-toolkit`;
//! `%APPDATA%\doctor-toolkit` on Windows, `~/Library/Application Support/doctor-toolkit`
//! on macOS), or in `$DOCTOR_TOOLKIT_CONFIG_DIR`.
//!
//! The file is plain TOML that may be edited by hand; unknown keys are
//! ignored and missing ones take their defaults. The layout of tabs and
//! panels is machine-written and lives next to it in `layout.json`.
//!
//! The screenshot and self-test modes (`DOCTOR_TOOLKIT_GUI_SHOT`,
//! `DOCTOR_TOOLKIT_GUI_SELFTEST`) neither read nor write the files unless
//! `DOCTOR_TOOLKIT_CONFIG_DIR` is set, so they always start from the defaults.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use doctor_toolkit::play::PlayConfig;
use serde::{Deserialize, Serialize};

/// Games remembered in File → Recent games.
pub const RECENT_LIMIT: usize = 10;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Format version of this file.
    pub version: u32,
    /// `dark`, `light` or `system`.
    pub theme: Theme,
    /// Scale of the whole interface (Ctrl+Plus, Ctrl+Minus, Ctrl+0).
    pub ui_scale: f32,
    /// Interface language; empty for the system language (stage 20).
    pub language: String,
    /// `auto`, `wgpu` or `glow`; used at the next start.
    pub renderer: Renderer,
    /// Open the last game, its tabs and selection at start.
    pub reopen_last: bool,
    pub last_game: Option<PathBuf>,
    /// Stream shown last, as its path in the game (`gf2/frontend.str`).
    pub last_stream: Option<String>,
    /// Resource selected last, as a workspace key (`stream:type:ids`).
    pub last_resource: Option<String>,
    /// Most recent first.
    pub recent_games: Vec<PathBuf>,
    pub window: Window,
    pub external_editors: ExternalEditors,
    pub export_dirs: ExportDirs,
    /// The level editor (stage 54).
    pub level: LevelSettings,
    /// The 3D views (stage 66).
    pub view: ViewSettings,
    /// Per game, by the game directory: how to play it, first-run answers.
    pub games: BTreeMap<String, GameSettings>,
}

/// Settings of one game.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GameSettings {
    /// How Play starts the game, and whether it applies and reverts the workspace.
    pub play: PlayConfig,
    /// Do not offer the snapshot of the originals or the old toolkit's
    /// adoption again when the game is opened.
    pub skip_first_run: bool,
    /// Play shows the game in a tab of the editor (stage 33): on Linux on an
    /// X server of its own, on Windows its window inside the editor's. The
    /// size of its screen is `play.start.resolution` (1280×720 by default).
    pub in_tab: bool,
}

impl Default for GameSettings {
    fn default() -> Self {
        Self {
            play: PlayConfig::default(),
            skip_first_run: false,
            in_tab: in_tab_by_default(),
        }
    }
}

/// Linux has the nested X server; Windows embedding is opt-in. Tests start
/// fake games, not X servers.
fn in_tab_by_default() -> bool {
    cfg!(all(unix, not(target_os = "macos"), not(test)))
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: 1,
            theme: Theme::Dark,
            ui_scale: 1.0,
            language: String::new(),
            renderer: Renderer::Auto,
            reopen_last: true,
            last_game: None,
            last_stream: None,
            last_resource: None,
            recent_games: Vec::new(),
            window: Window::default(),
            external_editors: ExternalEditors::default(),
            export_dirs: ExportDirs::default(),
            level: LevelSettings::default(),
            view: ViewSettings::default(),
            games: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Dark,
    Light,
    System,
}

impl Theme {
    pub fn preference(self) -> egui::ThemePreference {
        match self {
            Self::Dark => egui::ThemePreference::Dark,
            Self::Light => egui::ThemePreference::Light,
            Self::System => egui::ThemePreference::System,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Renderer {
    /// wgpu when an adapter exists, otherwise OpenGL.
    Auto,
    Wgpu,
    Glow,
}

/// Size and place of the main window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Window {
    pub width: f32,
    pub height: f32,
    /// Position of the outer top left corner; not every window system reports it.
    pub x: Option<f32>,
    pub y: Option<f32>,
    pub maximized: bool,
}

impl Default for Window {
    fn default() -> Self {
        Self {
            width: 1200.0,
            height: 760.0,
            x: None,
            y: None,
            maximized: true,
        }
    }
}

/// Programs that open exported files (stage 22); an empty command means the
/// desktop's default program. `{file}` in a command is the file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExternalEditors {
    pub image: String,
    pub model: String,
    pub text: String,
    pub audio: String,
}

/// Folders the file dialogs start in; remembered from the last use.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportDirs {
    /// Export and unpack of resources.
    pub export: Option<PathBuf>,
    /// Raw extraction of a stream's resources.
    pub extract: Option<PathBuf>,
    /// Folders packed back into the workspace.
    pub pack: Option<PathBuf>,
    /// Images, models and other files imported.
    pub import: Option<PathBuf>,
}

/// Settings of the level editor.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LevelSettings {
    /// The camera of the level view as before stage 54: right or middle
    /// drag looks round, W A S D Q E always fly, 1 2 3 choose the gizmo;
    /// off (the default): as in Unreal Engine — the letters fly only while
    /// the right button is held, Q W E R choose the gizmo.
    pub classic_camera: bool,
}

/// Settings of the 3D views.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewSettings {
    /// Shade as the game does: its light, sky and fog by the time of day
    /// (stage 66). Off: the simple shading of earlier versions (weak
    /// graphics cards, looking at textures as they are).
    pub game_look: bool,
}

impl Default for ViewSettings {
    fn default() -> Self {
        Self { game_look: true }
    }
}

impl Settings {
    /// Reads settings; a missing file gives the defaults, a broken one the
    /// defaults and the error.
    pub fn load(path: &Path) -> (Self, Option<String>) {
        match std::fs::read_to_string(path) {
            Ok(text) => match toml::from_str::<Settings>(&text) {
                Ok(settings) => (settings.sanitized(), None),
                Err(error) => (
                    Self::default(),
                    Some(format!("{}: {error}", path.display())),
                ),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (Self::default(), None),
            Err(error) => (
                Self::default(),
                Some(format!("{}: {error}", path.display())),
            ),
        }
    }

    /// Writes the settings through a temporary file, so a crash never leaves
    /// half a file.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let text = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        write_atomic(path, text.as_bytes())
    }

    fn sanitized(mut self) -> Self {
        if !(0.5..=3.0).contains(&self.ui_scale) {
            self.ui_scale = 1.0;
        }
        self.recent_games.truncate(RECENT_LIMIT);
        self
    }

    /// The settings of the game in `dir`.
    pub fn game(&self, dir: &Path) -> GameSettings {
        self.games
            .get(&dir.to_string_lossy().into_owned())
            .cloned()
            .unwrap_or_default()
    }

    /// The settings of the game in `dir`, to change.
    pub fn game_mut(&mut self, dir: &Path) -> &mut GameSettings {
        self.games
            .entry(dir.to_string_lossy().into_owned())
            .or_default()
    }

    /// Puts `dir` first in the recent games and makes it the last game.
    pub fn add_recent(&mut self, dir: &Path) {
        self.recent_games.retain(|d| d != dir);
        self.recent_games.insert(0, dir.to_owned());
        self.recent_games.truncate(RECENT_LIMIT);
        if self.last_game.as_deref() != Some(dir) {
            self.last_stream = None;
            self.last_resource = None;
        }
        self.last_game = Some(dir.to_owned());
    }
}

/// Writes `bytes` to `path` through a temporary file in the same folder.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// Folder of the settings files; `None` when no home folder is known.
pub fn config_dir() -> Option<PathBuf> {
    doctor_toolkit_mods::config_dir()
}

/// Where the settings of this run are read from and written to.
#[derive(Debug, Clone)]
pub struct Store {
    /// `None`: the settings live only in memory (screenshot and test modes).
    pub dir: Option<PathBuf>,
}

impl Store {
    /// The store for this run: the config folder, unless an automatic mode
    /// runs without an explicit `DOCTOR_TOOLKIT_CONFIG_DIR`.
    pub fn for_run() -> Self {
        let automatic = ["DOCTOR_TOOLKIT_GUI_SHOT", "DOCTOR_TOOLKIT_GUI_SELFTEST"]
            .iter()
            .any(|v| std::env::var_os(v).is_some());
        let explicit = std::env::var_os("DOCTOR_TOOLKIT_CONFIG_DIR").is_some();
        Self {
            dir: if automatic && !explicit {
                None
            } else {
                config_dir()
            },
        }
    }

    pub fn settings_path(&self) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join("settings.toml"))
    }

    pub fn layout_path(&self) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join("layout.json"))
    }

    pub fn load(&self) -> (Settings, Option<String>) {
        match self.settings_path() {
            Some(path) => Settings::load(&path),
            None => (Settings::default(), None),
        }
    }

    pub fn save(&self, settings: &Settings) -> Result<(), String> {
        match self.settings_path() {
            Some(path) => settings
                .save(&path)
                .map_err(|e| format!("{}: {e}", path.display())),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dt-settings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn round_trip() {
        let dir = temp_dir("round-trip");
        let path = dir.join("settings.toml");
        let mut settings = Settings {
            theme: Theme::Light,
            ui_scale: 1.25,
            renderer: Renderer::Glow,
            ..Settings::default()
        };
        settings.add_recent(Path::new("/games/a"));
        settings.add_recent(Path::new("/games/b"));
        settings.last_stream = Some("gf2/frontend.str".into());
        settings.last_resource =
            Some("gf2/frontend:apx:00000000.00000000.00000000.12345678".into());
        settings.window.maximized = false;
        settings.window.x = Some(10.0);
        settings.external_editors.image = "gimp {file}".into();
        settings.export_dirs.export = Some("/tmp/out".into());
        let game = settings.game_mut(Path::new("/games/a"));
        game.play.method = doctor_toolkit::play::LaunchMethod::Wine {
            prefix: Some("/prefixes/gf2".into()),
            wine: None,
            dll_overrides: Some("dinput8=n,b".into()),
        };
        game.play.apply_before = true;
        game.skip_first_run = true;
        settings.game_mut(Path::new("/games/b")).play.method =
            doctor_toolkit::play::LaunchMethod::Steam { app_id: 24830 };
        settings.save(&path).unwrap();
        let (loaded, error) = Settings::load(&path);
        assert_eq!(error, None);
        assert_eq!(loaded, settings);
        assert_eq!(
            loaded.recent_games,
            [PathBuf::from("/games/b"), PathBuf::from("/games/a")]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_partial_and_broken_files() {
        let dir = temp_dir("partial");
        let path = dir.join("settings.toml");
        assert_eq!(Settings::load(&path), (Settings::default(), None));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "theme = \"light\"\nui_scale = 9.0\nfuture_key = 1\n").unwrap();
        let (loaded, error) = Settings::load(&path);
        assert_eq!(error, None);
        assert_eq!(loaded.theme, Theme::Light);
        assert_eq!(loaded.ui_scale, 1.0, "out-of-range scale falls back");
        assert!(loaded.reopen_last);
        std::fs::write(&path, "theme = 3").unwrap();
        let (loaded, error) = Settings::load(&path);
        assert_eq!(loaded, Settings::default());
        assert!(error.is_some());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn recent_games_are_unique_and_bounded() {
        let mut settings = Settings::default();
        for i in 0..15 {
            settings.add_recent(Path::new(&format!("/g/{i}")));
        }
        settings.add_recent(Path::new("/g/7"));
        assert_eq!(settings.recent_games.len(), RECENT_LIMIT);
        assert_eq!(settings.recent_games[0], PathBuf::from("/g/7"));
        assert_eq!(
            settings
                .recent_games
                .iter()
                .filter(|p| p.ends_with("7"))
                .count(),
            1
        );
    }
}

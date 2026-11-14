//! Every action of the editor shell: the menus, the command palette and the
//! keyboard shortcuts are built from [`COMMANDS`], and [`BrowserApp::run`]
//! carries them out.
//!
//! Adding a command: a variant of [`Command`], a row in [`COMMANDS`] with its
//! text key (`cmd.*` in `crates/doctor-toolkit-mods/i18n/en.toml`), menu and
//! shortcut, and an arm in [`BrowserApp::run`]. The tests check that every
//! command has a row, a text and a shortcut no other command uses.

use egui::{Key, KeyboardShortcut, Modifiers};

use doctor_toolkit_mods::i18n::{tr, trf, trn};

use super::dock::{DockOp, Pane};
use super::{BrowserApp, Level};
use crate::settings::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Command {
    // File
    OpenGame,
    OpenStream,
    Save,
    Export,
    Unpack,
    Pack,
    CloseTab,
    Quit,
    // Edit
    Undo,
    Redo,
    Preferences,
    // View
    Palette,
    Back,
    Forward,
    ToggleFiles,
    ToggleDetails,
    ToggleChanges,
    ToggleLog,
    ToggleOutliner,
    ToggleObjectDetails,
    ContentDrawer,
    ShowWelcome,
    ThemeDark,
    ThemeLight,
    ThemeSystem,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    Maximize,
    Fullscreen,
    SaveLayout,
    ResetLayout,
    // Level
    GizmoSelect,
    GizmoMove,
    GizmoRotate,
    GizmoScale,
    GizmoLocal,
    GizmoSnap,
    // Tools
    Search,
    NewSearch,
    AllResources,
    LevelEditor,
    NewLevel,
    LogicEditor,
    ScriptEditor,
    Movies,
    Project,
    Mods,
    IntegrityCheck,
    RebuildIndex,
    ApplyToGame,
    RevertGame,
    RestoreOriginals,
    Play,
    PlayOptions,
    FindGames,
    // Wizards
    Tasks,
    WizardRetexture,
    WizardModel,
    WizardOutfit,
    WizardParams,
    WizardTexts,
    WizardPlace,
    WizardPackage,
    // Help
    UserGuide,
    Shortcuts,
    ShowLog,
    About,
}

/// The menu a command is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Menu {
    File,
    Edit,
    View,
    Level,
    Tools,
    Wizards,
    Help,
}

impl Menu {
    pub const ALL: [Menu; 7] = [
        Menu::File,
        Menu::Edit,
        Menu::View,
        Menu::Level,
        Menu::Tools,
        Menu::Wizards,
        Menu::Help,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::File => "menu.file",
            Self::Edit => "menu.edit",
            Self::View => "menu.view",
            Self::Level => "menu.level",
            Self::Tools => "menu.tools",
            Self::Wizards => "menu.wizards",
            Self::Help => "menu.help",
        }
    }
}

/// How a shortcut is matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Always, even while a text field has the keyboard: the key means
    /// nothing to text fields.
    Global,
    /// Only when no text field has the keyboard (text fields use it).
    NotInText,
    /// egui handles it (zoom); listed for the menus and the cheat sheet.
    Egui,
}

pub struct Spec {
    pub command: Command,
    /// Text key of the label.
    pub key: &'static str,
    pub menu: Menu,
    pub shortcuts: &'static [KeyboardShortcut],
    pub scope: Scope,
    /// Starts a group in its menu.
    pub separator: bool,
}

const CTRL: Modifiers = Modifiers::COMMAND;
const CTRL_SHIFT: Modifiers = Modifiers {
    shift: true,
    ..Modifiers::COMMAND
};
const ALT: Modifiers = Modifiers::ALT;
const NONE: Modifiers = Modifiers::NONE;

const fn k(modifiers: Modifiers, key: Key) -> KeyboardShortcut {
    KeyboardShortcut::new(modifiers, key)
}

const fn spec(
    command: Command,
    key: &'static str,
    menu: Menu,
    shortcuts: &'static [KeyboardShortcut],
    scope: Scope,
) -> Spec {
    Spec {
        command,
        key,
        menu,
        shortcuts,
        scope,
        separator: false,
    }
}

const fn group(mut s: Spec) -> Spec {
    s.separator = true;
    s
}

use Command as C;
use Scope::{Egui, Global, NotInText};

/// Every command, in menu order.
pub const COMMANDS: &[Spec] = &[
    spec(
        C::OpenGame,
        "cmd.open_game",
        Menu::File,
        &[k(CTRL, Key::O)],
        Global,
    ),
    spec(
        C::OpenStream,
        "cmd.open_stream",
        Menu::File,
        &[k(CTRL_SHIFT, Key::O)],
        Global,
    ),
    spec(C::Save, "cmd.save", Menu::File, &[], Global),
    group(spec(
        C::Export,
        "cmd.export",
        Menu::File,
        &[k(CTRL, Key::E)],
        Global,
    )),
    spec(C::Unpack, "cmd.unpack", Menu::File, &[], Global),
    spec(C::Pack, "cmd.pack", Menu::File, &[], Global),
    group(spec(
        C::CloseTab,
        "cmd.close_tab",
        Menu::File,
        &[k(CTRL, Key::W)],
        Global,
    )),
    spec(C::Quit, "cmd.quit", Menu::File, &[k(CTRL, Key::Q)], Global),
    spec(
        C::Undo,
        "cmd.undo",
        Menu::Edit,
        &[k(CTRL, Key::Z)],
        NotInText,
    ),
    spec(
        C::Redo,
        "cmd.redo",
        Menu::Edit,
        &[k(CTRL_SHIFT, Key::Z), k(CTRL, Key::Y)],
        NotInText,
    ),
    group(spec(
        C::Preferences,
        "cmd.preferences",
        Menu::Edit,
        &[k(CTRL, Key::Comma)],
        Global,
    )),
    spec(
        C::Palette,
        "cmd.palette",
        Menu::View,
        &[k(CTRL_SHIFT, Key::P)],
        Global,
    ),
    spec(
        C::Back,
        "cmd.back",
        Menu::View,
        &[k(ALT, Key::ArrowLeft)],
        NotInText,
    ),
    spec(
        C::Forward,
        "cmd.forward",
        Menu::View,
        &[k(ALT, Key::ArrowRight)],
        NotInText,
    ),
    group(spec(
        C::ToggleFiles,
        "cmd.toggle_files",
        Menu::View,
        &[k(CTRL, Key::Num1)],
        Global,
    )),
    spec(
        C::ToggleDetails,
        "cmd.toggle_details",
        Menu::View,
        &[k(CTRL, Key::Num2)],
        Global,
    ),
    spec(
        C::ToggleChanges,
        "cmd.toggle_changes",
        Menu::View,
        &[k(CTRL, Key::Num3)],
        Global,
    ),
    spec(
        C::ToggleLog,
        "cmd.toggle_log",
        Menu::View,
        &[k(CTRL, Key::Num4)],
        Global,
    ),
    spec(
        C::ToggleOutliner,
        "cmd.toggle_outliner",
        Menu::View,
        &[k(CTRL, Key::Num5)],
        Global,
    ),
    spec(
        C::ToggleObjectDetails,
        "cmd.toggle_object",
        Menu::View,
        &[k(CTRL, Key::Num6)],
        Global,
    ),
    spec(
        C::ContentDrawer,
        "cmd.content_drawer",
        Menu::View,
        &[k(CTRL, Key::Space)],
        Global,
    ),
    spec(C::ShowWelcome, "cmd.welcome", Menu::View, &[], Global),
    group(spec(
        C::ThemeDark,
        "cmd.theme_dark",
        Menu::View,
        &[],
        Global,
    )),
    spec(C::ThemeLight, "cmd.theme_light", Menu::View, &[], Global),
    spec(C::ThemeSystem, "cmd.theme_system", Menu::View, &[], Global),
    group(spec(
        C::ZoomIn,
        "cmd.zoom_in",
        Menu::View,
        &[k(CTRL, Key::Plus), k(CTRL, Key::Equals)],
        Egui,
    )),
    spec(
        C::ZoomOut,
        "cmd.zoom_out",
        Menu::View,
        &[k(CTRL, Key::Minus)],
        Egui,
    ),
    spec(
        C::ZoomReset,
        "cmd.zoom_reset",
        Menu::View,
        &[k(CTRL, Key::Num0)],
        Egui,
    ),
    group(spec(
        C::Maximize,
        "cmd.maximize",
        Menu::View,
        &[k(CTRL, Key::M)],
        Global,
    )),
    spec(
        C::Fullscreen,
        "cmd.fullscreen",
        Menu::View,
        &[k(NONE, Key::F11)],
        Global,
    ),
    group(spec(
        C::SaveLayout,
        "cmd.save_layout",
        Menu::View,
        &[],
        Global,
    )),
    spec(C::ResetLayout, "cmd.reset_layout", Menu::View, &[], Global),
    spec(C::GizmoSelect, "cmd.gizmo_select", Menu::Level, &[], Global),
    spec(C::GizmoMove, "cmd.gizmo_move", Menu::Level, &[], Global),
    spec(C::GizmoRotate, "cmd.gizmo_rotate", Menu::Level, &[], Global),
    spec(C::GizmoScale, "cmd.gizmo_scale", Menu::Level, &[], Global),
    group(spec(
        C::GizmoLocal,
        "cmd.gizmo_local",
        Menu::Level,
        &[],
        Global,
    )),
    spec(C::GizmoSnap, "cmd.gizmo_snap", Menu::Level, &[], Global),
    spec(
        C::Search,
        "cmd.search",
        Menu::Tools,
        &[k(CTRL, Key::F)],
        Global,
    ),
    spec(
        C::NewSearch,
        "cmd.new_search",
        Menu::Tools,
        &[k(CTRL_SHIFT, Key::F)],
        Global,
    ),
    spec(
        C::AllResources,
        "cmd.all_resources",
        Menu::Tools,
        &[k(CTRL_SHIFT, Key::R)],
        Global,
    ),
    spec(
        C::LevelEditor,
        "cmd.level_editor",
        Menu::Tools,
        &[k(CTRL, Key::L)],
        Global,
    ),
    spec(
        C::NewLevel,
        "cmd.new_level",
        Menu::Tools,
        &[k(CTRL_SHIFT, Key::L)],
        Global,
    ),
    spec(
        C::LogicEditor,
        "cmd.logic_editor",
        Menu::Tools,
        &[k(CTRL, Key::G)],
        Global,
    ),
    spec(
        C::ScriptEditor,
        "cmd.script_editor",
        Menu::Tools,
        &[],
        Global,
    ),
    spec(C::Movies, "cmd.movies", Menu::Tools, &[], Global),
    spec(C::Project, "project.cmd", Menu::Tools, &[], Global),
    group(spec(
        C::Mods,
        "cmd.mods",
        Menu::Tools,
        &[k(CTRL_SHIFT, Key::M)],
        Global,
    )),
    spec(C::IntegrityCheck, "cmd.integrity", Menu::Tools, &[], Global),
    spec(
        C::RebuildIndex,
        "cmd.rebuild_index",
        Menu::Tools,
        &[],
        Global,
    ),
    group(spec(C::ApplyToGame, "cmd.apply", Menu::Tools, &[], Global)),
    spec(C::RevertGame, "cmd.revert_game", Menu::Tools, &[], Global),
    spec(C::RestoreOriginals, "cmd.restore", Menu::Tools, &[], Global),
    group(spec(
        C::Play,
        "cmd.play",
        Menu::Tools,
        &[k(NONE, Key::F5)],
        Global,
    )),
    spec(C::PlayOptions, "cmd.play_options", Menu::Tools, &[], Global),
    spec(C::FindGames, "cmd.find_games", Menu::Tools, &[], Global),
    spec(
        C::Tasks,
        "cmd.tasks",
        Menu::Wizards,
        &[k(NONE, Key::F2)],
        Global,
    ),
    group(spec(
        C::WizardRetexture,
        "cmd.wizard_retexture",
        Menu::Wizards,
        &[],
        Global,
    )),
    spec(
        C::WizardModel,
        "cmd.wizard_model",
        Menu::Wizards,
        &[],
        Global,
    ),
    spec(
        C::WizardOutfit,
        "cmd.wizard_outfit",
        Menu::Wizards,
        &[],
        Global,
    ),
    spec(
        C::WizardParams,
        "cmd.wizard_params",
        Menu::Wizards,
        &[],
        Global,
    ),
    spec(
        C::WizardTexts,
        "cmd.wizard_texts",
        Menu::Wizards,
        &[],
        Global,
    ),
    spec(
        C::WizardPlace,
        "cmd.wizard_place",
        Menu::Wizards,
        &[],
        Global,
    ),
    group(spec(
        C::WizardPackage,
        "cmd.wizard_package",
        Menu::Wizards,
        &[],
        Global,
    )),
    spec(
        C::UserGuide,
        "cmd.guide",
        Menu::Help,
        &[k(NONE, Key::F1)],
        Global,
    ),
    spec(
        C::Shortcuts,
        "cmd.shortcuts",
        Menu::Help,
        &[k(CTRL, Key::Slash)],
        Global,
    ),
    spec(C::ShowLog, "cmd.show_log", Menu::Help, &[], Global),
    group(spec(C::About, "cmd.about", Menu::Help, &[], Global)),
];

impl Command {
    pub fn spec(self) -> &'static Spec {
        COMMANDS
            .iter()
            .find(|s| s.command == self)
            .expect("every command has a row in COMMANDS")
    }

    pub fn label(self) -> &'static str {
        tr(self.spec().key)
    }

    /// The first shortcut, as the platform writes it.
    pub fn shortcut_text(self, ctx: &egui::Context) -> Option<String> {
        self.spec()
            .shortcuts
            .first()
            .map(|s| ctx.format_shortcut(s))
    }
}

/// Shortcuts to check, those with more modifiers first: egui matches
/// Ctrl+Z also when Shift is held, so Ctrl+Shift+Z must be tried before it.
fn shortcut_order() -> Vec<(KeyboardShortcut, &'static Spec)> {
    let mut all: Vec<_> = COMMANDS
        .iter()
        .filter(|s| s.scope != Scope::Egui)
        .flat_map(|s| s.shortcuts.iter().map(move |k| (*k, s)))
        .collect();
    let weight = |m: Modifiers| m.alt as u8 + m.shift as u8 + m.command as u8 + m.ctrl as u8;
    all.sort_by_key(|(k, _)| std::cmp::Reverse(weight(k.modifiers)));
    all
}

impl BrowserApp {
    /// Takes the shortcuts of this frame before any widget sees them.
    pub(super) fn shortcuts(&mut self, ctx: &egui::Context) {
        let in_text = ctx.egui_wants_keyboard_input();
        for (shortcut, spec) in shortcut_order() {
            if spec.scope == Scope::NotInText && in_text {
                continue;
            }
            if ctx.input_mut(|i| i.consume_shortcut(&shortcut)) {
                self.pending.push(spec.command);
            }
        }
    }

    /// Whether a command can run now.
    pub(super) fn enabled(&self, command: Command) -> bool {
        let game = self.game.is_some();
        let idle = self.job.is_none();
        match command {
            C::OpenGame => idle,
            C::OpenStream
            | C::AllResources
            | C::LevelEditor
            | C::NewLevel
            | C::LogicEditor
            | C::Search
            | C::NewSearch => game,
            C::Export => game && (!self.nav.selected.is_empty() || self.details.is_some()),
            C::Unpack => game && !self.nav.selected.is_empty(),
            C::Pack => game && idle,
            C::Undo => self
                .workspace
                .as_ref()
                .is_some_and(|ws| ws.undo_label().is_some()),
            C::Redo => self
                .workspace
                .as_ref()
                .is_some_and(|ws| ws.redo_label().is_some()),
            C::Back => self.nav.can_go(false),
            C::Forward => self.nav.can_go(true),
            C::Mods | C::IntegrityCheck => game && idle,
            C::GizmoSelect
            | C::GizmoMove
            | C::GizmoRotate
            | C::GizmoScale
            | C::GizmoLocal
            | C::GizmoSnap => self.front_level().is_some(),
            C::Movies => game,
            C::Project => true,
            C::RestoreOriginals => game && idle && self.play.running.is_empty(),
            C::Play => game && idle && self.play.running.is_empty() && !self.play.launched(),
            C::PlayOptions => game,
            C::WizardRetexture
            | C::WizardModel
            | C::WizardOutfit
            | C::WizardParams
            | C::WizardTexts
            | C::WizardPlace => game,
            C::WizardPackage => game && self.workspace.as_ref().is_some_and(|ws| !ws.is_empty()),
            C::RebuildIndex => game && !self.nav.busy(),
            C::ApplyToGame => {
                self.play.running.is_empty()
                    && self
                        .ws_status
                        .as_ref()
                        .is_some_and(|s| !s.is_up_to_date() || !s.conflicts().is_empty())
            }
            C::RevertGame => {
                self.play.running.is_empty()
                    && self.ws_status.as_ref().is_some_and(|s| {
                        s.streams
                            .iter()
                            .any(|st| st.state == doctor_toolkit::workspace::StreamState::Applied)
                    })
            }
            _ => true,
        }
    }

    /// Whether a command is a switch that is on now (for check marks).
    pub(super) fn checked(&self, command: Command) -> Option<bool> {
        let theme = self.settings.theme;
        match command {
            C::ThemeDark => Some(theme == Theme::Dark),
            C::ThemeLight => Some(theme == Theme::Light),
            C::ThemeSystem => Some(theme == Theme::System),
            C::ToggleFiles => Some(self.panel_shown(&Pane::Files)),
            C::ToggleDetails => Some(self.panel_shown(&Pane::Details)),
            C::ToggleChanges => Some(self.panel_shown(&Pane::Changes)),
            C::ToggleLog => Some(self.panel_shown(&Pane::Log)),
            C::ToggleOutliner => Some(self.panel_shown(&Pane::Outliner)),
            C::ToggleObjectDetails => Some(self.panel_shown(&Pane::ObjectDetails)),
            C::ContentDrawer => Some(self.panel_shown(&Pane::ContentBrowser)),
            C::Maximize => Some(self.maximized.is_some()),
            C::GizmoSelect | C::GizmoMove | C::GizmoRotate | C::GizmoScale => {
                let level = self.levels.get(&self.front_level()?)?;
                Some(level.gizmo_mode() == gizmo_mode(command)?)
            }
            C::GizmoLocal => {
                let level = self.levels.get(&self.front_level()?)?;
                Some(level.gizmo_space() == crate::level_view::GizmoSpace::Local)
            }
            C::GizmoSnap => {
                let level = self.levels.get(&self.front_level()?)?;
                Some(level.snap().on)
            }
            _ => None,
        }
    }

    /// Whether a panel is shown: its tile in front, or its drawer open.
    pub(super) fn panel_shown(&self, pane: &Pane) -> bool {
        if self.drawers.contains(pane) {
            return self.drawers.is_open(pane);
        }
        self.dock.as_ref().is_some_and(|tree| {
            self.maximized
                .is_none_or(|m| super::dock::pane(tree, m) == Some(pane))
                && super::dock::find(tree, |p| p == pane)
                    .is_some_and(|t| super::dock::is_shown(tree, t))
        })
    }

    /// Carries out a command.
    pub(super) fn run(&mut self, command: Command, ctx: &egui::Context) {
        if !self.enabled(command) {
            return;
        }
        match command {
            C::OpenGame => {
                let mut dialog = rfd::FileDialog::new().set_title(tr("dialog.game_dir"));
                if let Some(dir) = self.settings.recent_games.first() {
                    dialog = dialog.set_directory(dir);
                }
                if let Some(dir) = dialog.pick_folder() {
                    self.open_game(dir, ctx);
                }
            }
            C::OpenStream => self.palette.open(super::palette::Mode::Streams),
            C::Export => {
                if let Some(game) = self.game.clone() {
                    let list: Vec<_> = if self.nav.selected.is_empty() {
                        self.details.iter().map(|d| d.loc).collect()
                    } else {
                        self.nav.selected.iter().copied().collect()
                    };
                    self.nav.export(&game, list);
                }
            }
            C::Unpack => {
                if let Some(game) = self.game.clone() {
                    let list = self.nav.selected.iter().copied().collect();
                    self.nav.export(&game, list);
                }
            }
            C::Pack => self.pack_dialog(ctx),
            C::CloseTab => self.dock_ops.push(DockOp::CloseActive),
            C::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            C::Undo => self.undo(),
            C::Redo => self.redo(),
            C::Preferences => self.windows.preferences = true,
            C::Palette => self.palette.open(super::palette::Mode::All),
            C::Back => {
                if let Some(action) = self.nav.go_back() {
                    self.nav_action(action);
                }
            }
            C::Forward => {
                if let Some(action) = self.nav.go_forward() {
                    self.nav_action(action);
                }
            }
            C::ToggleFiles => self.dock_ops.push(DockOp::TogglePanel(Pane::Files)),
            C::ToggleDetails => self.dock_ops.push(DockOp::TogglePanel(Pane::Details)),
            C::ToggleChanges => self.dock_ops.push(DockOp::TogglePanel(Pane::Changes)),
            C::ToggleLog => self.dock_ops.push(DockOp::TogglePanel(Pane::Log)),
            C::ToggleOutliner => self.dock_ops.push(DockOp::TogglePanel(Pane::Outliner)),
            C::ToggleObjectDetails => self.dock_ops.push(DockOp::TogglePanel(Pane::ObjectDetails)),
            C::ContentDrawer => self.toggle_content(),
            C::Save => self.save_all(),
            C::SaveLayout => {
                let current = self.layouts.current.clone();
                self.layout_window.show(&current);
            }
            C::GizmoSelect | C::GizmoMove | C::GizmoRotate | C::GizmoScale => {
                if let Some(mode) = gizmo_mode(command)
                    && let Some(level) = self.front_level().and_then(|id| self.levels.get_mut(&id))
                {
                    level.set_gizmo_mode(mode);
                }
            }
            C::GizmoLocal => {
                use crate::level_view::GizmoSpace;
                if let Some(level) = self.front_level().and_then(|id| self.levels.get_mut(&id)) {
                    let space = match level.gizmo_space() {
                        GizmoSpace::World => GizmoSpace::Local,
                        GizmoSpace::Local => GizmoSpace::World,
                    };
                    level.set_gizmo_space(space);
                }
            }
            C::GizmoSnap => {
                if let Some(level) = self.front_level().and_then(|id| self.levels.get_mut(&id)) {
                    let mut snap = level.snap();
                    snap.on = !snap.on;
                    level.set_snap(snap);
                }
            }
            C::ShowWelcome => self.dock_ops.push(DockOp::ShowPanel(Pane::Welcome)),
            C::ThemeDark | C::ThemeLight | C::ThemeSystem => {
                self.settings.theme = match command {
                    C::ThemeDark => Theme::Dark,
                    C::ThemeLight => Theme::Light,
                    _ => Theme::System,
                };
                ctx.set_theme(self.settings.theme.preference());
            }
            C::ZoomIn => egui::gui_zoom::zoom_in(ctx),
            C::ZoomOut => egui::gui_zoom::zoom_out(ctx),
            C::ZoomReset => ctx.set_zoom_factor(1.0),
            C::Maximize => {
                if self.maximized.is_some() {
                    self.maximized = None;
                } else {
                    self.dock_ops.push(DockOp::Maximize(None));
                }
            }
            C::Fullscreen => {
                let full = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!full));
            }
            C::ResetLayout => self.dock_ops.push(DockOp::Reset),
            C::Search => self.nav.focus_search = true,
            C::NewSearch => self.search(String::new(), true),
            C::AllResources => self.show_index(),
            C::LevelEditor => self.open_level_tab(false),
            C::NewLevel => self.open_level_tab(true),
            C::LogicEditor => self.open_logic_tab("", None),
            C::ScriptEditor => self.script_editor.open = !self.script_editor.open,
            C::Movies => self.movies.toggle(),
            C::Project => self.open_project_tab(),
            C::Mods => self.mods_open = !self.mods_open,
            C::IntegrityCheck => self.integrity_check(),
            C::RebuildIndex => {
                if let Some(game) = self.game.clone()
                    && self.nav.rebuild(game)
                {
                    self.notify(Level::Info, tr("status.rebuilding_index").to_owned());
                }
            }
            C::ApplyToGame => {
                let force = self
                    .ws_status
                    .as_ref()
                    .is_some_and(|s| !s.conflicts().is_empty());
                if force {
                    // Overwriting foreign changes needs the Changes panel's confirmation.
                    self.dock_ops.push(DockOp::ShowPanel(Pane::Changes));
                    self.notify(Level::Warning, tr("status.apply_conflicts").to_owned());
                } else {
                    self.apply(false);
                }
            }
            C::RevertGame => self.unapply(),
            C::RestoreOriginals => self.windows.confirm_restore = true,
            C::Play => self.play(ctx),
            C::PlayOptions => self.play.options_open = true,
            C::FindGames => self.find_games(),
            C::Tasks => self.open_task_list(),
            C::WizardRetexture
            | C::WizardModel
            | C::WizardOutfit
            | C::WizardParams
            | C::WizardTexts
            | C::WizardPlace
            | C::WizardPackage => {
                if let Some(kind) = super::wizard::Kind::ALL
                    .into_iter()
                    .find(|k| k.command() == command)
                {
                    // From the resource shown in Details, when it fits.
                    let start = super::wizard::Start {
                        loc: self.details.as_ref().map(|d| d.loc),
                        model: None,
                    };
                    self.start_wizard(kind, start);
                }
            }
            C::UserGuide => self.open_guide(),
            C::Shortcuts => self.windows.shortcuts = !self.windows.shortcuts,
            C::ShowLog => self.open_log_file(),
            C::About => self.windows.about = true,
        }
    }

    /// Shows the level tab (a new one when `new` or when there is none): in
    /// the layout of the level screen, or alone in the window when layouts
    /// are not remembered per screen.
    fn open_level_tab(&mut self, new: bool) {
        let existing = self
            .levels
            .keys()
            .copied()
            .find(|&id| !new && self.live(id));
        let id = match existing {
            Some(id) => {
                self.dock_ops.push(DockOp::Focus(id));
                id
            }
            None => {
                let id = self.new_pane_id();
                self.dock_ops.push(DockOp::Open(Pane::Level { id }));
                id
            }
        };
        if !self.layouts.remember {
            self.dock_ops.push(DockOp::Maximize(Some(id)));
        }
    }

    /// File → Save: edits are in the workspace as soon as they are made;
    /// this writes the settings and the layout and says what is kept where.
    fn save_all(&mut self) {
        self.saved.1.clear();
        self.save_settings();
        let unsaved = self.details.iter().any(|d| d.has_unsaved_edits())
            || self.docs.values().any(|d| d.has_unsaved_edits());
        let count = self
            .workspace
            .as_ref()
            .map_or(0, doctor_toolkit::workspace::Workspace::len);
        let mut text = trn("edits.saved", count as u64, &[]);
        if unsaved {
            text += &format!("; {}", tr("edits.saved_unsaved"));
        }
        self.notify(if unsaved { Level::Warning } else { Level::Info }, text);
    }

    /// File → Pack…: records an unpacked, edited folder in the workspace.
    fn pack_dialog(&mut self, ctx: &egui::Context) {
        let Some(game) = self.game.clone() else {
            return;
        };
        let mut dialog = rfd::FileDialog::new().set_title(tr("dialog.pack_dir"));
        if let Some(dir) = self.settings.export_dirs.pack.as_ref().or(self
            .settings
            .export_dirs
            .export
            .as_ref())
        {
            dialog = dialog.set_directory(dir);
        }
        if let Some(dir) = dialog.pick_folder() {
            self.settings.export_dirs.pack = Some(dir.clone());
            self.job = Some(crate::jobs::pack(game, dir, ctx, &self.tx));
        }
    }

    /// The pack job finished: the workspace on disk changed.
    pub(super) fn packed(
        &mut self,
        result: Result<
            (
                doctor_toolkit::navigation::unpack::PackReport,
                std::path::PathBuf,
            ),
            String,
        >,
    ) {
        match result {
            Ok((report, dir)) => {
                let recorded = report.changed.iter().filter(|f| f.recorded).count();
                self.open_workspace();
                self.after_edit(None);
                let text = trf(
                    "edits.packed",
                    &[
                        ("dir", &dir.display()),
                        ("recorded", &trn("count.files", recorded as u64, &[])),
                        ("unchanged", &report.unchanged),
                    ],
                );
                match report.warnings.first() {
                    Some(warning) => self.notify(
                        Level::Warning,
                        text + &trf("edits.applied_warning", &[("warning", warning)]),
                    ),
                    None => self.notify(Level::Info, text),
                }
            }
            Err(error) => self.notify(Level::Error, trf("edits.cannot_pack", &[("error", &error)])),
        }
    }
}

/// The gizmo mode of a gizmo command.
fn gizmo_mode(command: Command) -> Option<crate::level_view::GizmoMode> {
    use crate::level_view::GizmoMode as M;
    match command {
        C::GizmoSelect => Some(M::Select),
        C::GizmoMove => Some(M::Move),
        C::GizmoRotate => Some(M::Rotate),
        C::GizmoScale => Some(M::Scale),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_has_one_row_and_a_text() {
        let all = [
            C::OpenGame,
            C::OpenStream,
            C::Save,
            C::ToggleOutliner,
            C::ToggleObjectDetails,
            C::ContentDrawer,
            C::SaveLayout,
            C::GizmoSelect,
            C::GizmoMove,
            C::GizmoRotate,
            C::GizmoScale,
            C::GizmoLocal,
            C::GizmoSnap,
            C::Export,
            C::Unpack,
            C::Pack,
            C::CloseTab,
            C::Quit,
            C::Undo,
            C::Redo,
            C::Preferences,
            C::Palette,
            C::Back,
            C::Forward,
            C::ToggleFiles,
            C::ToggleDetails,
            C::ToggleChanges,
            C::ToggleLog,
            C::ShowWelcome,
            C::ThemeDark,
            C::ThemeLight,
            C::ThemeSystem,
            C::ZoomIn,
            C::ZoomOut,
            C::ZoomReset,
            C::Maximize,
            C::Fullscreen,
            C::ResetLayout,
            C::Search,
            C::NewSearch,
            C::AllResources,
            C::LevelEditor,
            C::NewLevel,
            C::LogicEditor,
            C::ScriptEditor,
            C::Movies,
            C::Project,
            C::Mods,
            C::IntegrityCheck,
            C::RebuildIndex,
            C::ApplyToGame,
            C::RevertGame,
            C::RestoreOriginals,
            C::Play,
            C::PlayOptions,
            C::FindGames,
            C::Tasks,
            C::WizardRetexture,
            C::WizardModel,
            C::WizardOutfit,
            C::WizardParams,
            C::WizardTexts,
            C::WizardPlace,
            C::WizardPackage,
            C::UserGuide,
            C::Shortcuts,
            C::ShowLog,
            C::About,
        ];
        assert_eq!(all.len(), COMMANDS.len());
        for command in all {
            assert_eq!(
                COMMANDS.iter().filter(|s| s.command == command).count(),
                1,
                "{command:?}"
            );
            let key = command.spec().key;
            assert!(doctor_toolkit_mods::i18n::has_key(key), "no text for {key}");
        }
        for menu in Menu::ALL {
            assert!(doctor_toolkit_mods::i18n::has_key(menu.key()));
            assert!(COMMANDS.iter().any(|s| s.menu == menu), "{menu:?} is empty");
        }
    }

    #[test]
    fn shortcuts_do_not_conflict() {
        let mut seen: Vec<(KeyboardShortcut, Command)> = Vec::new();
        for spec in COMMANDS {
            for &shortcut in spec.shortcuts {
                if let Some((_, other)) = seen.iter().find(|(s, _)| *s == shortcut) {
                    panic!("{shortcut:?} is used by {other:?} and {:?}", spec.command);
                }
                seen.push((shortcut, spec.command));
            }
        }
    }

    #[test]
    fn text_editing_keys_are_left_to_text_fields() {
        // Keys a text field uses must not fire while one has the keyboard.
        let text_keys = [
            Key::Z,
            Key::Y,
            Key::A,
            Key::C,
            Key::V,
            Key::X,
            Key::ArrowLeft,
        ];
        for spec in COMMANDS {
            for shortcut in spec.shortcuts {
                if text_keys.contains(&shortcut.logical_key) {
                    assert_ne!(spec.scope, Scope::Global, "{:?}", spec.command);
                }
                // A key without Ctrl or Alt would be typed into text fields.
                let plain = !shortcut.modifiers.command && !shortcut.modifiers.alt;
                let function_key = format!("{:?}", shortcut.logical_key).starts_with('F');
                assert!(!plain || function_key, "{:?}", spec.command);
            }
        }
    }

    #[test]
    fn more_specific_shortcuts_are_tried_first() {
        let order = shortcut_order();
        let pos = |command: Command, key: Key, shift: bool| {
            order
                .iter()
                .position(|(k, s)| {
                    s.command == command && k.logical_key == key && k.modifiers.shift == shift
                })
                .unwrap()
        };
        assert!(pos(C::Redo, Key::Z, true) < pos(C::Undo, Key::Z, false));
        assert!(pos(C::OpenStream, Key::O, true) < pos(C::OpenGame, Key::O, false));
    }
}

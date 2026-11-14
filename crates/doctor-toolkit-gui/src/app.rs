//! Application state and the frame.
//!
//! The window is a menu bar, a toolbar, the dock of tabs and panels and a
//! status bar. The parts live in submodules:
//!
//! - `commands`: every action of the menus, the palette and the shortcuts;
//! - `menu`, `toolbar`: menu bar and the main toolbar (stage 52);
//! - `dock`: tabs and dockable panels; `layouts`: built-in and named
//!   layouts, the layout of each screen and how they are saved; `drawer`:
//!   panels that slide up from the status bar (stage 52);
//! - `theme`: the look in the manner of Unreal Engine 5 and the icons;
//! - `content`: the Content Browser panels with previews (stage 55);
//!   `dnd`: the payload of an asset dragged out of them;
//! - `browse`: file list, stream tabs, all resources, search tabs, details;
//! - `docs`: an opened resource with its views ([`crate::views`]) and the
//!   open stream files; `asset_tab`: a resource in its own tab, an asset
//!   editor with a bar of tools and Details (stage 56);
//! - `edits`, `level`, `mods`, `model_import`: edits recorded in the
//!   workspace and the windows that make them;
//! - `palette`, `shell`: the command palette, welcome screen, notifications,
//!   status bar, preferences and help windows;
//! - `play`: games found on the computer, the edition and first run of a
//!   game, Play and the lock on writing while the game runs;
//! - `wizard`: guided wizards for typical modding tasks.

mod asset_tab;
#[cfg(test)]
mod asset_tests;
mod bridge;
mod browse;
#[cfg(test)]
mod cine_tests;
mod cinema;
mod commands;
mod content;
mod details;
pub mod dnd;
mod dock;
mod docs;
mod drawer;
mod edits;
mod external;
mod gametab;
mod history;
mod layouts;
mod level;
mod live;
mod live_debug;
mod live_state;
#[cfg(test)]
mod view_tests;
pub use live::LiveMarks;
mod logic;
mod menu;
mod model_import;
mod mods;
mod outliner;
mod palette;
mod play;
mod project;
mod scripts;
mod shell;
pub mod theme;
mod toolbar;
mod wizard;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use doctor_toolkit::game::{GameInstance, ResourceIndex, TypeSummary};
use doctor_toolkit::toc::Toc;
use doctor_toolkit::workspace::{Status, Workspace};
use doctor_toolkit_mods::i18n::{tr, trf, trn};

use crate::changes_view::ChangesWindow;
use crate::jobs::{self, Job, JobResult};
use crate::level_view::LevelView;
use crate::nav::{Location, Nav, NavAction, Search};
use crate::settings::{Settings, Store};

pub use commands::Command;
use dock::{DockOp, Pane};
use docs::{ResourceDoc, StreamCache};
use shell::{Level, Notes};

const ROW_HEIGHT: f32 = 18.0;
/// Resources larger than this are not loaded into the hex view automatically.
const AUTO_PREVIEW_LIMIT: usize = 64 << 20;
/// Settings and layout are written at most this often while running.
const AUTOSAVE: Duration = Duration::from_secs(30);

/// The file shown by a stream tab.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FileSel {
    Stream(usize),
    Toc(usize),
}

struct OpenStream {
    game_index: usize,
    /// Indices of the resources that pass the filter.
    rows: Vec<usize>,
    type_filter: Option<u32>,
    /// Types present in the stream with their counts, for the filter menu.
    types: Vec<(u32, usize)>,
    /// Resources the workspace changes.
    changed: HashSet<usize>,
}

/// A stream tab: the resources of a stream, or a table of contents.
#[derive(Default)]
struct StreamTab {
    sel: Option<FileSel>,
    stream: Option<OpenStream>,
    toc: Option<(usize, Result<Toc, String>)>,
    selected: Option<usize>,
    scroll_to: Option<usize>,
    /// Path of the file in the game, from a saved layout, to open once the game is.
    restore: Option<String>,
}

/// Frame-time measurement for the self-test mode.
struct SelfTest {
    stream: String,
    started: Option<Instant>,
    frame: usize,
    frame_times: Vec<Duration>,
    open_time: Duration,
    done: bool,
}

/// Saves a screenshot of the window and exits; used for checks without user input.
///
/// `DOCTOR_TOOLKIT_GUI_SHOT=<file.png>` enables it, `DOCTOR_TOOLKIT_GUI_SELECT=<index>`
/// selects a resource of the initial stream first and `DOCTOR_TOOLKIT_GUI_SHOT_FRAMES`
/// sets the number of frames before the screenshot (20).
/// `DOCTOR_TOOLKIT_GUI_SHELL=<window>[,…]` opens shell windows first:
/// `palette[=text]`, `prefs`, `about`, `shortcuts`, `welcome`, `log`, `menu`.
struct Capture {
    path: PathBuf,
    select: Option<usize>,
    frames: usize,
    delay: usize,
    requested: bool,
}

pub struct BrowserApp {
    tx: Sender<JobResult>,
    rx: Receiver<JobResult>,
    job: Option<Job>,
    /// The latest message; every change is kept by `notes` too.
    status: String,

    settings: Settings,
    store: Store,
    /// When the settings and layout were saved last, and what was saved.
    saved: (Instant, String),
    notes: Notes,
    dock: Option<egui_tiles::Tree<Pane>>,
    next_pane: u64,
    dock_ops: Vec<DockOp>,
    /// Built-in and named layouts, the layout of each screen.
    layouts: layouts::Layouts,
    layout_window: layouts::LayoutWindow,
    /// Panels shown as drawers over the bottom of the window.
    drawers: drawer::Drawers,
    /// The Content Browsers and their previews (stage 55).
    content: content::Content,
    /// The level tab drawn last, and the level the Outliner, Zones and
    /// Details panels show (stage 53).
    last_level: Option<u64>,
    panel_level: Option<u64>,
    /// The Outliner of each level view.
    outliners: HashMap<u64, outliner::Outline>,
    /// The Details panels.
    inspectors: details::Inspectors,
    /// What was selected last (Details follows it), and the selection of
    /// each level view as Details saw it.
    inspect: details::Focus,
    /// The main Details panel is open and follows the selection: the
    /// editors of the tabs leave their side column out (stage 56).
    details_open: bool,
    /// Asset commands of the tab menus, run after the dock is drawn.
    asset_cmds: Vec<(Location, asset_tab::AssetCmd)>,
    selection_seen: HashMap<u64, u64>,
    /// The tab shown alone, in place of the whole dock.
    maximized: Option<egui_tiles::TileId>,
    /// The document tab used last; new documents open next to it.
    active_doc: Option<egui_tiles::TileId>,
    /// Ids of the panes in the dock, after the last frame.
    live_panes: HashSet<u64>,
    palette: palette::Palette,
    windows: shell::Windows,
    /// Commands asked for during the frame, run at its end.
    pending: Vec<Command>,

    game: Option<Arc<GameInstance>>,
    index: Option<Arc<ResourceIndex>>,
    type_summary: Vec<TypeSummary>,
    files: StreamCache,
    file_filter: String,
    file_rows: Vec<FileSel>,
    selected_file: Option<FileSel>,
    streams: HashMap<u64, StreamTab>,
    /// The stream tab used last; the file list and links open files in it.
    focus_stream: Option<u64>,
    /// The resource shown in the Details panel.
    details: Option<ResourceDoc>,
    /// Resources open in their own tabs.
    docs: HashMap<u64, ResourceDoc>,
    searches: HashMap<u64, Search>,
    focus_search: Option<u64>,
    levels: HashMap<u64, LevelView>,
    /// Logic graph tabs.
    logics: HashMap<u64, crate::logic_editor::LogicEditor>,
    /// `DOCTOR_TOOLKIT_GUI_LEVEL` went to a level tab already.
    level_preset_used: bool,
    /// Material and texture lookup across the game, built on first use.
    assets: Option<doctor_toolkit::game::GameAssets>,
    /// Edits kept on disk; opened with the game, created by the first edit.
    workspace: Option<Workspace>,
    /// The lock of the workspace while the editor has it open: the command
    /// line and the MCP server do not write it meanwhile.
    ws_lock: Option<doctor_toolkit::workspace::WorkspaceLock>,
    /// The bridge with the MCP server (`docs/mcp.md` §6).
    bridge: bridge::BridgeState,
    /// The workspace compared with the game files.
    ws_status: Option<Status>,
    changes: ChangesWindow,
    /// Direct mode: every change is also written into the game.
    direct: bool,
    /// The mod manager of the open game.
    mods: doctor_toolkit_mods::ModsPanel,
    mods_open: bool,
    /// The Movies window (`movies/*.vp6`).
    movies: crate::video_view::MoviesWindow,
    /// The integrity check waits for the Mods window to read the mods.
    integrity_queued: bool,
    model_import: model_import::ModelImport,
    script_editor: crate::script_editor::ScriptEditor,
    /// Resources linked to working files of other programs.
    external: external::Links,

    index_type: Option<u32>,
    index_rows: Vec<usize>,
    /// Game index, history, multi-selection and export.
    nav: Nav,

    selftest: Option<SelfTest>,
    /// The 3D renderer; `None` when eframe does not run on wgpu.
    gpu: Option<crate::render::Gpu>,
    /// Stream to open as soon as the game is indexed.
    initial_stream: Option<String>,
    /// Restore the last stream and selection of the settings with the game.
    restore_selection: bool,
    capture: Option<Capture>,
    /// The screenshot mode opens the selected resource in a tab.
    pin_on_capture: bool,
    /// ... and maximizes that tab.
    maximize_pinned: bool,
    /// The program icon, for the welcome screen and About.
    icon: Option<egui::TextureHandle>,
    /// A resource tab with unsaved edits whose closing was warned about.
    close_warned: Option<u64>,
    /// Play, detection, edition and first run.
    play: play::PlayState,
    /// Wizard tabs.
    wizards: wizard::Wizards,
    /// The project browser.
    project: project::ProjectPanel,
    /// The cut-scene edited in the level view (stage 62).
    cine: Option<cinema::Cinema>,
    /// Its play head, shared with the cut-scene tabs.
    cine_cursor: crate::cutscene_view::SharedCursor,
    /// "Open in the level view" asked for this shot (opened next frame).
    cine_open: Option<Location>,
}

impl BrowserApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        game_dir: Option<PathBuf>,
        initial_stream: Option<String>,
        selftest: Option<String>,
        settings: Settings,
        store: Store,
    ) -> Self {
        let ctx = &cc.egui_ctx;
        theme::install(ctx);
        ctx.set_theme(settings.theme.preference());
        ctx.set_zoom_factor(settings.ui_scale);
        let (tx, rx) = channel();
        let gpu = crate::render::Gpu::from_creation_context(cc);
        let content = content::Content::new(ctx, gpu.as_ref().map(crate::render::Gpu::context));
        let loaded = layouts::load(&store);
        let mut drawers = drawer::Drawers::default();
        drawers.set(loaded.drawers);
        let mut app = Self {
            tx,
            rx,
            job: None,
            status: String::new(),
            settings,
            store,
            saved: (Instant::now(), String::new()),
            notes: Notes::for_app(),
            dock: Some(loaded.tree),
            next_pane: loaded.next_pane,
            dock_ops: Vec::new(),
            layouts: loaded.layouts,
            layout_window: layouts::LayoutWindow::default(),
            drawers,
            content,
            last_level: None,
            panel_level: None,
            outliners: HashMap::new(),
            inspectors: details::Inspectors::default(),
            inspect: details::Focus::default(),
            details_open: false,
            asset_cmds: Vec::new(),
            selection_seen: HashMap::new(),
            maximized: None,
            active_doc: None,
            live_panes: HashSet::new(),
            palette: palette::Palette::default(),
            windows: shell::Windows::default(),
            pending: Vec::new(),
            game: None,
            index: None,
            type_summary: Vec::new(),
            files: StreamCache::default(),
            file_filter: String::new(),
            file_rows: Vec::new(),
            selected_file: None,
            streams: HashMap::new(),
            focus_stream: None,
            details: None,
            docs: HashMap::new(),
            searches: HashMap::new(),
            focus_search: None,
            levels: HashMap::new(),
            logics: HashMap::new(),
            level_preset_used: false,
            assets: None,
            workspace: None,
            ws_lock: None,
            bridge: bridge::BridgeState::default(),
            ws_status: None,
            changes: ChangesWindow::default(),
            direct: false,
            mods: doctor_toolkit_mods::ModsPanel::new(),
            mods_open: std::env::var_os("DOCTOR_TOOLKIT_GUI_MODS").is_some(),
            movies: crate::video_view::MoviesWindow::new(),
            integrity_queued: false,
            model_import: Default::default(),
            script_editor: scripts::editor_from_env(),
            external: external::Links::new(ctx),
            index_type: None,
            index_rows: Vec::new(),
            nav: Nav::new(ctx),
            selftest: selftest.map(|stream| SelfTest {
                stream,
                started: None,
                frame: 0,
                frame_times: Vec::new(),
                open_time: Duration::ZERO,
                done: false,
            }),
            gpu,
            initial_stream,
            restore_selection: false,
            capture: std::env::var_os("DOCTOR_TOOLKIT_GUI_SHOT").map(|path| Capture {
                path: PathBuf::from(path),
                select: std::env::var("DOCTOR_TOOLKIT_GUI_SELECT")
                    .ok()
                    .and_then(|v| v.parse().ok()),
                frames: 0,
                delay: std::env::var("DOCTOR_TOOLKIT_GUI_SHOT_FRAMES")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(20),
                requested: false,
            }),
            pin_on_capture: false,
            maximize_pinned: false,
            icon: None,
            close_warned: None,
            play: play::PlayState::new(ctx),
            wizards: wizard::Wizards::default(),
            project: project::ProjectPanel::default(),
            cine: None,
            cine_cursor: Default::default(),
            cine_open: None,
        };
        app.nav.export_dir = app.settings.export_dirs.export.clone();
        app.saved.1 = app.persisted_state();
        // The level preset of the environment opens a level tab shown alone.
        let level = LevelView::new(app.gpu.clone(), ctx);
        if level.wants_focus() {
            app.level_preset_used = true;
            let id = app.new_pane_id();
            app.levels.insert(id, level);
            app.dock_ops.push(DockOp::Open(Pane::Level { id }));
            if !app.layouts.remember {
                app.dock_ops.push(DockOp::Maximize(Some(id)));
            }
        }
        app.logic_preset();
        // The movies and the script editor asked for at start open as tabs.
        if app.movies.open {
            app.dock_ops.push(DockOp::Open(Pane::Movies));
        }
        if app.script_editor.open {
            app.dock_ops.push(DockOp::Open(Pane::Script));
        }
        // `DOCTOR_TOOLKIT_GUI_PROJECT=<folder>`: the project browser with that project.
        if let Some(dir) = std::env::var_os("DOCTOR_TOOLKIT_GUI_PROJECT") {
            app.open_project_tab();
            app.project_open(PathBuf::from(dir));
        }
        if std::env::var_os("DOCTOR_TOOLKIT_GUI_CHANGES").is_some() {
            app.dock_ops.push(DockOp::ShowPanel(Pane::Changes));
        }
        shell::apply_capture_preset(&mut app);
        let restore = game_dir.is_none() && app.settings.reopen_last;
        let dir = game_dir.or_else(|| {
            restore
                .then(|| app.settings.last_game.clone())
                .flatten()
                .filter(|d| d.is_dir())
        });
        if let Some(dir) = dir {
            app.restore_selection = app.initial_stream.is_none()
                && app.settings.reopen_last
                && app.settings.last_game.as_deref() == Some(dir.as_path());
            app.open_game(dir, ctx);
        }
        app
    }

    fn new_pane_id(&mut self) -> u64 {
        self.next_pane += 1;
        self.next_pane
    }

    fn open_game(&mut self, dir: PathBuf, ctx: &egui::Context) {
        if self.job.is_some() {
            self.notify(Level::Warning, tr("status.wait_for_job").to_owned());
            return;
        }
        self.job = Some(jobs::open_game(dir, ctx, &self.tx));
    }

    fn handle_results(&mut self, ctx: &egui::Context) {
        while let Ok(result) = self.rx.try_recv() {
            self.job = None;
            match result {
                JobResult::GameOpened(Ok((game, index, errors))) => {
                    self.status = trf(
                        "status.opened_game",
                        &[
                            ("name", &game.profile().name),
                            (
                                "streams",
                                &trn("count.streams", game.streams().len() as u64, &[]),
                            ),
                            ("tocs", &trn("count.tocs", game.tocs().len() as u64, &[])),
                            (
                                "resources",
                                &trn("count.resources", index.entries.len() as u64, &[]),
                            ),
                        ],
                    );
                    if let Some(first) = errors.first() {
                        self.notify(
                            Level::Warning,
                            trn(
                                "status.unreadable",
                                errors.len() as u64,
                                &[("first", first)],
                            ),
                        );
                    }
                    self.game_opened(game, index, ctx);
                }
                JobResult::GameOpened(Err(error)) => {
                    self.notify(
                        Level::Error,
                        trf("status.cannot_open_game", &[("error", &error)]),
                    );
                }
                JobResult::Failed { label, message } => {
                    let log = doctor_toolkit::report::log_path()
                        .map_or_else(|| tr("app.the_log").to_owned(), |p| p.display().to_string());
                    self.notify(
                        Level::Error,
                        trf(
                            "app.internal_error",
                            &[("label", &label), ("message", &message), ("log", &log)],
                        ),
                    );
                }
                JobResult::Extracted {
                    written,
                    failed,
                    dir,
                } => {
                    self.status = trn("app.extracted", written as u64, &[("dir", &dir.display())]);
                    if let Some(first) = failed.first() {
                        self.status += &trn(
                            "app.extract_failed",
                            failed.len() as u64,
                            &[("first", first)],
                        );
                    }
                }
                JobResult::Packed(result) => self.packed(result),
            }
        }
    }

    /// A game was opened: everything shown before belongs to the old one.
    fn game_opened(&mut self, game: GameInstance, index: ResourceIndex, ctx: &egui::Context) {
        self.type_summary = index.type_summary();
        self.settings.add_recent(game.root());
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
            "doctor_toolkit — {} ({})",
            game.root().file_name().map_or_else(
                || game.root().display().to_string(),
                |n| n.to_string_lossy().into_owned()
            ),
            game.profile().name
        )));
        let game = Arc::new(game);
        self.check_opened_game(game.clone());
        self.nav.open_game(game.clone());
        self.game = Some(game.clone());
        self.files.clear();
        self.external.clear();
        self.open_workspace();
        self.assets = None;
        let index = Arc::new(index);
        for level in self.levels.values_mut() {
            level.open_game(game.clone(), index.clone());
        }
        for logic in self.logics.values_mut() {
            logic.open_game(game.clone(), index.clone());
        }
        self.index = Some(index);
        self.details = None;
        self.docs.clear();
        // Tabs keep what they show and open it again in this game.
        let paths: Vec<(u64, Option<String>)> = self
            .streams
            .iter()
            .map(|(&id, tab)| (id, tab.path(&game).or_else(|| tab.restore.clone())))
            .collect();
        for (id, path) in paths {
            self.streams.insert(
                id,
                StreamTab {
                    restore: path,
                    ..StreamTab::default()
                },
            );
        }
        for search in self.searches.values_mut() {
            search.pending = true;
        }
        self.selected_file = None;
        self.refilter_files();
        self.refilter_index();
        if let Some(name) = self.initial_stream.take() {
            match game.find_stream(&name) {
                Some(i) => self.open_file(FileSel::Stream(i), false),
                None => self.notify(
                    Level::Warning,
                    trf("app.stream_not_found", &[("name", &name)]),
                ),
            }
        } else if std::mem::take(&mut self.restore_selection) {
            self.restore_last_selection();
        }
        self.dock_ops.push(DockOp::GameOpened);
        self.save_settings();
        ctx.request_repaint();
    }

    /// Opens the stream and resource selected when the editor was closed.
    fn restore_last_selection(&mut self) {
        let Some(game) = self.game.clone() else {
            return;
        };
        if let Some(key) = self
            .settings
            .last_resource
            .as_deref()
            .and_then(|k| k.parse::<doctor_toolkit::workspace::ResourceKey>().ok())
            && game.find_stream(&key.stream).is_some()
        {
            self.open_key(&key);
            return;
        }
        if let Some(path) = self.settings.last_stream.clone()
            && let Some(sel) = find_file(&game, &path)
        {
            self.open_file(sel, false);
        }
    }

    /// Shows a warning: in the status bar, as a pop-up and in the Log panel.
    pub fn warn(&mut self, text: String) {
        self.notify(Level::Warning, text);
    }

    /// Records a message: the status bar shows it, the log panel keeps it,
    /// warnings and errors pop up and go to the log file.
    fn notify(&mut self, level: Level, text: String) {
        self.notes.push(level, &text);
        self.status = text.clone();
        self.notes.seen_status(&text);
    }

    fn nav_action(&mut self, action: NavAction) {
        match action {
            NavAction::Open(target) => self.open_location(target),
            NavAction::Search(query) => self.search(query, false),
            NavAction::IndexReady => {
                let nav = &mut self.nav;
                for search in self.searches.values_mut() {
                    if search.wants_rerun() {
                        search.run(nav);
                    }
                }
            }
        }
    }

    /// Searches in the search tab used last, or in a new one.
    fn search(&mut self, query: String, new_tab: bool) {
        let target = self
            .focus_search
            .filter(|id| !new_tab && self.searches.contains_key(id) && self.live(*id));
        let id = match target {
            Some(id) => id,
            None => {
                let id = self.new_pane_id();
                self.searches.insert(id, Search::default());
                self.dock_ops.push(DockOp::Open(Pane::Search {
                    id,
                    query: query.clone(),
                }));
                id
            }
        };
        self.focus_search = Some(id);
        let search = self.searches.get_mut(&id).expect("just made");
        search.query = query;
        if !search.query.trim().is_empty() {
            search.run(&mut self.nav);
        }
        self.dock_ops.push(DockOp::Focus(id));
    }

    /// Whether a pane with this id is in the dock (or is about to be).
    fn live(&self, id: u64) -> bool {
        self.live_panes.contains(&id)
            || self.dock_ops.iter().any(|op| match op {
                DockOp::Open(pane) => pane.id() == Some(id),
                _ => false,
            })
    }

    // ------------------------------------------------------------- self-test

    fn drive_capture(&mut self, ui: &egui::Ui) {
        let busy = self.mods_busy()
            || self.nav.busy()
            || self.levels.values().any(LevelView::busy)
            || crate::cutscene_view::busy()
            || self.play.busy()
            || self.movies.busy()
            || self.content.previews.busy()
            || self.job.is_some();
        let Some(capture) = &mut self.capture else {
            return;
        };
        // Without a game (no directory given) the window is shot as it is.
        if capture.requested || busy {
            ui.ctx().request_repaint();
            return;
        }
        capture.frames += 1;
        ui.ctx().request_repaint();
        if capture.frames == 3 {
            if let Some(index) = capture.select.take()
                && let Some(id) = self.focus_stream
                && let Some(stream) = self
                    .streams
                    .get(&id)
                    .and_then(|t| t.stream.as_ref())
                    .map(|s| s.game_index)
            {
                self.select_resource(id, Some(index));
                if let Some(tab) = self.streams.get_mut(&id) {
                    tab.scroll_to = Some(index);
                }
                let loc = Location {
                    stream,
                    resource: index,
                };
                self.nav.visit(loc);
                if std::env::var_os("DOCTOR_TOOLKIT_GUI_CINE").is_some() {
                    // The shot's cut-scene in the level view (stage 62).
                    self.cine_open = Some(loc);
                } else if self.pin_on_capture {
                    self.open_resource_tab(loc);
                }
            }
            return;
        }
        if capture.frames < capture.delay {
            return;
        }
        capture.requested = true;
        let path = capture.path.clone();
        // With DOCTOR_TOOLKIT_CONFIG_DIR the run leaves its settings and layout
        // behind, as a closed window does.
        self.save_settings();
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
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::Screenshot(callback));
    }

    fn drive_selftest(&mut self, ui: &egui::Ui, frame_time: Duration) {
        const WARMUP_FRAMES: usize = 30;
        const MEASURED_FRAMES: usize = 600;
        let Some(test) = &mut self.selftest else {
            return;
        };
        let Some(game) = self.game.clone() else {
            return;
        };
        if test.done {
            return;
        }
        ui.ctx().request_repaint();
        if test.started.is_none() {
            let Some(i) = game.find_stream(&test.stream) else {
                eprintln!("selftest: stream {} not found", test.stream);
                std::process::exit(2);
            };
            let started = Instant::now();
            test.started = Some(started);
            self.open_file(FileSel::Stream(i), false);
            if let Some(test) = &mut self.selftest {
                test.open_time = started.elapsed();
            }
            return;
        }
        test.frame += 1;
        if test.frame > WARMUP_FRAMES {
            test.frame_times.push(frame_time);
        }
        let step = test.frame;
        let Some(id) = self.focus_stream else { return };
        let rows = self
            .streams
            .get(&id)
            .and_then(|t| t.stream.as_ref())
            .map(|s| s.rows.clone())
            .unwrap_or_default();
        if test.frame > WARMUP_FRAMES + MEASURED_FRAMES {
            let mut times = test.frame_times.clone();
            times.sort();
            let ms = |d: Duration| d.as_secs_f64() * 1000.0;
            test.done = true;
            eprintln!(
                "selftest: stream {} with {} resources opened in {:.1} ms; {} frames: median {:.2} ms, p99 {:.2} ms, max {:.2} ms",
                test.stream,
                rows.len(),
                ms(test.open_time),
                times.len(),
                ms(times[times.len() / 2]),
                ms(times[times.len() * 99 / 100]),
                ms(*times.last().unwrap()),
            );
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        // Jump through the table and select a new resource every fourth frame.
        if step % 4 == 0 && !rows.is_empty() {
            let resource = rows[(step * 97) % rows.len()];
            if let Some(tab) = self.streams.get_mut(&id) {
                tab.scroll_to = Some(resource);
            }
            self.select_resource(id, Some(resource));
        }
    }

    // --------------------------------------------------------------- settings

    /// What is written to disk: the settings and the layout, as text.
    fn persisted_state(&mut self) -> String {
        let layout = self.layout_json();
        let settings = toml::to_string(&self.settings).unwrap_or_default();
        settings + &layout
    }

    fn layout_json(&mut self) -> String {
        let Some(mut tree) = self.dock.take() else {
            return String::new();
        };
        self.sync_descriptors(&mut tree);
        let json = layouts::to_json(&tree, self.next_pane, &self.drawers.panes, &self.layouts);
        self.dock = Some(tree);
        json
    }

    /// Writes the settings and the layout if they changed.
    fn save_settings(&mut self) {
        let state = self.persisted_state();
        self.saved.0 = Instant::now();
        if state == self.saved.1 {
            return;
        }
        self.saved.1 = state;
        if let Err(error) = self.store.save(&self.settings) {
            self.notify(
                Level::Warning,
                trf("app.cannot_save_settings", &[("error", &error)]),
            );
        }
        if let Some(path) = self.store.layout_path() {
            let json = self.layout_json();
            if let Err(error) = crate::settings::write_atomic(&path, json.as_bytes()) {
                self.notify(
                    Level::Warning,
                    trf(
                        "app.cannot_save_layout",
                        &[("path", &path.display()), ("error", &error)],
                    ),
                );
            }
        }
    }

    /// Follows the window, the scale and the selection into the settings;
    /// saves now and then and when the window closes.
    fn track_settings(&mut self, ctx: &egui::Context) {
        let (closing, maximized, fullscreen, inner, outer) = ctx.input(|i| {
            let vp = i.viewport();
            (
                vp.close_requested(),
                vp.maximized.unwrap_or(false),
                vp.fullscreen.unwrap_or(false),
                vp.inner_rect,
                vp.outer_rect,
            )
        });
        let window = &mut self.settings.window;
        if !fullscreen {
            window.maximized = maximized;
            if !maximized {
                if let Some(inner) = inner {
                    window.width = inner.width().round();
                    window.height = inner.height().round();
                }
                if let Some(outer) = outer {
                    window.x = Some(outer.min.x.round());
                    window.y = Some(outer.min.y.round());
                }
            }
        }
        self.settings.ui_scale = (ctx.zoom_factor() * 100.0).round() / 100.0;
        self.nav
            .export_dir
            .clone_into(&mut self.settings.export_dirs.export);
        if let Some(game) = &self.game
            && let Some(id) = self.focus_stream
            && let Some(tab) = self.streams.get(&id)
        {
            self.settings.last_stream = tab.path(game);
            self.settings.last_resource = tab
                .stream
                .as_ref()
                .zip(tab.selected)
                .and_then(|(s, r)| {
                    self.key_of(Location {
                        stream: s.game_index,
                        resource: r,
                    })
                })
                .map(|k| k.to_string());
        }
        if closing || self.saved.0.elapsed() > AUTOSAVE {
            self.save_settings();
        }
    }
}

impl StreamTab {
    /// Path in the game of the file shown.
    fn path(&self, game: &GameInstance) -> Option<String> {
        match self.sel? {
            FileSel::Stream(i) => game.streams().get(i).map(|f| f.rel_path.clone()),
            FileSel::Toc(i) => game.tocs().get(i).map(|f| f.rel_path.clone()),
        }
    }
}

/// The stream or table of contents with this path in the game.
fn find_file(game: &GameInstance, path: &str) -> Option<FileSel> {
    game.streams()
        .iter()
        .position(|f| f.rel_path == path)
        .map(FileSel::Stream)
        .or_else(|| {
            game.tocs()
                .iter()
                .position(|f| f.rel_path == path)
                .map(FileSel::Toc)
        })
}

impl eframe::App for BrowserApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let started = Instant::now();
        let ctx = ui.ctx().clone();
        // The game tab takes the input first while the game has it.
        self.play.game_view.take_input(&ctx);
        #[cfg(windows)]
        {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Ok(handle) = frame.window_handle()
                && let RawWindowHandle::Win32(h) = handle.as_raw()
            {
                self.play.game_view.parent_window = Some(h.hwnd.get());
            }
        }
        let _ = frame;
        self.handle_results(&ctx);
        self.poll_external(&ctx);
        self.poll_bridge(&ctx);
        self.wizard_preset();
        for action in self.nav.poll() {
            self.nav_action(action);
        }
        self.shortcuts(&ctx);
        self.document_keys(&ctx);
        if let Some(loc) = self.cine_open.take() {
            self.open_cutscene_in_level(loc, &ctx);
        }
        self.cine_tick(&ctx);
        self.update_panel_level();
        self.track_details_open();
        let visuals = ui.visuals().clone();
        let bar = |fill| {
            egui::Frame::new()
                .fill(fill)
                .inner_margin(egui::Margin::symmetric(6, 2))
        };
        egui::Panel::top("menu")
            .frame(bar(theme::bar_fill(&visuals)))
            .show(ui, |ui| self.menu_bar(ui));
        egui::Panel::top("toolbar")
            .frame(bar(theme::toolbar_fill(&visuals)).inner_margin(egui::Margin::symmetric(6, 4)))
            .show(ui, |ui| self.toolbar(ui));
        egui::Panel::bottom("status")
            .frame(bar(theme::bar_fill(&visuals)))
            .show(ui, |ui| self.status_bar(ui));
        let dock_rect = egui::CentralPanel::default()
            .frame(
                egui::Frame::central_panel(ui.style())
                    .fill(theme::bar_fill(&visuals))
                    .inner_margin(2.0),
            )
            .show(ui, |ui| {
                self.dock_ui(ui);
                ui.max_rect()
            })
            .inner;
        self.drawer_ui(&ctx, dock_rect);
        self.cine_after_dock();
        self.run_asset_cmds(&ctx);
        self.content_drag(&ctx);
        self.play_frame(&ctx);
        self.show_mods(&ctx);
        if let Some(game) = self.game.clone()
            && let Some(done) = self.movies.update(&ctx, game.root())
        {
            let key = doctor_toolkit::workspace::FileKey::loose(&done.rel_path);
            self.record_file(key, done.data, &done.label);
        }
        self.show_model_import(&ctx);
        self.show_windows(&ctx);
        self.layouts_window(&ctx);
        self.palette_ui(&ctx);
        for command in std::mem::take(&mut self.pending) {
            self.run(command, &ctx);
        }
        self.apply_dock_ops();
        self.content.previews.end_frame();
        self.notes.track(&self.status, self.nav.status.as_deref());
        self.notes.toasts(&ctx);
        self.track_settings(&ctx);
        if self.job.is_some() {
            // Keep the progress bar moving.
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        let frame_time = started.elapsed();
        self.drive_selftest(ui, frame_time);
        self.drive_capture(ui);
    }
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An editor without a window, a game or settings files.
    fn headless(store: Store) -> (egui::Context, BrowserApp) {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = BrowserApp::new(&cc, None, None, None, Settings::default(), store);
        app.notes.log_to_file = false;
        (ctx, app)
    }

    fn frame(
        ctx: &egui::Context,
        app: &mut BrowserApp,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        use eframe::App as _;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 800.0),
            )),
            events,
            ..Default::default()
        };
        let mut eframe_frame = eframe::Frame::_new_kittest();
        let mut output = ctx.run_ui(input, |ui| app.ui(ui, &mut eframe_frame));
        // No renderer takes the textures.
        output.textures_delta.clear();
        output
    }

    fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    fn temp_store(name: &str) -> Store {
        let dir = std::env::temp_dir().join(format!("dt-app-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Store { dir: Some(dir) }
    }

    #[test]
    fn menu_commands_toggle_panels_and_theme() {
        let (ctx, mut app) = headless(Store { dir: None });
        frame(&ctx, &mut app, Vec::new());
        assert_eq!(app.checked(Command::ToggleFiles), Some(true));
        app.pending.push(Command::ToggleFiles);
        frame(&ctx, &mut app, Vec::new());
        assert_eq!(app.checked(Command::ToggleFiles), Some(false));
        app.pending.push(Command::ToggleFiles);
        app.pending.push(Command::ThemeLight);
        frame(&ctx, &mut app, Vec::new());
        assert_eq!(app.checked(Command::ToggleFiles), Some(true));
        assert_eq!(app.settings.theme, crate::settings::Theme::Light);
        assert_eq!(app.checked(Command::ThemeLight), Some(true));
    }

    #[test]
    fn commands_that_need_a_game_wait_for_one() {
        let (ctx, mut app) = headless(Store { dir: None });
        for command in [
            Command::OpenStream,
            Command::Export,
            Command::Undo,
            Command::LevelEditor,
            Command::Mods,
        ] {
            assert!(!app.enabled(command), "{command:?}");
        }
        app.pending.push(Command::LevelEditor);
        frame(&ctx, &mut app, Vec::new());
        assert!(app.levels.is_empty());
        assert!(app.enabled(Command::OpenGame));
        assert!(app.enabled(Command::Palette));
    }

    #[test]
    fn shortcuts_run_commands() {
        let (ctx, mut app) = headless(Store { dir: None });
        frame(&ctx, &mut app, Vec::new());
        let ctrl_shift = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
        frame(&ctx, &mut app, vec![key(egui::Key::P, ctrl_shift)]);
        assert!(app.palette.open, "Ctrl+Shift+P opens the palette");
        frame(
            &ctx,
            &mut app,
            vec![key(egui::Key::Escape, egui::Modifiers::NONE)],
        );
        assert!(!app.palette.open, "Escape closes it");
        frame(
            &ctx,
            &mut app,
            vec![key(egui::Key::Num4, egui::Modifiers::COMMAND)],
        );
        assert_eq!(
            app.checked(Command::ToggleLog),
            Some(false),
            "Ctrl+4 hides the log"
        );
        frame(
            &ctx,
            &mut app,
            vec![key(egui::Key::Comma, egui::Modifiers::COMMAND)],
        );
        assert!(app.windows.preferences);
        let output = frame(
            &ctx,
            &mut app,
            vec![key(egui::Key::Q, egui::Modifiers::COMMAND)],
        );
        let closes = output
            .viewport_output
            .values()
            .any(|v| v.commands.contains(&egui::ViewportCommand::Close));
        assert!(closes, "Ctrl+Q closes the window");
    }

    #[test]
    fn palette_lists_matching_commands() {
        let (ctx, mut app) = headless(Store { dir: None });
        app.pending.push(Command::Palette);
        frame(&ctx, &mut app, Vec::new());
        app.palette.text = "preferences".to_owned();
        let entries = app.palette_entries(&ctx);
        assert!(!entries.is_empty());
        assert_eq!(
            entries[0].label,
            format!("Edit: {}", Command::Preferences.label())
        );
        app.palette.text = "zzzz qqqq".to_owned();
        assert!(app.palette_entries(&ctx).is_empty());
    }

    #[test]
    fn layout_and_settings_are_restored() {
        let store = temp_store("restore");
        let (ctx, mut app) = headless(store.clone());
        frame(&ctx, &mut app, Vec::new());
        app.pending.push(Command::ToggleDetails);
        app.pending.push(Command::ThemeSystem);
        app.settings
            .add_recent(std::path::Path::new("/nowhere/game"));
        frame(&ctx, &mut app, Vec::new());
        app.save_settings();
        let (ctx, mut again) = headless(store.clone());
        // The settings come from the store in main; the layout from the app.
        again.settings = store.load().0;
        frame(&ctx, &mut again, Vec::new());
        assert_eq!(again.checked(Command::ToggleDetails), Some(false));
        assert_eq!(again.checked(Command::ToggleFiles), Some(true));
        assert_eq!(again.settings.theme, crate::settings::Theme::System);
        assert_eq!(
            again.settings.recent_games,
            [std::path::PathBuf::from("/nowhere/game")]
        );
        std::fs::remove_dir_all(store.dir.unwrap()).unwrap();
    }

    #[test]
    fn reset_layout_brings_panels_back() {
        let (ctx, mut app) = headless(Store { dir: None });
        frame(&ctx, &mut app, Vec::new());
        app.pending.push(Command::ToggleFiles);
        app.pending.push(Command::ToggleDetails);
        frame(&ctx, &mut app, Vec::new());
        assert_eq!(app.checked(Command::ToggleDetails), Some(false));
        app.pending.push(Command::ResetLayout);
        frame(&ctx, &mut app, Vec::new());
        assert_eq!(app.checked(Command::ToggleFiles), Some(true));
        assert_eq!(app.checked(Command::ToggleDetails), Some(true));
    }

    /// A game folder that opens (without an index) and starts a fake game.
    fn fake_game(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("dt-app-game-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("gf2")).unwrap();
        std::fs::write(dir.join("godfather2.exe"), b"MZ").unwrap();
        std::fs::write(dir.join("gf2/a.str"), b"stream").unwrap();
        dir
    }

    #[test]
    fn writing_waits_while_the_game_runs() {
        let (ctx, mut app) = headless(Store { dir: None });
        let dir = fake_game("lock");
        // A backup that Restore originals would copy over the stream.
        std::fs::create_dir_all(dir.join("_doctor_toolkit/backups/gf2")).unwrap();
        std::fs::write(dir.join("_doctor_toolkit/backups/gf2/a.str"), b"original").unwrap();
        app.game = Some(Arc::new(GameInstance::open(&dir).unwrap()));
        frame(&ctx, &mut app, Vec::new());
        assert!(app.enabled(Command::Play));
        assert!(app.enabled(Command::RestoreOriginals));

        app.play.set_running(vec![4242]);
        frame(&ctx, &mut app, Vec::new());
        assert!(!app.enabled(Command::Play));
        assert!(!app.enabled(Command::RestoreOriginals));
        assert!(app.write_lock().unwrap().contains("4242"));
        assert!(
            app.mods.write_lock().is_some(),
            "the Mods window is locked too"
        );
        app.restore_originals();
        assert_eq!(std::fs::read(dir.join("gf2/a.str")).unwrap(), b"stream");
        assert!(app.status.starts_with("Not written"), "{}", app.status);

        app.play.set_running(Vec::new());
        frame(&ctx, &mut app, Vec::new());
        assert!(app.enabled(Command::Play));
        assert!(app.mods.write_lock().is_none());
        app.restore_originals();
        assert_eq!(std::fs::read(dir.join("gf2/a.str")).unwrap(), b"original");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn play_starts_and_follows_a_fake_game() {
        use std::os::unix::fs::PermissionsExt;
        let (ctx, mut app) = headless(Store { dir: None });
        let dir = fake_game("play");
        let script = dir.join("fake-game.sh");
        std::fs::write(&script, "#!/bin/sh\nsleep 0.3\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        app.game = Some(Arc::new(GameInstance::open(&dir).unwrap()));
        app.settings.game_mut(&dir).play.method = doctor_toolkit::play::LaunchMethod::Command {
            command: "{dir}/fake-game.sh".into(),
        };
        app.pending.push(Command::Play);
        frame(&ctx, &mut app, Vec::new());
        assert!(app.play.launched());
        assert!(!app.enabled(Command::Play), "one game at a time");
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.play.launched() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
            frame(&ctx, &mut app, Vec::new());
        }
        assert!(!app.play.launched());
        assert!(app.status.contains("exited"), "{}", app.status);
        assert!(app.enabled(Command::Play));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Play in the game tab: the game gets the tab's display and `cmd.txt`
    /// with the tab's screen; Stop ends it, the server and `cmd.txt` go.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn play_in_a_tab_follows_a_fake_game_on_a_fake_server() {
        use std::os::unix::fs::PermissionsExt;
        let (ctx, mut app) = headless(Store { dir: None });
        let dir = fake_game("tab");
        let script = dir.join("fake-game.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\necho \"$DISPLAY $(cat cmd.txt)\" > seen.txt\nexec sleep 60\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        app.game = Some(Arc::new(GameInstance::open(&dir).unwrap()));
        let settings = app.settings.game_mut(&dir);
        settings.in_tab = true;
        settings.play.method = doctor_toolkit::play::LaunchMethod::Command {
            command: "{dir}/fake-game.sh".into(),
        };
        settings.play.start.streams = vec!["gf2/cuba".into()];
        app.play.game_view.server = Some(crate::game_view::nested::Server::Custom(
            "sh".into(),
            vec!["-c".into(), "echo 77; exec sleep 60".into()],
        ));
        app.pending.push(Command::Play);
        frame(&ctx, &mut app, Vec::new());
        assert!(app.play.launched());
        assert!(app.play.game_view.is_running());
        let seen = dir.join("seen.txt");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !seen.is_file() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
            frame(&ctx, &mut app, Vec::new());
        }
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(
            std::fs::read_to_string(&seen).unwrap().trim(),
            ":77 -resolution 1280 720 -stream gf2 cuba"
        );
        assert!(dir.join("cmd.txt").is_file());
        frame(&ctx, &mut app, Vec::new());
        assert!(app.play.launched(), "the game runs until stopped");

        app.play.game_view.stop(&ctx);
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.play.launched() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
            frame(&ctx, &mut app, Vec::new());
        }
        assert!(!app.play.launched());
        assert!(!app.play.game_view.is_running());
        assert!(!dir.join("cmd.txt").exists(), "cmd.txt is taken away");
        assert!(app.status.contains("exited"), "{}", app.status);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn shell_strings_have_english_text() {
        // Every key of a tr or trf call of the editor shell is in en.toml.
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = vec![
            src.join("app.rs"),
            src.join("external_view.rs"),
            src.join("wizard_view.rs"),
        ];
        let mut dirs = vec![src.join("app")];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    dirs.push(path);
                } else {
                    files.push(path);
                }
            }
        }
        let mut missing = Vec::new();
        for file in files {
            let text = std::fs::read_to_string(&file).unwrap();
            for call in ["tr(\"", "trf(\""] {
                for part in text.split(call).skip(1) {
                    let key = part.split('"').next().unwrap_or_default();
                    if key.contains('.')
                        && !key.contains(' ')
                        && !doctor_toolkit_mods::i18n::has_key(key)
                    {
                        missing.push(format!("{}: {key}", file.display()));
                    }
                }
            }
        }
        assert!(missing.is_empty(), "keys without text: {missing:?}");
    }

    #[test]
    fn palette_lists_layouts() {
        let (ctx, mut app) = headless(Store { dir: None });
        app.pending.push(Command::Palette);
        frame(&ctx, &mut app, Vec::new());
        app.palette.text = "layouts level editor".to_owned();
        let entries = app.palette_entries(&ctx);
        assert!(
            entries
                .first()
                .is_some_and(|e| e.label.ends_with(tr("layout.level"))),
            "{:?}",
            entries.iter().map(|e| &e.label).collect::<Vec<_>>()
        );
    }

    fn click_at(pos: egui::Pos2) -> Vec<egui::Event> {
        let press = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        vec![egui::Event::PointerMoved(pos), press(true), press(false)]
    }

    fn in_tree(app: &BrowserApp, pane: &Pane) -> bool {
        app.dock
            .as_ref()
            .is_some_and(|t| dock::find(t, |p| p == pane).is_some())
    }

    #[test]
    fn layouts_are_saved_applied_and_deleted() {
        use layouts::{Builtin, LayoutId};
        let store = temp_store("layouts");
        let (ctx, mut app) = headless(store.clone());
        frame(&ctx, &mut app, Vec::new());
        assert_eq!(app.layouts.current, LayoutId::Builtin(Builtin::Resources));
        assert!(in_tree(&app, &Pane::Log) && in_tree(&app, &Pane::Files));

        // "Level Editor": Outliner over Details on the right, the bottom
        // panels are drawers.
        app.dock_ops
            .push(DockOp::ApplyLayout(LayoutId::Builtin(Builtin::Level)));
        frame(&ctx, &mut app, Vec::new());
        assert_eq!(app.checked(Command::ToggleOutliner), Some(true));
        assert_eq!(app.checked(Command::ToggleDetails), Some(true));
        assert!(in_tree(&app, &Pane::Zones), "behind the Outliner");
        assert!(!in_tree(&app, &Pane::Log) && app.drawers.contains(&Pane::Log));
        assert_eq!(app.checked(Command::ToggleLog), Some(false));
        assert!(in_tree(&app, &Pane::Welcome), "documents stay");

        // Saved under a name, it comes back after another layout.
        assert_eq!(
            app.checked(Command::ToggleFiles),
            Some(false),
            "behind the Outliner"
        );
        app.pending.push(Command::ToggleFiles);
        frame(&ctx, &mut app, Vec::new());
        assert_eq!(app.checked(Command::ToggleFiles), Some(true));
        app.dock_ops.push(DockOp::SaveLayout("Mine".into()));
        frame(&ctx, &mut app, Vec::new());
        assert_eq!(app.layouts.current, LayoutId::User("Mine".into()));
        app.dock_ops
            .push(DockOp::ApplyLayout(LayoutId::Builtin(Builtin::Resources)));
        frame(&ctx, &mut app, Vec::new());
        assert!(!in_tree(&app, &Pane::Outliner) && in_tree(&app, &Pane::Log));
        app.dock_ops
            .push(DockOp::ApplyLayout(LayoutId::User("Mine".into())));
        frame(&ctx, &mut app, Vec::new());
        assert!(in_tree(&app, &Pane::Outliner) && !in_tree(&app, &Pane::Log));
        assert_eq!(app.checked(Command::ToggleFiles), Some(true), "as saved");

        // Renamed, kept over a restart, deleted.
        app.dock_ops.push(DockOp::RenameLayout {
            from: "Mine".into(),
            to: "Wide view".into(),
        });
        frame(&ctx, &mut app, Vec::new());
        app.save_settings();
        let (ctx, mut again) = headless(store.clone());
        frame(&ctx, &mut again, Vec::new());
        assert_eq!(again.layouts.current, LayoutId::User("Wide view".into()));
        assert_eq!(again.layouts.user.len(), 1);
        assert!(again.drawers.contains(&Pane::Log));
        assert!(in_tree(&again, &Pane::Outliner));
        again
            .dock_ops
            .push(DockOp::DeleteLayout("Wide view".into()));
        frame(&ctx, &mut again, Vec::new());
        assert!(again.layouts.user.is_empty());
        // Reset goes back to the built-in layout of the screen.
        again.pending.push(Command::ResetLayout);
        frame(&ctx, &mut again, Vec::new());
        assert_eq!(again.layouts.current, LayoutId::Builtin(Builtin::Resources));
        assert!(in_tree(&again, &Pane::Log) && in_tree(&again, &Pane::Files));
        std::fs::remove_dir_all(store.dir.unwrap()).unwrap();
    }

    #[test]
    fn an_old_layout_file_is_read() {
        let store = temp_store("old-layout");
        let dir = store.dir.clone().unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let mut tree = dock::default_tree(Vec::new());
        dock::toggle_panel(&mut tree, Pane::Details, None);
        let old = serde_json::json!({ "version": 1, "next_pane": 0, "tree": tree });
        std::fs::write(store.layout_path().unwrap(), old.to_string()).unwrap();
        let (ctx, mut app) = headless(store.clone());
        frame(&ctx, &mut app, Vec::new());
        assert_eq!(app.checked(Command::ToggleDetails), Some(false));
        // Ctrl+Space finds no Content Browser in an old layout: it becomes a drawer.
        frame(
            &ctx,
            &mut app,
            vec![key(egui::Key::Space, egui::Modifiers::COMMAND)],
        );
        assert!(app.drawers.is_open(&Pane::ContentBrowser));
        app.save_settings();
        let text = std::fs::read_to_string(store.layout_path().unwrap()).unwrap();
        assert!(
            text.contains("\"version\": 6"),
            "written in the new version"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn drawers_open_and_close() {
        let (ctx, mut app) = headless(Store { dir: None });
        frame(&ctx, &mut app, Vec::new());
        let ctrl_space = || key(egui::Key::Space, egui::Modifiers::COMMAND);
        frame(&ctx, &mut app, vec![ctrl_space()]);
        assert!(
            app.drawers.is_open(&Pane::ContentBrowser),
            "Ctrl+Space opens"
        );
        assert_eq!(app.checked(Command::ContentDrawer), Some(true));
        frame(&ctx, &mut app, Vec::new());
        frame(&ctx, &mut app, vec![ctrl_space()]);
        assert_eq!(app.drawers.open, None, "and closes");
        frame(&ctx, &mut app, vec![ctrl_space()]);
        frame(
            &ctx,
            &mut app,
            vec![key(egui::Key::Escape, egui::Modifiers::NONE)],
        );
        assert_eq!(app.drawers.open, None, "Esc closes");
        frame(&ctx, &mut app, vec![ctrl_space()]);
        frame(&ctx, &mut app, Vec::new());
        // A click at the top of the window is outside the drawer.
        frame(&ctx, &mut app, click_at(egui::pos2(640.0, 90.0)));
        assert_eq!(app.drawers.open, None, "a click elsewhere closes");

        // Any panel can become a drawer and go back into the layout.
        app.dock_ops.push(DockOp::MakeDrawer(Pane::Details));
        frame(&ctx, &mut app, Vec::new());
        assert!(!in_tree(&app, &Pane::Details));
        assert!(app.drawers.is_open(&Pane::Details));
        assert_eq!(app.checked(Command::ToggleDetails), Some(true));
        app.pending.push(Command::ToggleDetails);
        frame(&ctx, &mut app, Vec::new());
        assert_eq!(app.checked(Command::ToggleDetails), Some(false));
        app.dock_ops.push(DockOp::DockDrawer(Pane::Details));
        frame(&ctx, &mut app, Vec::new());
        assert!(in_tree(&app, &Pane::Details) && !app.drawers.contains(&Pane::Details));
        assert_eq!(app.checked(Command::ToggleDetails), Some(true));
    }

    #[test]
    fn toolbar_follows_the_document_in_front() {
        use crate::level_view::{GizmoMode, GizmoSpace};
        use layouts::{Builtin, LayoutId, Screen};
        let (ctx, mut app) = headless(Store { dir: None });
        frame(&ctx, &mut app, Vec::new());
        assert_eq!(app.toolbar_context(), toolbar::Context::NoGame);
        let dir = fake_game("toolbar");
        app.game = Some(Arc::new(GameInstance::open(&dir).unwrap()));
        frame(&ctx, &mut app, Vec::new());
        assert_eq!(app.toolbar_context(), toolbar::Context::Other);
        assert!(!app.enabled(Command::GizmoRotate), "no level in front");

        // A level arranges the window as the level editor.
        app.pending.push(Command::LevelEditor);
        frame(&ctx, &mut app, Vec::new());
        frame(&ctx, &mut app, Vec::new());
        let id = *app.levels.keys().next().expect("a level tab");
        assert_eq!(app.toolbar_context(), toolbar::Context::Level(id));
        assert_eq!(app.layouts.screen, Screen::Level);
        assert_eq!(app.layouts.current, LayoutId::Builtin(Builtin::Level));
        assert!(app.maximized.is_none(), "the layout, not a maximized tab");
        assert_eq!(
            app.panel_level,
            Some(id),
            "Outliner, Zones and Details show it"
        );
        for (command, mode) in [
            (Command::GizmoRotate, GizmoMode::Rotate),
            (Command::GizmoScale, GizmoMode::Scale),
            (Command::GizmoSelect, GizmoMode::Select),
        ] {
            app.pending.push(command);
            frame(&ctx, &mut app, Vec::new());
            assert_eq!(app.levels[&id].gizmo_mode(), mode);
            assert_eq!(app.checked(command), Some(true));
        }
        app.pending.push(Command::GizmoLocal);
        app.pending.push(Command::GizmoSnap);
        frame(&ctx, &mut app, Vec::new());
        assert_eq!(app.levels[&id].gizmo_space(), GizmoSpace::Local);
        assert!(app.levels[&id].snap().on);

        // A search tab is another screen: the resources layout comes back.
        app.pending.push(Command::ToggleOutliner);
        frame(&ctx, &mut app, Vec::new());
        app.pending.push(Command::NewSearch);
        frame(&ctx, &mut app, Vec::new());
        frame(&ctx, &mut app, Vec::new());
        assert_eq!(app.layouts.screen, Screen::Resources);
        assert_eq!(app.toolbar_context(), toolbar::Context::Other);
        assert!(in_tree(&app, &Pane::Log) && !in_tree(&app, &Pane::Outliner));
        // ... and the level its own, as it was left (the Outliner hidden).
        app.pending.push(Command::LevelEditor);
        frame(&ctx, &mut app, Vec::new());
        frame(&ctx, &mut app, Vec::new());
        assert_eq!(app.layouts.screen, Screen::Level);
        assert!(in_tree(&app, &Pane::Outliner));
        assert_eq!(app.checked(Command::ToggleOutliner), Some(false));
        assert_eq!(app.checked(Command::ToggleZones), Some(true));
        assert!(app.live(id), "the documents stay open");

        // Without remembering, the layout stays as it is.
        app.layouts.remember = false;
        app.pending.push(Command::NewSearch);
        frame(&ctx, &mut app, Vec::new());
        assert_eq!(app.layouts.screen, Screen::Level);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn human_sizes() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1023), "1023 B");
        assert_eq!(human_size(1536), "1.5 KiB");
        assert_eq!(human_size(139_067_392), "132.6 MiB");
    }
}

//! Play and the first run: games found on this computer (welcome screen),
//! the edition of the open game, the offer to save the originals'
//! fingerprints or to take over the old toolkit's changes, the Play button
//! with its options, and the lock on writing into the game while it runs.
//!
//! The library does the work (`doctor_toolkit::install`, `edition`, `play`,
//! `mods::integrity`); this module runs it off the interface thread and
//! shows the results.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use egui::RichText;

use doctor_toolkit::edition::Edition;
use doctor_toolkit::game::GameInstance;
use doctor_toolkit::install::{self, Install, SearchRoots};
use doctor_toolkit::mods::integrity::{self, Condition, IntegrityOptions};
use doctor_toolkit::play::{self, GameWatch, LaunchMethod, Session, SessionState};
use doctor_toolkit_mods::i18n::{tr, trf, trn};

use super::dock::{DockOp, Pane};
use super::{BrowserApp, Level};

/// How often the process list is scanned.
const SCAN_INTERVAL: Duration = Duration::from_millis(1500);

/// State of Play, detection and the first run.
pub struct PlayState {
    watch: GameWatch,
    /// Processes of the game, as of the last scan.
    pub(super) running: Vec<u32>,
    session: Option<Launched>,
    detection: Detection,
    /// The edition of the open game, by its directory.
    edition: Option<(PathBuf, Edition)>,
    checks: Option<Receiver<Checked>>,
    /// The offer shown for the open game.
    first_run: Option<FirstRun>,
    tasks: Vec<Task>,
    /// The Mods window imports the old toolkit's patches once it is ready.
    import_old_queued: bool,
    pub(super) options_open: bool,
    /// The game shown in a tab (stage 33).
    pub(super) game_view: crate::game_view::GameView,
    /// Start the game again once it exited (the game tab's Restart).
    pub(super) restart: bool,
    /// How the last game ended, for the game tab.
    pub(super) last_exit: Option<String>,
    /// `PlayerSpawn` objects by stream, read in the background.
    pub(super) spawns: std::collections::HashMap<String, super::gametab::Spawns>,
    /// "Play from here" of a level tab, waiting for an answer.
    pub(super) play_here: Option<super::gametab::PlayHere>,
    /// The game exited; the revert waits for its processes to go.
    after_exit: Option<AfterExit>,
    /// The live link with the running game (stage 34).
    pub(super) live: super::live::LiveState,
    /// DXVK and 32-bit Vulkan of the computer, looked for once.
    dxvk_found: Option<(Option<play::dxvk::Dxvk>, play::dxvk::Vulkan32)>,
}

/// [`play::dxvk::check`] for one launch method.
pub(super) type DxvkCheck = Result<play::dxvk::Dxvk, play::dxvk::Problem>;

/// The text of why DXVK cannot be used.
pub(super) fn dxvk_problem(problem: &play::dxvk::Problem) -> String {
    use play::dxvk::Problem;
    match problem {
        Problem::NotLinux => tr("dxvk.not_linux").to_owned(),
        Problem::ProtonHasIt => tr("dxvk.proton").to_owned(),
        Problem::Method(kind) => trf("dxvk.method", &[("method", kind)]),
        Problem::NoDxvk => tr("dxvk.no_dxvk").to_owned(),
        Problem::NoLoader => tr("dxvk.no_loader").to_owned(),
        Problem::NoDriver { software_only } => {
            let mut text = tr("dxvk.no_driver").to_owned();
            if *software_only {
                text += tr("dxvk.software_only");
            }
            text
        }
        Problem::Occupied(path) => trf("dxvk.occupied", &[("file", &path.display().to_string())]),
    }
}

struct AfterExit {
    root: PathBuf,
    revert: bool,
    since: std::time::Instant,
}

enum Detection {
    Idle,
    Searching(Receiver<Vec<(Install, Edition)>>),
    Done(Vec<(Install, Edition)>),
}

struct Launched {
    /// `None`: the game runs in the game tab, which follows it.
    session: Option<Session>,
    root: PathBuf,
    method: LaunchMethod,
    revert_after: bool,
    announced: bool,
    /// `cmd.txt` written for this launch (start in a level), taken away
    /// when the game exits.
    cmd_file: Option<play::start::CmdFile>,
    /// DXVK's `d3d9.dll` for this launch, taken away when the game exits.
    dxvk: Option<play::dxvk::DxvkInstall>,
}

/// What opening a game found out about its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum FirstRun {
    /// No changes, no backups, no reference list yet.
    Pristine { files: usize },
    /// The old toolkit changed `changed` files and installed `patches`.
    OldToolkit {
        patches: Vec<String>,
        changed: usize,
    },
}

struct Checked {
    root: PathBuf,
    edition: Edition,
    first_run: Option<FirstRun>,
}

/// Background work of this module, shown in the status bar.
struct Task {
    label: String,
    done: Arc<AtomicUsize>,
    total: Arc<AtomicUsize>,
    rx: Receiver<Result<String, String>>,
    /// The game files change: release them first, reopen them after.
    writes_game: bool,
}

impl PlayState {
    pub fn new(ctx: &egui::Context) -> Self {
        let watch = if cfg!(test) || std::env::var_os("DOCTOR_TOOLKIT_GUI_SHOT").is_some() {
            GameWatch::disabled()
        } else {
            let ctx = ctx.clone();
            GameWatch::start(SCAN_INTERVAL, move || ctx.request_repaint())
        };
        Self {
            running: watch.running(),
            watch,
            session: None,
            detection: Detection::Idle,
            edition: None,
            checks: None,
            first_run: None,
            tasks: Vec::new(),
            import_old_queued: false,
            options_open: false,
            game_view: crate::game_view::GameView::default(),
            restart: false,
            last_exit: None,
            spawns: std::collections::HashMap::new(),
            play_here: None,
            after_exit: None,
            live: super::live::LiveState::default(),
            dxvk_found: None,
        }
    }

    /// Jobs for the status bar: label and progress.
    pub(super) fn jobs(&self) -> Vec<(String, Option<f32>)> {
        let mut jobs: Vec<(String, Option<f32>)> = self
            .tasks
            .iter()
            .map(|t| {
                let total = t.total.load(Ordering::Relaxed);
                let fraction =
                    (total > 0).then(|| t.done.load(Ordering::Relaxed) as f32 / total as f32);
                (t.label.clone(), fraction)
            })
            .collect();
        if matches!(self.detection, Detection::Searching(_)) {
            jobs.push((tr("play.searching").to_owned(), None));
        }
        jobs
    }

    /// Whether checks or detection still run (the screenshot mode waits).
    pub(super) fn busy(&self) -> bool {
        self.checks.is_some() || matches!(self.detection, Detection::Searching(_))
    }

    /// Whether a launched game is followed.
    pub(super) fn launched(&self) -> bool {
        self.session.is_some() || self.after_exit.is_some()
    }

    /// Records a process scan (tests).
    #[cfg(test)]
    pub(super) fn set_running(&mut self, pids: Vec<u32>) {
        self.watch.set_running(pids.clone());
        self.running = pids;
    }
}

impl BrowserApp {
    // ------------------------------------------------------------- frame

    /// Follows the game, the checks and the tasks; shows the windows.
    pub(super) fn play_frame(&mut self, ctx: &egui::Context) {
        self.play.running = self.play.watch.running();
        let lock = self.write_lock();
        self.mods.set_write_lock(lock);
        self.poll_checks();
        self.poll_detection();
        self.poll_tasks();
        self.poll_session(ctx);
        self.live_journal_to_log();
        self.live_debug_frame(ctx);
        self.finish_exit();
        if self.play.import_old_queued
            && self.mods_open
            && !self.mods.is_busy()
            && self.mods.game().is_some()
        {
            self.play.import_old_queued = false;
            self.mods.import_old_toolkit(ctx);
        }
        if self.play.session.is_some()
            || !self.play.tasks.is_empty()
            || self.play.after_exit.is_some()
        {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
        if matches!(self.play.detection, Detection::Searching(_)) || self.play.checks.is_some() {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        self.play_options_window(ctx);
        self.play_here_window(ctx);
        self.first_run_window(ctx);
        self.play.game_view.after_frame(ctx);
    }

    /// Why writing into the game is refused now: the game runs.
    pub(super) fn write_lock(&self) -> Option<String> {
        let pids = &self.play.running;
        (!pids.is_empty()).then(|| {
            trf(
                "play.locked",
                &[(
                    "pids",
                    &pids
                        .iter()
                        .map(u32::to_string)
                        .collect::<Vec<_>>()
                        .join(", "),
                )],
            )
        })
    }

    /// Warns and returns `true` when the game runs: callers about to write
    /// into the game stop.
    pub(super) fn refuse_write(&mut self) -> bool {
        match self.write_lock() {
            Some(reason) => {
                self.notify(
                    Level::Warning,
                    trf("play.not_written", &[("reason", &reason)]),
                );
                true
            }
            None => false,
        }
    }

    // ------------------------------------------------------------ edition

    /// A game was opened: its edition, and what to offer on its first run.
    pub(super) fn check_opened_game(&mut self, game: Arc<GameInstance>) {
        let (tx, rx) = channel();
        self.play.checks = Some(rx);
        self.play.first_run = None;
        let skip = self.settings.game(game.root()).skip_first_run;
        std::thread::spawn(move || {
            let edition = Edition::identify(game.root());
            let first_run = if skip { None } else { first_run_state(&game) };
            let _ = tx.send(Checked {
                root: game.root().to_path_buf(),
                edition,
                first_run,
            });
        });
    }

    fn poll_checks(&mut self) {
        let Some(rx) = &self.play.checks else { return };
        let Ok(checked) = rx.try_recv() else { return };
        self.play.checks = None;
        let current = self.game.as_ref().map(|g| g.root().to_path_buf());
        if current.as_deref() != Some(checked.root.as_path()) {
            return;
        }
        doctor_toolkit::report::log(&format!("edition: {}", checked.edition.summary()));
        self.notify(
            Level::Info,
            trf("play.edition", &[("edition", &checked.edition.summary())]),
        );
        let skip = self.settings.game(&checked.root).skip_first_run;
        if !skip {
            self.play.first_run = checked.first_run;
        }
        self.play.edition = Some((checked.root, checked.edition));
    }

    /// One line about the edition of the open game.
    pub(super) fn edition_summary(&self) -> Option<String> {
        let (root, edition) = self.play.edition.as_ref()?;
        (self.game.as_ref()?.root() == root).then(|| edition.summary())
    }

    // ---------------------------------------------------------- detection

    /// Starts searching the computer for installed games, once.
    pub(super) fn detect_games(&mut self, again: bool) {
        if matches!(self.play.detection, Detection::Searching(_))
            || (matches!(self.play.detection, Detection::Done(_)) && !again)
            || cfg!(test)
            || std::env::var_os("DOCTOR_TOOLKIT_NO_DETECT").is_some()
        {
            return;
        }
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let installs = install::detect(&SearchRoots::from_env());
            let found = installs
                .into_iter()
                .map(|i| {
                    let edition = Edition::identify(&i.path);
                    (i, edition)
                })
                .collect();
            let _ = tx.send(found);
        });
        self.play.detection = Detection::Searching(rx);
    }

    fn poll_detection(&mut self) {
        let Detection::Searching(rx) = &self.play.detection else {
            return;
        };
        if let Ok(found) = rx.try_recv() {
            doctor_toolkit::report::log(&format!("detected {} installed game(s)", found.len()));
            self.play.detection = Detection::Done(found);
        }
    }

    /// Games found so far.
    fn installs(&self) -> Vec<Install> {
        match &self.play.detection {
            Detection::Done(found) => found.iter().map(|(i, _)| i.clone()).collect(),
            _ => Vec::new(),
        }
    }

    /// Tools → Find installed games.
    pub(super) fn find_games(&mut self) {
        self.detect_games(true);
        self.dock_ops.push(DockOp::ShowPanel(Pane::Welcome));
    }

    /// The list of the welcome screen; returns a game to open.
    pub(super) fn welcome_detected(&mut self, ui: &mut egui::Ui) -> Option<PathBuf> {
        self.detect_games(false);
        let mut open = None;
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.strong(tr("welcome.detected"));
            if matches!(self.play.detection, Detection::Searching(_)) {
                ui.spinner();
            } else if ui
                .small_button(tr("welcome.search_again"))
                .on_hover_text(tr("cmd.find_games"))
                .clicked()
            {
                self.detect_games(true);
            }
        });
        match &self.play.detection {
            Detection::Done(found) if found.is_empty() => {
                ui.label(RichText::new(tr("welcome.none_found")).weak());
            }
            Detection::Done(found) => {
                for (install, edition) in found {
                    let name = install.path.file_name().map_or_else(
                        || install.path.display().to_string(),
                        |n| n.to_string_lossy().into_owned(),
                    );
                    let response = ui
                        .link(name)
                        .on_hover_text(install.path.display().to_string());
                    if response.clicked() {
                        open = Some(install.path.clone());
                    }
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!("{} — {}", edition.name(), install.sources()))
                                .weak()
                                .small(),
                        )
                        .truncate(),
                    )
                    .on_hover_text(format!(
                        "{}\n{}",
                        install.path.display(),
                        edition.summary()
                    ));
                }
            }
            _ => {}
        }
        open
    }

    // --------------------------------------------------------------- play

    /// The Play button of the toolbar and the state of the game.
    pub(super) fn play_button(&mut self, ui: &mut egui::Ui) {
        if let Some(lock) = self.write_lock() {
            ui.label(RichText::new(tr("play.running")).color(doctor_toolkit_mods::WARN))
                .on_hover_text(lock);
            if self.play.game_view.is_running()
                && ui
                    .small_button(tr("gametab.title"))
                    .on_hover_text(tr("gametab.show_hint"))
                    .clicked()
            {
                self.show_game_tab();
            }
            return;
        }
        if self.play.game_view.is_running() {
            if ui
                .button(tr("gametab.title"))
                .on_hover_text(tr("gametab.show_hint"))
                .clicked()
            {
                self.show_game_tab();
            }
            return;
        }
        if self.play.session.is_some() {
            ui.spinner();
            ui.label(tr("play.starting"));
            return;
        }
        let method = self.play_method();
        let hover = trf("play.hover", &[("method", &method.describe())]);
        if ui
            .add_enabled(
                self.enabled(super::Command::Play),
                egui::Button::new(tr("play.button")),
            )
            .on_hover_text(hover)
            .clicked()
        {
            self.pending.push(super::Command::Play);
        }
        if ui
            .small_button("…")
            .on_hover_text(tr("cmd.play_options"))
            .clicked()
        {
            self.play.options_open = true;
        }
    }

    /// The configured method of the open game, made concrete.
    pub(super) fn play_method(&self) -> LaunchMethod {
        let Some(game) = &self.game else {
            return LaunchMethod::Auto;
        };
        match self.settings.game(game.root()).play.method {
            LaunchMethod::Auto => play::auto_method(game.root(), &self.installs()),
            method => method,
        }
    }

    /// Tools → Play: applies the workspace when asked, starts the game and
    /// follows it.
    pub(super) fn play(&mut self, ctx: &egui::Context) {
        let Some(game) = self.game.clone() else {
            return;
        };
        if self.refuse_write() {
            return;
        }
        let settings = self.settings.game(game.root());
        let config = settings.play;
        let method = self.play_method();
        if let Some(problem) = super::gametab::start_problem(&config.start) {
            self.notify(
                Level::Error,
                trf("play.cannot_start", &[("error", &problem)]),
            );
            return;
        }
        // In a tab when asked for and possible; else as usual, with a word why.
        let in_tab = settings.in_tab
            && match super::gametab::tab_possible(&method) {
                Ok(_) => true,
                Err(reason) => {
                    self.notify(
                        Level::Warning,
                        trf("gametab.not_possible", &[("reason", &reason)]),
                    );
                    false
                }
            };
        let options = super::gametab::launch_options(&config.start, in_tab);
        let mut plan = match play::plan(&method, game.root()) {
            Ok(plan) => plan,
            Err(error) => {
                self.notify(Level::Error, trf("play.cannot_start", &[("error", &error)]));
                return;
            }
        };
        if in_tab {
            super::gametab::in_virtual_desktop(&mut plan, &super::gametab::tab_size(&config.start));
        }
        // Hardware rendering under Wine (settings: play.dxvk). When it cannot
        // be used the game still starts, with a word why.
        let dxvk = if config.dxvk {
            match self.dxvk_check(&method, game.root()) {
                Ok(found) => {
                    play::dxvk::apply_env(&mut plan, game.root());
                    Some(found)
                }
                Err(problem) => {
                    self.notify(
                        Level::Warning,
                        trf("dxvk.not_used", &[("reason", &dxvk_problem(&problem))]),
                    );
                    None
                }
            }
        } else {
            None
        };
        if config.apply_before && self.workspace.as_ref().is_some_and(|ws| ws.is_saved()) {
            self.apply(false);
            let applied = self
                .ws_status
                .as_ref()
                .is_some_and(|s| s.is_up_to_date() && s.conflicts().is_empty());
            if !applied {
                self.notify(Level::Warning, tr("play.not_applied").to_owned());
                return;
            }
        }
        // The live link of an earlier launch, then the one of this launch
        // (settings: play.live).
        self.live_after_exit();
        if let Err(error) = doctor_toolkit::live::restore_stale(game.root()) {
            self.notify(Level::Error, trf("play.cannot_start", &[("error", &error)]));
            return;
        }
        let mut live_module = self.install_live(game.root(), &config.live);
        // Straight into a level the player has no controller without the
        // module (live-link.md §5.1): install it for that alone.
        if live_module.is_none() && !config.live.enabled && !options.streams.is_empty() {
            live_module = self.install_for_input(game.root());
        }
        // Start in a level (settings: play.start): the game reads cmd.txt.
        let cmd_file = match play::start::restore_stale(game.root()).and_then(|_| {
            (!options.is_empty())
                .then(|| play::start::CmdFile::write(game.root(), &options))
                .transpose()
        }) {
            Ok(file) => file,
            Err(error) => {
                self.notify(Level::Error, trf("play.cannot_start", &[("error", &error)]));
                return;
            }
        };
        if let Err(error) = play::dxvk::restore_stale(game.root()) {
            self.notify(Level::Error, trf("play.cannot_start", &[("error", &error)]));
            return;
        }
        let dxvk = match dxvk.map(|found| play::dxvk::DxvkInstall::write(game.root(), &found)) {
            None => None,
            Some(Ok(install)) => Some(install),
            Some(Err(error)) => {
                self.notify(Level::Warning, trf("dxvk.not_used", &[("reason", &error)]));
                // The override alone would make Wine look for a d3d9.dll
                // that is not there: start without it.
                remove_dxvk_env(&mut plan);
                None
            }
        };
        self.play.last_exit = None;
        // The spawn points of the game files as they are now (applied).
        self.play.spawns.clear();
        let started = if in_tab {
            let size = super::gametab::tab_size(&config.start);
            self.show_game_tab();
            self.play
                .game_view
                .start(&plan, size, ctx)
                .map(|()| None)
                .map_err(|e| e.to_string())
        } else {
            Session::start(&plan).map(Some).map_err(|e| e.to_string())
        };
        match started {
            Ok(session) => {
                self.notify(
                    Level::Info,
                    trf("play.started", &[("method", &method.describe())]),
                );
                if let Some(module) = live_module {
                    self.play.live.connect(module.endpoint(), ctx);
                    self.play.live.module = Some(module);
                }
                self.play.session = Some(Launched {
                    session,
                    root: game.root().to_path_buf(),
                    method,
                    revert_after: config.revert_after,
                    announced: false,
                    cmd_file,
                    dxvk,
                });
            }
            Err(error) => self.notify(Level::Error, trf("play.cannot_start", &[("error", &error)])),
        }
    }

    fn poll_session(&mut self, ctx: &egui::Context) {
        let running = self.play.running.clone();
        let PlayState {
            session, game_view, ..
        } = &mut self.play;
        let Some(launched) = session else {
            return;
        };
        let state = match &mut launched.session {
            Some(session) => session.poll(&running),
            None => game_view
                .poll(&running, ctx)
                .unwrap_or(SessionState::Exited),
        };
        match state {
            SessionState::Starting => {}
            SessionState::Running(_) => {
                if !launched.announced {
                    launched.announced = true;
                    self.notify(Level::Info, tr("play.is_running").to_owned());
                }
            }
            SessionState::Exited | SessionState::NotStarted(_) => {
                let mut launched = self.play.session.take().expect("checked above");
                if let Some(file) = launched.cmd_file.take()
                    && let Err(error) = file.restore()
                {
                    doctor_toolkit::report::log(&format!("cmd.txt not taken away: {error}"));
                }
                if let Some(install) = launched.dxvk.take()
                    && let Err(error) = install.restore()
                {
                    doctor_toolkit::report::log(&format!("d3d9.dll not taken away: {error}"));
                }
                self.live_after_exit();
                let text = match &state {
                    SessionState::NotStarted(reason) => trf(
                        "play.not_started",
                        &[("method", &launched.method.describe()), ("reason", reason)],
                    ),
                    _ => tr("play.exited").to_owned(),
                };
                self.play.last_exit = Some(text.clone());
                let level = if matches!(state, SessionState::NotStarted(_)) {
                    Level::Warning
                } else {
                    Level::Info
                };
                self.notify(level, text);
                // The scan of the process list lags behind: look again now.
                if launched.session.is_none() {
                    self.play.running = play::game_processes();
                }
                self.play.after_exit = Some(AfterExit {
                    root: launched.root.clone(),
                    revert: launched.revert_after,
                    since: std::time::Instant::now(),
                });
                self.finish_exit();
            }
        }
    }

    /// After the game exited: puts the originals back (once no game
    /// process is left, waiting up to a minute) and starts the game again
    /// for the tab's Restart.
    fn finish_exit(&mut self) {
        let Some(after) = &self.play.after_exit else {
            return;
        };
        let same_game = self.game.as_ref().is_some_and(|g| g.root() == after.root);
        if after.revert && same_game {
            if self.write_lock().is_some() {
                if after.since.elapsed() < Duration::from_secs(60) {
                    return;
                }
                self.notify(Level::Warning, tr("play.not_reverted").to_owned());
            } else {
                self.unapply();
            }
        }
        self.play.after_exit = None;
        // The tab's Restart: the same start once the files are back.
        if std::mem::take(&mut self.play.restart) && same_game {
            self.pending.push(super::Command::Play);
        }
    }

    // ------------------------------------------------------------ options

    /// Whether DXVK can be used with `method`. DXVK and 32-bit Vulkan are
    /// looked for once (folders and Vulkan manifests); the game folder each
    /// time.
    pub(super) fn dxvk_check(
        &mut self,
        method: &LaunchMethod,
        root: &std::path::Path,
    ) -> DxvkCheck {
        let (dxvk, vulkan) = self
            .play
            .dxvk_found
            .get_or_insert_with(|| (play::dxvk::find(), play::dxvk::vulkan32()));
        play::dxvk::check_with(method, root, dxvk.clone(), vulkan)
    }

    fn play_options_window(&mut self, ctx: &egui::Context) {
        if !self.play.options_open {
            return;
        }
        // Waits for the game (a window asked for at start).
        let Some(game) = self.game.clone() else {
            return;
        };
        let mut open = true;
        let settings = self.settings.game(game.root());
        let mut config = settings.play;
        let mut in_tab = settings.in_tab;
        let levels = play::start::level_streams(&game);
        let points = config
            .start
            .streams
            .first()
            .cloned()
            .and_then(|s| self.spawns(&s).cloned());
        let mut reload_points = false;
        let installs = self.installs();
        let check = {
            let method = match &config.method {
                LaunchMethod::Auto => play::auto_method(game.root(), &installs),
                m => m.clone(),
            };
            self.dxvk_check(&method, game.root())
        };
        let suggested: Vec<LaunchMethod> = installs
            .iter()
            .filter(|i| same_dir(&i.path, game.root()))
            .flat_map(|i| i.launch.clone())
            .collect();
        egui::Window::new(tr("play.options_title"))
            .open(&mut open)
            .default_width(560.0)
            .vscroll(true)
            .show(ctx, |ui| {
                ui.label(RichText::new(game.root().display().to_string()).weak());
                ui.separator();
                method_editor(ui, &mut config.method, game.root(), &installs);
                if !suggested.is_empty() {
                    ui.add_space(4.0);
                    ui.label(tr("play.suggested"));
                    for method in &suggested {
                        ui.horizontal(|ui| {
                            if ui.small_button(tr("play.use")).clicked() {
                                config.method = method.clone();
                            }
                            ui.label(method.describe());
                        });
                    }
                }
                ui.separator();
                ui.checkbox(&mut config.apply_before, tr("play.apply_before"));
                ui.checkbox(&mut config.revert_after, tr("play.revert_after"))
                    .on_hover_text(tr("play.revert_hint"));
                ui.separator();
                reload_points = super::gametab::start_editor(
                    ui,
                    &mut config.start,
                    &mut in_tab,
                    &levels,
                    points.as_ref(),
                    &super::gametab::tab_possible(&match &config.method {
                        LaunchMethod::Auto => play::auto_method(game.root(), &installs),
                        m => m.clone(),
                    }),
                );
                ui.separator();
                let method = match &config.method {
                    LaunchMethod::Auto => play::auto_method(game.root(), &installs),
                    m => m.clone(),
                };
                ui.strong(tr("dxvk.title"));
                ui.checkbox(&mut config.dxvk, tr("dxvk.enabled"))
                    .on_hover_text(tr("dxvk.enabled_hint"));
                let check = dxvk_ui(ui, config.dxvk, &check);
                ui.separator();
                ui.label(tr("play.command"));
                match play::plan(&method, game.root()) {
                    Ok(mut plan) => {
                        if config.dxvk && check {
                            play::dxvk::apply_env(&mut plan, game.root());
                        }
                        ui.add(
                            egui::Label::new(RichText::new(plan.command_line()).monospace()).wrap(),
                        );
                        let options = super::gametab::launch_options(&config.start, in_tab);
                        if !options.is_empty() {
                            let text = options.cmd_txt().map_or_else(
                                |e| e.to_string(),
                                |t| format!("cmd.txt: {}", t.trim_end()),
                            );
                            ui.add(egui::Label::new(RichText::new(text).monospace()).wrap());
                        }
                    }
                    Err(error) => {
                        ui.colored_label(doctor_toolkit_mods::BAD, error.to_string());
                    }
                }
                ui.label(RichText::new(tr("play.safety")).weak().small());
            });
        if self.settings.game(game.root()).play != config {
            self.settings.game_mut(game.root()).play = config;
        }
        if self.settings.game(game.root()).in_tab != in_tab {
            self.settings.game_mut(game.root()).in_tab = in_tab;
        }
        if reload_points {
            self.play.spawns.clear();
        }
        if !open {
            self.play.options_open = false;
            self.save_settings();
        }
    }

    // ----------------------------------------------------------- first run

    fn first_run_window(&mut self, ctx: &egui::Context) {
        let Some(first_run) = self.play.first_run.clone() else {
            return;
        };
        let Some(game) = self.game.clone() else {
            self.play.first_run = None;
            return;
        };
        let mut answer = None;
        egui::Modal::new(egui::Id::unique("first-run")).show(ctx, |ui| {
            ui.set_max_width(520.0);
            match &first_run {
                FirstRun::Pristine { files } => {
                    ui.heading(tr("first_run.pristine_title"));
                    ui.label(trf("first_run.pristine_text", &[("files", files)]));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button(tr("first_run.save")).clicked() {
                            answer = Some(Answer::SaveReference);
                        }
                        if ui.button(tr("first_run.later")).clicked() {
                            answer = Some(Answer::Later);
                        }
                        if ui.button(tr("first_run.never")).clicked() {
                            answer = Some(Answer::Never);
                        }
                    });
                }
                FirstRun::OldToolkit { patches, changed } => {
                    ui.heading(tr("first_run.old_title"));
                    ui.label(trf(
                        "first_run.old_text",
                        &[
                            ("patches", &trn("count.patches", patches.len() as u64, &[])),
                            ("changed", &trn("count.files", *changed as u64, &[])),
                        ],
                    ));
                    for name in patches.iter().take(12) {
                        ui.label(RichText::new(format!("• {name}")).weak());
                    }
                    ui.add_space(8.0);
                    ui.label(tr("first_run.old_choice"));
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .button(tr("first_run.take_over"))
                            .on_hover_text(tr("first_run.take_over_hint"))
                            .clicked()
                        {
                            answer = Some(Answer::TakeOver);
                        }
                        if ui
                            .button(tr("first_run.restore"))
                            .on_hover_text(tr("first_run.restore_hint"))
                            .clicked()
                        {
                            answer = Some(Answer::RestoreOld);
                        }
                        if ui.button(tr("first_run.later")).clicked() {
                            answer = Some(Answer::Later);
                        }
                        if ui.button(tr("first_run.never")).clicked() {
                            answer = Some(Answer::Never);
                        }
                    });
                }
            }
        });
        let Some(answer) = answer else { return };
        self.play.first_run = None;
        match answer {
            Answer::Later => {}
            Answer::Never => {
                self.settings.game_mut(game.root()).skip_first_run = true;
                self.save_settings();
            }
            Answer::SaveReference => self.save_reference(game),
            Answer::TakeOver => {
                self.mods_open = true;
                self.play.import_old_queued = true;
            }
            Answer::RestoreOld => self.restore_old_toolkit(game),
        }
    }

    /// Hashes the originals into `_doctor_toolkit/reference.json`.
    fn save_reference(&mut self, game: Arc<GameInstance>) {
        let (done, total) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let (d, t) = (Arc::clone(&done), Arc::clone(&total));
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let result = integrity::save_reference(&game, |n, of| {
                d.store(n, Ordering::Relaxed);
                t.store(of, Ordering::Relaxed);
            })
            .map(|count| {
                trf(
                    "first_run.saved",
                    &[
                        ("count", &trn("count.files", count as u64, &[])),
                        (
                            "path",
                            &game.root().join(integrity::REFERENCE_FILE).display(),
                        ),
                    ],
                )
            })
            .map_err(|e| e.to_string());
            let _ = tx.send(result);
        });
        self.play.tasks.push(Task {
            label: tr("first_run.saving").to_owned(),
            done,
            total,
            rx,
            writes_game: false,
        });
    }

    /// Adopts the old toolkit's backups and puts the originals back.
    fn restore_old_toolkit(&mut self, game: Arc<GameInstance>) {
        if self.refuse_write() {
            return;
        }
        self.release_files();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let result = integrity::adopt_old_backups(&game)
                .and_then(|adopted| Ok((adopted.len(), game.restore_backups()?.len())))
                .map(|(adopted, restored)| {
                    trf(
                        "first_run.restored",
                        &[
                            ("adopted", &trn("count.backups", adopted as u64, &[])),
                            ("restored", &trn("count.files", restored as u64, &[])),
                        ],
                    )
                })
                .map_err(|e| e.to_string());
            let _ = tx.send(result);
        });
        self.play.tasks.push(Task {
            label: tr("first_run.restoring").to_owned(),
            done: Arc::default(),
            total: Arc::default(),
            rx,
            writes_game: true,
        });
    }

    fn poll_tasks(&mut self) {
        let mut finished = Vec::new();
        self.play.tasks.retain(|task| match task.rx.try_recv() {
            Ok(result) => {
                finished.push((result, task.writes_game));
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                finished.push((Err(tr("play.task_stopped").to_owned()), task.writes_game));
                false
            }
        });
        for (result, writes_game) in finished {
            if writes_game {
                self.refresh_status();
                self.files_changed();
            }
            match result {
                Ok(text) => self.notify(Level::Info, text),
                Err(error) => self.notify(Level::Error, error),
            }
        }
    }
}

enum Answer {
    SaveReference,
    TakeOver,
    RestoreOld,
    Later,
    Never,
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let real = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    real(a) == real(b)
}

/// What to offer when `game` is opened: nothing once a reference list
/// exists or the toolkit keeps backups; the old toolkit's adoption when it
/// changed files; the snapshot when nothing is changed.
fn first_run_state(game: &GameInstance) -> Option<FirstRun> {
    let root = game.root();
    // The toolkit has been at work here: it keeps the originals already.
    let backups = root.join(doctor_toolkit::game::BACKUP_DIR);
    let has_backups = std::fs::read_dir(&backups).is_ok_and(|mut d| d.next().is_some());
    if root.join(integrity::REFERENCE_FILE).is_file() || has_backups {
        return None;
    }
    // Without our backups this hashes only the old toolkit's backups and
    // the files they belong to; other files are compared by size.
    let report = integrity::check(game, IntegrityOptions::default(), |_, _| {}).ok()?;
    if let Some(old) = &report.old_toolkit {
        let changed = report.count(Condition::OldToolkit);
        if changed > 0 {
            return Some(FirstRun::OldToolkit {
                patches: old.installed_patches.clone(),
                changed,
            });
        }
    }
    report
        .changed()
        .next()
        .is_none()
        .then_some(FirstRun::Pristine {
            files: report.files.len(),
        })
}

/// Choice of the launch method and its fields.
fn method_editor(ui: &mut egui::Ui, method: &mut LaunchMethod, root: &Path, installs: &[Install]) {
    let kinds = [
        ("auto", tr("play.method_auto")),
        ("steam", "Steam"),
        ("steam_shortcut", tr("play.method_shortcut")),
        ("wine", "Wine"),
        ("proton", "Proton"),
        ("lutris", "Lutris"),
        ("heroic", "Heroic"),
        ("bottles", "Bottles"),
        ("direct", tr("play.method_direct")),
        ("command", tr("play.method_command")),
    ];
    let current = method.kind();
    let label = kinds
        .iter()
        .find(|(k, _)| *k == current)
        .map_or(current, |(_, l)| l);
    let mut chosen = current;
    egui::ComboBox::from_label(tr("play.method"))
        .selected_text(label)
        .show_ui(ui, |ui| {
            for (kind, label) in kinds {
                ui.selectable_value(&mut chosen, kind, label);
            }
        });
    if chosen != current {
        *method = match chosen {
            "auto" => LaunchMethod::Auto,
            "steam" => LaunchMethod::Steam {
                app_id: install::STEAM_APP_ID,
            },
            "steam_shortcut" => LaunchMethod::SteamShortcut { app_id: 0 },
            "wine" => match play::fallback_method(root) {
                wine @ LaunchMethod::Wine { .. } => wine,
                _ => LaunchMethod::Wine {
                    prefix: None,
                    wine: None,
                    dll_overrides: None,
                },
            },
            "proton" => LaunchMethod::Proton {
                proton: PathBuf::new(),
                compat_data: PathBuf::new(),
            },
            "lutris" => LaunchMethod::Lutris {
                game: String::new(),
            },
            "heroic" => LaunchMethod::Heroic {
                app: String::new(),
                runner: "sideload".into(),
            },
            "bottles" => LaunchMethod::Bottles {
                bottle: String::new(),
                flatpak: false,
            },
            "direct" => LaunchMethod::Direct,
            _ => LaunchMethod::Command {
                command: "wine \"{exe}\"".into(),
            },
        };
    }
    let text = |ui: &mut egui::Ui, key: &str, value: &mut String| {
        ui.horizontal(|ui| {
            ui.label(tr(key));
            ui.add(
                egui::TextEdit::singleline(value)
                    .hint_text(tr("play.default"))
                    .desired_width(360.0),
            );
        });
    };
    let path = |ui: &mut egui::Ui, key: &str, value: &mut PathBuf| {
        let mut s = value.to_string_lossy().into_owned();
        ui.horizontal(|ui| {
            ui.label(tr(key));
            ui.add(
                egui::TextEdit::singleline(&mut s)
                    .hint_text(tr("play.default"))
                    .desired_width(300.0),
            );
            if ui.small_button(tr("prefs.choose")).clicked()
                && let Some(dir) = rfd::FileDialog::new().pick_folder()
            {
                s = dir.to_string_lossy().into_owned();
            }
        });
        *value = PathBuf::from(s);
    };
    match method {
        LaunchMethod::Auto => {
            ui.label(
                RichText::new(trf(
                    "play.auto_is",
                    &[("method", &play::auto_method(root, installs).describe())],
                ))
                .weak(),
            );
        }
        LaunchMethod::Steam { app_id } | LaunchMethod::SteamShortcut { app_id } => {
            ui.horizontal(|ui| {
                ui.label(tr("play.app_id"));
                ui.add(egui::DragValue::new(app_id));
            });
        }
        LaunchMethod::Wine {
            prefix,
            wine,
            dll_overrides,
        } => {
            let mut p = prefix.clone().unwrap_or_default();
            path(ui, "play.prefix", &mut p);
            *prefix = (!p.as_os_str().is_empty()).then_some(p);
            let mut w = wine.clone().unwrap_or_default();
            text(ui, "play.wine", &mut w);
            *wine = (!w.is_empty()).then_some(w);
            let mut d = dll_overrides.clone().unwrap_or_default();
            text(ui, "play.dll_overrides", &mut d);
            *dll_overrides = (!d.is_empty()).then_some(d);
        }
        LaunchMethod::Proton {
            proton,
            compat_data,
        } => {
            path(ui, "play.proton", proton);
            path(ui, "play.compat_data", compat_data);
        }
        LaunchMethod::Lutris { game } => text(ui, "play.lutris_game", game),
        LaunchMethod::Heroic { app, runner } => {
            text(ui, "play.heroic_app", app);
            text(ui, "play.heroic_runner", runner);
        }
        LaunchMethod::Bottles { bottle, flatpak } => {
            text(ui, "play.bottle", bottle);
            ui.checkbox(flatpak, "Flatpak");
        }
        LaunchMethod::Direct => {}
        LaunchMethod::Command { command } => {
            text(ui, "play.command_line", command);
            ui.label(RichText::new(tr("play.command_hint")).weak().small());
        }
    }
}

/// The state of DXVK under its check box; returns whether it can be used.
fn dxvk_ui(ui: &mut egui::Ui, enabled: bool, check: &DxvkCheck) -> bool {
    match check {
        Ok(found) => {
            if enabled {
                ui.label(
                    RichText::new(trf("dxvk.found", &[("source", &found.source)]))
                        .weak()
                        .small(),
                );
            }
            true
        }
        Err(problem) => {
            if enabled {
                ui.colored_label(doctor_toolkit_mods::WARN, dxvk_problem(problem));
            }
            false
        }
    }
}

/// Takes the DXVK variables of [`play::dxvk::apply_env`] out of a plan
/// again (DXVK could not be installed after all).
fn remove_dxvk_env(plan: &mut play::LaunchPlan) {
    plan.env.retain(|(k, _)| !k.starts_with("DXVK_"));
    for (key, value) in &mut plan.env {
        if key == "WINEDLLOVERRIDES" {
            let kept: Vec<String> = value
                .to_string_lossy()
                .split(';')
                .filter(|p| !p.is_empty() && *p != "d3d9=n,b")
                .map(str::to_owned)
                .collect();
            *value = kept.join(";").into();
        }
    }
    plan.env
        .retain(|(k, v)| !(k == "WINEDLLOVERRIDES" && v.is_empty()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_dxvk_install_leaves_the_asi_loader_override() {
        let root = std::path::Path::new("/game");
        let mut plan = play::LaunchPlan {
            program: "wine".into(),
            args: vec![],
            env: vec![("WINEDLLOVERRIDES".into(), "dinput8=n,b".into())],
            dir: root.into(),
            detached: false,
        };
        play::dxvk::apply_env(&mut plan, root);
        remove_dxvk_env(&mut plan);
        assert_eq!(
            plan.env,
            [("WINEDLLOVERRIDES".to_owned(), "dinput8=n,b".into())]
        );
        // Without the loader nothing is left.
        let mut plan = play::LaunchPlan {
            env: vec![],
            ..plan
        };
        play::dxvk::apply_env(&mut plan, root);
        remove_dxvk_env(&mut plan);
        if std::env::var_os("WINEDLLOVERRIDES").is_none() {
            assert!(plan.env.is_empty(), "{:?}", plan.env);
        }
    }

    #[test]
    fn first_run_offers_follow_the_game_files() {
        let dir = std::env::temp_dir().join(format!("dt-first-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let root = dir.as_path();
        std::fs::create_dir_all(root.join("gf2")).unwrap();
        std::fs::write(root.join("godfather2.exe"), b"MZ").unwrap();
        std::fs::write(root.join("gf2/a.str"), b"stream").unwrap();
        let game = GameInstance::open(root).unwrap();
        assert_eq!(
            first_run_state(&game),
            Some(FirstRun::Pristine { files: 1 })
        );
        // The old toolkit backed up the stream and changed it.
        std::fs::create_dir_all(root.join("_toolkit/backups/gf2")).unwrap();
        std::fs::write(root.join("_toolkit/backups/gf2/a.str"), b"original").unwrap();
        std::fs::write(
            root.join("_toolkit/game_instance.xml"),
            "<installed_patches><patch>P.vpat</patch></installed_patches>",
        )
        .unwrap();
        assert_eq!(
            first_run_state(&game),
            Some(FirstRun::OldToolkit {
                patches: vec!["P.vpat".into()],
                changed: 1
            })
        );
        // Once a reference list exists nothing is offered.
        std::fs::create_dir_all(root.join("_doctor_toolkit")).unwrap();
        std::fs::write(root.join(integrity::REFERENCE_FILE), b"{}").unwrap();
        assert_eq!(first_run_state(&game), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

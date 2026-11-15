//! The mod manager interface, shared by the standalone Open Mod Manager
//! (`open-mod-manager`) and the Mods window of the editor.
//!
//! All the logic lives in [`doctor_toolkit::mods`]; this crate only shows it.
//! Operations that read or write game files run on a worker thread, so the
//! window stays responsive.
//!
//! The panel is made for players (stage 64): a card per mod with its
//! picture and its state in words, order by dragging, a switch to turn a mod
//! on and off, saved sets of mods, confirmation before removing and
//! writing, undo of the last change, the progress of writing; technical
//! details are folded away.

pub mod games;
pub mod guard;
pub mod i18n;
pub mod icon;
pub mod play;
pub mod state;
pub mod theme;
pub mod widgets;

mod build;
mod cards;
mod details;
mod dialogs;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

use egui::{Color32, RichText};

use crate::i18n::{tr, trf, trn};

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::mods::ModManager;
use doctor_toolkit::mods::integrity::{self, Condition, IntegrityOptions, IntegrityReport};
use doctor_toolkit::mods::manager::{
    Clash, CompatIssue, Conflict, ModApplyOptions, RemovedMod, SetEntry,
};
use doctor_toolkit::workspace::{ApplyReport, DeployStatus, StreamAction, StreamState, Workspace};

pub const GOOD: Color32 = Color32::from_rgb(0x60, 0xc0, 0x60);
pub const WARN: Color32 = Color32::from_rgb(0xe0, 0xa0, 0x30);
pub const BAD: Color32 = Color32::from_rgb(0xe0, 0x50, 0x50);

/// Folder of the settings files of both programs (`~/.config/doctor-toolkit`,
/// `%APPDATA%\doctor-toolkit`, or `$DOCTOR_TOOLKIT_CONFIG_DIR`); `None` when
/// no home folder is known.
pub fn config_dir() -> Option<PathBuf> {
    let var = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if let Some(dir) = var("DOCTOR_TOOLKIT_CONFIG_DIR") {
        return Some(dir);
    }
    let base = if cfg!(windows) {
        var("APPDATA")
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|h| h.join("Library").join("Application Support"))
    } else {
        var("XDG_CONFIG_HOME").or_else(|| var("HOME").map(|h| h.join(".config")))
    };
    base.map(|b| b.join("doctor-toolkit"))
}

/// Whether `path` names a file the panel installs: a mod (`.zip`) or a
/// patch of the old toolkit (`.vpat`).
pub fn is_mod_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("zip") || e.eq_ignore_ascii_case("vpat"))
}

/// What the panel tells its host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelEvent {
    /// Game files are about to be replaced: release open files.
    WillWrite,
    /// Game files were replaced: reopen what is shown.
    GameChanged,
    /// "Play with mods" was pressed (Open Mod Manager only).
    PlayWithMods,
}

/// The state of the mods compared with the game, computed on the worker thread.
#[derive(Default)]
pub struct Overview {
    pub(crate) status: Option<DeployStatus>,
    pub(crate) issues: Vec<CompatIssue>,
    pub(crate) conflicts: Vec<Conflict>,
    pub(crate) all_conflicts: Vec<Conflict>,
    /// Clashes of mods that add resources or create streams.
    pub(crate) clashes: Vec<Clash>,
    pub(crate) all_clashes: Vec<Clash>,
    /// Streams the editor's workspace holds in the game.
    pub(crate) workspace_held: Vec<String>,
    /// Streams whose last write by the mod manager did not finish.
    pub(crate) interrupted: Vec<String>,
    pub(crate) old_toolkit: bool,
    /// Names of the saved sets of mods.
    pub(crate) sets: Vec<String>,
    /// The saved set the mods are arranged as now.
    pub(crate) current_set: Option<String>,
}

impl Overview {
    fn up_to_date(&self) -> bool {
        self.status
            .as_ref()
            .is_none_or(|s| s.streams.iter().all(|s| s.up_to_date))
    }

    fn foreign_changes(&self) -> bool {
        self.status.as_ref().is_some_and(|s| {
            s.streams
                .iter()
                .any(|s| s.state == StreamState::ChangedExternally && !s.up_to_date)
        })
    }

    fn issues_of<'a>(&'a self, id: &'a str) -> impl Iterator<Item = &'a CompatIssue> {
        self.issues.iter().filter(move |i| i.mod_id == id)
    }
}

/// How to bring back the state before the last change.
enum UndoKind {
    /// The order and the enabled mods before.
    Arrangement(Vec<SetEntry>),
    /// A removed mod, kept aside.
    Removed(RemovedMod),
}

/// The last change, which can be undone.
struct Undo {
    /// What was done, e.g. "Turned off New York".
    label: String,
    kind: UndoKind,
}

/// A result shown in a window: of writing, restoring or checking the files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Report {
    title: String,
    text: String,
    error: bool,
}

/// How a job is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobKind {
    /// In the status line.
    Quiet,
    /// With a window of its progress and a window of its result.
    Writing,
    /// With a window of its result.
    Checking,
}

/// What a job gives back besides the manager.
#[derive(Default)]
struct Outcome {
    message: String,
    integrity: Option<IntegrityReport>,
    undo: Option<Undo>,
    report: Option<Report>,
    /// A mod to select, e.g. the one just installed.
    select: Option<String>,
    /// The message tells of a problem besides what was done.
    warning: bool,
}

impl Outcome {
    fn message(message: String) -> Self {
        Self {
            message,
            ..Self::default()
        }
    }
}

/// Result of a job.
struct Done {
    manager: ModManager,
    outcome: Result<Outcome, String>,
    overview: Overview,
    wrote: bool,
    kind: JobKind,
}

/// How far a job is: done, total, what is done now.
type Progress = Arc<Mutex<Option<(usize, usize, String)>>>;

struct Running {
    label: String,
    rx: Receiver<Done>,
    progress: Progress,
    kind: JobKind,
}

/// The mod manager interface for one game.
pub struct ModsPanel {
    game: Option<Arc<GameInstance>>,
    /// `None` while a job holds it, or when no game is open.
    manager: Option<ModManager>,
    overview: Overview,
    running: Option<Running>,
    message: Option<(String, bool)>,
    selected: Option<String>,
    force: bool,
    ignore_compatibility: bool,
    show_all_conflicts: bool,
    integrity: Option<IntegrityReport>,
    previews: HashMap<String, Option<egui::TextureHandle>>,
    build: build::BuildForm,
    pending_event: Option<PanelEvent>,
    /// Why writing into the game is refused now (the game runs).
    write_lock: Option<String>,
    /// Open Mod Manager: the Play button, dropped files and keys.
    standalone: bool,
    /// What the Play button says while a launch is prepared or runs.
    play_busy: Option<String>,
    undo: Option<Undo>,
    dialog: Option<dialogs::Dialog>,
}

impl Default for ModsPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ModsPanel {
    fn drop(&mut self) {
        self.forget_undo();
    }
}

impl ModsPanel {
    /// The panel of the editor's Mods window.
    pub fn new() -> Self {
        Self {
            game: None,
            manager: None,
            overview: Overview::default(),
            running: None,
            message: None,
            selected: None,
            force: false,
            ignore_compatibility: false,
            show_all_conflicts: false,
            integrity: None,
            previews: HashMap::new(),
            build: build::BuildForm::default(),
            pending_event: None,
            write_lock: None,
            standalone: false,
            play_busy: None,
            undo: None,
            dialog: None,
        }
    }

    /// The panel of Open Mod Manager: with "Play with mods", files dropped
    /// on the window and keyboard shortcuts.
    pub fn standalone() -> Self {
        let mut panel = Self::new();
        panel.standalone = true;
        panel
    }

    /// Refuses (`Some(reason)`) or allows writing into the game: applying,
    /// restoring and installing with apply. Set while the game runs.
    pub fn set_write_lock(&mut self, reason: Option<String>) {
        self.write_lock = reason;
    }

    /// Why writing into the game is refused now.
    pub fn write_lock(&self) -> Option<&str> {
        self.write_lock.as_deref()
    }

    /// What the Play button says instead while a launch is prepared or the
    /// game runs; `None` when it can be pressed.
    pub fn set_play_busy(&mut self, busy: Option<String>) {
        self.play_busy = busy;
    }

    /// Shows the mods of `game`.
    pub fn set_game(&mut self, game: Option<Arc<GameInstance>>, ctx: &egui::Context) {
        self.forget_undo();
        self.game = game;
        self.manager = None;
        self.overview = Overview::default();
        self.integrity = None;
        self.previews.clear();
        self.selected = None;
        self.message = None;
        self.dialog = None;
        let Some(game) = &self.game else { return };
        match ModManager::open(game) {
            Ok(manager) => {
                // Mods kept aside for an undo by a program that stopped.
                let _ = manager.purge_removed();
                self.manager = Some(manager);
                self.refresh(ctx);
            }
            Err(error) => {
                self.message = Some((trf("mods.cannot_open", &[("error", &error)]), true));
            }
        }
    }

    pub fn game(&self) -> Option<&Arc<GameInstance>> {
        self.game.as_ref()
    }

    /// The mod manager, unless a job holds it.
    pub fn manager(&self) -> Option<&ModManager> {
        self.manager.as_ref()
    }

    /// Whether a job is running.
    pub fn is_busy(&self) -> bool {
        self.running.is_some()
    }

    /// Label of the running job.
    pub fn busy_label(&self) -> Option<&str> {
        self.running.as_ref().map(|r| r.label.as_str())
    }

    /// The message of the last job: text and whether it is an error.
    pub fn message(&self) -> Option<(&str, bool)> {
        self.message.as_ref().map(|(m, e)| (m.as_str(), *e))
    }

    /// Selects a mod, e.g. from the command line.
    pub fn select(&mut self, id: &str) {
        self.selected = Some(id.to_owned());
    }

    /// The selected mod.
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// The label of the change [`Self::undo`] takes back, if any.
    pub fn undo_label(&self) -> Option<&str> {
        self.undo.as_ref().map(|u| u.label.as_str())
    }

    /// Whether the game has what the enabled mods want.
    pub fn up_to_date(&self) -> bool {
        self.overview.up_to_date()
    }

    /// The saved sets of mods and the one the mods are arranged as now.
    pub fn sets(&self) -> (&[String], Option<&str>) {
        (&self.overview.sets, self.overview.current_set.as_deref())
    }

    /// The state of mod `id` in plain words (for the cards).
    pub fn state_of(&self, id: &str) -> Option<state::ModState> {
        let manager = self.manager.as_ref()?;
        let m = manager.get(id)?;
        Some(state::ModState::of(m, manager, &self.overview))
    }

    /// Whether a window asks or tells something now.
    pub fn has_dialog(&self) -> bool {
        self.dialog.is_some()
    }

    /// Waits for the running job (tests and screenshots).
    pub fn wait(&mut self) {
        while let Some(running) = &self.running {
            match running.rx.recv() {
                Ok(done) => self.finish(done),
                Err(_) => self.running = None,
            }
        }
    }

    /// Compares the mods with the game again.
    pub fn refresh(&mut self, ctx: &egui::Context) {
        self.run(
            ctx,
            tr("mods.job_checking"),
            false,
            |_, _| Ok(String::new()),
        );
    }

    /// Starts the integrity check; its result is told in plain words.
    pub fn check_integrity(&mut self, ctx: &egui::Context) {
        self.run_job(
            ctx,
            tr("mods.job_integrity"),
            false,
            JobKind::Checking,
            |game, _, _| {
                let report = integrity::check(game, IntegrityOptions::default(), |_, _| {})
                    .map_err(|e| e.to_string())?;
                let (text, error) = integrity_words(&report);
                Ok(Outcome {
                    message: text.lines().next().unwrap_or_default().to_owned(),
                    report: Some(Report {
                        title: tr("mods.integrity_title").to_owned(),
                        text,
                        error,
                    }),
                    integrity: Some(report),
                    ..Outcome::default()
                })
            },
        );
    }

    fn run(
        &mut self,
        ctx: &egui::Context,
        label: &str,
        writes: bool,
        work: impl FnOnce(&GameInstance, &mut ModManager) -> Result<String, String> + Send + 'static,
    ) {
        self.run_job(
            ctx,
            label,
            writes,
            JobKind::Quiet,
            move |game, manager, _| work(game, manager).map(Outcome::message),
        );
    }

    /// Runs `work` on a worker thread with the manager, then compares the
    /// result with the game.
    fn run_job(
        &mut self,
        ctx: &egui::Context,
        label: &str,
        writes: bool,
        kind: JobKind,
        work: impl FnOnce(&GameInstance, &mut ModManager, &Progress) -> Result<Outcome, String>
        + Send
        + 'static,
    ) {
        if writes && let Some(reason) = &self.write_lock {
            self.message = Some((
                trf("mods.not_now", &[("label", &label), ("reason", reason)]),
                true,
            ));
            return;
        }
        let (Some(game), Some(mut manager)) = (self.game.clone(), self.manager.take()) else {
            return;
        };
        if writes {
            self.pending_event = Some(PanelEvent::WillWrite);
        }
        let (tx, rx) = channel();
        let progress: Progress = Arc::default();
        let shared = progress.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let outcome = work(&game, &mut manager, &shared);
            let overview = overview(&game, &manager);
            let _ = tx.send(Done {
                manager,
                outcome,
                overview,
                wrote: writes,
                kind,
            });
            ctx.request_repaint();
        });
        self.running = Some(Running {
            label: label.to_owned(),
            rx,
            progress,
            kind,
        });
    }

    fn poll(&mut self) {
        let Some(running) = &self.running else { return };
        let Ok(done) = running.rx.try_recv() else {
            return;
        };
        self.finish(done);
    }

    fn finish(&mut self, done: Done) {
        self.running = None;
        self.manager = Some(done.manager);
        self.overview = done.overview;
        match done.outcome {
            Ok(outcome) => {
                if outcome.integrity.is_some() {
                    self.integrity = outcome.integrity;
                }
                if let Some(undo) = outcome.undo {
                    self.forget_undo();
                    self.undo = Some(undo);
                }
                if let Some(id) = outcome.select {
                    self.selected = Some(id);
                }
                if !outcome.message.is_empty() {
                    self.message = Some((outcome.message, outcome.warning));
                }
                if let Some(report) = outcome.report
                    && done.kind != JobKind::Quiet
                {
                    self.dialog = Some(dialogs::Dialog::Report(report));
                }
            }
            Err(error) => {
                if done.kind != JobKind::Quiet {
                    self.dialog = Some(dialogs::Dialog::Report(Report {
                        title: tr("mods.failed_title").to_owned(),
                        text: error.clone(),
                        error: true,
                    }));
                }
                self.message = Some((error, true));
            }
        }
        if done.wrote {
            self.pending_event = Some(PanelEvent::GameChanged);
        }
        if let Some(manager) = &self.manager {
            // Pictures of removed mods and of the versions an update replaced.
            let current: std::collections::HashSet<String> =
                manager.mods().iter().map(cards::preview_key).collect();
            self.previews.retain(|key, _| current.contains(key));
            if let Some(id) = &self.selected
                && manager.get(id).is_none()
            {
                self.selected = None;
            }
        }
    }

    /// Deletes the files of a removed mod kept for the undo.
    fn forget_undo(&mut self) {
        if let Some(Undo {
            kind: UndoKind::Removed(removed),
            ..
        }) = self.undo.take()
            && let Some(manager) = &self.manager
        {
            let _ = manager.discard_removed(&removed);
        }
    }

    /// The name of mod `id`.
    fn name_of(&self, id: &str) -> String {
        self.manager
            .as_ref()
            .and_then(|m| m.get(id))
            .map_or_else(|| id.to_owned(), |m| m.manifest.name.clone())
    }

    // ------------------------------------------------------------- actions

    /// Installs mod files (`.zip`, `.vpat`) at once.
    pub fn install_files(&mut self, ctx: &egui::Context, paths: Vec<PathBuf>) {
        self.install_with_note(ctx, paths, None);
    }

    /// Installs mod files; `note` (files refused) is told after the result.
    fn install_with_note(
        &mut self,
        ctx: &egui::Context,
        paths: Vec<PathBuf>,
        note: Option<String>,
    ) {
        self.run_job(
            ctx,
            tr("mods.job_installing"),
            false,
            JobKind::Quiet,
            move |game, manager, _| {
                let mut lines = Vec::new();
                let mut select = None;
                for path in &paths {
                    let name = path.file_name().map_or_else(
                        || path.display().to_string(),
                        |n| n.to_string_lossy().into_owned(),
                    );
                    let report = manager.install_file(game, path, true).map_err(|e| {
                        trf("mods.cannot_install", &[("name", &name), ("error", &e)])
                    })?;
                    let mut line = match &report.replaced_version {
                        Some(old) => trf("mods.updated", &[("name", &report.name), ("old", old)]),
                        None => trf("mods.installed", &[("name", &report.name)]),
                    };
                    if !report.issues.is_empty() {
                        line += "; ";
                        line += &trn("mods.misfit_resources", report.issues.len() as u64, &[]);
                    }
                    if let Some(warning) = report.warnings.first() {
                        line += &format!("; {warning}");
                    }
                    lines.push(line);
                    select = Some(report.id);
                }
                let mut message = lines.join(". ") + ".";
                if let Some(note) = &note {
                    message += " ";
                    message += note;
                }
                Ok(Outcome {
                    message,
                    select,
                    warning: note.is_some(),
                    ..Outcome::default()
                })
            },
        );
    }

    /// Asks whether to install `paths` (opened from the system: a double
    /// click on a mod file, "Open with"), then installs them.
    pub fn offer_install(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        self.dialog = Some(dialogs::Dialog::Install(paths));
    }

    /// Installs the files dropped on the window; other files are refused
    /// with a word.
    pub fn drop_files(&mut self, ctx: &egui::Context, paths: Vec<PathBuf>) {
        let (mods, other): (Vec<PathBuf>, Vec<PathBuf>) =
            paths.into_iter().partition(|p| is_mod_file(p));
        let note = (!other.is_empty()).then(|| {
            let names: Vec<String> = other
                .iter()
                .map(|p| {
                    p.file_name()
                        .map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into())
                })
                .collect();
            trf("mods.not_mod_files", &[("files", &names.join(", "))])
        });
        if mods.is_empty() {
            if let Some(note) = note {
                self.message = Some((note, true));
            }
            return;
        }
        if self.running.is_some() || self.manager.is_none() {
            self.message = Some((tr("mods.busy_try_again").to_owned(), true));
            return;
        }
        self.install_with_note(ctx, mods, note);
    }

    /// Writes the enabled mods into the game, with progress and a result.
    pub fn apply(&mut self, ctx: &egui::Context) {
        let options = ModApplyOptions {
            force: self.force,
            ignore_compatibility: self.ignore_compatibility,
        };
        self.run_job(
            ctx,
            tr("mods.job_applying"),
            true,
            JobKind::Writing,
            move |game, manager, progress| {
                let report = manager
                    .apply_with_progress(game, options, &mut |p| {
                        *progress.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some((p.done, p.total, p.stream.to_owned()));
                    })
                    .map_err(|e| e.to_string())?;
                let enabled = manager.mods().iter().filter(|m| m.enabled).count();
                Ok(Outcome {
                    message: describe(&report, tr("mods.applied")),
                    report: Some(Report {
                        title: tr("mods.written_title").to_owned(),
                        text: written_words(&report, enabled),
                        error: false,
                    }),
                    ..Outcome::default()
                })
            },
        );
    }

    /// Puts the originals back, with progress and a result.
    pub fn unapply(&mut self, ctx: &egui::Context) {
        let force = self.force;
        self.run_job(
            ctx,
            tr("mods.job_restoring"),
            true,
            JobKind::Writing,
            move |game, manager, progress| {
                let report = manager
                    .unapply_with_progress(game, force, &mut |p| {
                        *progress.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some((p.done, p.total, p.stream.to_owned()));
                    })
                    .map_err(|e| e.to_string())?;
                Ok(Outcome {
                    message: describe(&report, tr("mods.restored")),
                    report: Some(Report {
                        title: tr("mods.restored_title").to_owned(),
                        text: restored_words(&report),
                        error: false,
                    }),
                    ..Outcome::default()
                })
            },
        );
    }

    /// A change of the order or the enabled mods that can be undone.
    fn rearrange(
        &mut self,
        ctx: &egui::Context,
        label: String,
        work: impl FnOnce(&mut ModManager) -> doctor_toolkit::Result<String> + Send + 'static,
    ) {
        let Some(before) = self.manager.as_ref().map(ModManager::arrangement) else {
            return;
        };
        self.run_job(
            ctx,
            tr("mods.job_updating"),
            false,
            JobKind::Quiet,
            move |_, manager, _| {
                let message = work(manager).map_err(|e| e.to_string())?;
                Ok(Outcome {
                    message,
                    undo: Some(Undo {
                        label,
                        kind: UndoKind::Arrangement(before),
                    }),
                    ..Outcome::default()
                })
            },
        );
    }

    /// Turns mod `id` on or off.
    pub fn set_enabled(&mut self, ctx: &egui::Context, id: &str, on: bool) {
        let name = self.name_of(id);
        let label = if on {
            trf("mods.undo_on", &[("name", &name)])
        } else {
            trf("mods.undo_off", &[("name", &name)])
        };
        let id = id.to_owned();
        self.rearrange(ctx, label, move |m| {
            m.set_enabled(&id, on).map(|()| String::new())
        });
    }

    /// Moves mod `id` to `position` in the load order.
    pub fn move_mod(&mut self, ctx: &egui::Context, id: &str, position: usize) {
        let label = trf("mods.undo_move", &[("name", &self.name_of(id))]);
        let id = id.to_owned();
        self.rearrange(ctx, label, move |m| {
            m.move_mod(&id, position).map(|()| String::new())
        });
    }

    /// Arranges the mods as the saved set `name`.
    pub fn use_set(&mut self, ctx: &egui::Context, name: &str) {
        let label = trf("mods.undo_set", &[("name", &name)]);
        let set = name.to_owned();
        self.rearrange(ctx, label, move |m| {
            let missing = m.use_set(&set)?;
            Ok(if missing.is_empty() {
                trf("mods.set_used", &[("name", &set)])
            } else {
                trf(
                    "mods.set_used_missing",
                    &[("name", &set), ("mods", &missing.join(", "))],
                )
            })
        });
    }

    /// Saves the order and the enabled mods as the set `name`.
    pub fn save_set(&mut self, ctx: &egui::Context, name: &str) {
        let name = name.trim().to_owned();
        self.run(ctx, tr("mods.job_updating"), false, move |_, m| {
            m.save_set(&name).map_err(|e| e.to_string())?;
            Ok(trf("mods.set_saved", &[("name", &name)]))
        });
    }

    /// Forgets the saved set `name`.
    pub fn delete_set(&mut self, ctx: &egui::Context, name: &str) {
        let name = name.to_owned();
        self.run(ctx, tr("mods.job_updating"), false, move |_, m| {
            m.delete_set(&name).map_err(|e| e.to_string())?;
            Ok(trf("mods.set_deleted", &[("name", &name)]))
        });
    }

    /// Removes mod `id`; [`Self::undo`] brings it back.
    pub fn remove(&mut self, ctx: &egui::Context, id: &str) {
        let id = id.to_owned();
        self.run_job(
            ctx,
            tr("mods.job_updating"),
            false,
            JobKind::Quiet,
            move |_, manager, _| {
                let removed = manager.remove_undoable(&id).map_err(|e| e.to_string())?;
                Ok(Outcome {
                    message: trf("mods.removed", &[("name", &removed.name)]),
                    undo: Some(Undo {
                        label: trf("mods.undo_remove", &[("name", &removed.name)]),
                        kind: UndoKind::Removed(removed),
                    }),
                    ..Outcome::default()
                })
            },
        );
    }

    /// Takes back the last change: switching, moving, removing a mod or
    /// using a set.
    pub fn undo(&mut self, ctx: &egui::Context) {
        if self.running.is_some() || self.manager.is_none() {
            return;
        }
        let Some(undo) = self.undo.take() else { return };
        let label = undo.label.clone();
        self.run(ctx, tr("mods.job_updating"), false, move |_, manager| {
            match &undo.kind {
                UndoKind::Arrangement(before) => manager.arrange(before).map(|_| ()),
                UndoKind::Removed(removed) => manager.restore_removed(removed),
            }
            .map_err(|e| e.to_string())?;
            Ok(trf("mods.undone", &[("what", &label)]))
        });
    }

    /// Adopts the old toolkit's backups and installs its patches as mods.
    pub fn import_old_toolkit(&mut self, ctx: &egui::Context) {
        self.run(ctx, tr("mods.job_importing"), false, |game, manager| {
            let import = manager
                .import_old_toolkit(game)
                .map_err(|e| e.to_string())?;
            let ok = import.installed.iter().filter(|(_, r)| r.is_ok()).count();
            let failed: Vec<String> = import
                .installed
                .iter()
                .filter_map(|(name, r)| r.as_ref().err().map(|e| format!("{name}: {e}")))
                .collect();
            let mut message = trf(
                "mods.imported",
                &[
                    (
                        "originals",
                        &trn("count.originals", import.adopted.len() as u64, &[]),
                    ),
                    ("patches", &trn("count.patches", ok as u64, &[])),
                ],
            );
            if !failed.is_empty() {
                message += " ";
                message += &trf("mods.not_installed", &[("list", &failed.join("; "))]);
            }
            Ok(message)
        });
    }

    /// Asks before writing the mods into the game.
    pub fn ask_apply(&mut self) {
        self.dialog = Some(dialogs::Dialog::Write);
    }

    /// Asks before removing mod `id`.
    pub fn ask_remove(&mut self, id: &str) {
        self.dialog = Some(dialogs::Dialog::Remove {
            id: id.to_owned(),
            name: self.name_of(id),
        });
    }

    // -------------------------------------------------------------- layout

    /// Shows the panel. `workspace` enables the build section (editor only).
    pub fn show(&mut self, ui: &mut egui::Ui, workspace: Option<&Workspace>) -> Option<PanelEvent> {
        self.poll();
        if self.running.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        if self.standalone {
            let dropped: Vec<PathBuf> = ui.ctx().input(|i| {
                i.raw
                    .dropped_files
                    .iter()
                    .map(|f| f.path().to_path_buf())
                    .collect()
            });
            if !dropped.is_empty() && self.game.is_some() {
                self.drop_files(ui.ctx(), dropped);
            }
        }
        if self.game.is_none() {
            ui.label(RichText::new(tr("mods.no_game")).weak());
            return self.pending_event.take();
        }
        let area = ui.max_rect();
        self.action_bar(ui);
        ui.add_space(4.0);
        self.status_line(ui);
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt("mods-panel")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.cards(ui);
                ui.add_space(8.0);
                self.details(ui);
                if let Some(ws) = workspace {
                    self.build_section(ui, ws);
                }
            });
        if self.standalone {
            self.keys(ui.ctx());
            self.drop_overlay(ui, area);
        }
        self.dialogs(ui.ctx());
        self.pending_event.take()
    }

    fn action_bar(&mut self, ui: &mut egui::Ui) {
        use widgets::{Prominence, action_button};
        let idle = self.running.is_none() && self.manager.is_some();
        let has_mods = self.manager.as_ref().is_some_and(|m| !m.mods().is_empty());
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if self.standalone {
                let can_play = idle && self.play_busy.is_none() && self.write_lock.is_none();
                let button = match &self.play_busy {
                    Some(busy) => action_button(ui, false, busy, Prominence::Main),
                    None => widgets::play_button(ui, can_play, tr("mods.play")),
                };
                if button.on_hover_text(tr("mods.play_hint")).clicked() {
                    self.pending_event = Some(PanelEvent::PlayWithMods);
                }
            }
            let up_to_date = self.overview.up_to_date();
            if action_button(
                ui,
                idle && !up_to_date,
                tr("mods.apply"),
                Prominence::Normal,
            )
            .on_hover_text(tr("mods.apply_hint"))
            .clicked()
            {
                self.dialog = Some(dialogs::Dialog::Write);
            }
            let has_written = self.manager.as_ref().is_some_and(ModManager::has_written);
            if action_button(
                ui,
                idle && has_written,
                tr("mods.unapply"),
                Prominence::Normal,
            )
            .on_hover_text(tr("mods.unapply_hint"))
            .clicked()
            {
                self.dialog = Some(dialogs::Dialog::Restore);
            }
            theme::separator(ui);
            if action_button(ui, idle, tr("mods.install"), Prominence::Normal)
                .on_hover_text(tr("mods.install_hint"))
                .clicked()
                && let Some(paths) = rfd::FileDialog::new()
                    .set_title(tr("mods.install_title"))
                    .add_filter(tr("mods.filter_mods"), &["zip", "vpat"])
                    .pick_files()
            {
                self.install_files(ui.ctx(), paths);
            }
            if has_mods || !self.overview.sets.is_empty() {
                self.set_choice(ui, idle);
            }
            self.menu(ui, idle);
        });
    }

    /// The saved sets of mods: switch, save, delete.
    fn set_choice(&mut self, ui: &mut egui::Ui, idle: bool) {
        let current = self.overview.current_set.clone();
        let shown = current
            .clone()
            .unwrap_or_else(|| tr("mods.set_unsaved").to_owned());
        ui.add_enabled_ui(idle, |ui| {
            ui.label(RichText::new(tr("mods.set")).weak());
            egui::ComboBox::from_id_salt("mod-set")
                .selected_text(shown)
                .width(170.0)
                .show_ui(ui, |ui| {
                    for name in self.overview.sets.clone() {
                        if ui
                            .selectable_label(current.as_deref() == Some(name.as_str()), &name)
                            .clicked()
                            && current.as_deref() != Some(name.as_str())
                        {
                            self.use_set(ui.ctx(), &name);
                        }
                    }
                    if !self.overview.sets.is_empty() {
                        ui.separator();
                    }
                    if ui.button(tr("mods.set_save")).clicked() {
                        self.dialog = Some(dialogs::Dialog::SaveSet(
                            current.clone().unwrap_or_default(),
                        ));
                    }
                    if let Some(name) = &current
                        && ui
                            .button(trf("mods.set_delete", &[("name", name)]))
                            .clicked()
                    {
                        self.dialog = Some(dialogs::Dialog::DeleteSet(name.clone()));
                    }
                })
                .response
                .on_hover_text(tr("mods.set_hint"));
        });
    }

    /// The menu of what players rarely need.
    fn menu(&mut self, ui: &mut egui::Ui, idle: bool) {
        ui.menu_button(tr("mods.more"), |ui| {
            if ui
                .add_enabled(idle, egui::Button::new(tr("mods.check_integrity")))
                .on_hover_text(tr("mods.check_integrity_hint"))
                .clicked()
            {
                self.check_integrity(ui.ctx());
                ui.close();
            }
            if self.overview.old_toolkit
                && ui
                    .add_enabled(idle, egui::Button::new(tr("mods.import_old")))
                    .on_hover_text(tr("mods.import_old_hint"))
                    .clicked()
            {
                self.import_old_toolkit(ui.ctx());
                ui.close();
            }
            ui.separator();
            ui.label(RichText::new(tr("mods.advanced")).weak().small());
            ui.checkbox(&mut self.force, tr("mods.force"))
                .on_hover_text(tr("mods.force_hint"));
            ui.checkbox(
                &mut self.ignore_compatibility,
                tr("mods.ignore_compatibility"),
            );
        })
        .response
        .on_hover_text(tr("mods.more_hint"));
    }

    fn status_line(&mut self, ui: &mut egui::Ui) {
        let Some(manager) = &self.manager else {
            if self.running.is_none() {
                ui.label(RichText::new(tr("mods.unreadable")).color(BAD));
            }
            self.message_line(ui);
            return;
        };
        let enabled = manager.mods().iter().filter(|m| m.enabled).count();
        let count = manager.mods().len();
        ui.horizontal_wrapped(|ui| {
            if count > 0 {
                ui.label(trf(
                    "mods.summary",
                    &[
                        ("installed", &trn("mods.installed_count", count as u64, &[])),
                        ("enabled", &enabled),
                    ],
                ));
            }
            if let Some(reason) = &self.write_lock {
                ui.label(RichText::new(tr("mods.game_running")).color(WARN))
                    .on_hover_text(reason.as_str());
            } else if !self.overview.interrupted.is_empty() {
                let text = trf(
                    "mods.interrupted",
                    &[("streams", &self.overview.interrupted.join(", "))],
                );
                ui.add(egui::Label::new(RichText::new(text).color(BAD)).wrap());
                if ui
                    .add_enabled(
                        self.running.is_none(),
                        egui::Button::new(tr("mods.unapply")).small(),
                    )
                    .on_hover_text(tr("mods.unapply_hint"))
                    .clicked()
                {
                    self.dialog = Some(dialogs::Dialog::Restore);
                }
            } else if !self.overview.workspace_held.is_empty() {
                ui.colored_label(BAD, tr("mods.workspace_held"));
            } else if self.overview.foreign_changes() {
                ui.colored_label(BAD, tr("mods.foreign_changes"));
            } else if !self.overview.up_to_date() {
                if self.standalone {
                    ui.colored_label(theme::FOCUS, tr("mods.not_applied_play"));
                } else {
                    ui.colored_label(theme::FOCUS, tr("mods.not_applied"));
                }
            } else if enabled > 0 {
                ui.colored_label(GOOD, tr("mods.up_to_date"));
            }
        });
        self.message_line(ui);
    }

    /// The message of the last job, the running job and the undo button.
    fn message_line(&mut self, ui: &mut egui::Ui) {
        let running = self.running.as_ref().map(|r| r.label.clone());
        let undo = self
            .undo
            .as_ref()
            .map(|u| trf("mods.undo", &[("what", &u.label)]));
        if self.message.is_none() && running.is_none() && undo.is_none() {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            if let Some(label) = running {
                ui.spinner();
                ui.label(label);
            } else if let Some((message, error)) = &self.message {
                let text = RichText::new(message).color(if *error { BAD } else { GOOD });
                ui.add(egui::Label::new(text).wrap());
            }
            if let Some(label) = undo
                && ui
                    .add_enabled(self.running.is_none(), egui::Button::new(label).small())
                    .on_hover_text(tr("mods.undo_hint"))
                    .clicked()
            {
                self.undo(ui.ctx());
            }
        });
    }

    /// Keys of Open Mod Manager: Ctrl+Z undoes, Up/Down select a mod,
    /// Alt+Up/Down move it, Space switches it, Delete removes it.
    fn keys(&mut self, ctx: &egui::Context) {
        use egui::{Key, Modifiers};
        if self.dialog.is_some() || ctx.egui_wants_keyboard_input() {
            return;
        }
        let idle = self.running.is_none() && self.manager.is_some();
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Z)) {
            self.undo(ctx);
            return;
        }
        let Some(manager) = &self.manager else { return };
        // (id, enabled, damaged) in load order.
        let mods: Vec<(String, bool, bool)> = manager
            .mods()
            .iter()
            .map(|m| (m.id.clone(), m.enabled, m.problem.is_some()))
            .collect();
        if mods.is_empty() {
            return;
        }
        let index = self
            .selected
            .as_ref()
            .and_then(|s| mods.iter().position(|(id, _, _)| id == s));
        let pressed = |key, modifiers| ctx.input_mut(|i| i.consume_key(modifiers, key));
        if let Some(i) = index {
            let (id, enabled, damaged) = mods[i].clone();
            if pressed(Key::ArrowUp, Modifiers::ALT) {
                if idle && i > 0 {
                    self.move_mod(ctx, &id, i - 1);
                }
                return;
            }
            if pressed(Key::ArrowDown, Modifiers::ALT) {
                if idle && i + 1 < mods.len() {
                    self.move_mod(ctx, &id, i + 1);
                }
                return;
            }
            if pressed(Key::Delete, Modifiers::NONE) {
                if idle {
                    self.ask_remove(&id);
                }
                return;
            }
            if pressed(Key::Space, Modifiers::NONE) {
                if idle && !damaged {
                    self.set_enabled(ctx, &id, !enabled);
                }
                return;
            }
        }
        if pressed(Key::ArrowUp, Modifiers::NONE) {
            let i = index.map_or(0, |i| i.saturating_sub(1));
            self.selected = Some(mods[i].0.clone());
        } else if pressed(Key::ArrowDown, Modifiers::NONE) {
            let i = index.map_or(0, |i| (i + 1).min(mods.len() - 1));
            self.selected = Some(mods[i].0.clone());
        }
    }

    /// A frame over the window while files are dragged over it.
    fn drop_overlay(&self, ui: &egui::Ui, area: egui::Rect) {
        let hovering = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
        if !hovering {
            return;
        }
        let painter = ui.ctx().layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::unique("mods-drop"),
        ));
        let rect = area.shrink(6.0);
        painter.rect(
            rect,
            egui::CornerRadius::same(8),
            ui.visuals().panel_fill.gamma_multiply(0.92),
            egui::Stroke::new(2.0, theme::FOCUS),
            egui::StrokeKind::Inside,
        );
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            tr("mods.drop_here"),
            egui::FontId::proportional(22.0),
            ui.visuals().strong_text_color(),
        );
    }
}

/// Compares the mods with the game (worker thread).
fn overview(game: &GameInstance, manager: &ModManager) -> Overview {
    let held = |dir: PathBuf| {
        doctor_toolkit::workspace::Deployment::load(&dir.join("applied.json"))
            .and_then(|d| d.held_streams(game))
            .unwrap_or_default()
    };
    Overview {
        status: manager.status(game).ok(),
        issues: manager.check(game).unwrap_or_default(),
        conflicts: manager.conflicts(true),
        all_conflicts: manager.conflicts(false),
        clashes: manager.clashes(true),
        all_clashes: manager.clashes(false),
        workspace_held: held(Workspace::default_dir(game.root())),
        interrupted: manager.interrupted(game).unwrap_or_default(),
        old_toolkit: integrity::detect_old_toolkit(game.root()).is_some(),
        sets: manager
            .sets()
            .unwrap_or_default()
            .into_iter()
            .map(|s| s.name)
            .collect(),
        current_set: manager.current_set(),
    }
}

/// The state of a checked file in the interface language.
fn condition_text(condition: Condition) -> &'static str {
    tr(match condition {
        Condition::Original => "mods.condition_original",
        Condition::Mods => "mods.condition_mods",
        Condition::Workspace => "mods.condition_workspace",
        Condition::OldToolkit => "mods.condition_old_toolkit",
        Condition::OtherProgram => "mods.condition_other_program",
        Condition::DiffersFromReference => "mods.condition_differs",
        Condition::Unverified => "mods.condition_unverified",
    })
}

/// The integrity check in plain words, and whether it found a problem.
fn integrity_words(report: &IntegrityReport) -> (String, bool) {
    let by = |c: Condition| report.count(c);
    let foreign = by(Condition::OtherProgram) + by(Condition::DiffersFromReference);
    let ours = by(Condition::Mods) + by(Condition::Workspace);
    let old = by(Condition::OldToolkit);
    let problem = foreign > 0 || old > 0;
    let mut lines = Vec::new();
    if !problem {
        lines.push(tr("mods.integrity_fine").to_owned());
    }
    if foreign > 0 {
        lines.push(trn("mods.integrity_foreign", foreign as u64, &[]));
    }
    if old > 0 {
        lines.push(trn("mods.integrity_old", old as u64, &[]));
    }
    if ours > 0 {
        lines.push(trn("mods.integrity_ours", ours as u64, &[]));
    }
    let unverified = by(Condition::Unverified);
    if unverified > 0 {
        lines.push(trn("mods.integrity_unverified", unverified as u64, &[]));
    }
    if !report.temp_files.is_empty() {
        lines.push(trn(
            "mods.integrity_leftovers",
            report.temp_files.len() as u64,
            &[],
        ));
    }
    (lines.join("\n"), problem)
}

/// What writing did, for players.
fn written_words(report: &ApplyReport, enabled: usize) -> String {
    let mut text = if report.streams.is_empty() {
        tr("mods.nothing_to_do").to_owned()
    } else {
        trn(
            "mods.written_text",
            enabled as u64,
            &[(
                "files",
                &trn("count.game_files", report.written.len() as u64, &[]),
            )],
        )
    };
    for warning in report.warnings.iter().take(3) {
        text += "\n";
        text += &trf("mods.report_warning", &[("warning", warning)]);
    }
    text
}

/// What restoring did, for players.
fn restored_words(report: &ApplyReport) -> String {
    if report.streams.is_empty() {
        tr("mods.nothing_to_do").to_owned()
    } else {
        trn("mods.restored_text", report.written.len() as u64, &[])
    }
}

fn describe(report: &ApplyReport, verb: &str) -> String {
    if report.streams.is_empty() {
        return tr("mods.nothing_to_do").to_owned();
    }
    let rebuilt = report
        .streams
        .iter()
        .filter(|(_, a)| matches!(a, StreamAction::Rebuilt(_)))
        .count();
    let restored = report.streams.len() - rebuilt;
    let mut text = trf(
        "mods.report",
        &[
            ("verb", &verb),
            ("rebuilt", &trn("mods.streams_rebuilt", rebuilt as u64, &[])),
            ("restored", &restored),
            (
                "written",
                &trn("mods.files_written", report.written.len() as u64, &[]),
            ),
        ],
    );
    if let Some(warning) = report.warnings.first() {
        text += "; ";
        text += &trf("mods.report_warning", &[("warning", warning)]);
    }
    text
}

#[cfg(test)]
mod tests;

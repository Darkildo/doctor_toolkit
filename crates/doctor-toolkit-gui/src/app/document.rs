//! The project the editor has open as its document (stage 106).
//!
//! A project with its own workspace (`doctor_toolkit::project::document`)
//! is opened like a file of any editor: File → New project…, Open project…,
//! the recent projects. While it is open, the workspace the editor works
//! with is the project's ([`BrowserApp::workspace_dir`]): its edits and
//! their history are apart from the game folder and from other projects.
//! Closing it gives the game's own workspace back.
//!
//! A project is never written into the files of the game by the editor
//! ([`BrowserApp::project_refuses_write`]), nor by its workspace
//! (`doctor_toolkit::workspace::Owner::Project`). The files of the game
//! have one owner besides the game's own workspace — the mod manager — so
//! two projects never fight over them. A project reaches the game as a mod
//! of its id (stage 107): Play stages it on top of the enabled mods
//! without writing anything (`play::overlay::Source::Project`), and File →
//! Install project into Mods ([`BrowserApp::install_document`]) puts it
//! into the mod manager, last in the load order.
//!
//! A project of stage 36 (sources built into the game's workspace) opens
//! the same way and leaves the workspace where it was.
//!
//! The base of the project (stage 108, `doctor_toolkit::project::base`) —
//! the mods it is made on — is compared with the mods of the game while
//! the project is open ([`BaseBar`]): what differs is told in a bar at the
//! top of the window, with a button that brings the mods to the base. A
//! project without a base opened on a game with mods gets the offer to
//! record them; the base is never recorded by itself.
//!
//! When a mod under the edits is updated, the edits still hold what it
//! gave before (stage 111, `doctor_toolkit::workspace::rebase`). The bar
//! tells how many edits were made on other data than the game gives now,
//! with a button that rebases them ([`BrowserApp::rebase`]); the Changes
//! panel marks them and offers the three ways for each.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::mods::ModManager;
use doctor_toolkit::mods::manager::Payloads;
use doctor_toolkit::project::{self, Base, BaseDifference, NewDocument, Project, ProjectConfig};
use doctor_toolkit::workspace::{
    Given, RebaseOutcome, RebaseReport, RebaseRequest, ResourceKey, Stale, Workspace,
};
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::RichText;

use super::{BrowserApp, Level};

/// How often the files the base is compared with are looked at.
const BASE_LOOK: Duration = Duration::from_secs(2);

/// Size and time of the files a comparison of the base reads: the project
/// file, the registry of the mod manager and its record of what is written.
type BaseStamps = [Option<(u64, SystemTime)>; 3];

fn base_stamps(root: &Path, game: &GameInstance) -> BaseStamps {
    let mods = doctor_toolkit::mods::mods_dir(game.root());
    [
        root.join(project::PROJECT_FILE),
        mods.join("registry.json"),
        doctor_toolkit::mods::manager::applied_path(game.root()),
    ]
    .map(|path| {
        let meta = std::fs::metadata(path).ok()?;
        Some((meta.len(), meta.modified().ok()?))
    })
}

/// The base of the open project against the mods of the game.
#[derive(Debug, Default)]
pub(super) struct BaseBar {
    /// What differs; empty when the game holds the base, or the project
    /// names none.
    pub differences: Vec<BaseDifference>,
    /// The project names no base and the mod manager has written these
    /// mods into the game: the offer to record them as the base.
    pub offer: Vec<String>,
    /// "Not now" was the answer to the offer, for this project.
    dismissed: bool,
    /// The base and the name of the project, for bringing the mods to it.
    base: Option<(Base, String)>,
    /// Why the comparison could not be made.
    error: Option<String>,
    /// The files the comparison was made of, and when they were looked at.
    seen: Option<BaseStamps>,
    looked: Option<Instant>,
    /// The mods are brought to the base as soon as the Mods window has
    /// read them.
    pub bring_queued: bool,
    /// The edits of the workspace made on other data than the game gives
    /// now (stage 111): of the open project, or of the game's own
    /// workspace.
    pub stale: Vec<Stale>,
}

impl BaseBar {
    /// Whether bringing the mods to the base would change anything, and is
    /// possible: every mod of the base is installed.
    pub fn can_bring(&self) -> bool {
        self.base.is_some()
            && self.differences.iter().any(BaseDifference::brought)
            && !self
                .differences
                .iter()
                .any(|d| matches!(d, BaseDifference::NotInstalled { .. }))
    }

    /// Whether the stale edits can be rebased from the bar: the game has
    /// to hold the base, because an edit is rebased onto what the game
    /// gives. An updated mod of the base ([`BaseDifference::Changed`]) is
    /// what a rebase is for.
    pub fn can_rebase(&self) -> bool {
        !self.stale.is_empty()
            && self.error.is_none()
            && self
                .differences
                .iter()
                .all(|d| matches!(d, BaseDifference::Changed { .. }))
    }
}

/// What the game gives the resources `ws` changes where its files hold the
/// workspace's own mod (`doctor_toolkit::mods::game_without_mod`): one
/// installed from this workspace, or the mod `own_id` — of the open
/// project, else the id the Mods window builds the workspace under.
pub(super) fn given_of(
    game: &GameInstance,
    ws: &Workspace,
    own_id: Option<&str>,
) -> Result<Given, String> {
    let ids: Vec<&str> = own_id.into_iter().collect();
    doctor_toolkit::mods::game_without_mod(game, ws, &ids).map_err(|e| e.to_string())
}

/// What a rebase is asked for: every stale edit, or one way for some.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Rebase {
    /// Merge every stale edit that can be merged.
    All,
    /// Merge these.
    Merge(Vec<ResourceKey>),
    /// Keep these as they are.
    KeepMine(Vec<ResourceKey>),
    /// Give these up for the data of the game.
    TakeBase(Vec<ResourceKey>),
}

/// A difference in words.
fn difference_words(difference: &BaseDifference) -> String {
    use BaseDifference as D;
    match difference {
        D::NotInstalled { id } => trf("document.base_not_installed", &[("id", id)]),
        D::Disabled { id } => trf("document.base_disabled", &[("id", id)]),
        D::NotWritten { id } => trf("document.base_not_written", &[("id", id)]),
        D::Version { id, wanted, found } => trf(
            "document.base_version",
            &[("id", id), ("wanted", wanted), ("found", found)],
        ),
        D::Changed {
            id,
            built_on,
            found,
        } => trf(
            "document.base_changed",
            &[("id", id), ("built_on", built_on), ("found", found)],
        ),
        D::Order { base, game } => trf(
            "document.base_order",
            &[("base", &base.join(", ")), ("game", &game.join(", "))],
        ),
        D::Extra { id, written: true } => trf("document.base_extra_written", &[("id", id)]),
        D::Extra { id, written: false } => trf("document.base_extra", &[("id", id)]),
        D::OwnMod { id } => trf("document.base_own_mod", &[("id", id)]),
        D::Leftover { resources } => trn("document.base_leftover", *resources as u64, &[]),
    }
}

fn differences_words(differences: &[BaseDifference]) -> String {
    differences
        .iter()
        .map(difference_words)
        .collect::<Vec<_>>()
        .join("; ")
}

/// The project that is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OpenDocument {
    pub root: PathBuf,
    pub name: String,
    /// It has a workspace of its own: the editor works with that one.
    pub own_workspace: bool,
}

fn same_folder(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

impl BrowserApp {
    /// Name of the open project.
    pub(super) fn document_name(&self) -> Option<&str> {
        self.document.as_ref().map(|d| d.name.as_str())
    }

    /// Whether the editor works with the workspace of a project.
    pub(super) fn has_project_workspace(&self) -> bool {
        self.document.as_ref().is_some_and(|d| d.own_workspace)
    }

    /// The title of the window: the project, then the game.
    pub(super) fn send_title(&self, ctx: &egui::Context) {
        let mut title = String::from("doctor_toolkit");
        if let Some(name) = self.document_name() {
            title += &format!(" — {name}");
        }
        if let Some(game) = &self.game {
            title += &format!(
                " — {} ({})",
                game.root().file_name().map_or_else(
                    || game.root().display().to_string(),
                    |n| n.to_string_lossy().into_owned()
                ),
                game.profile().name
            );
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
    }

    /// A project's workspace is not written into the files of the game by
    /// the editor (Apply to game, the direct mode, Play without the
    /// overlay): says so and returns `true` when a project is open.
    pub(super) fn project_refuses_write(&mut self) -> bool {
        if !self.has_project_workspace() {
            return false;
        }
        self.notify(Level::Warning, tr("document.not_written").to_owned());
        true
    }

    /// The folder of the open project when the editor works with its
    /// workspace: what Play stages as a mod.
    pub(super) fn project_root(&self) -> Option<PathBuf> {
        self.document
            .as_ref()
            .filter(|d| d.own_workspace)
            .map(|d| d.root.clone())
    }

    /// File → Install project into Mods: the project becomes a mod of the
    /// game's mod manager — its id, name and version, enabled, last in the
    /// load order; installed before, it is updated. The game files change
    /// only when the mods are written (the Mods window, which opens).
    pub(super) fn install_document(&mut self, ctx: &egui::Context) {
        match self.install_document_now(ctx) {
            Ok(message) => {
                self.mods_open = true;
                self.notify(Level::Info, message);
            }
            Err(error) => self.notify(
                Level::Error,
                trf("document.cannot_install", &[("error", &error)]),
            ),
        }
    }

    fn install_document_now(&mut self, ctx: &egui::Context) -> Result<String, String> {
        // The mod of stale edits takes back what its base changed since:
        // installed all the same, and said (`docs/project.md` §0.3).
        let game = self.disk_game().ok_or(tr("edits.no_game"))?;
        let root = self.project_root().ok_or(tr("document.no_project"))?;
        // The mods are not changed under a running game.
        if let Some(reason) = self.write_lock() {
            return Err(reason);
        }
        let ws = self
            .workspace
            .as_ref()
            .filter(|ws| ws.is_saved())
            .ok_or(tr("document.nothing_to_install"))?;
        let project = Project::open(&root).map_err(|e| e.to_string())?;
        let mut manager = ModManager::open(&game).map_err(|e| e.to_string())?;
        let report = project
            .install_into(&game, &mut manager, ws, Payloads::Copied)
            .map_err(|e| e.to_string())?;
        // The Mods window reads the mods again and shows the project's.
        self.mods.select(&report.id);
        self.mods.refresh(ctx);
        let mut message = trf(
            "document.installed",
            &[
                ("name", &report.name),
                ("version", &project.config.project.version),
                (
                    "resources",
                    &trn("count.resources", report.resources as u64, &[]),
                ),
            ],
        );
        for issue in report.issues.iter().filter(|i| !i.problem.is_notice()) {
            message.push('\n');
            message.push_str(&issue.describe());
        }
        // What the installation itself has to tell: a mod of edits made on
        // other data than it is built on, in the library's words.
        for warning in &report.warnings {
            message.push('\n');
            message.push_str(warning);
        }
        // The mod depends on the base: said now when the game has another.
        self.check_base();
        if !self.base.differences.is_empty() {
            message.push('\n');
            message.push_str(&trf(
                "document.base_differs",
                &[("what", &differences_words(&self.base.differences))],
            ));
        }
        Ok(message)
    }

    // ------------------------------------------------- the base (stage 108)

    /// Compares the base of the open project with the mods of the game now.
    pub(super) fn check_base(&mut self) {
        let bring_queued = self.base.bring_queued;
        let dismissed = self.base.dismissed;
        self.base = BaseBar {
            dismissed,
            bring_queued,
            looked: Some(Instant::now()),
            ..BaseBar::default()
        };
        // The mods of the game may be others now: so may what the edits
        // stand on.
        self.check_stale();
        let (Some(root), Some(game)) = (
            self.document.as_ref().map(|d| d.root.clone()),
            self.disk_game(),
        ) else {
            return;
        };
        self.base.seen = Some(base_stamps(&root, &game));
        let checked = ProjectConfig::read(&root).and_then(|config| {
            let in_game = ModManager::open(&game)?.in_game(&game)?;
            Ok((config, in_game))
        });
        match checked {
            Ok((config, in_game)) => {
                self.base.differences = config.base_differences(&in_game);
                if config.base.is_none() {
                    self.base.offer = in_game
                        .written()
                        .filter(|m| m.id != config.project.id)
                        .map(|m| m.name.clone())
                        .collect();
                }
                self.base.base = config.base.map(|base| (base, config.project.name));
            }
            Err(error) => self.base.error = Some(error.to_string()),
        }
    }

    /// Once a frame: compares the base again when the project file or the
    /// mods of the game changed (looked at every two seconds), so that the
    /// bar follows what the Mods window, Open Mod Manager and the command
    /// line do.
    pub(super) fn base_tick(&mut self) {
        if self.document.is_none() {
            if self.base.looked.is_some() {
                self.base = BaseBar::default();
            }
            return;
        }
        // Not in the middle of a write of the Mods window: it tells when it
        // is done. (A closed window does not, and is not waited for.)
        if self.job.is_some() || (self.mods_open && self.mods.is_busy()) {
            return;
        }
        if self.base.looked.is_some_and(|at| at.elapsed() < BASE_LOOK) {
            return;
        }
        let (Some(root), Some(game)) = (
            self.document.as_ref().map(|d| d.root.clone()),
            self.disk_game(),
        ) else {
            return;
        };
        if self.base.seen == Some(base_stamps(&root, &game)) {
            self.base.looked = Some(Instant::now());
        } else {
            self.check_base();
        }
    }

    /// Whether the bar of the base has something to tell.
    pub(super) fn base_bar_shown(&self) -> bool {
        self.document.is_some()
            && (!self.base.differences.is_empty()
                || (!self.base.offer.is_empty() && !self.base.dismissed)
                || self.base.error.is_some()
                || !self.base.stale.is_empty())
    }

    // ---------------------------------------------- rebasing (stage 111)

    /// Finds the edits of the workspace made on other data than the game
    /// gives now ([`Workspace::stale`]; of a project, without its own mod
    /// where the game holds it). Reads the game: called when a workspace
    /// is opened, when the mods of the game changed and after an undo or a
    /// rebase — an edit of a resource never makes its change stale.
    /// The id of the mod the workspace of the editor is installed as: the
    /// id of the open project, else the one of the build form of the Mods
    /// window. Written into the game, that mod is the workspace's own
    /// installed state and not what its edits stand on.
    pub(super) fn own_mod_id(&self) -> Option<String> {
        let id = match &self.document {
            Some(document) => ProjectConfig::read(&document.root).ok()?.project.id,
            None => self.mods.build_info().id,
        };
        (!id.is_empty()).then_some(id)
    }

    pub(super) fn check_stale(&mut self) {
        self.base.stale.clear();
        let (Some(game), Some(ws)) = (self.disk_game(), self.workspace.as_ref()) else {
            return;
        };
        // Without a changed resource there is nothing to read the game for.
        if ws.changes().next().is_none() {
            return;
        }
        let own = self.own_mod_id();
        let found = given_of(&game, ws, own.as_deref())
            .and_then(|given| ws.stale_given(&game, &given).map_err(|e| e.to_string()));
        match found {
            Ok(stale) => self.base.stale = stale,
            Err(error) => self.notify(
                Level::Warning,
                trf("rebase.cannot_check", &[("error", &error)]),
            ),
        }
    }

    /// After an edit: the stale edits that are still there, without
    /// reading the game — a change that is gone, or that was made anew, is
    /// stale no more.
    pub(super) fn prune_stale(&mut self) {
        let ws = self.workspace.as_ref();
        self.base.stale.retain(|s| {
            ws.and_then(|ws| ws.change(&s.key))
                .is_some_and(|change| *change.made_on() == s.made_on)
        });
    }

    /// The stale edits in words, for a warning; `None` when there are none.
    pub(super) fn stale_words(&self) -> Option<String> {
        let stale = &self.base.stale;
        if stale.is_empty() {
            return None;
        }
        Some(trf(
            "document.stale_warning",
            &[("edits", &trn("document.stale", stale.len() as u64, &[]))],
        ))
    }

    /// Rebases stale edits of the workspace as one undoable operation
    /// ([`Workspace::rebase_given`]) and tells what came of it: the edits
    /// merged, the operations left out and those that met another value,
    /// and the edits that need a decision. The result goes to the status
    /// line and the log.
    pub(crate) fn rebase(&mut self, how: Rebase) {
        let Some(game) = self.disk_game() else {
            return;
        };
        let own = self.own_mod_id();
        let (keys, keep, take_base) = match how {
            Rebase::All => (None, Vec::new(), Vec::new()),
            Rebase::Merge(keys) => (Some(keys), Vec::new(), Vec::new()),
            Rebase::KeepMine(keys) => (Some(Vec::new()), keys, Vec::new()),
            Rebase::TakeBase(keys) => (Some(Vec::new()), Vec::new(), keys),
        };
        let result = self.workspace_mut().and_then(|ws| {
            let given = given_of(&game, ws, own.as_deref())?;
            let request = RebaseRequest {
                keys: keys.as_deref(),
                keep: &keep,
                take_base: &take_base,
            };
            ws.rebase_given(&game, &given, &request)
                .map_err(|e| e.to_string())
        });
        match result {
            Ok(report) => {
                let (level, text) = rebase_words(&report);
                self.notify(level, text);
            }
            Err(error) => self.notify(Level::Error, trf("rebase.cannot", &[("error", &error)])),
        }
        self.after_edit(None);
        self.check_stale();
    }

    /// File → Take the base from the mods in the game: the mods the mod
    /// manager has written into the game become the base of the project
    /// (`[base]` of its `project.toml`), and the mod of the project depends
    /// on them from now on.
    pub(super) fn record_base(&mut self) {
        let (Some(root), Some(game)) = (
            self.document.as_ref().map(|d| d.root.clone()),
            self.disk_game(),
        ) else {
            return;
        };
        match ProjectConfig::record_base(&root, &game) {
            Ok(recorded) => {
                let ids: Vec<&str> = recorded.base.mods.iter().map(|m| m.id.as_str()).collect();
                let mut message = if ids.is_empty() {
                    tr("document.base_recorded_none").to_owned()
                } else {
                    trf("document.base_recorded", &[("mods", &ids.join(", "))])
                };
                if !recorded.not_written.is_empty() {
                    message.push('\n');
                    message.push_str(&trf(
                        "document.base_left_out",
                        &[("mods", &recorded.not_written.join(", "))],
                    ));
                }
                let level = if recorded.not_written.is_empty() {
                    Level::Info
                } else {
                    Level::Warning
                };
                self.notify(level, message);
                // The Mods window builds the mod with these dependencies.
                if let Ok(project) = Project::open(&root)
                    && project.has_own_workspace()
                {
                    self.mods.set_build_info(&project.mod_info(), true);
                }
            }
            Err(error) => self.notify(
                Level::Error,
                trf("document.base_cannot_record", &[("error", &error)]),
            ),
        }
        self.check_base();
    }

    /// File → Bring the mods to the project's base: the arrangement of the
    /// mods is saved as a set, the mods of the base are enabled in its
    /// order and written into the game. The Mods window opens and does it
    /// ([`doctor_toolkit_mods::ModsPanel::bring_to_base`]): it shows the
    /// progress and the result, and its sets are the way back.
    pub(super) fn bring_mods_to_base(&mut self) {
        self.check_base();
        if !self.base.can_bring() {
            let missing: Vec<String> = self
                .base
                .differences
                .iter()
                .filter(|d| matches!(d, BaseDifference::NotInstalled { .. }))
                .map(difference_words)
                .collect();
            let text = if missing.is_empty() {
                tr("document.base_nothing_to_bring").to_owned()
            } else {
                trf(
                    "document.base_cannot_bring",
                    &[("what", &missing.join("; "))],
                )
            };
            self.notify(Level::Warning, text);
            return;
        }
        // The mods are not written under a running game.
        if self.refuse_write() {
            return;
        }
        self.mods_open = true;
        self.base.bring_queued = true;
    }

    /// The Mods window has read the mods: brings them to the base when
    /// that was asked for.
    pub(super) fn bring_queued_base(&mut self, ctx: &egui::Context) {
        if !self.base.bring_queued || self.mods.is_busy() || self.mods.manager().is_none() {
            return;
        }
        self.base.bring_queued = false;
        if let Some((base, name)) = self.base.base.clone() {
            self.mods.bring_to_base(ctx, base, &name);
        }
    }

    /// Play of the project: compares the base with the game first. A
    /// difference the mod of the project cannot be applied with (a mod of
    /// the base that is missing, off or of another version) stops the
    /// launch and is told — `true`; any other is told and the game starts.
    pub(super) fn base_stops_play(&mut self) -> bool {
        self.check_base();
        // Stale edits do not stop the launch: the game shows what the
        // editor shows, and nothing is written (`docs/project.md` §0.3).
        let stale = self.stale_words();
        if self.base.differences.is_empty() {
            if let Some(stale) = stale {
                self.notify(Level::Warning, stale);
            }
            return false;
        }
        let stops: Vec<BaseDifference> = self
            .base
            .differences
            .iter()
            .filter(|d| d.blocks_the_mod())
            .cloned()
            .collect();
        if stops.is_empty() {
            let what = differences_words(&self.base.differences);
            let mut text = trf("document.base_differs", &[("what", &what)]);
            if let Some(stale) = stale {
                text.push('\n');
                text.push_str(&stale);
            }
            self.notify(Level::Warning, text);
            return false;
        }
        self.notify(
            Level::Error,
            trf(
                "document.base_stops_play",
                &[("what", &differences_words(&stops))],
            ),
        );
        true
    }

    /// The bar at the top of the window: what differs between the base of
    /// the project and the mods of the game, with the way to make them the
    /// same; or the offer to record the mods of the game as the base.
    pub(super) fn base_bar(&mut self, ui: &mut egui::Ui) {
        let warn = doctor_toolkit_mods::WARN;
        let mut bring = false;
        let mut record = false;
        let mut dismiss = false;
        let mut open_mods = false;
        let mut rebase = false;
        let mut show_changes = false;
        if !self.base.stale.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                doctor_toolkit_mods::widgets::warning_icon(ui, warn);
                let stale = &self.base.stale;
                let keys: Vec<String> = stale.iter().map(|s| s.key.to_string()).collect();
                ui.label(RichText::new(trn("document.stale", stale.len() as u64, &[])).color(warn))
                    .on_hover_text(keys.join("\n"));
                let can = self.base.can_rebase() && self.job.is_none();
                let hint = if self.base.can_rebase() {
                    tr("document.stale_hint")
                } else {
                    tr("document.stale_bring_first")
                };
                rebase = ui
                    .add_enabled(can, egui::Button::new(tr("document.stale_rebase")))
                    .on_hover_text(hint)
                    .on_disabled_hover_text(hint)
                    .clicked();
                show_changes = ui
                    .button(tr("document.stale_show"))
                    .on_hover_text(tr("document.stale_show_hint"))
                    .clicked();
            });
        }
        let base_told = !self.base.differences.is_empty()
            || self.base.error.is_some()
            || (!self.base.offer.is_empty() && !self.base.dismissed);
        if base_told {
            self.base_differences_bar(ui, &mut bring, &mut record, &mut dismiss, &mut open_mods);
        }
        if rebase {
            self.rebase(Rebase::All);
        }
        if show_changes {
            self.dock_ops
                .push(super::dock::DockOp::ShowPanel(super::dock::Pane::Changes));
        }
        if bring {
            self.bring_mods_to_base();
        }
        if record {
            self.record_base();
        }
        if dismiss {
            self.base.dismissed = true;
        }
        if open_mods {
            self.mods_open = true;
        }
    }

    /// The line of [`Self::base_bar`] about the mods of the game.
    fn base_differences_bar(
        &self,
        ui: &mut egui::Ui,
        bring: &mut bool,
        record: &mut bool,
        dismiss: &mut bool,
        open_mods: &mut bool,
    ) {
        let warn = doctor_toolkit_mods::WARN;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            doctor_toolkit_mods::widgets::warning_icon(ui, warn);
            if let Some(error) = &self.base.error {
                ui.label(trf("document.base_unknown", &[("error", error)]));
            } else if !self.base.differences.is_empty() {
                let all = &self.base.differences;
                let mut text = trf(
                    "document.base_differs",
                    &[("what", &difference_words(&all[0]))],
                );
                if all.len() > 1 {
                    text += &trf("document.base_more", &[("n", &(all.len() - 1))]);
                }
                let list: Vec<String> = all.iter().map(difference_words).collect();
                ui.label(RichText::new(text).color(warn))
                    .on_hover_text(list.join("\n"));
                let can = self.base.can_bring() && self.job.is_none() && !self.mods.is_busy();
                if self.base.base.is_some() {
                    let hint = if self.base.can_bring() {
                        tr("document.base_bring_hint")
                    } else {
                        tr("document.base_bring_cannot_hint")
                    };
                    *bring = ui
                        .add_enabled(can, egui::Button::new(tr("cmd.project_base_apply")))
                        .on_hover_text(hint)
                        .on_disabled_hover_text(hint)
                        .clicked();
                    *record = ui
                        .button(tr("document.base_record_again"))
                        .on_hover_text(tr("document.base_record_again_hint"))
                        .clicked();
                }
                *open_mods = ui.button(tr("document.base_open_mods")).clicked();
            } else {
                ui.label(trf(
                    "document.base_offer",
                    &[("mods", &self.base.offer.join(", "))],
                ))
                .on_hover_text(tr("document.base_offer_hint"));
                *record = ui
                    .button(tr("document.base_offer_record"))
                    .on_hover_text(tr("document.base_offer_hint"))
                    .clicked();
                *dismiss = ui.button(tr("document.base_offer_later")).clicked();
            }
        });
    }
}

/// What a rebase did, for the status line and the log: a line of counts,
/// then every change as the library tells it (operations left out by
/// object or string and reason).
fn rebase_words(report: &RebaseReport) -> (Level, String) {
    if report.changes.is_empty() {
        return (Level::Info, tr("rebase.nothing").to_owned());
    }
    let count = |wanted: fn(&RebaseOutcome) -> bool| {
        report.changes.iter().filter(|c| wanted(&c.outcome)).count() as u64
    };
    let merged = count(|o| matches!(o, RebaseOutcome::Merged { .. }));
    let kept = count(|o| matches!(o, RebaseOutcome::Kept));
    let taken = count(|o| matches!(o, RebaseOutcome::Taken));
    let undecided = count(|o| matches!(o, RebaseOutcome::Undecided { .. }));
    let (skipped, conflicts) = (report.skipped() as u64, report.conflicts() as u64);
    let mut parts = Vec::new();
    for (n, key) in [
        (merged, "rebase.merged"),
        (kept, "rebase.kept"),
        (taken, "rebase.taken"),
        (skipped, "rebase.skipped"),
        (conflicts, "rebase.conflicts"),
        (undecided, "rebase.undecided"),
    ] {
        if n > 0 {
            parts.push(trn(key, n, &[]));
        }
    }
    let mut text = match parts.is_empty() {
        true => tr("rebase.nothing").to_owned(),
        false => parts.join("; "),
    };
    if report.seq.is_some() {
        text.push_str(tr("rebase.undo_hint"));
    }
    for line in report.describe() {
        text.push('\n');
        text.push_str(&line);
    }
    let level = match skipped + conflicts + undecided {
        0 => Level::Info,
        _ => Level::Warning,
    };
    (level, text)
}

impl BrowserApp {
    /// A game was opened: the project it was opened for becomes the
    /// document, and its workspace the one the editor works with; a game
    /// opened by itself has no project.
    pub(super) fn document_for_opened_game(&mut self) {
        let project = self
            .pending_document
            .take()
            .filter(|p| match (p.game_path(), &self.game) {
                (Some(path), Some(game)) => same_folder(&path, game.root()),
                _ => false,
            });
        match project {
            Some(project) => self.set_document(&project),
            None => {
                if let Some(document) = self.document.take() {
                    self.document_gone(&document);
                }
                self.workspace_at = self.workspace_preset.clone();
                self.settings.last_project = None;
            }
        }
    }

    /// The project `document` is the document no more: its base is not
    /// compared any longer, and the Mods window builds the workspace of the
    /// game again — with dependencies that are guessed, not those of the
    /// project's base.
    fn document_gone(&mut self, document: &OpenDocument) {
        self.base = BaseBar::default();
        if document.own_workspace {
            let mut info = self.mods.build_info();
            info.relations.depends.clear();
            self.mods.set_build_info(&info, false);
        }
    }

    fn set_document(&mut self, project: &Project) {
        self.workspace_at = project.workspace_dir();
        self.document = Some(OpenDocument {
            root: project.root().to_owned(),
            name: project.config.project.name.clone(),
            own_workspace: project.has_own_workspace(),
        });
        self.settings.add_recent_project(project.root());
        if project.has_own_workspace() {
            // No direct mode for a project.
            self.direct = false;
            // The Mods window builds and installs it as the project's mod.
            self.mods
                .set_build_info(&project.mod_info(), project.config.base.is_some());
        }
        // The base of this project against the mods of the game; an offer
        // answered for another project is asked again.
        self.base = BaseBar::default();
        self.check_base();
    }

    /// The workspace the editor works with is another one now (a project
    /// was opened or closed): reads it and shows every view its version.
    fn workspace_replaced(&mut self) {
        // The sets of zones are the project's or the game's (stage 113),
        // and so is the choice a level tab is remembered with.
        self.zones_level = None;
        self.reload_zone_sets();
        self.open_workspace();
        self.reload_docs(None);
        for level in self.levels.values_mut() {
            level.refresh(self.workspace.as_ref(), None);
        }
        if let (Some(game), Some(index), Some(dir)) =
            (self.game.clone(), self.index.clone(), self.workspace_dir())
        {
            for logic in self.logics.values_mut() {
                logic.open_game(game.clone(), index.clone(), dir.clone());
            }
        }
    }

    /// Opens the project in `dir` as the document: its game when another
    /// one (or none) is open, then its workspace.
    pub(super) fn open_document(&mut self, dir: PathBuf, ctx: &egui::Context) {
        if self.job.is_some() {
            self.notify(Level::Warning, tr("status.wait_for_job").to_owned());
            return;
        }
        let project = match Project::open(&dir) {
            Ok(project) => project,
            Err(error) => {
                self.notify(
                    Level::Error,
                    trf("project.cannot_open", &[("error", &error)]),
                );
                return;
            }
        };
        let open_root = self.game.as_ref().map(|g| g.root().to_owned());
        let game_dir = match (project.game_path(), &open_root) {
            (Some(path), _) if path.is_dir() => path,
            // The project does not know where the game is on this computer
            // (a fresh clone, a moved game): the open game is taken.
            (_, Some(root)) => {
                if let Err(error) = project.set_game_path(root) {
                    self.notify(Level::Warning, error.to_string());
                }
                root.clone()
            }
            (_, None) => {
                self.notify(Level::Warning, tr("document.no_game").to_owned());
                return;
            }
        };
        if open_root.is_some_and(|root| same_folder(&root, &game_dir)) {
            self.attach_document(&project, ctx);
        } else {
            self.open_game(game_dir, ctx);
            self.pending_document = Some(project);
        }
    }

    /// Makes `project` (of the open game) the document.
    fn attach_document(&mut self, project: &Project, ctx: &egui::Context) {
        self.remember_level_camera();
        self.set_document(project);
        self.workspace_replaced();
        self.restore_empty_level();
        // The Project tab shows the open project.
        self.project_open(project.root().to_owned());
        self.send_title(ctx);
        self.status = trf("project.opened", &[("name", &project.config.project.name)]);
    }

    /// File → Close project: the game stays open, with its own workspace.
    pub(super) fn close_document(&mut self, ctx: &egui::Context) {
        self.remember_level_camera();
        let Some(document) = self.document.take() else {
            return;
        };
        self.workspace_at = self.workspace_preset.clone();
        self.settings.last_project = None;
        self.document_gone(&document);
        self.workspace_replaced();
        self.send_title(ctx);
        self.status = trf("document.closed", &[("name", &document.name)]);
    }

    /// Makes a project in `dir` (a new or empty folder, its name is the
    /// project's) for the open game and opens it. With `from_workspace` the
    /// game's own workspace — its edits and their history — moves into it.
    pub(super) fn new_document(
        &mut self,
        dir: &Path,
        from_workspace: bool,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        let game = self.disk_game().ok_or(tr("edits.no_game"))?;
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let new = NewDocument {
            name,
            id: None,
            author: String::new(),
        };
        let from = Workspace::default_dir(game.root());
        if from_workspace {
            if self.document.is_some() || self.workspace_at.is_some() {
                return Err(tr("document.from_own_only").to_owned());
            }
            if !Workspace::exists(&from) {
                return Err(tr("document.nothing_to_move").to_owned());
            }
        }
        let made = !dir.exists();
        let project = project::create_document(&game, dir, &new).map_err(|e| e.to_string())?;
        if from_workspace {
            // The editor lets the workspace go: its folder moves.
            self.workspace = None;
            self.ws_lock = None;
            if let Err(error) = project::adopt_workspace(&game, &from, &project) {
                // Nothing moved: the folder made for the project goes.
                if made {
                    let _ = std::fs::remove_dir_all(dir);
                } else {
                    for name in [project::PROJECT_FILE, ".gitignore", ".doctor"] {
                        let path = dir.join(name);
                        let _ = std::fs::remove_file(&path);
                        let _ = std::fs::remove_dir_all(&path);
                    }
                }
                self.open_workspace();
                return Err(error.to_string());
            }
        }
        self.attach_document(&project, ctx);
        Ok(())
    }

    /// File → New project… and File → Make a project of the workspace…:
    /// asks for the folder.
    pub(super) fn new_document_dialog(&mut self, from_workspace: bool, ctx: &egui::Context) {
        let title = if from_workspace {
            tr("document.from_workspace_title")
        } else {
            tr("document.new_title")
        };
        let mut dialog = rfd::FileDialog::new().set_title(title);
        if let Some(parent) = self
            .settings
            .recent_projects
            .first()
            .and_then(|d| d.parent())
        {
            dialog = dialog.set_directory(parent);
        }
        let Some(dir) = dialog.pick_folder() else {
            return;
        };
        if let Err(error) = self.new_document(&dir, from_workspace, ctx) {
            self.notify(
                Level::Error,
                trf("document.cannot_make", &[("error", &error)]),
            );
        }
    }

    /// File → Open project…: asks for the folder.
    pub(super) fn open_document_dialog(&mut self, ctx: &egui::Context) {
        let mut dialog = rfd::FileDialog::new().set_title(tr("document.open_title"));
        if let Some(dir) = self.settings.recent_projects.first() {
            dialog = dialog.set_directory(dir);
        }
        if let Some(dir) = dialog.pick_folder() {
            self.open_document(dir, ctx);
        }
    }
}

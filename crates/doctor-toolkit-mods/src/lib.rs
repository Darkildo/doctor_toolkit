//! The mod manager interface, shared by the standalone `doctor-toolkit-mods`
//! program and the Mods window of the editor.
//!
//! All the logic lives in [`doctor_toolkit::mods`]; this crate only shows it.
//! Operations that read or write game files run on a worker thread, so the
//! window stays responsive.

pub mod guard;
pub mod i18n;
pub mod icon;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use egui::{Color32, RichText};

use crate::i18n::{tr, trf, trn};
use egui_extras::{Column, TableBuilder};

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::mods::integrity::{self, Condition, IntegrityOptions, IntegrityReport};
use doctor_toolkit::mods::manager::{Clash, CompatIssue, Conflict, ModApplyOptions, Problem};
use doctor_toolkit::mods::{self, ModInfo, ModManager, Preview};
use doctor_toolkit::workspace::{ApplyReport, DeployStatus, StreamAction, StreamState, Workspace};

pub const GOOD: Color32 = Color32::from_rgb(0x60, 0xc0, 0x60);
pub const WARN: Color32 = Color32::from_rgb(0xe0, 0xa0, 0x30);
pub const BAD: Color32 = Color32::from_rgb(0xe0, 0x50, 0x50);
const ROW_HEIGHT: f32 = 22.0;

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

/// What the panel tells its host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelEvent {
    /// Game files are about to be replaced: release open files.
    WillWrite,
    /// Game files were replaced: reopen what is shown.
    GameChanged,
}

/// The state of the mods compared with the game, computed on the worker thread.
#[derive(Default)]
struct Overview {
    status: Option<DeployStatus>,
    issues: Vec<CompatIssue>,
    conflicts: Vec<Conflict>,
    all_conflicts: Vec<Conflict>,
    /// Clashes of mods that add resources or create streams.
    clashes: Vec<Clash>,
    all_clashes: Vec<Clash>,
    /// Streams the editor's workspace holds in the game.
    workspace_held: Vec<String>,
    old_toolkit: bool,
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

/// Result of a job.
struct Done {
    manager: ModManager,
    outcome: Result<String, String>,
    overview: Overview,
    integrity: Option<IntegrityReport>,
    wrote: bool,
}

struct Running {
    label: String,
    rx: Receiver<Done>,
}

/// Metadata typed into the build form.
#[derive(Default)]
struct BuildForm {
    id: String,
    name: String,
    version: String,
    author: String,
    description: String,
    url: String,
    preview: Option<PathBuf>,
}

impl BuildForm {
    fn info(&self) -> ModInfo {
        let id = if self.id.trim().is_empty() {
            mods::id_from_name(&self.name)
        } else {
            self.id.trim().to_owned()
        };
        ModInfo {
            id,
            name: self.name.trim().to_owned(),
            version: if self.version.trim().is_empty() {
                "1.0".to_owned()
            } else {
                self.version.trim().to_owned()
            },
            author: self.author.trim().to_owned(),
            description: self.description.clone(),
            url: Some(self.url.trim().to_owned()).filter(|u| !u.is_empty()),
        }
    }
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
    confirm: Option<String>,
    force: bool,
    ignore_compatibility: bool,
    show_all_conflicts: bool,
    integrity: Option<IntegrityReport>,
    previews: HashMap<String, Option<egui::TextureHandle>>,
    build: BuildForm,
    pending_event: Option<PanelEvent>,
    /// Why writing into the game is refused now (the game runs).
    write_lock: Option<String>,
}

impl Default for ModsPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl ModsPanel {
    pub fn new() -> Self {
        Self {
            game: None,
            manager: None,
            overview: Overview::default(),
            running: None,
            message: None,
            selected: None,
            confirm: None,
            force: false,
            ignore_compatibility: false,
            show_all_conflicts: false,
            integrity: None,
            previews: HashMap::new(),
            build: BuildForm {
                version: "1.0".into(),
                ..BuildForm::default()
            },
            pending_event: None,
            write_lock: None,
        }
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

    /// Shows the mods of `game`.
    pub fn set_game(&mut self, game: Option<Arc<GameInstance>>, ctx: &egui::Context) {
        self.game = game;
        self.manager = None;
        self.overview = Overview::default();
        self.integrity = None;
        self.previews.clear();
        self.selected = None;
        self.message = None;
        let Some(game) = &self.game else { return };
        match ModManager::open(game) {
            Ok(manager) => {
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

    /// Whether a job is running.
    pub fn is_busy(&self) -> bool {
        self.running.is_some()
    }

    /// Label of the running job.
    pub fn busy_label(&self) -> Option<&str> {
        self.running.as_ref().map(|r| r.label.as_str())
    }

    /// Selects a mod, e.g. from the command line.
    pub fn select(&mut self, id: &str) {
        self.selected = Some(id.to_owned());
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

    /// Starts the integrity check.
    pub fn check_integrity(&mut self, ctx: &egui::Context) {
        self.run_with(ctx, tr("mods.job_integrity"), false, |game, _| {
            let report = integrity::check(game, IntegrityOptions::default(), |_, _| {})
                .map_err(|e| e.to_string())?;
            let changed = report.changed().count();
            let message = if changed == 0 {
                trf(
                    "mods.integrity_clean",
                    &[("unverified", &report.count(Condition::Unverified))],
                )
            } else {
                trn("mods.integrity_changed", changed as u64, &[])
            };
            Ok((message, Some(report)))
        });
    }

    fn run(
        &mut self,
        ctx: &egui::Context,
        label: &str,
        writes: bool,
        work: impl FnOnce(&GameInstance, &mut ModManager) -> Result<String, String> + Send + 'static,
    ) {
        self.run_with(ctx, label, writes, move |game, manager| {
            work(game, manager).map(|m| (m, None))
        });
    }

    /// Runs `work` on a worker thread with the manager, then compares the
    /// result with the game.
    fn run_with(
        &mut self,
        ctx: &egui::Context,
        label: &str,
        writes: bool,
        work: impl FnOnce(
            &GameInstance,
            &mut ModManager,
        ) -> Result<(String, Option<IntegrityReport>), String>
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
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let (outcome, integrity) = match work(&game, &mut manager) {
                Ok((message, report)) => (Ok(message), report),
                Err(error) => (Err(error), None),
            };
            let overview = overview(&game, &manager);
            let _ = tx.send(Done {
                manager,
                outcome,
                overview,
                integrity,
                wrote: writes,
            });
            ctx.request_repaint();
        });
        self.running = Some(Running {
            label: label.to_owned(),
            rx,
        });
    }

    fn poll(&mut self) {
        let Some(running) = &self.running else { return };
        let Ok(done) = running.rx.try_recv() else {
            return;
        };
        self.running = None;
        self.manager = Some(done.manager);
        self.overview = done.overview;
        if done.integrity.is_some() {
            self.integrity = done.integrity;
        }
        match done.outcome {
            Ok(message) if message.is_empty() => {}
            Ok(message) => self.message = Some((message, false)),
            Err(error) => self.message = Some((error, true)),
        }
        if done.wrote {
            self.pending_event = Some(PanelEvent::GameChanged);
        }
        if let Some(manager) = &self.manager {
            self.previews.retain(|id, _| manager.get(id).is_some());
        }
    }

    // ------------------------------------------------------------- actions

    fn install(&mut self, ctx: &egui::Context, paths: Vec<PathBuf>) {
        self.run(
            ctx,
            tr("mods.job_installing"),
            false,
            move |game, manager| {
                let mut lines = Vec::new();
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
                }
                Ok(lines.join(". ") + ". " + tr("mods.apply_to_write"))
            },
        );
    }

    fn apply(&mut self, ctx: &egui::Context) {
        let options = ModApplyOptions {
            force: self.force,
            ignore_compatibility: self.ignore_compatibility,
        };
        self.run(ctx, tr("mods.job_applying"), true, move |game, manager| {
            let report = manager.apply(game, options).map_err(|e| e.to_string())?;
            Ok(describe(&report, tr("mods.applied")))
        });
    }

    fn unapply(&mut self, ctx: &egui::Context) {
        let force = self.force;
        self.run(ctx, tr("mods.job_restoring"), true, move |game, manager| {
            let report = manager.unapply(game, force).map_err(|e| e.to_string())?;
            Ok(describe(&report, tr("mods.restored")))
        });
    }

    fn change(
        &mut self,
        ctx: &egui::Context,
        work: impl FnOnce(&mut ModManager) -> doctor_toolkit::Result<()> + Send + 'static,
    ) {
        self.run(ctx, tr("mods.job_updating"), false, move |_, manager| {
            work(manager)
                .map(|()| String::new())
                .map_err(|e| e.to_string())
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

    // -------------------------------------------------------------- layout

    /// Shows the panel. `workspace` enables the build section (editor only).
    pub fn show(&mut self, ui: &mut egui::Ui, workspace: Option<&Workspace>) -> Option<PanelEvent> {
        self.poll();
        if self.running.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        if self.game.is_none() {
            ui.label(RichText::new(tr("mods.no_game")).weak());
            return self.pending_event.take();
        }
        self.toolbar(ui);
        self.status_line(ui);
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt("mods-panel")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.mod_table(ui);
                ui.add_space(6.0);
                self.details(ui);
                self.conflicts_section(ui);
                self.integrity_section(ui);
                if let Some(ws) = workspace {
                    self.build_section(ui, ws);
                }
            });
        self.pending_event.take()
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let idle = self.running.is_none() && self.manager.is_some();
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(idle, egui::Button::new(tr("mods.install")))
                .on_hover_text(tr("mods.install_hint"))
                .clicked()
                && let Some(paths) = rfd::FileDialog::new()
                    .set_title(tr("mods.install_title"))
                    .add_filter(tr("mods.filter_mods"), &["zip", "vpat"])
                    .pick_files()
            {
                self.install(ui.ctx(), paths);
            }
            ui.separator();
            let up_to_date = self.overview.up_to_date();
            if ui
                .add_enabled(idle && !up_to_date, egui::Button::new(tr("mods.apply")))
                .on_hover_text(tr("mods.apply_hint"))
                .clicked()
            {
                self.apply(ui.ctx());
            }
            let has_written = self.manager.as_ref().is_some_and(ModManager::has_written);
            ui.add_enabled_ui(idle && has_written, |ui| {
                if self
                    .confirm_button(ui, "unapply", tr("mods.unapply"))
                    .on_hover_text(tr("mods.unapply_hint"))
                    .clicked_twice
                {
                    self.unapply(ui.ctx());
                }
            });
            ui.separator();
            if ui
                .add_enabled(idle, egui::Button::new(tr("mods.check_integrity")))
                .on_hover_text(tr("mods.check_integrity_hint"))
                .clicked()
            {
                self.check_integrity(ui.ctx());
            }
            if self.overview.old_toolkit
                && ui
                    .add_enabled(idle, egui::Button::new(tr("mods.import_old")))
                    .on_hover_text(tr("mods.import_old_hint"))
                    .clicked()
            {
                self.import_old_toolkit(ui.ctx());
            }
            if let Some(running) = &self.running {
                ui.separator();
                ui.spinner();
                ui.label(&running.label);
            }
            if let Some(reason) = &self.write_lock {
                ui.separator();
                // Whole on the next row rather than broken over two.
                ui.add(
                    egui::Label::new(RichText::new(tr("mods.game_running")).color(WARN))
                        .wrap_mode(egui::TextWrapMode::Extend),
                )
                .on_hover_text(reason.as_str());
            }
        });
        let foreign = self.overview.foreign_changes();
        let differ = self
            .overview
            .issues
            .iter()
            .any(|i| matches!(i.problem, Problem::OriginalDiffers { .. }));
        if foreign || differ || self.force || self.ignore_compatibility {
            ui.horizontal_wrapped(|ui| {
                if foreign || self.force {
                    ui.checkbox(&mut self.force, tr("mods.force"))
                        .on_hover_text(tr("mods.force_hint"));
                }
                if differ || self.ignore_compatibility {
                    ui.checkbox(
                        &mut self.ignore_compatibility,
                        tr("mods.ignore_compatibility"),
                    );
                }
            });
        }
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
        ui.horizontal_wrapped(|ui| {
            ui.label(trf(
                "mods.summary",
                &[
                    (
                        "installed",
                        &trn("mods.installed_count", manager.mods().len() as u64, &[]),
                    ),
                    ("enabled", &enabled),
                ],
            ));
            if self.running.is_some() {
                return;
            }
            if !self.overview.workspace_held.is_empty() {
                ui.colored_label(BAD, tr("mods.workspace_held"));
            } else if self.overview.foreign_changes() {
                ui.colored_label(BAD, tr("mods.foreign_changes"));
            } else if !self.overview.up_to_date() {
                ui.colored_label(WARN, tr("mods.not_applied"));
            } else if enabled > 0 {
                ui.colored_label(GOOD, tr("mods.up_to_date"));
            }
            let misfits = self.overview.issues.len();
            if misfits > 0 {
                ui.colored_label(WARN, trn("mods.misfits", misfits as u64, &[]));
            }
            let fatal = self
                .overview
                .clashes
                .iter()
                .filter(|c| c.is_fatal())
                .count();
            if fatal > 0 {
                ui.colored_label(BAD, trn("mods.fatal_clashes", fatal as u64, &[]));
            }
            if !self.overview.conflicts.is_empty() {
                ui.colored_label(
                    WARN,
                    trn(
                        "mods.in_conflict",
                        self.overview.conflicts.len() as u64,
                        &[],
                    ),
                );
            }
        });
        self.message_line(ui);
    }

    fn message_line(&mut self, ui: &mut egui::Ui) {
        if let Some((message, error)) = &self.message {
            let text = RichText::new(message).color(if *error { BAD } else { GOOD });
            ui.add(egui::Label::new(text).wrap());
        }
    }

    fn mod_table(&mut self, ui: &mut egui::Ui) {
        let Some(manager) = &self.manager else { return };
        if manager.mods().is_empty() {
            ui.label(RichText::new(tr("mods.none_installed")).weak());
            return;
        }
        let idle = self.running.is_none();
        let count = manager.mods().len();
        // Rows: (id, name, version, author, resources, enabled, damaged, misfits, conflicts).
        let rows: Vec<_> = manager
            .mods()
            .iter()
            .map(|m| {
                let conflicts = self
                    .overview
                    .conflicts
                    .iter()
                    .filter(|c| c.mods.contains(&m.id))
                    .count()
                    + self
                        .overview
                        .clashes
                        .iter()
                        .filter(|c| c.mods().contains(&m.id.as_str()))
                        .count();
                let added = m.manifest.added().count();
                let mut resources = m.manifest.resources.len().to_string();
                if added > 0 || !m.manifest.streams.is_empty() {
                    resources += &format!(" (+{added}");
                    if !m.manifest.streams.is_empty() {
                        resources += ", ";
                        resources += &trf(
                            "mods.new_streams_short",
                            &[("count", &m.manifest.streams.len())],
                        );
                    }
                    resources += ")";
                }
                (
                    m.id.clone(),
                    m.manifest.name.clone(),
                    m.manifest.version.clone(),
                    m.manifest.author.clone(),
                    resources,
                    m.enabled,
                    m.problem.is_some(),
                    self.overview.issues_of(&m.id).count(),
                    conflicts,
                )
            })
            .collect();
        let mut toggle = None;
        let mut moves = None;
        let mut remove = None;
        let mut select = None;
        TableBuilder::new(ui)
            .id_salt("mods-table")
            .striped(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(28.0))
            .column(Column::exact(24.0))
            .column(Column::remainder().at_least(180.0).clip(true))
            .column(Column::auto().at_least(70.0))
            .column(Column::auto().at_least(80.0))
            .column(Column::auto().at_least(150.0))
            .column(Column::auto().at_least(130.0))
            .header(ROW_HEIGHT, |mut header| {
                for title in [
                    "#",
                    "",
                    tr("mods.col_mod"),
                    tr("mods.col_version"),
                    tr("mods.col_resources"),
                    tr("mods.col_state"),
                    "",
                ] {
                    header.col(|ui| {
                        ui.strong(title);
                    });
                }
            })
            .body(|body| {
                body.rows(ROW_HEIGHT, rows.len(), |mut row| {
                    let i = row.index();
                    let (
                        id,
                        name,
                        version,
                        author,
                        resources,
                        enabled,
                        damaged,
                        misfits,
                        conflicts,
                    ) = &rows[i];
                    row.col(|ui| {
                        ui.label(format!("{}", i + 1));
                    });
                    row.col(|ui| {
                        let mut on = *enabled;
                        if ui
                            .add_enabled(idle && !damaged, egui::Checkbox::without_text(&mut on))
                            .on_hover_text(tr("mods.enabled"))
                            .changed()
                        {
                            toggle = Some((id.clone(), on));
                        }
                    });
                    row.col(|ui| {
                        let selected = self.selected.as_deref() == Some(id.as_str());
                        let text = if author.is_empty() {
                            name.clone()
                        } else {
                            format!("{name}  — {author}")
                        };
                        if ui.selectable_label(selected, text).clicked() {
                            select = Some(id.clone());
                        }
                    });
                    row.col(|ui| {
                        ui.label(version);
                    });
                    row.col(|ui| {
                        ui.label(resources.as_str())
                            .on_hover_text(tr("mods.resources_hint"));
                    });
                    row.col(|ui| {
                        if *damaged {
                            ui.colored_label(BAD, tr("mods.state_damaged"));
                        } else if *misfits > 0 {
                            ui.colored_label(
                                WARN,
                                trf("mods.state_misfits", &[("count", misfits)]),
                            );
                        } else if *conflicts > 0 {
                            ui.colored_label(
                                WARN,
                                trf("mods.state_conflicts", &[("count", conflicts)]),
                            );
                        } else if *enabled {
                            ui.colored_label(GOOD, tr("mods.state_ok"));
                        } else {
                            ui.weak(tr("mods.state_disabled"));
                        }
                    });
                    row.col(|ui| {
                        ui.add_enabled_ui(idle, |ui| {
                            if ui
                                .add_enabled(i > 0, egui::Button::new(tr("mods.up")).small())
                                .on_hover_text(tr("mods.up_hint"))
                                .clicked()
                            {
                                moves = Some((id.clone(), i - 1));
                            }
                            if ui
                                .add_enabled(
                                    i + 1 < count,
                                    egui::Button::new(tr("mods.down")).small(),
                                )
                                .on_hover_text(tr("mods.down_hint"))
                                .clicked()
                            {
                                moves = Some((id.clone(), i + 1));
                            }
                            let key = format!("uninstall-{id}");
                            let armed = self.confirm.as_deref() == Some(key.as_str());
                            let label = if armed {
                                tr("mods.sure")
                            } else {
                                tr("mods.uninstall")
                            };
                            let button = ui.add(egui::Button::new(label).small());
                            if button.clicked() {
                                if armed {
                                    remove = Some(id.clone());
                                    self.confirm = None;
                                } else {
                                    self.confirm = Some(key);
                                }
                            }
                        });
                    });
                });
            });
        if let Some(id) = select {
            self.selected = Some(id);
        }
        let ctx = ui.ctx().clone();
        if let Some((id, on)) = toggle {
            self.change(&ctx, move |m| m.set_enabled(&id, on));
        } else if let Some((id, position)) = moves {
            self.change(&ctx, move |m| m.move_mod(&id, position));
        } else if let Some(id) = remove {
            if self.selected.as_ref() == Some(&id) {
                self.selected = None;
            }
            self.change(&ctx, move |m| m.uninstall(&id));
        }
        ui.label(RichText::new(tr("mods.order_hint")).weak().small());
    }

    fn details(&mut self, ui: &mut egui::Ui) {
        let (Some(manager), Some(id)) = (&self.manager, self.selected.clone()) else {
            return;
        };
        let Some(m) = manager.get(&id) else { return };
        let preview = self
            .previews
            .entry(id.clone())
            .or_insert_with(|| {
                let data = m.preview()?;
                let (w, h, rgba) = doctor_toolkit::resource::pixels::decode_png(&data).ok()?;
                let image = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
                Some(ui.ctx().load_texture(
                    format!("mod-preview-{id}"),
                    image,
                    egui::TextureOptions::LINEAR,
                ))
            })
            .clone();
        ui.separator();
        ui.horizontal_top(|ui| {
            if let Some(texture) = &preview {
                let size = texture.size_vec2();
                let scale = (160.0 / size.x).min(160.0 / size.y).min(1.0);
                ui.image((texture.id(), size * scale));
            }
            ui.vertical(|ui| {
                ui.heading(format!("{} {}", m.manifest.name, m.manifest.version));
                let mut about = format!("id {}", m.id);
                if !m.manifest.author.is_empty() {
                    about += ", ";
                    about += &trf("mods.by", &[("author", &m.manifest.author)]);
                }
                if !m.source.is_empty() {
                    about += ", ";
                    about += &trf("mods.installed_from", &[("source", &m.source)]);
                }
                ui.label(RichText::new(about).weak());
                if let Some(url) = &m.manifest.url {
                    ui.hyperlink(url);
                }
                if let Some(problem) = &m.problem {
                    ui.colored_label(BAD, problem);
                }
                if !m.manifest.description.is_empty() {
                    ui.add(egui::Label::new(&m.manifest.description).wrap());
                }
                ui.label(trf(
                    "mods.resources_in",
                    &[
                        (
                            "resources",
                            &trn("count.resources", m.manifest.resources.len() as u64, &[]),
                        ),
                        ("streams", &m.manifest.streams().join(", ")),
                    ],
                ));
                let added = m.manifest.added().count();
                if added > 0 {
                    ui.label(trn("mods.adds", added as u64, &[]));
                }
                for stream in &m.manifest.streams {
                    let loaded = if stream.with_parent {
                        trf("mods.loaded_with", &[("parent", &stream.parent)])
                    } else {
                        trf("mods.loaded_after", &[("parent", &stream.parent)])
                    };
                    ui.label(trf(
                        "mods.creates_stream",
                        &[
                            ("name", &stream.name),
                            ("loaded", &loaded),
                            ("tocs", &stream.tocs.join(", ")),
                        ],
                    ));
                }
            });
        });
        let issues: Vec<String> = self
            .overview
            .issues_of(&id)
            .map(CompatIssue::describe)
            .collect();
        if !issues.is_empty() {
            ui.colored_label(WARN, trn("mods.misfits_list", issues.len() as u64, &[]));
            for issue in issues.iter().take(20) {
                ui.label(RichText::new(issue).small());
            }
        }
        egui::CollapsingHeader::new(trf("mods.resources_of", &[("name", &m.manifest.name)]))
            .id_salt(("mod-resources", &id))
            .show(ui, |ui| {
                for resource in m.manifest.resources.iter().take(500) {
                    let text = if resource.is_added() {
                        trf(
                            "mods.resource_added",
                            &[
                                ("key", &resource.key),
                                ("bytes", &trn("count.bytes", resource.payload.size, &[])),
                            ],
                        )
                    } else {
                        trf(
                            "mods.resource_replaced",
                            &[
                                ("key", &resource.key),
                                ("old", &resource.original.size),
                                ("bytes", &trn("count.bytes", resource.payload.size, &[])),
                            ],
                        )
                    };
                    ui.label(RichText::new(text).monospace().small());
                }
                if m.manifest.resources.len() > 500 {
                    ui.weak(trf(
                        "mods.and_more",
                        &[("count", &(m.manifest.resources.len() - 500))],
                    ));
                }
            });
    }

    fn conflicts_section(&mut self, ui: &mut egui::Ui) {
        let (list, clashes) = if self.show_all_conflicts {
            (&self.overview.all_conflicts, &self.overview.all_clashes)
        } else {
            (&self.overview.conflicts, &self.overview.clashes)
        };
        let title = trf(
            "mods.conflicts",
            &[("count", &(list.len() + clashes.len()))],
        );
        let mut show_all = self.show_all_conflicts;
        egui::CollapsingHeader::new(title)
            .id_salt(("mods-conflicts", list.is_empty() && clashes.is_empty()))
            .default_open(!list.is_empty() || !clashes.is_empty())
            .show(ui, |ui| {
                ui.checkbox(&mut show_all, tr("mods.include_disabled"));
                for clash in clashes {
                    let (color, kind) = if clash.is_fatal() {
                        (BAD, tr("mods.clash_fatal"))
                    } else {
                        (WARN, tr("mods.clash_note"))
                    };
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(color, kind);
                        ui.label(RichText::new(clash.describe()).small());
                    });
                }
                if list.is_empty() {
                    ui.weak(tr("mods.no_conflicts"));
                }
                for conflict in list.iter().take(300) {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(conflict.key.to_string()).monospace().small());
                        ui.label(trf(
                            "mods.conflict_used",
                            &[("mods", &conflict.mods.join(", "))],
                        ));
                        ui.colored_label(GOOD, conflict.winner());
                        if conflict.same_data {
                            ui.weak(tr("mods.same_data"));
                        }
                    });
                }
                if list.len() > 300 {
                    ui.weak(trf("mods.and_more", &[("count", &(list.len() - 300))]));
                }
            });
        self.show_all_conflicts = show_all;
    }

    fn integrity_section(&mut self, ui: &mut egui::Ui) {
        let Some(report) = &self.integrity else {
            return;
        };
        egui::CollapsingHeader::new(tr("mods.integrity_title"))
            .id_salt("mods-integrity")
            .default_open(true)
            .show(ui, |ui| {
                ui.label(trf(
                    "mods.integrity_counts",
                    &[
                        ("files", &trn("count.files", report.files.len() as u64, &[])),
                        ("original", &report.count(Condition::Original)),
                        ("changed", &report.changed().count()),
                        ("unverified", &report.count(Condition::Unverified)),
                    ],
                ));
                for file in report.changed() {
                    let color = match file.condition {
                        Condition::Mods | Condition::Workspace => GOOD,
                        Condition::OldToolkit => WARN,
                        _ => BAD,
                    };
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&file.rel_path).monospace());
                        ui.colored_label(color, condition_text(file.condition));
                    });
                }
                if let Some(old) = &report.old_toolkit {
                    ui.label(trf(
                        "mods.old_toolkit_present",
                        &[
                            (
                                "patches",
                                &trn("count.patches", old.installed_patches.len() as u64, &[]),
                            ),
                            (
                                "backups",
                                &trn("count.backups", old.backups.len() as u64, &[]),
                            ),
                        ],
                    ));
                    for name in &old.installed_patches {
                        ui.label(RichText::new(format!("  {name}")).small());
                    }
                }
                if !report.reference && report.count(Condition::Unverified) > 0 {
                    ui.weak(tr("mods.unverified_hint"));
                }
                for path in &report.temp_files {
                    ui.weak(trf("mods.leftover", &[("path", path)]));
                }
            });
    }

    fn build_section(&mut self, ui: &mut egui::Ui, ws: &Workspace) {
        egui::CollapsingHeader::new(tr("mods.build_title"))
            .id_salt("mods-build")
            .default_open(true)
            .show(ui, |ui| {
                if ws.is_empty() {
                    ui.weak(tr("mods.workspace_empty"));
                    return;
                }
                ui.label(trf(
                    "mods.workspace_changes",
                    &[
                        ("changes", &trn("count.changes", ws.len() as u64, &[])),
                        ("dir", &ws.dir().display()),
                    ],
                ));
                let form = &mut self.build;
                egui::Grid::new("mod-build-form")
                    .num_columns(2)
                    .spacing([8.0, 4.0])
                    .show(ui, |ui| {
                        ui.label(tr("mods.field_name"));
                        ui.text_edit_singleline(&mut form.name);
                        ui.end_row();
                        ui.label(tr("mods.field_id"));
                        ui.add(
                            egui::TextEdit::singleline(&mut form.id)
                                .hint_text(mods::id_from_name(&form.name)),
                        );
                        ui.end_row();
                        ui.label(tr("mods.field_version"));
                        ui.text_edit_singleline(&mut form.version);
                        ui.end_row();
                        ui.label(tr("mods.field_author"));
                        ui.text_edit_singleline(&mut form.author);
                        ui.end_row();
                        ui.label(tr("mods.field_url"));
                        ui.add(egui::TextEdit::singleline(&mut form.url).hint_text("https://…"));
                        ui.end_row();
                        ui.label(tr("mods.field_description"));
                        ui.text_edit_multiline(&mut form.description);
                        ui.end_row();
                        ui.label(tr("mods.field_preview"));
                        ui.horizontal(|ui| {
                            let name = form.preview.as_ref().map_or_else(
                                || tr("mods.no_preview").to_owned(),
                                |p| p.display().to_string(),
                            );
                            ui.label(name);
                            if ui.button(tr("prefs.choose")).clicked()
                                && let Some(path) = rfd::FileDialog::new()
                                    .add_filter(tr("mods.filter_pictures"), &["png", "jpg", "jpeg"])
                                    .pick_file()
                            {
                                form.preview = Some(path);
                            }
                            if form.preview.is_some() && ui.button(tr("mods.clear")).clicked() {
                                form.preview = None;
                            }
                        });
                        ui.end_row();
                    });
                let ready = !self.build.name.trim().is_empty() && self.running.is_none();
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(ready, egui::Button::new(tr("mods.save_file")))
                        .on_hover_text(tr("mods.save_file_hint"))
                        .clicked()
                    {
                        let info = self.build.info();
                        if let Some(out) = rfd::FileDialog::new()
                            .set_file_name(format!("{}-{}.zip", info.id, info.version))
                            .add_filter(tr("mods.filter_mod"), &["zip"])
                            .save_file()
                        {
                            self.message = Some(match self.build_file(ws, &out) {
                                Ok(message) => (message, false),
                                Err(error) => (error, true),
                            });
                        }
                    }
                    if ui
                        .add_enabled(
                            ready && self.manager.is_some(),
                            egui::Button::new(tr("mods.install_workspace")),
                        )
                        .on_hover_text(tr("mods.install_workspace_hint"))
                        .clicked()
                    {
                        let result = self.install_workspace(ws);
                        self.message = Some(match result {
                            Ok(message) => (message, false),
                            Err(error) => (error, true),
                        });
                        self.refresh(ui.ctx());
                    }
                });
            });
    }

    fn build_file(&self, ws: &Workspace, out: &std::path::Path) -> Result<String, String> {
        let info = self.build.info();
        let preview = self.load_preview()?;
        let report = mods::build_from_workspace(ws, &info, None, preview.as_ref(), out)
            .map_err(|e| trf("mods.cannot_build", &[("error", &e)]))?;
        Ok(trf(
            "mods.built",
            &[
                ("file", &out.display()),
                (
                    "resources",
                    &trn("count.resources", report.resources as u64, &[]),
                ),
                ("added", &report.added),
                (
                    "streams",
                    &trn("count.new_streams", report.new_streams as u64, &[]),
                ),
                ("kib", &(report.file_size / 1024)),
            ],
        ))
    }

    fn load_preview(&self) -> Result<Option<Preview>, String> {
        self.build
            .preview
            .as_ref()
            .map(|path| {
                let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
                Preview::from_image(data).map_err(|e| e.to_string())
            })
            .transpose()
    }

    fn install_workspace(&mut self, ws: &Workspace) -> Result<String, String> {
        let (Some(game), Some(manager)) = (self.game.clone(), self.manager.as_mut()) else {
            return Err(tr("mods.no_game_open").to_owned());
        };
        let info = self.build.info();
        let report = manager
            .install_workspace(&game, ws, &info, None)
            .map_err(|e| trf("mods.cannot_install_workspace", &[("error", &e)]))?;
        self.previews.remove(&report.id);
        self.selected = Some(report.id.clone());
        Ok(trf(
            "mods.installed_workspace",
            &[
                ("name", &report.name),
                (
                    "resources",
                    &trn("count.resources", report.resources as u64, &[]),
                ),
            ],
        ))
    }

    /// A button that needs a second click.
    fn confirm_button(&mut self, ui: &mut egui::Ui, id: &str, label: &str) -> Confirm {
        let armed = self.confirm.as_deref() == Some(id);
        let response = ui.button(if armed {
            tr("mods.confirm_again")
        } else {
            label
        });
        let mut clicked_twice = false;
        if response.clicked() {
            if armed {
                self.confirm = None;
                clicked_twice = true;
            } else {
                self.confirm = Some(id.to_owned());
            }
        } else if armed && ui.input(|i| i.pointer.any_click()) && !response.hovered() {
            self.confirm = None;
        }
        Confirm {
            response,
            clicked_twice,
        }
    }
}

struct Confirm {
    response: egui::Response,
    clicked_twice: bool,
}

impl Confirm {
    fn on_hover_text(self, text: &str) -> Self {
        Self {
            response: self.response.on_hover_text(text),
            clicked_twice: self.clicked_twice,
        }
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
        old_toolkit: integrity::detect_old_toolkit(game.root()).is_some(),
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
mod tests {
    use super::*;

    #[test]
    fn the_build_form_fills_in_defaults() {
        let form = BuildForm {
            name: " Better Cars ".into(),
            url: "  ".into(),
            ..BuildForm::default()
        };
        let info = form.info();
        assert_eq!(info.id, "better-cars");
        assert_eq!(info.name, "Better Cars");
        assert_eq!(info.version, "1.0");
        assert_eq!(info.url, None);
        mods::validate_id(&info.id).unwrap();
    }

    #[test]
    fn reports_are_described() {
        let mut report = ApplyReport::default();
        assert!(describe(&report, "Applied").starts_with("Nothing to do"));
        report
            .streams
            .push(("gf2/a".into(), StreamAction::Rebuilt(2)));
        report
            .streams
            .push(("gf2/b".into(), StreamAction::Restored));
        report.written.push("gf2/a.str".into());
        assert_eq!(
            describe(&report, "Applied"),
            "Applied: 1 stream rebuilt, 1 restored, 1 file written"
        );
    }
}

//! Checks in the window (stage 103): "Check edits" — the check of the
//! workspace's edits before they are written into the game
//! (`doctor_toolkit::validate`, the same as the MCP tool `validate` and the
//! command `check-edits`), "Check references" of streams
//! (`create::ReferenceChecker`), both shown in the Findings panel; and the
//! Hashes panel: a name under every hash function of the engine, the names
//! known for a hash and where the hash occurs.

use std::sync::mpsc::{Receiver, channel};

use doctor_toolkit::create::ReferenceChecker;
use doctor_toolkit::game::GameInstance;
use doctor_toolkit::navigation::search::search;
use doctor_toolkit::navigation::{HashFn, Query};
use doctor_toolkit::resource::names::type_tags;
use doctor_toolkit::validate::{self, Report};
use doctor_toolkit::workspace::ResourceKey;
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::RichText;

use super::dock::{DockOp, Pane};
use super::{BrowserApp, Level};
use crate::nav::{Location, NavAction};

/// One line of the Findings panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Finding {
    pub error: bool,
    /// Where: a resource key, a stream, a project entry, a message.
    pub place: String,
    pub what: String,
    /// The changed resource to open.
    key: Option<ResourceKey>,
    /// The resource of the game to open.
    at: Option<Location>,
}

/// What a check of references found.
type RefsResult = Result<(Vec<Finding>, usize), String>;

/// The Findings panel: what the last check found.
#[derive(Default)]
pub(crate) struct Findings {
    /// What was checked, one line.
    pub title: String,
    pub rows: Vec<Finding>,
    /// The result in a few words.
    pub summary: String,
    /// The check of references at work.
    job: Option<(String, Receiver<RefsResult>)>,
    /// Apply was held back by errors: the panel offers to apply anyway.
    pub apply_held: bool,
}

impl Findings {
    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    pub fn errors(&self) -> usize {
        self.rows.iter().filter(|r| r.error).count()
    }
}

/// The Hashes panel.
#[derive(Default)]
pub(crate) struct Hashes {
    pub text: String,
    /// What `text` was looked up last, and whether the game index was there.
    looked_up: Option<(String, bool)>,
    pub result: HashResult,
}

/// What the Hashes panel shows for its text.
#[derive(Default)]
pub(crate) struct HashResult {
    /// The text as a name: function, hash, resources the hash occurs in.
    pub of_name: Vec<(&'static str, u32, usize)>,
    /// The text as a hash, when it reads as one.
    pub hash: Option<u32>,
    pub type_tag: Option<&'static str>,
    /// Names known for the hash, with how each is known.
    pub names: Vec<(String, String)>,
    /// Resources the hash occurs in, and the first of them.
    pub occurs: usize,
    pub first: Vec<(Location, String)>,
}

/// `0x9cf57f36`, `9cf57f36` or a decimal number.
fn parse_hash(text: &str) -> Option<u32> {
    let t = text.trim();
    match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        Some(h) => u32::from_str_radix(h, 16).ok(),
        None if t.len() == 8 && t.chars().all(|c| c.is_ascii_hexdigit()) => {
            u32::from_str_radix(t, 16).ok()
        }
        None => t.parse().ok(),
    }
}

fn rows_of(report: &Report) -> Vec<Finding> {
    let row = |error: bool, f: &validate::Finding| Finding {
        error,
        place: f.place(),
        what: f.what.clone(),
        key: f.key.as_deref().and_then(|k| k.parse().ok()),
        at: None,
    };
    report
        .errors
        .iter()
        .map(|f| row(true, f))
        .chain(report.warnings.iter().map(|f| row(false, f)))
        .collect()
}

/// Checks the references of `streams` of `game` (another thread).
fn check_refs(game: &GameInstance, streams: &[usize]) -> RefsResult {
    let mut checker = ReferenceChecker::new(game).map_err(|e| e.to_string())?;
    let mut rows = Vec::new();
    for &stream in streams {
        for p in checker.check(stream).map_err(|e| e.to_string())? {
            rows.push(Finding {
                error: true,
                place: format!(
                    "{}#{} ({})",
                    p.stream,
                    p.resource,
                    type_tags().display(p.type_id)
                ),
                what: format!(
                    "unresolved reference: {} is in no stream loaded with it",
                    validate::target_text(&p.target)
                ),
                key: None,
                at: Some(Location {
                    stream,
                    resource: p.resource,
                }),
            });
        }
    }
    Ok((rows, streams.len()))
}

impl BrowserApp {
    /// The report of the check of the workspace's edits; `None` without a
    /// workspace (nothing to check).
    pub(crate) fn validate_edits(&self) -> Result<Option<Report>, String> {
        let (Some(game), Some(index)) = (&self.game, &self.index) else {
            return Err(tr("edits.no_game").to_owned());
        };
        let Some(ws) = &self.workspace else {
            return Ok(None);
        };
        let params = validate::Params {
            filter: None,
            project: self.project_dir(),
        };
        validate::validate(game, index, ws, &params)
            .map(Some)
            .map_err(|e| e.to_string())
    }

    /// "Check edits": checks the workspace's edits and shows what was found
    /// in the Findings panel. Returns whether there are no errors.
    pub(crate) fn check_edits(&mut self) -> bool {
        self.findings.apply_held = false;
        self.findings.job = None;
        self.findings.title = tr("findings.title_edits").to_owned();
        let report = match self.validate_edits() {
            Ok(Some(report)) => report,
            Ok(None) => {
                self.findings.rows.clear();
                self.findings.summary = tr("findings.no_workspace").to_owned();
                self.dock_ops.push(DockOp::ShowPanel(Pane::Findings));
                self.notify(Level::Info, tr("findings.no_workspace").to_owned());
                return true;
            }
            Err(error) => {
                self.findings.rows.clear();
                self.findings.summary = trf("findings.cannot", &[("error", &error)]);
                self.dock_ops.push(DockOp::ShowPanel(Pane::Findings));
                self.notify(Level::Error, trf("findings.cannot", &[("error", &error)]));
                return false;
            }
        };
        self.findings.rows = rows_of(&report);
        let mut summary = trf(
            "findings.summary_edits",
            &[
                (
                    "changes",
                    &trn("count.changes", report.checked_changes as u64, &[]),
                ),
                ("errors", &report.errors.len()),
                ("warnings", &report.warnings.len()),
            ],
        );
        if !report.new_messages.is_empty() {
            summary += &trf(
                "findings.new_messages",
                &[("names", &report.new_messages.join(", "))],
            );
        }
        if report.game_running {
            summary += tr("findings.game_running");
        }
        self.findings.summary = summary.clone();
        self.dock_ops.push(DockOp::ShowPanel(Pane::Findings));
        let level = if !report.ok {
            Level::Error
        } else if report.warnings.is_empty() {
            Level::Info
        } else {
            Level::Warning
        };
        self.notify(level, summary);
        report.ok
    }

    /// "Apply to game" of the menus and of the Changes panel: the edits are
    /// checked first, and errors hold the writing back until "Apply anyway"
    /// of the Findings panel.
    pub(crate) fn apply_checked(&mut self) {
        match self.validate_edits() {
            Ok(Some(report)) if !report.ok => {
                self.check_edits();
                self.findings.apply_held = true;
                self.notify(
                    Level::Error,
                    trf("findings.apply_held", &[("errors", &report.errors.len())]),
                );
            }
            // A check that cannot run does not stand in the way.
            _ => self.apply(false),
        }
    }

    /// "Check references" of streams of the game (the Content Browser: a
    /// stream or a folder): another thread, the result in the Findings panel.
    pub(crate) fn check_refs(&mut self, streams: Vec<usize>, label: String) {
        let Some(game) = self.game.clone() else {
            return;
        };
        self.findings.apply_held = false;
        self.findings.rows.clear();
        self.findings.title = trf("findings.title_refs", &[("what", &label)]);
        self.findings.summary = trf(
            "findings.checking",
            &[("streams", &trn("count.streams", streams.len() as u64, &[]))],
        );
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(check_refs(&game, &streams));
        });
        self.findings.job = Some((label, rx));
        self.dock_ops.push(DockOp::ShowPanel(Pane::Findings));
    }

    /// "Check references" of the stream or folder at `path` of the game
    /// (`gf2/queens`; empty: every stream).
    pub(crate) fn check_refs_at(&mut self, path: &str) -> bool {
        let Some(game) = self.game.clone() else {
            return false;
        };
        let path = path.trim_matches('/');
        let prefix = format!("{path}/");
        let streams: Vec<usize> = game
            .streams()
            .iter()
            .enumerate()
            .filter(|(_, f)| {
                let name = f.stream_name();
                path.is_empty() || name == path || name.starts_with(&prefix)
            })
            .map(|(i, _)| i)
            .collect();
        if streams.is_empty() {
            self.notify(Level::Error, trf("findings.no_stream", &[("path", &path)]));
            return false;
        }
        let label = if path.is_empty() {
            tr("findings.every_stream").to_owned()
        } else {
            path.to_owned()
        };
        self.check_refs(streams, label);
        true
    }

    /// Takes the result of the check of references, when it is ready.
    pub(crate) fn poll_findings(&mut self, ctx: &egui::Context) {
        let Some((_, rx)) = &self.findings.job else {
            return;
        };
        let Ok(result) = rx.try_recv() else {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            return;
        };
        self.findings.job = None;
        match result {
            Ok((rows, streams)) => {
                let summary = trf(
                    "findings.summary_refs",
                    &[
                        ("streams", &trn("count.streams", streams as u64, &[])),
                        ("unresolved", &rows.len()),
                    ],
                );
                let level = if rows.is_empty() {
                    Level::Info
                } else {
                    Level::Warning
                };
                self.findings.rows = rows;
                self.findings.summary = summary.clone();
                self.notify(level, summary);
            }
            Err(error) => {
                let text = trf("findings.cannot", &[("error", &error)]);
                self.findings.summary = text.clone();
                self.notify(Level::Error, text);
            }
        }
    }

    /// The Findings panel.
    pub(super) fn findings_pane(&mut self, ui: &mut egui::Ui) {
        let idle = !self.findings.busy();
        let mut again = false;
        let mut apply = false;
        let mut open = None;
        ui.horizontal(|ui| {
            let can_check = idle && self.game.is_some();
            if ui
                .add_enabled(can_check, egui::Button::new(tr("cmd.check_edits")))
                .on_hover_text(tr("findings.check_hint"))
                .clicked()
            {
                again = true;
            }
            if self.findings.apply_held
                && ui
                    .button(tr("findings.apply_anyway"))
                    .on_hover_text(tr("findings.apply_anyway_hint"))
                    .clicked()
            {
                apply = true;
            }
            if !idle {
                ui.spinner();
            }
        });
        if self.findings.title.is_empty() {
            ui.label(RichText::new(tr("findings.hint")).weak());
        } else {
            ui.label(RichText::new(&self.findings.title).strong());
            ui.add(egui::Label::new(&self.findings.summary).wrap());
        }
        ui.separator();
        let visuals = ui.visuals().clone();
        egui::ScrollArea::vertical()
            .id_salt("findings")
            .auto_shrink(false)
            .show(ui, |ui| {
                for (i, row) in self.findings.rows.iter().enumerate() {
                    let (tag, color) = if row.error {
                        (tr("log.error"), doctor_toolkit_mods::BAD)
                    } else {
                        (tr("log.warning"), doctor_toolkit_mods::WARN)
                    };
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(tag).color(color).strong());
                        let can_open = row.key.is_some() || row.at.is_some();
                        if !row.place.is_empty() {
                            let place = RichText::new(&row.place).monospace();
                            if can_open {
                                if ui
                                    .link(place)
                                    .on_hover_text(tr("findings.open_hint"))
                                    .clicked()
                                {
                                    open = Some(i);
                                }
                            } else {
                                ui.label(place.color(visuals.weak_text_color()));
                            }
                        }
                        ui.label(&row.what);
                    });
                }
            });
        if again {
            self.check_edits();
        }
        if apply {
            self.findings.apply_held = false;
            self.apply(false);
        }
        if let Some(row) = open.and_then(|i| self.findings.rows.get(i).cloned()) {
            if let Some(key) = &row.key {
                self.open_key(key);
            } else if let Some(at) = row.at {
                self.open_resource_tab(at);
            }
        }
    }

    /// Looks `text` up in the Hashes panel.
    pub(crate) fn look_up_hash(&mut self, text: &str) {
        self.hashes.text = text.to_owned();
        self.hashes.looked_up = Some((text.to_owned(), self.nav.index.is_some()));
        let mut result = HashResult::default();
        let text = text.trim();
        if text.is_empty() {
            self.hashes.result = result;
            return;
        }
        let index = self.nav.index.clone();
        let occurrences = |hash: u32, limit: usize| match &index {
            Some(index) => match Query::parse(&format!("0x{hash:08x}")) {
                Ok(query) => search(index, &query, limit),
                Err(_) => (Vec::new(), 0),
            },
            None => (Vec::new(), 0),
        };
        for f in HashFn::ALL {
            let hash = f.apply(text);
            result
                .of_name
                .push((f.name(), hash, occurrences(hash, 1).1));
        }
        if let Some(hash) = parse_hash(text) {
            result.hash = Some(hash);
            result.type_tag = type_tags().get(hash);
            if let Some(dictionary) = self.nav.dictionary() {
                result.names = dictionary
                    .lookup(hash)
                    .into_iter()
                    .take(50)
                    .map(|(name, origin)| (name.to_owned(), origin.describe()))
                    .collect();
            }
            let (first, total) = occurrences(hash, 10);
            result.occurs = total;
            if let Some(index) = &index {
                result.first = first
                    .iter()
                    .map(|r| {
                        let record = index.record(r.record);
                        (
                            Location {
                                stream: index.stream_of(r.record),
                                resource: record.resource as usize,
                            },
                            format!(
                                "{}  {}  {}",
                                index.label(r.record),
                                type_tags().display(record.header.type_id),
                                index.stream_name(index.stream_of(r.record))
                            ),
                        )
                    })
                    .collect();
            }
        }
        self.hashes.result = result;
    }

    /// The Hashes panel.
    pub(super) fn hashes_pane(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new(tr("hashes.hint")).weak());
        ui.horizontal(|ui| {
            ui.label(tr("hashes.text"));
            ui.add(
                egui::TextEdit::singleline(&mut self.hashes.text)
                    .desired_width(320.0)
                    .hint_text(tr("hashes.text_hint")),
            );
        });
        // Looked up again when the text changes and when the index arrives.
        let now = (self.hashes.text.clone(), self.nav.index.is_some());
        if self.hashes.looked_up.as_ref() != Some(&now) {
            let text = self.hashes.text.clone();
            self.look_up_hash(&text);
        }
        if self.nav.index.is_none() {
            ui.label(RichText::new(tr("hashes.no_index")).weak());
        }
        let mut action = None;
        let mut open = None;
        let result = &self.hashes.result;
        egui::ScrollArea::vertical()
            .id_salt("hashes")
            .auto_shrink(false)
            .show(ui, |ui| {
                if !result.of_name.is_empty() {
                    ui.add_space(6.0);
                    ui.strong(tr("hashes.as_name"));
                    egui::Grid::new("hashes-of-name")
                        .num_columns(3)
                        .striped(true)
                        .show(ui, |ui| {
                            for (function, hash, occurs) in &result.of_name {
                                ui.label(*function);
                                let text = format!("{hash:08x}");
                                if ui
                                    .link(RichText::new(&text).monospace())
                                    .on_hover_text(tr("hashes.search_hint"))
                                    .clicked()
                                {
                                    action = Some(NavAction::Search(format!("0x{text}")));
                                }
                                ui.label(if *occurs == 0 {
                                    RichText::new(tr("hashes.nowhere")).weak()
                                } else {
                                    RichText::new(trn("count.resources", *occurs as u64, &[]))
                                });
                                ui.end_row();
                            }
                        });
                }
                if let Some(hash) = result.hash {
                    ui.add_space(8.0);
                    ui.strong(trf(
                        "hashes.as_hash",
                        &[("hash", &format!("{hash:08x}")), ("decimal", &hash)],
                    ));
                    if let Some(tag) = result.type_tag {
                        ui.label(trf("hashes.type_tag", &[("tag", &tag)]));
                    }
                    if result.names.is_empty() {
                        ui.label(RichText::new(tr("hashes.no_names")).weak());
                    }
                    for (name, origin) in &result.names {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(name).monospace());
                            ui.label(RichText::new(origin).weak());
                        });
                    }
                    ui.add_space(4.0);
                    if result.occurs == 0 {
                        ui.label(RichText::new(tr("hashes.nowhere")).weak());
                    } else if ui
                        .link(trf(
                            "hashes.occurs",
                            &[(
                                "resources",
                                &trn("count.resources", result.occurs as u64, &[]),
                            )],
                        ))
                        .on_hover_text(tr("hashes.search_hint"))
                        .clicked()
                    {
                        action = Some(NavAction::Search(format!("0x{hash:08x}")));
                    }
                    for (at, label) in &result.first {
                        if ui.link(RichText::new(label).small()).clicked() {
                            open = Some(*at);
                        }
                    }
                }
            });
        if let Some(action) = action {
            self.nav_action(action);
        }
        if let Some(at) = open {
            self.open_resource_tab(at);
        }
    }

    /// The folder of the open mod project, if any.
    fn project_dir(&self) -> Option<std::path::PathBuf> {
        self.project.dir()
    }
}

//! Navigation: the game index built in the background, global search,
//! references of the selected resource, back/forward history, multi-selection
//! and batch export.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::{Duration, Instant};

use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::{Color32, RichText, Sense};
use egui_extras::{Column, TableBuilder};

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::navigation::index::default_cache_file;
use doctor_toolkit::navigation::unpack::{self, UnpackOptions, UnpackReport};
use doctor_toolkit::navigation::{Dictionary, GameIndex, Query, RecordId, SearchResult, search};
use doctor_toolkit::resource::names::type_tags;

use crate::jobs::Progress;

const ROW_HEIGHT: f32 = 18.0;
/// Entries shown per reference list.
const LIST_LIMIT: usize = 200;
/// Results kept per search.
const SEARCH_LIMIT: usize = 5000;
/// Entries remembered for back and forward.
const HISTORY_LIMIT: usize = 200;

/// A resource of the game: stream index and resource index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Location {
    pub stream: usize,
    pub resource: usize,
}

/// What navigation asks the application to do.
pub enum NavAction {
    /// Show this resource; the history already has it.
    Open(Location),
    /// Search the game for this query in a search tab.
    Search(String),
    /// A new game index is ready: searches run again in it.
    IndexReady,
}

enum Message {
    Indexed {
        index: Box<GameIndex>,
        elapsed: Duration,
        rebuilt: usize,
        error: Option<String>,
    },
    Exported(Result<(UnpackReport, PathBuf), String>),
}

struct Running {
    label: String,
    progress: Arc<Progress>,
}

pub struct Nav {
    tx: Sender<Message>,
    rx: Receiver<Message>,
    ctx: egui::Context,
    pub index: Option<Arc<GameIndex>>,
    /// Status line of the last navigation job.
    pub status: Option<String>,
    indexing: Option<Running>,
    /// The game to index once the running pass is done ([`Self::restage`]).
    again: Option<Arc<GameInstance>>,
    exporting: Option<Running>,
    dictionary: Option<Dictionary>,

    /// The search box of the toolbar.
    pub query: String,
    /// Give the search box the keyboard in the next frame.
    pub focus_search: bool,
    /// Search for the initial query once the index is ready.
    show_search_when_ready: bool,

    back: Vec<Location>,
    forward: Vec<Location>,
    current: Option<Location>,

    /// Resources picked for export.
    pub selected: BTreeSet<Location>,
    anchor: Option<Location>,
    /// Folder of the last export; the settings keep it.
    pub export_dir: Option<PathBuf>,
}

impl Nav {
    pub fn new(ctx: &egui::Context) -> Self {
        let (tx, rx) = channel();
        Self {
            tx,
            rx,
            ctx: ctx.clone(),
            index: None,
            status: None,
            indexing: None,
            again: None,
            exporting: None,
            dictionary: None,
            query: std::env::var("DOCTOR_TOOLKIT_GUI_SEARCH").unwrap_or_default(),
            focus_search: false,
            show_search_when_ready: std::env::var_os("DOCTOR_TOOLKIT_GUI_SEARCH").is_some(),
            back: Vec::new(),
            forward: Vec::new(),
            current: None,
            selected: BTreeSet::new(),
            anchor: None,
            export_dir: None,
        }
    }

    /// Whether a navigation job runs.
    pub fn busy(&self) -> bool {
        self.indexing.is_some() || self.exporting.is_some()
    }

    // ----------------------------------------------------------------- index

    /// Starts indexing a newly opened game; the old index and history go away.
    pub fn open_game(&mut self, game: Arc<GameInstance>) {
        self.index = None;
        self.dictionary = None;
        self.back.clear();
        self.forward.clear();
        self.current = None;
        self.selected.clear();
        self.start_indexing(game, tr("nav.indexing"));
    }

    /// Re-reads the streams that changed, after the game files were written.
    pub fn refresh(&mut self, game: Arc<GameInstance>) {
        if self.indexing.is_none() {
            self.start_indexing(game, tr("nav.updating"));
        }
    }

    /// The list of streams changed (the workspace's new streams were staged
    /// again): the index is read for `game`, after the running pass if one
    /// is running.
    pub fn restage(&mut self, game: Arc<GameInstance>) {
        if self.indexing.is_none() {
            self.start_indexing(game, tr("nav.updating"));
        } else {
            self.again = Some(game);
        }
    }

    /// Reads every stream again, ignoring the cache file.
    pub fn rebuild(&mut self, game: Arc<GameInstance>) -> bool {
        if self.indexing.is_some() {
            return false;
        }
        if let Some(cache) = default_cache_file(game.root()) {
            let _ = std::fs::remove_file(cache);
        }
        self.start_indexing(game, tr("nav.rebuilding"));
        true
    }

    /// Running navigation jobs: label, progress and whether it can be cancelled.
    pub fn jobs(&self) -> impl Iterator<Item = (&str, &Arc<Progress>, bool)> {
        let indexing = self
            .indexing
            .iter()
            .map(|r| (r.label.as_str(), &r.progress, false));
        let exporting = self
            .exporting
            .iter()
            .map(|r| (r.label.as_str(), &r.progress, true));
        indexing.chain(exporting)
    }

    fn start_indexing(&mut self, game: Arc<GameInstance>, label: &str) {
        let ctx = self.ctx.clone();
        let progress = Arc::new(Progress::default());
        progress
            .total
            .store(game.streams().len(), Ordering::Relaxed);
        let (tx, shared) = (self.tx.clone(), progress.clone());
        thread::spawn(move || {
            let started = Instant::now();
            let cache = default_cache_file(game.root());
            let report_progress = |done: usize, _total: usize| {
                shared.done.store(done, Ordering::Relaxed);
            };
            let (index, report) = GameIndex::open(&game, cache.as_deref(), &report_progress);
            let _ = tx.send(Message::Indexed {
                index: Box::new(index),
                elapsed: started.elapsed(),
                rebuilt: report.rebuilt,
                error: report.cache_error,
            });
            ctx.request_repaint();
        });
        self.indexing = Some(Running {
            label: label.to_owned(),
            progress,
        });
    }

    /// Takes finished jobs; returns actions for the application.
    pub fn poll(&mut self) -> Vec<NavAction> {
        let mut actions = Vec::new();
        while let Ok(message) = self.rx.try_recv() {
            match message {
                Message::Indexed {
                    index,
                    elapsed,
                    rebuilt,
                    error,
                } => {
                    self.indexing = None;
                    self.dictionary = None;
                    if let Some(game) = self.again.take() {
                        self.start_indexing(game, tr("nav.updating"));
                    }
                    crate::profile::mark("index");
                    self.status = Some(trf(
                        "nav.indexed",
                        &[
                            (
                                "resources",
                                &trn("count.resources", index.len() as u64, &[]),
                            ),
                            ("streams", &trn("count.streams_read", rebuilt as u64, &[])),
                            ("seconds", &format!("{:.1}", elapsed.as_secs_f64())),
                            (
                                "error",
                                &error.map(|e| format!("; {e}")).unwrap_or_default(),
                            ),
                        ],
                    ));
                    self.index = Some(Arc::from(index));
                    // Results point into the index: searches run again in the new one.
                    actions.push(NavAction::IndexReady);
                    if std::mem::take(&mut self.show_search_when_ready) {
                        actions.push(NavAction::Search(self.query.clone()));
                    }
                }
                Message::Exported(result) => {
                    self.exporting = None;
                    self.status = Some(match result {
                        Ok((report, dir)) => {
                            let mut text = trf(
                                "nav.exported",
                                &[
                                    (
                                        "resources",
                                        &trn("count.resources", report.resources as u64, &[]),
                                    ),
                                    ("files", &trn("count.files", report.files as u64, &[])),
                                    ("dir", &dir.display()),
                                ],
                            );
                            if let Some(first) = report.failed.first() {
                                text += &trf(
                                    "nav.export_failed_some",
                                    &[("count", &report.failed.len()), ("first", &first)],
                                );
                            }
                            if report.cancelled {
                                text += tr("nav.export_cancelled");
                            }
                            text
                        }
                        Err(error) => trf("nav.export_failed", &[("error", &error)]),
                    });
                }
            }
        }
        actions
    }

    // --------------------------------------------------------------- history

    /// Records a visit to `location` (a click in a list, a followed link).
    pub fn visit(&mut self, location: Location) {
        if self.current == Some(location) {
            return;
        }
        if let Some(current) = self.current {
            self.back.push(current);
            if self.back.len() > HISTORY_LIMIT {
                self.back.remove(0);
            }
        }
        self.forward.clear();
        self.current = Some(location);
    }

    /// Whether there is a resource to go back or forward to.
    pub fn can_go(&self, forward: bool) -> bool {
        if forward {
            !self.forward.is_empty()
        } else {
            !self.back.is_empty()
        }
    }

    pub fn go_back(&mut self) -> Option<NavAction> {
        let target = self.back.pop()?;
        if let Some(current) = self.current {
            self.forward.push(current);
        }
        self.current = Some(target);
        Some(NavAction::Open(target))
    }

    pub fn go_forward(&mut self) -> Option<NavAction> {
        let target = self.forward.pop()?;
        if let Some(current) = self.current {
            self.back.push(current);
        }
        self.current = Some(target);
        Some(NavAction::Open(target))
    }

    /// The search box of the toolbar; Enter searches in a search tab.
    pub fn search_box(&mut self, ui: &mut egui::Ui) -> Option<NavAction> {
        let edit = egui::TextEdit::singleline(&mut self.query)
            .hint_text(tr("search.box_hint"))
            .desired_width(260.0);
        let response = ui.add(edit).on_hover_text(tr("search.syntax"));
        if std::mem::take(&mut self.focus_search) {
            response.request_focus();
        }
        (response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
            .then(|| NavAction::Search(self.query.clone()))
    }

    /// The names known for hashes, built on first use; `None` without an index.
    pub fn dictionary(&mut self) -> Option<&Dictionary> {
        let index = self.index.clone()?;
        Some(
            self.dictionary
                .get_or_insert_with(|| Dictionary::for_game(&index)),
        )
    }

    /// Resources whose name, key or text matches `text`, for quick opening.
    pub fn quick_search(&self, text: &str, limit: usize) -> Vec<(Location, String)> {
        let Some(index) = &self.index else {
            return Vec::new();
        };
        let Ok(query) = Query::parse(text) else {
            return Vec::new();
        };
        if query.is_empty() {
            return Vec::new();
        }
        let (results, _) = search::search(index, &query, limit);
        results
            .iter()
            .map(|r| {
                let record = index.record(r.record);
                let label = format!(
                    "{}  {}  {}",
                    index.label(r.record),
                    type_tags().display(record.header.type_id),
                    index.stream_name(index.stream_of(r.record))
                );
                (location_of(index, r.record), label)
            })
            .collect()
    }
}

/// One search: a query and its results, shown in a search tab.
#[derive(Default)]
pub struct Search {
    pub query: String,
    results: Vec<SearchResult>,
    total: usize,
    error: Option<String>,
    time: Duration,
    dictionary_names: Vec<String>,
    /// Run the query once the game index is ready.
    pub pending: bool,
}

impl Search {
    pub fn new(query: String) -> Self {
        Self {
            query,
            pending: true,
            ..Self::default()
        }
    }

    /// Whether the search should run again in a new index.
    pub fn wants_rerun(&self) -> bool {
        self.pending || !self.results.is_empty()
    }

    /// Runs the query in the game index; waits when it is not ready.
    pub fn run(&mut self, nav: &mut Nav) {
        self.results.clear();
        self.total = 0;
        self.error = None;
        self.dictionary_names.clear();
        let Some(index) = nav.index.clone() else {
            self.pending = true;
            self.error = Some(tr("search.index_not_ready").into());
            return;
        };
        self.pending = false;
        let query = match Query::parse(&self.query) {
            Ok(query) if query.is_empty() => {
                self.error = Some(tr("search.empty_query").into());
                return;
            }
            Ok(query) => query,
            Err(error) => {
                self.error = Some(error.to_string());
                return;
            }
        };
        let started = Instant::now();
        if let Some(hash) = query.hash {
            let dictionary = nav
                .dictionary
                .get_or_insert_with(|| Dictionary::for_game(&index));
            self.dictionary_names = dictionary
                .lookup(hash)
                .into_iter()
                .map(|(name, origin)| format!("{hash:08x} = {name:?} ({})", origin.describe()))
                .collect();
        }
        let (results, total) = search::search(&index, &query, SEARCH_LIMIT);
        self.results = results;
        self.total = total;
        self.time = started.elapsed();
    }

    /// Title of the search tab.
    pub fn title(&self) -> String {
        if self.query.trim().is_empty() {
            tr("search.title").to_owned()
        } else {
            trf("search.title_query", &[("query", &self.query.trim())])
        }
    }

    /// The search results; a click opens a resource, ctrl/shift-click picks
    /// several for export.
    pub fn view(
        &mut self,
        ui: &mut egui::Ui,
        nav: &mut Nav,
        game: &Arc<GameInstance>,
    ) -> Option<NavAction> {
        let mut action = None;
        ui.horizontal(|ui| {
            ui.strong(tr("search.title"));
            let edit = egui::TextEdit::singleline(&mut self.query).desired_width(360.0);
            let response = ui.add(edit);
            if (response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                | ui.button(tr("search.run")).clicked()
            {
                self.run(nav);
            }
            if !self.results.is_empty() {
                ui.label(trf(
                    "search.summary",
                    &[
                        ("resources", &trn("count.resources", self.total as u64, &[])),
                        (
                            "shown",
                            &if self.total > self.results.len() {
                                trf("search.first_shown", &[("n", &self.results.len())])
                            } else {
                                String::new()
                            },
                        ),
                        ("ms", &format!("{:.0}", self.time.as_secs_f64() * 1000.0)),
                    ],
                ));
            }
        });
        if let Some(error) = &self.error {
            ui.colored_label(Color32::from_rgb(0xe0, 0x50, 0x50), error);
        }
        for line in &self.dictionary_names {
            ui.monospace(line);
        }
        let Some(index) = nav.index.clone() else {
            ui.label(RichText::new(tr("search.index_building")).weak());
            return None;
        };
        let locations: Vec<Location> = self
            .results
            .iter()
            .map(|r| location_of(&index, r.record))
            .collect();
        ui.horizontal(|ui| {
            let export_all = ui.add_enabled(
                !self.results.is_empty() && nav.exporting.is_none(),
                egui::Button::new(trn("search.export_results", self.results.len() as u64, &[])),
            );
            if export_all.clicked() {
                nav.export(game, locations.clone());
            }
            nav.export_selected_button(ui, game);
        });
        let mut clicked = None;
        let modifiers = ui.input(|i| i.modifiers);
        TableBuilder::new(ui)
            .id_salt("search-results")
            .striped(true)
            .sense(Sense::click())
            .column(Column::initial(420.0).resizable(true).clip(true))
            .column(Column::initial(130.0).resizable(true).clip(true))
            .column(Column::initial(200.0).resizable(true).clip(true))
            .column(Column::remainder().clip(true))
            .header(ROW_HEIGHT + 2.0, |mut header| {
                for title in [
                    tr("search.col_stream"),
                    tr("search.col_type"),
                    tr("search.col_name"),
                    tr("search.col_match"),
                ] {
                    header.col(|ui| {
                        ui.strong(title);
                    });
                }
            })
            .body(|body| {
                body.rows(ROW_HEIGHT, self.results.len(), |mut row| {
                    let i = row.index();
                    let result = &self.results[i];
                    let record = index.record(result.record);
                    row.set_selected(nav.selected.contains(&locations[i]));
                    let hits: Vec<String> = result.hits.iter().map(|h| h.describe()).collect();
                    let cells = [
                        index.stream_name(index.stream_of(result.record)).to_owned(),
                        type_tags().display(record.header.type_id),
                        index.label(result.record),
                        hits.join("; "),
                    ];
                    for cell in cells {
                        row.col(|ui| {
                            ui.add(egui::Label::new(cell).truncate().selectable(false));
                        });
                    }
                    if row.response().clicked() {
                        clicked = Some(i);
                    }
                });
            });
        if let Some(i) = clicked {
            let location = locations[i];
            if nav.pick(location, modifiers, &locations) {
                nav.visit(location);
                action = Some(NavAction::Open(location));
            }
        }
        action
    }
}

impl Nav {
    // ------------------------------------------------------------ selection

    /// Updates the multi-selection for a click on `location` in a list whose
    /// rows are `rows`. Returns whether the click was a plain click, which
    /// opens the resource.
    pub fn pick(
        &mut self,
        location: Location,
        modifiers: egui::Modifiers,
        rows: &[Location],
    ) -> bool {
        if modifiers.command {
            if !self.selected.remove(&location) {
                self.selected.insert(location);
            }
            self.anchor = Some(location);
            false
        } else if modifiers.shift
            && let Some(anchor) = self.anchor
            && let (Some(a), Some(b)) = (
                rows.iter().position(|&r| r == anchor),
                rows.iter().position(|&r| r == location),
            )
        {
            let (from, to) = (a.min(b), a.max(b));
            self.selected.extend(rows[from..=to].iter().copied());
            false
        } else {
            self.selected.clear();
            self.selected.insert(location);
            self.anchor = Some(location);
            true
        }
    }

    /// "Export selected (n)…" and "Clear selection".
    pub fn export_selected_button(&mut self, ui: &mut egui::Ui, game: &Arc<GameInstance>) {
        let n = self.selected.len();
        let button = ui
            .add_enabled(
                n > 0 && self.exporting.is_none(),
                egui::Button::new(trf("nav.export_selected", &[("n", &n)])),
            )
            .on_hover_text(tr("nav.export_selected_hint"));
        if button.clicked() {
            let list = self.selected.iter().copied().collect();
            self.export(game, list);
        }
        if n > 1 && ui.small_button(tr("nav.clear_selection")).clicked() {
            self.selected.clear();
        }
    }

    /// Exports `locations` into a folder the user picks, in the background.
    pub fn export(&mut self, game: &Arc<GameInstance>, locations: Vec<Location>) {
        let mut dialog = rfd::FileDialog::new().set_title(tr("nav.export_into"));
        if let Some(dir) = &self.export_dir {
            dialog = dialog.set_directory(dir);
        }
        let Some(dir) = dialog.pick_folder() else {
            return;
        };
        self.export_dir = Some(dir.clone());
        self.export_to(game, locations, dir);
    }

    pub fn export_to(&mut self, game: &Arc<GameInstance>, locations: Vec<Location>, dir: PathBuf) {
        let ctx = self.ctx.clone();
        let mut by_stream: std::collections::BTreeMap<usize, Vec<usize>> = Default::default();
        for l in &locations {
            by_stream.entry(l.stream).or_default().push(l.resource);
        }
        let selection: unpack::Selection = by_stream.into_iter().collect();
        let progress = Arc::new(Progress::default());
        progress.total.store(locations.len(), Ordering::Relaxed);
        let (tx, shared, game, index) = (
            self.tx.clone(),
            progress.clone(),
            game.clone(),
            self.index.clone(),
        );
        thread::spawn(move || {
            let report_progress = |done: usize, _total: usize| {
                shared.done.store(done, Ordering::Relaxed);
            };
            let result = unpack::unpack(
                &game,
                index.as_deref(),
                &selection,
                &dir,
                UnpackOptions::default(),
                &report_progress,
                &shared.cancel,
            )
            .map(|report| (report, dir))
            .map_err(|e| e.to_string());
            let _ = tx.send(Message::Exported(result));
            ctx.request_repaint();
        });
        self.exporting = Some(Running {
            label: trf(
                "nav.exporting",
                &[(
                    "resources",
                    &trn("count.resources", locations.len() as u64, &[]),
                )],
            ),
            progress,
        });
    }

    // ------------------------------------------------------------ references

    /// Names and references of the resource at `location`, with links.
    pub fn refs_panel(&mut self, ui: &mut egui::Ui, location: Location) -> Option<NavAction> {
        let Some(index) = self.index.clone() else {
            if self.indexing.is_some() {
                ui.label(RichText::new(tr("nav.refs_building")).weak());
            }
            return None;
        };
        let id = index.locate(location.stream, location.resource)?;
        let record = index.record(id);
        let mut open = None;
        let uses: Vec<_> = index.uses(id).collect();
        let used_by = index.used_by(id);
        let copies = index.copies(id);
        if let Some(error) = &record.error {
            ui.colored_label(
                Color32::from_rgb(0xe0, 0x50, 0x50),
                trf("nav.not_indexed", &[("error", &error)]),
            );
        }
        if !record.names.is_empty() {
            egui::CollapsingHeader::new(trf("nav.names", &[("n", &record.names.len())]))
                .id_salt("nav-names")
                .show(ui, |ui| {
                    for (kind, name) in record.names.iter().take(LIST_LIMIT) {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(kind.label()).weak());
                            ui.monospace(name);
                        });
                    }
                    more(ui, record.names.len());
                });
        }
        egui::CollapsingHeader::new(trf("nav.uses", &[("n", &uses.len())]))
            .id_salt("nav-uses")
            .default_open(true)
            .show(ui, |ui| {
                for (reference, targets) in uses.iter().take(LIST_LIMIT) {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&reference.label).weak());
                        ui.label("->");
                        match targets.first() {
                            None => {
                                ui.label(
                                    RichText::new(trf(
                                        "nav.not_found",
                                        &[("target", &reference.target.describe())],
                                    ))
                                    .weak(),
                                );
                            }
                            Some(&target) => {
                                if link(ui, &index, target).clicked() {
                                    open = Some(target);
                                }
                                if targets.len() > 1 {
                                    let others: Vec<String> = targets[1..]
                                        .iter()
                                        .take(20)
                                        .map(|&t| index.key(t).to_string())
                                        .collect();
                                    ui.label(format!("+{}", targets.len() - 1))
                                        .on_hover_text(others.join("\n"));
                                }
                            }
                        }
                    });
                }
                more(ui, uses.len());
            });
        egui::CollapsingHeader::new(trf("nav.used_by", &[("n", &used_by.len())]))
            .id_salt("nav-used-by")
            .default_open(true)
            .show(ui, |ui| {
                for &(source, k) in used_by.iter().take(LIST_LIMIT) {
                    ui.horizontal(|ui| {
                        if link(ui, &index, source).clicked() {
                            open = Some(source);
                        }
                        let label = &index.record(source).refs[k as usize].label;
                        ui.label(RichText::new(label).weak());
                    });
                }
                more(ui, used_by.len());
            });
        if !copies.is_empty() {
            egui::CollapsingHeader::new(trf("nav.also_in", &[("n", &copies.len())]))
                .id_salt("nav-copies")
                .show(ui, |ui| {
                    for &copy in copies.iter().take(LIST_LIMIT) {
                        if link(ui, &index, copy).clicked() {
                            open = Some(copy);
                        }
                    }
                    more(ui, copies.len());
                });
        }
        let target = location_of(&index, open?);
        self.visit(target);
        Some(NavAction::Open(target))
    }
}

/// A clickable `stream:type:ids name` of a record.
fn link(ui: &mut egui::Ui, index: &GameIndex, id: RecordId) -> egui::Response {
    let record = index.record(id);
    let text = match record.name() {
        Some(name) => format!("{} {name}", type_tags().display(record.header.type_id)),
        None => format!(
            "{} {:08x}",
            type_tags().display(record.header.type_id),
            record.header.ids[3]
        ),
    };
    ui.link(text).on_hover_text(index.key(id).to_string())
}

fn more(ui: &mut egui::Ui, total: usize) {
    if total > LIST_LIMIT {
        ui.label(RichText::new(trf("nav.more", &[("n", &(total - LIST_LIMIT))])).weak());
    }
}

fn location_of(index: &GameIndex, id: RecordId) -> Location {
    Location {
        stream: index.stream_of(id),
        resource: index.record(id).resource as usize,
    }
}

//! Edit texts: pick a string table (by language), find strings, edit them
//! one by one, replace a word everywhere or edit the whole table as CSV in
//! a spreadsheet; the edited table is recorded at the end.

use doctor_toolkit::external::{EditorRole, csv};
use doctor_toolkit::resource::lh2::StringTable;
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::RichText;

use super::{Start, Task, Tx, Watched};
use crate::app::BrowserApp;
use crate::nav::Location;

const TYPE_LH2: u32 = doctor_toolkit::hash::sdbm(b"lh2");
/// Rows shown at most in the editing list.
const SHOWN: usize = 500;

/// A string table to choose, with its language when the game names it.
#[derive(Debug, Clone)]
pub(crate) struct TableChoice {
    pub loc: Location,
    pub language: Option<String>,
}

pub(crate) struct Texts {
    pub tables: Vec<TableChoice>,
    pub filter: String,
    pub chosen: Option<Location>,
    /// The table as the workspace has it, and the edited copy.
    pub original: Option<StringTable>,
    pub edited: Option<StringTable>,
    /// Column (language) shown and edited.
    pub column: usize,
    pub search: String,
    pub find: String,
    pub replace: String,
    pub match_case: bool,
    /// Show the changed strings only.
    pub changed_only: bool,
    /// The table as CSV, open in a spreadsheet.
    pub csv: Option<Watched>,
    pub message: Option<String>,
}

impl Texts {
    pub fn new(app: &mut BrowserApp, start: &Start) -> Self {
        let mut tables = Vec::new();
        if let (Some(game), Some(index)) = (&app.game, &app.index) {
            let languages = doctor_toolkit::text::language_tables(game, index).unwrap_or_default();
            for t in doctor_toolkit::text::all_tables(index) {
                let language = languages
                    .iter()
                    .find(|l| l.stream == t.stream && l.resource == t.resource)
                    .and_then(|l| l.language.clone());
                tables.push(TableChoice {
                    loc: Location {
                        stream: t.stream,
                        resource: t.resource,
                    },
                    language,
                });
            }
            // Language tables first: they are what players read.
            tables.sort_by_key(|t| t.language.is_none());
        }
        let mut texts = Self {
            tables,
            filter: String::new(),
            chosen: None,
            original: None,
            edited: None,
            column: 0,
            search: String::new(),
            find: String::new(),
            replace: String::new(),
            match_case: false,
            changed_only: false,
            csv: None,
            message: None,
        };
        if let Some(loc) = start.loc.filter(|&l| app.type_of(l) == Some(TYPE_LH2)) {
            texts.choose(app, loc);
        }
        texts
    }

    pub fn preset(&mut self, key: &str, value: &str, app: &mut BrowserApp) {
        match key {
            "table" => {
                if let Some(t) = self.tables.iter().find(|t| {
                    t.language.as_deref() == Some(value)
                        || app.location_label(t.loc).contains(value)
                }) {
                    let loc = t.loc;
                    self.choose(app, loc);
                }
            }
            "search" => self.search = value.to_owned(),
            "find" => self.find = value.to_owned(),
            "replace" => self.replace = value.to_owned(),
            "changed" => self.changed_only = true,
            "replace_all" => {
                self.replace_all();
            }
            _ => {}
        }
    }

    /// Loads table `loc` as the workspace has it.
    pub fn choose(&mut self, app: &BrowserApp, loc: Location) {
        self.chosen = Some(loc);
        self.csv = None;
        self.column = 0;
        match app
            .current_data(loc)
            .and_then(|d| StringTable::parse(&d).map_err(|e| e.to_string()))
        {
            Ok(table) => {
                self.original = Some(table.clone());
                self.edited = Some(table);
                self.message = None;
            }
            Err(error) => {
                self.original = None;
                self.edited = None;
                self.message = Some(error);
            }
        }
    }

    /// `(entry, old, new)` of every changed string of the shown column… of
    /// every column.
    pub fn changes(&self) -> Vec<(u32, String, String)> {
        let (Some(a), Some(b)) = (&self.original, &self.edited) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (lang, (old, new)) in a.strings.iter().zip(&b.strings).enumerate() {
            for (i, (o, n)) in old.iter().zip(new).enumerate() {
                if o != n {
                    let column = csv::columns(b).get(lang).cloned().unwrap_or_default();
                    let hash = b.hashes[i];
                    out.push((hash, format!("[{column}] {o}"), n.clone()));
                }
            }
        }
        out
    }

    /// Strings of the shown column that contain the find text.
    pub fn matches(&self) -> usize {
        let Some(table) = &self.edited else { return 0 };
        if self.find.is_empty() {
            return 0;
        }
        table
            .strings
            .get(self.column)
            .map_or(0, |col| col.iter().filter(|s| self.contains(s)).count())
    }

    fn contains(&self, text: &str) -> bool {
        if self.match_case {
            text.contains(&self.find)
        } else {
            text.to_lowercase().contains(&self.find.to_lowercase())
        }
    }

    /// Replaces the find text in every string of the shown column; returns
    /// the number of strings changed.
    pub fn replace_all(&mut self) -> usize {
        if self.find.is_empty() {
            return 0;
        }
        let (find, replace, case) = (self.find.clone(), self.replace.clone(), self.match_case);
        let Some(column) = self
            .edited
            .as_mut()
            .and_then(|t| t.strings.get_mut(self.column))
        else {
            return 0;
        };
        let mut changed = 0;
        for text in column.iter_mut() {
            let new = if case {
                text.replace(&find, &replace)
            } else {
                replace_ignoring_case(text, &find, &replace)
            };
            if new != *text {
                *text = new;
                changed += 1;
            }
        }
        self.message = Some(trn("wizard.texts.replaced_n", changed as u64, &[]));
        changed
    }

    /// Writes the edited table as CSV and opens it in the text program.
    fn open_csv(&mut self, app: &mut BrowserApp) {
        let Some(table) = &self.edited else { return };
        let name = self.chosen.map_or_else(
            || "table".to_owned(),
            |l| format!("strings_{}_{}", l.stream, l.resource),
        );
        let path = super::work_dir("texts").join(format!("{name}.csv"));
        match Watched::write(path, csv::write_table(table).as_bytes()) {
            Ok(watched) => {
                app.wizard_open_editor(EditorRole::Text, &watched.path);
                self.csv = Some(watched);
            }
            Err(error) => self.message = Some(error),
        }
    }

    /// Reads a saved CSV file back into the edited table.
    pub fn load_csv(&mut self, bytes: &[u8]) {
        let Some(table) = &mut self.edited else {
            return;
        };
        let mut copy = table.clone();
        self.message = Some(match csv::apply_table(&mut copy, bytes) {
            Ok(n) => {
                *table = copy;
                trn("wizard.texts.csv_loaded_n", n as u64, &[])
            }
            Err(error) => trf("wizard.texts.csv_failed", &[("error", &error)]),
        });
    }

    fn table_step(&mut self, ui: &mut egui::Ui, app: &mut BrowserApp) {
        ui.label(tr("wizard.texts.pick"));
        ui.horizontal(|ui| {
            ui.label(tr("wizard.filter"));
            ui.text_edit_singleline(&mut self.filter);
        });
        let filter = self.filter.to_lowercase();
        let mut pick = None;
        egui::ScrollArea::vertical()
            .id_salt("texts-tables")
            .show(ui, |ui| {
                for t in &self.tables {
                    let mut label = app.location_label(t.loc);
                    if let Some(language) = &t.language {
                        label = format!("[{language}]  {label}");
                    }
                    if !filter.is_empty() && !label.to_lowercase().contains(&filter) {
                        continue;
                    }
                    if ui
                        .selectable_label(self.chosen == Some(t.loc), label)
                        .clicked()
                    {
                        pick = Some(t.loc);
                    }
                }
            });
        if let Some(loc) = pick {
            self.choose(app, loc);
        }
    }

    fn edit_step(&mut self, ui: &mut egui::Ui, app: &mut BrowserApp) {
        if let Some(bytes) = self.csv.as_mut().and_then(Watched::poll) {
            self.load_csv(&bytes);
        }
        if self.csv.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(500));
        }
        let Some(table) = &self.edited else {
            ui.label(tr("wizard.texts.no_table"));
            return;
        };
        let columns = csv::columns(table);
        let rows = table.hashes.len();
        ui.horizontal_wrapped(|ui| {
            if columns.len() > 1 {
                egui::ComboBox::from_id_salt("texts-column")
                    .selected_text(columns.get(self.column).cloned().unwrap_or_default())
                    .show_ui(ui, |ui| {
                        for (i, c) in columns.iter().enumerate() {
                            ui.selectable_value(&mut self.column, i, c);
                        }
                    });
            }
            ui.label(tr("wizard.texts.search"));
            ui.text_edit_singleline(&mut self.search);
            ui.checkbox(&mut self.changed_only, tr("wizard.texts.changed_only"));
            ui.label(
                RichText::new(trf(
                    "wizard.texts.count",
                    &[
                        ("rows", &trn("count.strings", rows as u64, &[])),
                        ("changed", &self.changes().len()),
                    ],
                ))
                .weak(),
            );
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(tr("wizard.texts.find"));
            ui.add(egui::TextEdit::singleline(&mut self.find).desired_width(160.0));
            ui.label(tr("wizard.texts.replace_with"));
            ui.add(egui::TextEdit::singleline(&mut self.replace).desired_width(160.0));
            ui.checkbox(&mut self.match_case, tr("wizard.texts.match_case"));
            let n = self.matches();
            if ui
                .add_enabled(
                    n > 0,
                    egui::Button::new(trn("wizard.texts.replace_all_n", n as u64, &[])),
                )
                .clicked()
            {
                self.replace_all();
            }
        });
        ui.horizontal_wrapped(|ui| {
            if ui
                .button(tr("wizard.texts.open_csv"))
                .on_hover_text(tr("wizard.texts.open_csv_hint"))
                .clicked()
            {
                self.open_csv(app);
            }
            if let Some(csv) = &self.csv {
                ui.label(RichText::new(csv.path.display().to_string()).small().weak());
                if ui.small_button(tr("wizard.texts.reload_csv")).clicked()
                    && let Ok(bytes) = std::fs::read(&csv.path)
                {
                    self.load_csv(&bytes);
                }
            }
            if ui.button(tr("wizard.texts.reset")).clicked() {
                self.edited = self.original.clone();
            }
        });
        if let Some(message) = &self.message {
            ui.label(RichText::new(message).weak());
        }
        ui.separator();
        let search = self.search.to_lowercase();
        let column = self.column;
        let changed_only = self.changed_only;
        let Some(table) = &mut self.edited else {
            return;
        };
        let original = self.original.as_ref();
        egui::ScrollArea::vertical()
            .id_salt("texts-rows")
            .auto_shrink(false)
            .show(ui, |ui| {
                egui::Grid::new("texts-grid")
                    .num_columns(2)
                    .striped(true)
                    .show(ui, |ui| {
                        let mut shown = 0;
                        for i in 0..table.hashes.len() {
                            let hash = table.hashes[i];
                            let Some(text) =
                                table.strings.get_mut(column).and_then(|c| c.get_mut(i))
                            else {
                                continue;
                            };
                            if !search.is_empty()
                                && !text.to_lowercase().contains(&search)
                                && !format!("{hash:08x}").contains(&search)
                            {
                                continue;
                            }
                            let changed = original
                                .and_then(|o| o.strings.get(column)?.get(i))
                                .is_some_and(|o| o != text);
                            if changed_only && !changed {
                                continue;
                            }
                            shown += 1;
                            if shown > SHOWN {
                                ui.label(RichText::new(tr("wizard.texts.more")).weak());
                                ui.end_row();
                                break;
                            }
                            let id = format!("{hash:08x}");
                            let id = if changed {
                                RichText::new(format!("* {id}")).strong()
                            } else {
                                RichText::new(id).monospace()
                            };
                            ui.label(id);
                            ui.add(egui::TextEdit::singleline(text).desired_width(f32::INFINITY));
                            ui.end_row();
                        }
                    });
            });
    }
}

/// `text` with every `find` replaced, ignoring case.
fn replace_ignoring_case(text: &str, find: &str, replace: &str) -> String {
    let lower = text.to_lowercase();
    let needle = find.to_lowercase();
    // Lower-casing can change lengths; then fall back to an exact replace.
    if lower.len() != text.len() || needle.is_empty() {
        return text.replace(find, replace);
    }
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while let Some(pos) = lower[at..].find(&needle) {
        out.push_str(&text[at..at + pos]);
        out.push_str(replace);
        at += pos + needle.len();
    }
    out.push_str(&text[at..]);
    out
}

impl Task for Texts {
    fn steps(&self) -> &'static [&'static str] {
        &["wizard.texts.step.table", "wizard.texts.step.edit"]
    }

    fn ui(&mut self, step: usize, ui: &mut egui::Ui, app: &mut BrowserApp, _tx: &mut Tx) {
        match step {
            0 => self.table_step(ui, app),
            _ => self.edit_step(ui, app),
        }
    }

    fn check(&self, step: usize, _app: &BrowserApp) -> Result<(), String> {
        match step {
            0 if self.edited.is_none() => Err(tr("wizard.texts.need_table").to_owned()),
            1 if self.changes().is_empty() => Err(tr("wizard.texts.need_change").to_owned()),
            _ => Ok(()),
        }
    }

    fn summary(&self, app: &BrowserApp) -> Vec<String> {
        let changes = self.changes();
        let mut out = vec![trf(
            "wizard.texts.summary",
            &[
                ("count", &trn("count.strings", changes.len() as u64, &[])),
                (
                    "table",
                    &self
                        .chosen
                        .map(|l| app.location_label(l))
                        .unwrap_or_default(),
                ),
            ],
        )];
        for (hash, old, new) in changes.iter().take(40) {
            out.push(format!("{hash:08x}: {old} -> {new}"));
        }
        if changes.len() > 40 {
            out.push(trf(
                "wizard.more_lines",
                &[("count", &(changes.len() - 40))],
            ));
        }
        out
    }

    fn record(&mut self, app: &mut BrowserApp, _tx: &mut Tx) -> Result<String, String> {
        let (Some(loc), Some(table)) = (self.chosen, &self.edited) else {
            return Err(tr("wizard.texts.need_table").to_owned());
        };
        let game = app.game.clone().ok_or(tr("edits.no_game"))?;
        let count = self.changes().len();
        app.workspace_mut()?
            .set_string_table(&game, loc.stream, loc.resource, table)
            .map_err(|e| e.to_string())?;
        Ok(trn("wizard.texts.label_n", count as u64, &[]))
    }
}

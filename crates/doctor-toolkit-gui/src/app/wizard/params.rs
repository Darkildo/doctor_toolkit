//! Change game parameters: weapons, health, police, vehicles… from the
//! curated list of `data/gf2/tunables.toml` (`doctor_toolkit::tunables`),
//! or any SimGroup attribute found by class ("find attribute"). Values are
//! checked against the kind and bounds of each parameter; the summary is a
//! diff; everything is recorded as one operation.

use std::collections::HashMap;

use doctor_toolkit::resource::simgroup::{SimGroup, Value};
use doctor_toolkit::tunables::{self, CATEGORIES, Change, Found, NewValues, Reading, Tunable};
use doctor_toolkit::workspace::ResourceKey;
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::{Color32, RichText};

use super::{Start, Task, Tx};
use crate::app::BrowserApp;

const TYPE_SIMGROUP: u32 = doctor_toolkit::hash::sdbm(b"simgroup");
/// Results of "find attribute" at most.
const FIND_LIMIT: usize = 400;

/// Russian label and description of the curated parameters, by id.
const RUSSIAN: &str = include_str!("../../../../../data/gf2/tunables.ru.toml");

/// `id` → (label, description) of [`RUSSIAN`].
pub(crate) fn russian() -> &'static HashMap<String, (String, String)> {
    static TABLE: std::sync::OnceLock<HashMap<String, (String, String)>> =
        std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let table: toml::Table = match RUSSIAN.parse() {
            Ok(table) => table,
            Err(error) => {
                doctor_toolkit::report::log(&format!("bad tunables.ru.toml: {error}"));
                return HashMap::new();
            }
        };
        let Some(toml::Value::Table(list)) = table.get("tunable") else {
            return HashMap::new();
        };
        list.iter()
            .filter_map(|(id, entry)| {
                let text = |key| Some(entry.get(key)?.as_str()?.to_owned());
                Some((id.clone(), (text("label")?, text("description")?)))
            })
            .collect()
    })
}

/// Label and description of `t` in the interface language; English for a
/// parameter without a translation (and for those found by class).
fn texts(t: &Tunable) -> (&str, &str) {
    if doctor_toolkit_mods::i18n::language() == "ru"
        && let Some((label, description)) = russian().get(&t.id)
    {
        return (label, description);
    }
    (&t.label, &t.description)
}

/// The label of the curated parameter labelled `english` in the interface
/// language; `english` itself for the others.
fn label_text(english: &str) -> &str {
    tunables::gf2()
        .iter()
        .find(|t| t.label == english)
        .map_or(english, |t| texts(t).0)
}

/// How the new values of a parameter are given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// One value for every object.
    All,
    /// Every current value times a factor (keeps upgrade levels apart).
    Multiply,
    /// A value per object.
    Each,
}

/// A parameter chosen for editing.
pub(crate) struct Entry {
    pub tunable: Tunable,
    pub reading: Reading,
    pub mode: Mode,
    pub value: String,
    pub factor: String,
    pub each: Vec<String>,
}

impl Entry {
    fn new(tunable: Tunable, reading: Reading) -> Self {
        let first = reading.objects.first().map_or(0.0, |o| o.value);
        let each = reading
            .objects
            .iter()
            .map(|o| tunable.format(o.value))
            .collect();
        let mode = if reading.distinct().len() > 1 {
            Mode::Multiply
        } else {
            Mode::All
        };
        Self {
            value: tunable.format(first),
            factor: "1".into(),
            each,
            mode,
            tunable,
            reading,
        }
    }

    /// The new value of every object, or why the input is wrong.
    pub fn values(&self) -> Result<Vec<(usize, f64)>, String> {
        let t = &self.tunable;
        let objects = &self.reading.objects;
        match self.mode {
            Mode::All => {
                let v = t.parse_value(&self.value)?;
                self.reading.new_values(t, &NewValues::All(v))
            }
            Mode::Multiply => {
                let factor: f64 = self
                    .factor
                    .trim()
                    .parse()
                    .map_err(|_| trf("wizard.params.bad_factor", &[("text", &self.factor)]))?;
                self.reading.new_values(t, &NewValues::Multiply(factor))
            }
            Mode::Each => objects
                .iter()
                .zip(&self.each)
                .map(|(o, text)| {
                    t.parse_value(text)
                        .map(|v| (o.object, v))
                        .map_err(|e| format!("{}: {e}", o.label))
                })
                .collect(),
        }
    }
}

/// The "find attribute" fallback.
#[derive(Default)]
pub(crate) struct Finder {
    pub class: String,
    pub attribute: String,
    pub stream: String,
    pub results: Vec<Found>,
    pub error: Option<String>,
}

pub(crate) struct Params {
    pub category: Option<&'static str>,
    pub filter: String,
    /// Current values of the curated list, by id.
    pub readings: HashMap<String, Result<Reading, String>>,
    pub chosen: Vec<Entry>,
    pub finder: Finder,
}

impl Params {
    pub fn new(app: &mut BrowserApp, start: &Start) -> Self {
        let mut params = Self {
            category: Some(CATEGORIES[0]),
            filter: String::new(),
            readings: HashMap::new(),
            chosen: Vec::new(),
            finder: Finder {
                stream: "gf2/globals_global".into(),
                ..Default::default()
            },
        };
        params.read_all(app);
        // From a SimGroup: look for attributes in its stream.
        if let (Some(loc), Some(game)) = (start.loc, &app.game)
            && app.type_of(loc) == Some(TYPE_SIMGROUP)
        {
            params.finder.stream = game.streams()[loc.stream].stream_name();
            params.category = None;
        }
        params
    }

    pub fn preset(&mut self, key: &str, value: &str, app: &mut BrowserApp) {
        match key {
            "add" => {
                for id in value.split('+') {
                    self.add_curated(id);
                }
            }
            "category" => self.category = CATEGORIES.iter().copied().find(|c| *c == value),
            "value" => {
                if let Some(e) = self.chosen.last_mut() {
                    e.mode = Mode::All;
                    e.value = value.to_owned();
                }
            }
            "factor" => {
                if let Some(e) = self.chosen.last_mut() {
                    e.mode = Mode::Multiply;
                    e.factor = value.to_owned();
                }
            }
            "find" => {
                let (class, attr) = value.split_once('.').unwrap_or((value, ""));
                self.finder.class = class.to_owned();
                self.finder.attribute = attr.to_owned();
                self.find(app);
            }
            _ => {}
        }
    }

    /// Reads every curated parameter; each SimGroup is parsed once.
    pub fn read_all(&mut self, app: &BrowserApp) {
        let Some(game) = app.game.clone() else { return };
        let mut groups: HashMap<ResourceKey, Result<(Reading, SimGroup), String>> = HashMap::new();
        for t in tunables::gf2() {
            let group = groups.entry(t.resource.clone()).or_insert_with(|| {
                let (stream, resource) =
                    tunables::locate(&game, &t.resource).map_err(|e| e.to_string())?;
                let loc = crate::nav::Location { stream, resource };
                let data = app.current_data(loc)?;
                let group = SimGroup::parse(&data).map_err(|e| e.to_string())?;
                let empty = Reading {
                    stream,
                    resource,
                    objects: Vec::new(),
                };
                Ok((empty, group))
            });
            let reading = match group {
                Ok((empty, group)) => t
                    .read_group(group)
                    .map(|objects| Reading {
                        objects,
                        ..empty.clone()
                    })
                    .map_err(|e| e.to_string()),
                Err(e) => Err(e.clone()),
            };
            self.readings.insert(t.id.clone(), reading);
        }
    }

    /// Adds a parameter of the curated list by id.
    pub fn add_curated(&mut self, id: &str) -> bool {
        if self.chosen.iter().any(|e| e.tunable.id == id) {
            return false;
        }
        let Some(t) = tunables::gf2().iter().find(|t| t.id == id) else {
            return false;
        };
        let Some(Ok(reading)) = self.readings.get(id) else {
            return false;
        };
        self.chosen.push(Entry::new(t.clone(), reading.clone()));
        true
    }

    /// Runs "find attribute".
    pub fn find(&mut self, app: &BrowserApp) {
        let f = &mut self.finder;
        f.results.clear();
        f.error = None;
        let Some(game) = app.game.clone() else { return };
        let Some(class) = tunables::class_hash(&f.class) else {
            f.error = Some(trf("wizard.params.no_class", &[("class", &f.class)]));
            return;
        };
        let attribute = match f.attribute.trim() {
            "" => None,
            text => match text.parse::<u16>() {
                Ok(i) => Some(i),
                Err(_) => {
                    f.error = Some(trf("wizard.params.bad_index", &[("text", &text)]));
                    return;
                }
            },
        };
        let wanted = f.stream.trim().to_ascii_lowercase();
        let streams: Vec<usize> = (0..game.streams().len())
            .filter(|&i| {
                let name = game.streams()[i].stream_name();
                name == wanted || (!wanted.is_empty() && name.contains(&wanted))
            })
            .take(64)
            .collect();
        if streams.is_empty() {
            f.error = Some(trf("wizard.params.no_stream", &[("stream", &f.stream)]));
            return;
        }
        match tunables::find_attribute(
            &game,
            app.workspace.as_ref(),
            &streams,
            class,
            attribute,
            FIND_LIMIT,
        ) {
            Ok(found) => {
                if found.is_empty() {
                    f.error = Some(tr("wizard.nothing_found").to_owned());
                }
                f.results = found;
            }
            Err(error) => f.error = Some(error.to_string()),
        }
    }

    /// Adds one attribute of a found object.
    pub(super) fn add_found(
        &mut self,
        app: &BrowserApp,
        found: &Found,
        attribute: u16,
        value: &Value,
    ) {
        let Some(class) = tunables::class_hash(&self.finder.class) else {
            return;
        };
        let Some(kind) = tunables::kind_of(class, attribute, Some(value)) else {
            return;
        };
        let Some(game) = app.game.clone() else { return };
        let object = found.label.clone();
        let tunable = Tunable::custom(found.resource.clone(), class, attribute, object, kind);
        if self.chosen.iter().any(|e| e.tunable.id == tunable.id) {
            return;
        }
        match tunable.read(&game, app.workspace.as_ref()) {
            Ok(reading) => self.chosen.push(Entry::new(tunable, reading)),
            Err(error) => self.finder.error = Some(error.to_string()),
        }
    }

    pub fn changes(&self) -> Result<Vec<Change>, String> {
        self.chosen
            .iter()
            .map(|e| {
                e.values().map(|values| Change {
                    tunable: e.tunable.clone(),
                    values,
                })
            })
            .collect()
    }

    fn choose_step(&mut self, ui: &mut egui::Ui, app: &mut BrowserApp) {
        ui.horizontal_wrapped(|ui| {
            for c in CATEGORIES {
                let text = tr(&format!("wizard.params.category.{c}"));
                if ui
                    .selectable_label(self.category == Some(c), text)
                    .clicked()
                {
                    self.category = Some(c);
                }
            }
            if ui
                .selectable_label(self.category.is_none(), tr("wizard.params.find_title"))
                .clicked()
            {
                self.category = None;
            }
        });
        ui.label(
            RichText::new(trn("wizard.params.chosen_n", self.chosen.len() as u64, &[])).strong(),
        );
        ui.separator();
        match self.category {
            Some(category) => self.curated_list(ui, category),
            None => self.finder_ui(ui, app),
        }
    }

    fn curated_list(&mut self, ui: &mut egui::Ui, category: &str) {
        ui.horizontal(|ui| {
            ui.label(tr("wizard.filter"));
            ui.text_edit_singleline(&mut self.filter);
        });
        ui.label(
            RichText::new(tr("wizard.params.curated_hint"))
                .weak()
                .small(),
        );
        let filter = self.filter.to_lowercase();
        let mut add = None;
        let mut remove = None;
        egui::ScrollArea::vertical()
            .id_salt("params-list")
            .auto_shrink(false)
            .show(ui, |ui| {
                egui::Grid::new("params-grid")
                    .num_columns(3)
                    .striped(true)
                    .show(ui, |ui| {
                        for t in tunables::gf2().iter().filter(|t| t.category == category) {
                            let (label, description) = texts(t);
                            // Both languages match: the guide and the
                            // community name parameters in English.
                            if !filter.is_empty()
                                && ![&t.label, &t.description, label, description]
                                    .iter()
                                    .any(|s| s.to_lowercase().contains(&filter))
                            {
                                continue;
                            }
                            let mut on = self.chosen.iter().any(|e| e.tunable.id == t.id);
                            let reading = self.readings.get(&t.id);
                            let ok = matches!(reading, Some(Ok(_)));
                            if ui
                                .add_enabled(ok, egui::Checkbox::new(&mut on, label))
                                .on_hover_text(description)
                                .changed()
                            {
                                if on {
                                    add = Some(t.id.clone());
                                } else {
                                    remove = Some(t.id.clone());
                                }
                            }
                            match reading {
                                Some(Ok(r)) => {
                                    let values: Vec<String> =
                                        r.distinct().iter().map(|v| t.format(*v)).collect();
                                    ui.monospace(values.join(" / "));
                                }
                                Some(Err(e)) => {
                                    ui.colored_label(Color32::from_rgb(230, 90, 80), e);
                                }
                                None => {
                                    ui.label("");
                                }
                            }
                            ui.label(RichText::new(description).weak().small());
                            ui.end_row();
                        }
                    });
            });
        if let Some(id) = add {
            self.add_curated(&id);
        }
        if let Some(id) = remove {
            self.chosen.retain(|e| e.tunable.id != id);
        }
    }

    fn finder_ui(&mut self, ui: &mut egui::Ui, app: &mut BrowserApp) {
        ui.label(RichText::new(tr("wizard.params.find_hint")).weak());
        let mut run = false;
        ui.horizontal_wrapped(|ui| {
            ui.label(tr("wizard.params.class"));
            run |= ui
                .add(egui::TextEdit::singleline(&mut self.finder.class).desired_width(180.0))
                .lost_focus();
            ui.label(tr("wizard.params.attribute"));
            run |= ui
                .add(egui::TextEdit::singleline(&mut self.finder.attribute).desired_width(50.0))
                .lost_focus();
            ui.label(tr("wizard.params.stream"));
            run |= ui
                .add(egui::TextEdit::singleline(&mut self.finder.stream).desired_width(220.0))
                .lost_focus();
            run |= ui.button(tr("wizard.params.find")).clicked();
        });
        if run && !self.finder.class.trim().is_empty() {
            self.find(app);
        }
        if let Some(error) = &self.finder.error {
            ui.colored_label(Color32::from_rgb(230, 90, 80), error);
        }
        let results = std::mem::take(&mut self.finder.results);
        let mut add = None;
        egui::ScrollArea::vertical()
            .id_salt("params-found")
            .auto_shrink(false)
            .show(ui, |ui| {
                for (i, found) in results.iter().enumerate() {
                    ui.horizontal_wrapped(|ui| {
                        ui.monospace(format!(
                            "{}  #{}  {}",
                            found.resource.stream, found.object, found.label
                        ));
                        for (attr, value) in &found.attrs {
                            let numeric =
                                matches!(value, Value::Int(_) | Value::Float(_) | Value::Hash(_));
                            let text = format!("[{attr}] {value}");
                            if numeric {
                                if ui
                                    .small_button(text)
                                    .on_hover_text(tr("wizard.params.add_found"))
                                    .clicked()
                                {
                                    add = Some((i, *attr, value.clone()));
                                }
                            } else {
                                ui.label(RichText::new(text).weak());
                            }
                        }
                    });
                }
            });
        if let Some((i, attr, value)) = add {
            let found = results[i].clone();
            self.add_found(app, &found, attr, &value);
        }
        self.finder.results = results;
    }

    fn edit_step(&mut self, ui: &mut egui::Ui) {
        let mut remove = None;
        egui::ScrollArea::vertical()
            .id_salt("params-edit")
            .auto_shrink(false)
            .show(ui, |ui| {
                for (i, e) in self.chosen.iter_mut().enumerate() {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.strong(texts(&e.tunable).0);
                            ui.label(
                                RichText::new(format!(
                                    "{} {}.{}",
                                    e.tunable.resource,
                                    e.tunable.class_name(),
                                    e.tunable.attribute
                                ))
                                .weak()
                                .small(),
                            );
                            if ui.small_button("×").clicked() {
                                remove = Some(i);
                            }
                        });
                        let description = texts(&e.tunable).1;
                        if !description.is_empty() {
                            ui.label(RichText::new(description).weak().small());
                        }
                        let distinct: Vec<String> = e
                            .reading
                            .distinct()
                            .iter()
                            .map(|v| e.tunable.format(*v))
                            .collect();
                        ui.label(trf(
                            "wizard.params.current",
                            &[
                                ("values", &distinct.join(" / ")),
                                ("objects", &e.reading.objects.len()),
                            ],
                        ));
                        ui.horizontal_wrapped(|ui| {
                            ui.radio_value(&mut e.mode, Mode::All, tr("wizard.params.mode_all"));
                            ui.radio_value(
                                &mut e.mode,
                                Mode::Multiply,
                                tr("wizard.params.mode_multiply"),
                            );
                            if e.reading.objects.len() > 1 {
                                ui.radio_value(
                                    &mut e.mode,
                                    Mode::Each,
                                    tr("wizard.params.mode_each"),
                                );
                            }
                        });
                        match e.mode {
                            Mode::All => {
                                ui.add(
                                    egui::TextEdit::singleline(&mut e.value).desired_width(120.0),
                                );
                            }
                            Mode::Multiply => {
                                ui.add(
                                    egui::TextEdit::singleline(&mut e.factor).desired_width(80.0),
                                );
                            }
                            Mode::Each => {
                                egui::Grid::new(("params-each", i)).show(ui, |ui| {
                                    for (o, text) in e.reading.objects.iter().zip(&mut e.each) {
                                        ui.label(RichText::new(&o.label).small());
                                        ui.add(
                                            egui::TextEdit::singleline(text).desired_width(100.0),
                                        );
                                        ui.end_row();
                                    }
                                });
                            }
                        }
                        if let (Some(min), Some(max)) = (e.tunable.min, e.tunable.max) {
                            ui.label(
                                RichText::new(trf(
                                    "wizard.params.range",
                                    &[
                                        ("min", &e.tunable.format(min)),
                                        ("max", &e.tunable.format(max)),
                                    ],
                                ))
                                .weak()
                                .small(),
                            );
                        }
                        if let Err(error) = e.values() {
                            ui.colored_label(Color32::from_rgb(230, 90, 80), error);
                        }
                    });
                }
            });
        if let Some(i) = remove {
            self.chosen.remove(i);
        }
    }
}

impl Task for Params {
    fn steps(&self) -> &'static [&'static str] {
        &["wizard.params.step.choose", "wizard.params.step.edit"]
    }

    fn ui(&mut self, step: usize, ui: &mut egui::Ui, app: &mut BrowserApp, _tx: &mut Tx) {
        match step {
            0 => self.choose_step(ui, app),
            _ => self.edit_step(ui),
        }
    }

    fn check(&self, step: usize, app: &BrowserApp) -> Result<(), String> {
        if self.chosen.is_empty() {
            return Err(tr("wizard.params.need_choice").to_owned());
        }
        if step == 0 {
            return Ok(());
        }
        let changes = self.changes()?;
        let game = app.game.as_ref().ok_or(tr("edits.no_game"))?;
        let (_, diff) =
            tunables::plan(game, app.workspace.as_ref(), &changes).map_err(|e| e.to_string())?;
        if diff.is_empty() {
            return Err(tr("wizard.params.need_change").to_owned());
        }
        Ok(())
    }

    fn summary(&self, app: &BrowserApp) -> Vec<String> {
        let Some(game) = &app.game else {
            return Vec::new();
        };
        let diff = self
            .changes()
            .and_then(|c| {
                tunables::plan(game, app.workspace.as_ref(), &c).map_err(|e| e.to_string())
            })
            .map(|(_, diff)| diff);
        match diff {
            Ok(diff) => {
                let mut out = vec![trn("wizard.params.summary_n", diff.len() as u64, &[])];
                for d in diff.iter().take(60) {
                    out.push(format!(
                        "{} — {}: {} -> {}",
                        label_text(&d.label),
                        d.object,
                        d.old,
                        d.new
                    ));
                }
                if diff.len() > 60 {
                    out.push(trf("wizard.more_lines", &[("count", &(diff.len() - 60))]));
                }
                out
            }
            Err(error) => vec![error],
        }
    }

    fn record(&mut self, app: &mut BrowserApp, _tx: &mut Tx) -> Result<String, String> {
        let changes = self.changes()?;
        let game = app.game.clone().ok_or(tr("edits.no_game"))?;
        let labels: Vec<&str> = changes.iter().map(|c| texts(&c.tunable).0).collect();
        let label = trn(
            "wizard.params.label_n",
            labels.len() as u64,
            &[("first", &labels[0])],
        );
        let ws = app.workspace_mut()?;
        tunables::apply(&game, ws, &changes, &label)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| tr("wizard.params.need_change").to_owned())?;
        self.read_all(app);
        Ok(label)
    }
}

//! Editor for localised string tables (`lh2`).

use doctor_toolkit::game::StreamFile;
use doctor_toolkit::hash::sdbm;
use doctor_toolkit::resource::lh2::StringTable;
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::RichText;
use egui_extras::{Column, TableBuilder};

const LH2: u32 = sdbm(b"lh2");
const ROW_HEIGHT: f32 = 20.0;

pub struct TextView {
    table: Result<StringTable, String>,
    /// The table as saved (in the workspace or the game): edits are counted against it.
    saved: Option<StringTable>,
    /// The game's original when the saved table is an edited version.
    game_original: Option<StringTable>,
    filter: String,
    rows: Vec<usize>,
    language: usize,
    /// Show only rows that differ from the game's original.
    changed_only: bool,
    /// The "Add string" form.
    new_name: String,
    new_text: String,
    new_everywhere: bool,
    /// Texts of single languages for the new string: (language code, text).
    new_translations: Vec<(String, String)>,
    /// Save gives the changed entries their text in every table and
    /// language of the game, not only here (stage 103).
    pub save_everywhere: bool,
}

/// What the user asked the editor to do.
pub enum TextAction {
    Save(StringTable),
    /// Add a string with this name and text to this table, or to the tables
    /// of every language.
    AddString {
        name: String,
        text: String,
        everywhere: bool,
        translations: Vec<(String, String)>,
    },
    /// Give these entries (hash, text) the text in every table and language.
    SaveEverywhere(Vec<(u32, String)>),
}

const CHANGED: egui::Color32 = crate::changes_view::PENDING;

impl TextView {
    /// Loads resource `index` if it is a string table.
    pub fn load(stream: &StreamFile, index: usize) -> Option<Self> {
        if stream.layout().resources[index].header.type_id != LH2 {
            return None;
        }
        let table = stream
            .resource_data(index)
            .and_then(|data| StringTable::parse(&data))
            .map_err(|e| e.to_string());
        Some(Self::from_table(table, None))
    }

    /// Opens a table; `game_original` is set when `table` is an edited version.
    pub fn from_table(
        table: Result<StringTable, String>,
        game_original: Option<StringTable>,
    ) -> Self {
        let saved = table.as_ref().ok().cloned();
        let rows = table
            .as_ref()
            .map(|t| (0..t.hashes.len()).collect())
            .unwrap_or_default();
        Self {
            table,
            saved,
            game_original,
            filter: String::new(),
            rows,
            language: 0,
            changed_only: false,
            new_name: String::new(),
            new_text: String::new(),
            new_everywhere: true,
            new_translations: Vec::new(),
            save_everywhere: false,
        }
    }

    /// Whether the editor holds edits that are not saved yet.
    pub fn has_unsaved_edits(&self) -> bool {
        matches!((&self.table, &self.saved), (Ok(t), Some(s)) if t != s)
    }

    /// The game's original text of `entry` in the current language, when it differs.
    fn original_text(&self, entry: usize) -> Option<&str> {
        let table = self.table.as_ref().ok()?;
        let original = self.game_original.as_ref()?;
        let row = original.find(table.hashes[entry])?;
        let text = original.strings.get(self.language)?.get(row)?;
        (Some(text) != table.strings[self.language].get(entry)).then_some(text.as_str())
    }

    /// The entries whose text differs from the saved table: (hash, text),
    /// the text of the shown language first.
    pub fn changed_entries(&self) -> Vec<(u32, String)> {
        let (Ok(table), Some(saved)) = (&self.table, &self.saved) else {
            return Vec::new();
        };
        let mut out: Vec<(u32, String)> = Vec::new();
        let columns = std::iter::once(self.language)
            .chain((0..table.strings.len()).filter(|&c| c != self.language));
        for c in columns {
            let (Some(new), Some(old)) = (table.strings.get(c), saved.strings.get(c)) else {
                continue;
            };
            for (i, (n, o)) in new.iter().zip(old).enumerate() {
                let hash = table.hashes[i];
                if n != o && !out.iter().any(|(h, _)| *h == hash) {
                    out.push((hash, n.clone()));
                }
            }
        }
        out
    }

    /// Puts `text` into the entry `hash` of the shown language (tests).
    #[cfg(test)]
    pub fn set_text(&mut self, hash: u32, text: &str) -> bool {
        let Ok(table) = &mut self.table else {
            return false;
        };
        match table.find(hash) {
            Some(at) => {
                table.strings[self.language][at] = text.to_owned();
                true
            }
            None => false,
        }
    }

    fn refilter(&mut self) {
        let Ok(table) = &self.table else { return };
        let needle = self.filter.to_lowercase();
        let column = &table.strings[self.language];
        let rows: Vec<usize> = (0..table.hashes.len())
            .filter(|&i| {
                needle.is_empty()
                    || column[i].to_lowercase().contains(&needle)
                    || format!("{:08x}", table.hashes[i]).contains(&needle)
            })
            .collect();
        self.rows = if self.changed_only {
            rows.into_iter()
                .filter(|&i| self.original_text(i).is_some())
                .collect()
        } else {
            rows
        };
    }

    pub fn show(&mut self, ui: &mut egui::Ui) -> Option<TextAction> {
        let table = match &mut self.table {
            Ok(table) => table,
            Err(error) => {
                ui.colored_label(egui::Color32::RED, error.as_str());
                return None;
            }
        };
        let mut action = None;
        let mut refilter = false;
        let languages: Vec<String> = if table.languages.is_empty() {
            (0..table.strings.len())
                .map(|i| trf("text.column", &[("index", &i)]))
                .collect()
        } else {
            table.languages.clone()
        };
        let changed = {
            let original = self.saved.as_ref();
            original.map_or(0, |o| {
                table
                    .strings
                    .iter()
                    .zip(&o.strings)
                    .map(|(a, b)| a.iter().zip(b).filter(|(x, y)| x != y).count())
                    .sum()
            })
        };
        ui.horizontal_wrapped(|ui| {
            ui.strong(trn("count.strings", table.hashes.len() as u64, &[]));
            if languages.len() > 1 {
                egui::ComboBox::from_id_salt("lh2-language")
                    .selected_text(&languages[self.language])
                    .show_ui(ui, |ui| {
                        for (i, name) in languages.iter().enumerate() {
                            refilter |= ui.selectable_value(&mut self.language, i, name).changed();
                        }
                    });
            }
            refilter |= ui
                .add(
                    egui::TextEdit::singleline(&mut self.filter)
                        .hint_text(tr("text.search"))
                        .desired_width(160.0),
                )
                .changed();
            let save = egui::Button::new(trn("text.save_changes", changed as u64, &[]));
            if ui
                .add_enabled(changed > 0, save)
                .on_hover_text(tr("text.save_hint"))
                .clicked()
            {
                action = Some(TextAction::Save(table.clone()));
            }
            ui.checkbox(&mut self.save_everywhere, tr("text.save_everywhere"))
                .on_hover_text(tr("text.save_everywhere_hint"));
            if ui
                .add_enabled(changed > 0, egui::Button::new(tr("text.discard")))
                .on_hover_text(tr("text.discard_hint"))
                .clicked()
                && let Some(saved) = &self.saved
            {
                *table = saved.clone();
            }
            if let Some(original) = &self.game_original {
                let edited: usize = table
                    .strings
                    .iter()
                    .zip(&original.strings)
                    .map(|(a, b)| a.iter().zip(b).filter(|(x, y)| x != y).count())
                    .sum();
                ui.label(RichText::new(trn("text.differ", edited as u64, &[])).color(CHANGED));
                refilter |= ui
                    .checkbox(&mut self.changed_only, tr("text.changed_only"))
                    .changed();
                if ui
                    .button(tr("text.revert"))
                    .on_hover_text(tr("text.revert_hint"))
                    .clicked()
                {
                    *table = original.clone();
                }
            }
        });
        egui::CollapsingHeader::new(tr("text.add_string"))
            .id_salt("lh2-add")
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(tr("text.name"));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_name)
                            .hint_text("doctor_hello")
                            .desired_width(180.0),
                    );
                    let valid = doctor_toolkit::text::valid_name(&self.new_name);
                    let hash = doctor_toolkit::hash::string_id(&self.new_name);
                    let taken = valid && table.find(hash).is_some();
                    ui.label(
                        RichText::new(match (valid, taken) {
                            (false, _) => tr("text.name_rule").to_owned(),
                            (true, true) => {
                                trf("text.name_taken", &[("hash", &format!("{hash:08x}"))])
                            }
                            (true, false) => trf(
                                "text.name_id",
                                &[("hash", &format!("{hash:08x}")), ("name", &self.new_name)],
                            ),
                        })
                        .small(),
                    );
                });
                ui.horizontal(|ui| {
                    ui.label(tr("text.text"));
                    ui.add(egui::TextEdit::singleline(&mut self.new_text).desired_width(320.0));
                });
                if self.new_everywhere {
                    // Texts of single languages; the others get the text above.
                    let mut remove = None;
                    for (i, (code, text)) in self.new_translations.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(code)
                                    .hint_text(tr("text.language_code"))
                                    .desired_width(50.0),
                            )
                            .on_hover_text(tr("text.language_code_hint"));
                            ui.add(egui::TextEdit::singleline(text).desired_width(320.0));
                            if ui.small_button("×").clicked() {
                                remove = Some(i);
                            }
                        });
                    }
                    if let Some(i) = remove {
                        self.new_translations.remove(i);
                    }
                    if ui
                        .small_button(tr("text.add_translation"))
                        .on_hover_text(tr("text.language_code_hint"))
                        .clicked()
                    {
                        self.new_translations.push(Default::default());
                    }
                }
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.new_everywhere, tr("text.every_language"))
                        .on_hover_text(tr("text.every_language_hint"));
                    let ok = doctor_toolkit::text::valid_name(&self.new_name)
                        && table
                            .find(doctor_toolkit::hash::string_id(&self.new_name))
                            .is_none();
                    if ui
                        .add_enabled(ok, egui::Button::new(tr("text.add")))
                        .on_hover_text(tr("text.add_hint"))
                        .clicked()
                    {
                        action = Some(TextAction::AddString {
                            name: self.new_name.trim().to_owned(),
                            text: self.new_text.clone(),
                            everywhere: self.new_everywhere,
                            translations: self
                                .new_translations
                                .iter()
                                .filter(|(code, _)| !code.trim().is_empty())
                                .map(|(code, text)| (code.trim().to_owned(), text.clone()))
                                .collect(),
                        });
                    }
                });
            });
        let column = self.language;
        let rows = &self.rows;
        let original_rows: Vec<Option<String>> = match &self.game_original {
            Some(original) => rows
                .iter()
                .map(|&entry| {
                    let row = original.find(table.hashes[entry])?;
                    let text = original.strings.get(column)?.get(row)?;
                    (*text != table.strings[column][entry]).then(|| text.clone())
                })
                .collect(),
            None => Vec::new(),
        };
        // In a tab the table takes the rest of it (stage 56); in a
        // scrolling panel it keeps its height.
        let height = match ui.available_height() {
            h if h.is_finite() && h > 420.0 && h < 10_000.0 => h - 4.0,
            _ => 420.0,
        };
        TableBuilder::new(ui)
            .id_salt("lh2-table")
            .striped(true)
            .column(Column::exact(80.0))
            .column(Column::remainder())
            .max_scroll_height(height)
            .body(|body| {
                body.rows(ROW_HEIGHT, rows.len(), |mut row| {
                    let entry = rows[row.index()];
                    let original = original_rows.get(row.index()).cloned().flatten();
                    row.col(|ui| {
                        let hash =
                            RichText::new(format!("{:08x}", table.hashes[entry])).monospace();
                        match &original {
                            Some(text) => {
                                ui.label(hash.color(CHANGED))
                                    .on_hover_text(trf("text.original", &[("text", text)]));
                            }
                            None => {
                                ui.label(hash.weak());
                            }
                        }
                    });
                    row.col(|ui| {
                        let mut edit =
                            egui::TextEdit::singleline(&mut table.strings[column][entry])
                                .desired_width(f32::INFINITY);
                        if original.is_some() {
                            edit = edit.text_color(CHANGED);
                        }
                        let response = ui.add(edit);
                        if let Some(text) = &original {
                            response.on_hover_text(trf("text.original", &[("text", text)]));
                        }
                    });
                });
            });
        if refilter {
            self.refilter();
        }
        if self.save_everywhere && matches!(action, Some(TextAction::Save(_))) {
            action = Some(TextAction::SaveEverywhere(self.changed_entries()));
        }
        action
    }
}

// ------------------------------------------------------------ view registry

/// Localised string tables (`lh2`).
pub const VIEW: crate::views::ViewEntry = crate::views::ViewEntry {
    id: "text",
    load: load_view,
};

fn load_view(
    source: &mut crate::views::ViewSource<'_>,
) -> Option<Box<dyn crate::views::ResourceViewer>> {
    if source.type_id != LH2 {
        return None;
    }
    let view = match source.edited(source.index) {
        Some(data) => {
            let table = data.and_then(|d| StringTable::parse(&d).map_err(|e| e.to_string()));
            let original = source
                .original(source.index)
                .and_then(|d| StringTable::parse(&d).ok());
            TextView::from_table(table, original)
        }
        None => TextView::load(source.stream, source.index)?,
    };
    Some(Box::new(view))
}

impl crate::views::ResourceViewer for TextView {
    /// The editor fills its tab (stage 56).
    fn tab_body(&self) -> crate::views::TabBody {
        crate::views::TabBody::Fill
    }

    fn show(
        &mut self,
        ui: &mut egui::Ui,
        _cx: &mut crate::views::ViewContext<'_>,
    ) -> Option<crate::views::ViewAction> {
        match TextView::show(self, ui)? {
            TextAction::Save(table) => Some(crate::views::ViewAction::SaveStrings(table)),
            TextAction::AddString {
                name,
                text,
                everywhere,
                translations,
            } => Some(crate::views::ViewAction::AddString {
                name,
                text,
                everywhere,
                translations,
            }),
            TextAction::SaveEverywhere(entries) => Some(crate::views::ViewAction::EditStrings(
                entries
                    .into_iter()
                    .map(|(hash, text)| doctor_toolkit::text::StringEdit {
                        hash,
                        text,
                        language: None,
                    })
                    .collect(),
            )),
        }
    }

    fn has_unsaved_edits(&self) -> bool {
        TextView::has_unsaved_edits(self)
    }
}

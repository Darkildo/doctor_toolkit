//! Tools → Sound: the sound of the game that lives outside the sound banks
//! (stage 103). Five pages over [`doctor_toolkit::sound`]:
//!
//! - **Archives** — the streamed sounds and speech files of the `.viv`
//!   archives: list, listen, export to WAV, replace with a sound file, the
//!   subtitles of a speech file by language;
//! - **Speech banks** — the entries of the `smb` banks (read only: a new
//!   entry is made by the New voice line wizard);
//! - **Speech events** — the records of the event table `get`: add a
//!   record, change the pause, priority, flags and rank;
//! - **Audio configuration** — the mix categories, bank pools and ducks of
//!   `auc`: add a category;
//! - **Mixes** — the mixer snapshots `mix`: volume, low-pass frequency and
//!   times of a category, a channel for a category the mix lacks.
//!
//! Every edit is one undoable operation of the workspace, the same as the
//! commands `sound-import`, `subtitle`, `speech-events`, `audio-config` and
//! `mix` record.
//!
//! `DOCTOR_TOOLKIT_GUI_SOUND=<item>[,…]` drives the window without input
//! (see [`BrowserApp::run_sound_preset`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::resource::auc::AudioConfig;
use doctor_toolkit::resource::audio::Sample;
use doctor_toolkit::resource::get::SpeechEventTable;
use doctor_toolkit::resource::mix::{LEVEL_TIMES, Mix};
use doctor_toolkit::resource::smb::SpeechBank;
use doctor_toolkit::sound::{self, ArchiveSound, EventEdit, MixEdit, SoundInfo};
use doctor_toolkit::workspace::Workspace;
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::RichText;

use super::{BrowserApp, Level};

/// A page of the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Page {
    #[default]
    Archives,
    Banks,
    Events,
    Config,
    Mixes,
}

impl Page {
    const ALL: [Self; 5] = [
        Self::Archives,
        Self::Banks,
        Self::Events,
        Self::Config,
        Self::Mixes,
    ];

    fn key(self) -> &'static str {
        match self {
            Self::Archives => "sounds.page_archives",
            Self::Banks => "sounds.page_banks",
            Self::Events => "sounds.page_events",
            Self::Config => "sounds.page_config",
            Self::Mixes => "sounds.page_mixes",
        }
    }

    fn from_slug(slug: &str) -> Option<Self> {
        Some(match slug {
            "archives" => Self::Archives,
            "banks" => Self::Banks,
            "events" => Self::Events,
            "config" => Self::Config,
            "mixes" => Self::Mixes,
            _ => return None,
        })
    }
}

/// What the window asks the app to do.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SoundAction {
    Select(String),
    /// The buttons are there only with sound playback built in.
    #[cfg_attr(not(feature = "audio"), allow(dead_code))]
    Play,
    #[cfg_attr(not(feature = "audio"), allow(dead_code))]
    Stop,
    /// Write the selected sound as WAV; asks for the file when `None`.
    Export(Option<PathBuf>),
    /// Replace the selected sound; asks for the file when `None`.
    Replace(Option<PathBuf>),
    /// Record the subtitle texts of the form.
    Subtitles,
    /// Add or change the event records of the form.
    SetEvents,
    /// Add the mix category of the form.
    AddCategory,
    /// Apply the form of mix channel `category`.
    SetMix {
        category: String,
    },
}

/// The archive entry shown on the right of the Archives page.
struct Selected {
    name: String,
    archive: String,
    data: Vec<u8>,
    info: Result<SoundInfo, String>,
    /// The subtitle texts as edited, by language.
    texts: Vec<(String, String)>,
}

impl Selected {
    /// The texts that differ from the file's.
    fn changed_texts(&self) -> Vec<(String, String)> {
        let Ok(SoundInfo {
            subtitles: Some(original),
            ..
        }) = &self.info
        else {
            return Vec::new();
        };
        self.texts
            .iter()
            .zip(original)
            .filter(|(new, old)| new.1 != old.1)
            .map(|(new, _)| new.clone())
            .collect()
    }
}

/// The form of one mix channel: texts of the numbers.
#[derive(Default, Clone)]
struct ChannelForm {
    volume: String,
    time: String,
    frequency: String,
}

/// The speech banks, for display.
struct Banks {
    banks: Vec<(String, SpeechBank)>,
    /// `(bank, entry)` of the rows the filter keeps.
    rows: Vec<(usize, usize)>,
    filtered_by: String,
}

struct Events {
    stream: String,
    table: SpeechEventTable,
    names: HashMap<u32, String>,
    rows: Vec<usize>,
    filtered_by: String,
}

/// The Sound window.
#[derive(Default)]
pub(crate) struct SoundWindow {
    pub open: bool,
    pub page: Page,
    /// The workspace as the cached data saw it: (head, operations).
    seen: Option<(usize, usize)>,
    error: Option<String>,

    entries: Option<Vec<ArchiveSound>>,
    filter: String,
    rows: Vec<usize>,
    filtered_by: Option<String>,
    selected: Option<Selected>,
    #[cfg(feature = "audio")]
    playing: Option<crate::audio_out::Playback>,

    banks: Option<Banks>,
    bank_filter: String,

    events: Option<Events>,
    event_filter: String,
    event_names: String,
    /// Names of the events set in this window: a new event is in no
    /// chatter set yet, so the game's data has only its hash.
    typed_events: HashMap<u32, String>,
    event_cooldown: String,
    event_priority: String,
    event_flags: String,
    event_rank: String,

    configs: Option<Vec<(String, AudioConfig)>>,
    new_category: String,
    new_parent: String,

    mixes: Option<Vec<(String, Mix)>>,
    mix: Option<String>,
    channels: HashMap<u32, ChannelForm>,
    new_channel: String,

    /// `DOCTOR_TOOLKIT_GUI_SOUND` was used.
    preset_used: bool,
}

/// A temporary workspace for reading when the editor has none yet.
fn read_ws<'a>(
    game: &GameInstance,
    ws: Option<&'a Workspace>,
    dir: Option<&Path>,
    temp: &'a mut Option<Workspace>,
) -> Result<&'a Workspace, String> {
    match ws {
        Some(ws) => Ok(ws),
        None => {
            *temp = Some(Workspace::for_game(game, dir).map_err(|e| e.to_string())?);
            Ok(temp.as_ref().expect("just made"))
        }
    }
}

fn parse_number<T: std::str::FromStr>(text: &str, what: &str) -> Result<Option<T>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    text.parse()
        .map(Some)
        .map_err(|_| trf("sounds.bad_number", &[("what", &what), ("text", &text)]))
}

fn parse_flags(text: &str) -> Result<Option<u32>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let parsed = match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16),
        None => text.parse(),
    };
    parsed.map(Some).map_err(|_| {
        trf(
            "sounds.bad_number",
            &[("what", &tr("sounds.flags")), ("text", &text)],
        )
    })
}

impl SoundWindow {
    /// Drops what was read from the game: the workspace changed.
    fn invalidate(&mut self) {
        self.banks = None;
        self.events = None;
        self.configs = None;
        self.mixes = None;
        self.channels.clear();
    }

    /// Loads what the page shows, once.
    /// `dir` is the folder of the editor's workspace
    /// (`BrowserApp::workspace_dir`), read when the editor has none open.
    fn ensure(&mut self, game: &GameInstance, ws: Option<&Workspace>, dir: Option<&Path>) {
        let result = self.load(game, ws, dir);
        if let Err(error) = result {
            self.error = Some(error);
        }
    }

    fn load(
        &mut self,
        game: &GameInstance,
        ws: Option<&Workspace>,
        dir: Option<&Path>,
    ) -> Result<(), String> {
        let mut temp = None;
        match self.page {
            Page::Archives => {
                if self.entries.is_none() {
                    self.entries =
                        Some(sound::archive_sounds(game.root(), None).map_err(|e| e.to_string())?);
                    self.filtered_by = None;
                }
                if self.filtered_by.as_deref() != Some(self.filter.as_str()) {
                    let entries = self.entries.as_deref().unwrap_or_default();
                    let filter = doctor_toolkit::container::big::normalize_name(self.filter.trim());
                    self.rows = entries
                        .iter()
                        .enumerate()
                        .filter(|(_, e)| {
                            filter.is_empty() || e.name.to_ascii_lowercase().contains(&filter)
                        })
                        .map(|(i, _)| i)
                        .collect();
                    self.filtered_by = Some(self.filter.clone());
                }
            }
            Page::Banks => {
                if self.banks.is_none() {
                    let ws = read_ws(game, ws, dir, &mut temp)?;
                    let banks = doctor_toolkit::voice::banks(game, ws)
                        .map_err(|e| e.to_string())?
                        .into_iter()
                        .map(|((s, _), bank)| (game.streams()[s].stream_name(), bank))
                        .collect();
                    self.banks = Some(Banks {
                        banks,
                        rows: Vec::new(),
                        filtered_by: "\u{0}".to_owned(),
                    });
                    self.ensure_categories(game, ws)?;
                }
                let filter = self.bank_filter.trim().to_lowercase();
                if let Some(b) = &mut self.banks
                    && b.filtered_by != filter
                {
                    let id = u32::from_str_radix(filter.trim_start_matches("0x"), 16).ok();
                    b.rows.clear();
                    for (i, (stream, bank)) in b.banks.iter().enumerate() {
                        let whole = !filter.is_empty() && stream.to_lowercase().contains(&filter);
                        for (j, e) in bank.entries.iter().enumerate() {
                            if filter.is_empty()
                                || whole
                                || e.file.to_lowercase().contains(&filter)
                                || Some(e.id[0]) == id
                            {
                                b.rows.push((i, j));
                            }
                        }
                    }
                    b.filtered_by = filter;
                }
            }
            Page::Events => {
                if self.events.is_none() {
                    let ws = read_ws(game, ws, dir, &mut temp)?;
                    let ((s, _), table) =
                        doctor_toolkit::voice::event_table(game, ws).map_err(|e| e.to_string())?;
                    let mut names =
                        sound::speech_event_names(game, ws).map_err(|e| e.to_string())?;
                    for (hash, name) in &self.typed_events {
                        names.entry(*hash).or_insert_with(|| name.clone());
                    }
                    self.events = Some(Events {
                        stream: game.streams()[s].stream_name(),
                        table,
                        names,
                        rows: Vec::new(),
                        filtered_by: "\u{0}".to_owned(),
                    });
                }
                let filter = self.event_filter.trim().to_lowercase();
                if let Some(e) = &mut self.events
                    && e.filtered_by != filter
                {
                    e.rows = e
                        .table
                        .events
                        .iter()
                        .enumerate()
                        .filter(|(_, event)| {
                            filter.is_empty()
                                || format!("{:08x}", event.hash).contains(&filter)
                                || e.names
                                    .get(&event.hash)
                                    .is_some_and(|n| n.to_lowercase().contains(&filter))
                        })
                        .map(|(i, _)| i)
                        .collect();
                    e.filtered_by = filter;
                }
            }
            Page::Config => {
                let ws = read_ws(game, ws, dir, &mut temp)?;
                self.ensure_categories(game, ws)?;
            }
            Page::Mixes => {
                let ws = read_ws(game, ws, dir, &mut temp)?;
                self.ensure_categories(game, ws)?;
                if self.mixes.is_none() {
                    let mixes: Vec<(String, Mix)> = sound::mixes(game, ws)
                        .map_err(|e| e.to_string())?
                        .into_iter()
                        .map(|((s, _), mix)| (game.streams()[s].stream_name(), mix))
                        .collect();
                    self.mixes = Some(mixes);
                    self.channels.clear();
                }
            }
        }
        Ok(())
    }

    fn ensure_categories(&mut self, game: &GameInstance, ws: &Workspace) -> Result<(), String> {
        if self.configs.is_none() {
            let configs = sound::audio_configs(game, ws)
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|((s, _), config)| (game.streams()[s].stream_name(), config))
                .collect();
            self.configs = Some(configs);
        }
        Ok(())
    }

    /// Name of mix category `hash`.
    fn category_name(&self, hash: u32) -> String {
        let name = self
            .configs
            .as_ref()
            .and_then(|c| c.first())
            .and_then(|(_, c)| c.name_of(hash));
        match name {
            Some(name) => name.to_owned(),
            None if hash == 0 => "-".to_owned(),
            None => format!("{hash:08x}"),
        }
    }

    /// The first copy of the chosen mix and the number of its copies.
    fn chosen_mix(&self) -> Option<(&Mix, usize)> {
        let name = self.mix.as_deref()?;
        let mixes = self.mixes.as_ref()?;
        let mut copies = mixes
            .iter()
            .filter(|(_, m)| m.name.eq_ignore_ascii_case(name));
        let first = copies.next()?;
        Some((&first.1, 1 + copies.count()))
    }

    /// Reads archive entry `name` for the right side of the Archives page.
    fn select(
        &mut self,
        game: &GameInstance,
        ws: Option<&Workspace>,
        name: &str,
    ) -> Result<(), String> {
        #[cfg(feature = "audio")]
        {
            self.playing = None;
        }
        let data = sound::archive_sound_data(game, ws, name).map_err(|e| e.to_string())?;
        let key = doctor_toolkit::container::big::normalize_name(name.trim());
        let found = self
            .entries
            .as_ref()
            .and_then(|all| all.iter().find(|e| e.name.to_ascii_lowercase() == key));
        let info = sound::sound_info(&data).map_err(|e| e.to_string());
        let texts = info
            .as_ref()
            .ok()
            .and_then(|i| i.subtitles.clone())
            .unwrap_or_default();
        self.selected = Some(Selected {
            name: found.map_or_else(|| name.trim().to_owned(), |e| e.name.clone()),
            archive: found.map(|e| e.archive.clone()).unwrap_or_default(),
            data,
            info,
            texts,
        });
        Ok(())
    }

    /// The event edit of the form.
    fn event_edit(&self) -> Result<(Vec<String>, EventEdit), String> {
        let names: Vec<String> = self
            .event_names
            .split([' ', '+', ','])
            .filter(|n| !n.is_empty())
            .map(str::to_owned)
            .collect();
        if names.is_empty() {
            return Err(tr("sounds.need_event").to_owned());
        }
        Ok((
            names,
            EventEdit {
                cooldown: parse_number(&self.event_cooldown, tr("sounds.cooldown"))?,
                priority: parse_number(&self.event_priority, tr("sounds.priority"))?,
                flags: parse_flags(&self.event_flags)?,
                rank: parse_number(&self.event_rank, tr("sounds.rank"))?,
            },
        ))
    }

    /// The edit of the form of channel `hash` of the chosen mix: only what
    /// differs from the mix.
    fn mix_edit(&self, hash: u32) -> Result<MixEdit, String> {
        let form = self.channels.get(&hash).cloned().unwrap_or_default();
        let channel = self
            .chosen_mix()
            .and_then(|(m, _)| m.channel(hash))
            .cloned();
        let mut edit = MixEdit {
            volume: parse_number(&form.volume, tr("sounds.volume"))?,
            frequency: parse_number(&form.frequency, tr("sounds.frequency"))?,
            time: parse_number(&form.time, tr("sounds.time"))?,
        };
        if let Some(c) = channel {
            if edit.volume == Some(c.volume()) {
                edit.volume = None;
            }
            if edit.frequency == Some(c.frequency) {
                edit.frequency = None;
            }
            if edit.time == Some(c.levels[LEVEL_TIMES[0]])
                && c.levels[LEVEL_TIMES[0]] == c.levels[LEVEL_TIMES[1]]
            {
                edit.time = None;
            }
        }
        Ok(edit)
    }

    fn ui(&mut self, ui: &mut egui::Ui) -> Vec<SoundAction> {
        let mut actions = Vec::new();
        ui.horizontal(|ui| {
            for page in Page::ALL {
                ui.selectable_value(&mut self.page, page, tr(page.key()));
            }
        });
        ui.separator();
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        match self.page {
            Page::Archives => self.archives_ui(ui, &mut actions),
            Page::Banks => self.banks_ui(ui),
            Page::Events => self.events_ui(ui, &mut actions),
            Page::Config => self.config_ui(ui, &mut actions),
            Page::Mixes => self.mixes_ui(ui, &mut actions),
        }
        actions
    }

    fn archives_ui(&mut self, ui: &mut egui::Ui, actions: &mut Vec<SoundAction>) {
        let total = self.entries.as_ref().map_or(0, Vec::len);
        egui::Panel::left("sound_archive_list")
            .resizable(true)
            .default_size(430.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(tr("sounds.filter"));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.filter)
                            .desired_width(220.0)
                            .hint_text(tr("sounds.filter_hint")),
                    );
                    ui.weak(trf(
                        "sounds.shown",
                        &[("shown", &self.rows.len()), ("total", &total)],
                    ));
                });
                let chosen = self.selected.as_ref().map(|s| s.name.clone());
                let row_height = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
                let entries = self.entries.as_deref().unwrap_or_default();
                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show_rows(ui, row_height, self.rows.len(), |ui, range| {
                        for &i in &self.rows[range] {
                            let e = &entries[i];
                            let is = chosen.as_deref() == Some(e.name.as_str());
                            let label = egui::Button::selectable(is, e.name.as_str())
                                .wrap_mode(egui::TextWrapMode::Extend);
                            if ui.add(label).clicked() {
                                actions.push(SoundAction::Select(e.name.clone()));
                            }
                        }
                    });
            });
        let Some(selected) = &mut self.selected else {
            ui.weak(tr("sounds.pick_entry"));
            return;
        };
        ui.strong(&selected.name);
        ui.weak(&selected.archive);
        match &selected.info {
            Ok(info) => {
                ui.label(trf(
                    "sounds.info",
                    &[
                        ("rate", &info.sample_rate),
                        (
                            "channels",
                            &trn("sound.channels", u64::from(info.channels), &[]),
                        ),
                        ("seconds", &format!("{:.2}", info.seconds)),
                        ("bytes", &selected.data.len()),
                    ],
                ));
            }
            Err(error) => {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    trf("sounds.not_a_sound", &[("error", error)]),
                );
                return;
            }
        }
        ui.horizontal(|ui| {
            #[cfg(feature = "audio")]
            {
                if self.playing.as_ref().is_some_and(|p| p.finished()) {
                    self.playing = None;
                }
                match &self.playing {
                    Some(playback) => {
                        ui.ctx().request_repaint();
                        let percent = format!("{:.0}", playback.progress() * 100.0);
                        if ui
                            .button(trf("sound.stop", &[("percent", &percent)]))
                            .clicked()
                        {
                            actions.push(SoundAction::Stop);
                        }
                    }
                    None => {
                        if ui.button(tr("sound.play")).clicked() {
                            actions.push(SoundAction::Play);
                        }
                    }
                }
            }
            if ui
                .button(tr("sounds.export"))
                .on_hover_text(tr("sound.save_wav_hint"))
                .clicked()
            {
                actions.push(SoundAction::Export(None));
            }
            if ui
                .button(tr("sounds.replace"))
                .on_hover_text(tr("sound.import_hint"))
                .clicked()
            {
                actions.push(SoundAction::Replace(None));
            }
        });
        #[cfg(not(feature = "audio"))]
        ui.label(RichText::new(tr("sound.no_playback")).weak().small());
        ui.separator();
        if selected.texts.is_empty() {
            ui.weak(tr("sounds.no_subtitles"));
            return;
        }
        ui.strong(tr("sounds.subtitles"));
        egui::ScrollArea::vertical()
            .id_salt("sound_subtitles")
            .auto_shrink([false, true])
            .show(ui, |ui| {
                egui::Grid::new("sound_subtitle_grid")
                    .num_columns(2)
                    .show(ui, |ui| {
                        for (language, text) in &mut selected.texts {
                            ui.monospace(language.as_str());
                            ui.add(
                                egui::TextEdit::multiline(text)
                                    .desired_rows(1)
                                    .desired_width(f32::INFINITY),
                            );
                            ui.end_row();
                        }
                    });
            });
        let changed = selected.changed_texts().len();
        if ui
            .add_enabled(changed > 0, egui::Button::new(tr("sounds.apply_subtitles")))
            .clicked()
        {
            actions.push(SoundAction::Subtitles);
        }
    }

    fn banks_ui(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new(tr("sounds.banks_note")).weak());
        let Some(banks) = &self.banks else { return };
        let total: usize = banks.banks.iter().map(|(_, b)| b.entries.len()).sum();
        ui.horizontal(|ui| {
            ui.label(tr("sounds.filter"));
            ui.add(
                egui::TextEdit::singleline(&mut self.bank_filter)
                    .desired_width(260.0)
                    .hint_text(tr("sounds.bank_filter_hint")),
            );
            ui.weak(trf(
                "sounds.bank_counts",
                &[
                    ("banks", &banks.banks.len()),
                    ("shown", &banks.rows.len()),
                    ("total", &total),
                ],
            ));
        });
        let row_height = ui.text_style_height(&egui::TextStyle::Monospace) + 4.0;
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .show_rows(ui, row_height, banks.rows.len(), |ui, range| {
                for &(i, j) in &banks.rows[range] {
                    let (stream, bank) = &banks.banks[i];
                    let e = &bank.entries[j];
                    let line = format!(
                        "{:08x}  {:<28} flags {:#04x}  {} / {} / {}  {:?}  {}    {stream}",
                        e.id[0],
                        e.file,
                        e.flags,
                        self.category_name(e.category),
                        self.category_name(e.category2),
                        self.category_name(e.duck),
                        e.settings,
                        e.stream_path(None),
                    );
                    ui.add(
                        egui::Label::new(RichText::new(line).monospace())
                            .extend()
                            .selectable(true),
                    );
                }
            });
    }

    fn events_ui(&mut self, ui: &mut egui::Ui, actions: &mut Vec<SoundAction>) {
        let Some(events) = &self.events else { return };
        ui.horizontal(|ui| {
            ui.label(tr("sounds.filter"));
            ui.add(
                egui::TextEdit::singleline(&mut self.event_filter)
                    .desired_width(220.0)
                    .hint_text(tr("sounds.event_filter_hint")),
            );
            ui.weak(trf(
                "sounds.event_counts",
                &[
                    ("shown", &events.rows.len()),
                    ("total", &events.table.events.len()),
                    ("stream", &events.stream),
                ],
            ));
        });
        let mut fill = None;
        egui::Panel::bottom("sound_event_form")
            .resizable(false)
            .show(ui, |ui| {
                ui.add_space(4.0);
                ui.label(RichText::new(tr("sounds.event_note")).weak());
                egui::Grid::new("sound_event_grid")
                    .num_columns(2)
                    .show(ui, |ui| {
                        ui.label(tr("sounds.event"));
                        ui.add(
                            egui::TextEdit::singleline(&mut self.event_names)
                                .desired_width(360.0)
                                .hint_text("F2F_HELLO"),
                        );
                        ui.end_row();
                        for (label, value) in [
                            ("sounds.cooldown", &mut self.event_cooldown),
                            ("sounds.priority", &mut self.event_priority),
                            ("sounds.flags", &mut self.event_flags),
                            ("sounds.rank", &mut self.event_rank),
                        ] {
                            ui.label(tr(label));
                            ui.add(
                                egui::TextEdit::singleline(value)
                                    .desired_width(120.0)
                                    .hint_text(tr("sounds.keep")),
                            );
                            ui.end_row();
                        }
                    });
                if ui.button(tr("sounds.set_event")).clicked() {
                    actions.push(SoundAction::SetEvents);
                }
                ui.add_space(4.0);
            });
        let row_height = ui.text_style_height(&egui::TextStyle::Monospace) + 4.0;
        ui.label(
            RichText::new(format!(
                "{:<8}  {:>8}  {:>8}  {:>4}  {:>10}  {:>8}  {:>7}  {}",
                tr("sounds.col_hash"),
                tr("sounds.col_cooldown"),
                tr("sounds.col_second"),
                tr("sounds.col_rank"),
                tr("sounds.col_flags"),
                tr("sounds.col_priority"),
                tr("sounds.col_range"),
                tr("sounds.col_name"),
            ))
            .monospace()
            .weak(),
        );
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .show_rows(ui, row_height, events.rows.len(), |ui, range| {
                for &i in &events.rows[range] {
                    let e = &events.table.events[i];
                    let name = events.names.get(&e.hash).cloned().unwrap_or_default();
                    let line = format!(
                        "{:08x}  {:>8}  {:>8}  {:>4}  {:#010x}  {:>8}  {:>7}  {name}",
                        e.hash, e.cooldown, e.second, e.rank, e.flags, e.priority, e.range
                    );
                    let chosen = !name.is_empty() && self.event_names == name;
                    let row = egui::Button::selectable(chosen, RichText::new(line).monospace())
                        .wrap_mode(egui::TextWrapMode::Extend);
                    if ui.add(row).clicked() && !name.is_empty() {
                        fill = Some((name, e.clone()));
                    }
                }
            });
        if let Some((name, e)) = fill {
            self.event_names = name;
            self.event_cooldown = e.cooldown.to_string();
            self.event_priority = e.priority.to_string();
            self.event_flags = format!("{:#x}", e.flags);
            self.event_rank = e.rank.to_string();
        }
    }

    fn config_ui(&mut self, ui: &mut egui::Ui, actions: &mut Vec<SoundAction>) {
        let Some(configs) = &self.configs else { return };
        let Some((_, config)) = configs.first() else {
            ui.weak(tr("sounds.no_config"));
            return;
        };
        ui.horizontal_wrapped(|ui| {
            ui.label(tr("sounds.config_streams"));
            for (stream, other) in configs {
                if other == config {
                    ui.monospace(stream.as_str());
                } else {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        trf("sounds.config_differs", &[("stream", stream)]),
                    );
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label(tr("sounds.new_category"));
            ui.add(
                egui::TextEdit::singleline(&mut self.new_category)
                    .desired_width(200.0)
                    .hint_text("DOCTOR_VOICE"),
            );
            ui.label(tr("sounds.parent"));
            egui::ComboBox::from_id_salt("sound_category_parent")
                .selected_text(self.new_parent.as_str())
                .show_ui(ui, |ui| {
                    for c in &config.categories {
                        ui.selectable_value(&mut self.new_parent, c.name.clone(), c.name.as_str());
                    }
                });
            let ready = !self.new_category.trim().is_empty() && !self.new_parent.is_empty();
            if ui
                .add_enabled(ready, egui::Button::new(tr("sounds.add_category")))
                .on_hover_text(tr("sounds.add_category_hint"))
                .clicked()
            {
                actions.push(SoundAction::AddCategory);
            }
        });
        ui.separator();
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let mono = |ui: &mut egui::Ui, line: String| {
                    ui.add(
                        egui::Label::new(RichText::new(line).monospace())
                            .extend()
                            .selectable(true),
                    );
                };
                ui.strong(trf("sounds.categories", &[("n", &config.categories.len())]));
                for c in &config.categories {
                    mono(ui, format!("{:08x}  {:<24} {}", c.hash(), c.name, c.parent));
                }
                ui.add_space(6.0);
                ui.strong(trf("sounds.pools", &[("n", &config.pools.len())]));
                for p in &config.pools {
                    mono(ui, format!("{:<16} {:>6}  {}", p.name, p.size, p.kind));
                }
                ui.add_space(6.0);
                ui.strong(trf("sounds.ducks", &[("n", &config.ducks.len())]));
                for d in &config.ducks {
                    mono(
                        ui,
                        format!(
                            "{:<28} {} / {}  {}",
                            d.name, d.levels[0], d.levels[1], d.mix
                        ),
                    );
                }
            });
    }

    fn mixes_ui(&mut self, ui: &mut egui::Ui, actions: &mut Vec<SoundAction>) {
        let Some(mixes) = &self.mixes else { return };
        let mut names: Vec<(String, usize)> = Vec::new();
        for (_, mix) in mixes {
            match names
                .iter_mut()
                .find(|(n, _)| n.eq_ignore_ascii_case(&mix.name))
            {
                Some((_, copies)) => *copies += 1,
                None => names.push((mix.name.clone(), 1)),
            }
        }
        names.sort_by_key(|(n, _)| n.to_lowercase());
        let mut choose = None;
        egui::Panel::left("sound_mix_list")
            .resizable(true)
            .default_size(300.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (name, copies) in &names {
                            let is = self
                                .mix
                                .as_deref()
                                .is_some_and(|m| m.eq_ignore_ascii_case(name));
                            let label = if *copies > 1 {
                                format!("{name}  ×{copies}")
                            } else {
                                name.clone()
                            };
                            if ui.selectable_label(is, label).clicked() {
                                choose = Some(name.clone());
                            }
                        }
                    });
            });
        if let Some(name) = choose {
            self.mix = Some(name);
            self.channels.clear();
        }
        let Some((mix, copies)) = self.chosen_mix() else {
            ui.weak(tr("sounds.pick_mix"));
            return;
        };
        let mix = mix.clone();
        ui.strong(trf(
            "sounds.mix_title",
            &[
                ("name", &mix.name),
                ("channels", &mix.channels.len()),
                ("copies", &copies),
            ],
        ));
        ui.label(RichText::new(tr("sounds.mix_note")).weak());
        egui::ScrollArea::both()
            .auto_shrink([false, true])
            .max_height((ui.available_height() - 40.0).max(80.0))
            .show(ui, |ui| {
                egui::Grid::new("sound_mix_grid")
                    .num_columns(5)
                    .striped(true)
                    .show(ui, |ui| {
                        ui.weak(tr("sounds.category"));
                        ui.weak(tr("sounds.volume"));
                        ui.weak(tr("sounds.time"));
                        ui.weak(tr("sounds.frequency"));
                        ui.end_row();
                        for c in &mix.channels {
                            let name = self.category_name(c.category);
                            let form =
                                self.channels
                                    .entry(c.category)
                                    .or_insert_with(|| ChannelForm {
                                        volume: c.volume().to_string(),
                                        time: c.levels[LEVEL_TIMES[0]].to_string(),
                                        frequency: c.frequency.to_string(),
                                    });
                            ui.monospace(name.as_str());
                            for value in [&mut form.volume, &mut form.time, &mut form.frequency] {
                                ui.add(egui::TextEdit::singleline(value).desired_width(80.0));
                            }
                            let changed = self
                                .mix_edit(c.category)
                                .map_or(true, |edit| !edit.is_empty());
                            if ui
                                .add_enabled(changed, egui::Button::new(tr("sounds.apply")))
                                .clicked()
                            {
                                actions.push(SoundAction::SetMix { category: name });
                            }
                            ui.end_row();
                        }
                    });
            });
        ui.horizontal(|ui| {
            ui.label(tr("sounds.new_channel"));
            let categories: Vec<String> = self
                .configs
                .as_ref()
                .and_then(|c| c.first())
                .map(|(_, c)| {
                    c.categories
                        .iter()
                        .filter(|k| mix.channel(k.hash()).is_none())
                        .map(|k| k.name.clone())
                        .collect()
                })
                .unwrap_or_default();
            egui::ComboBox::from_id_salt("sound_mix_new_channel")
                .selected_text(self.new_channel.as_str())
                .show_ui(ui, |ui| {
                    for name in &categories {
                        ui.selectable_value(&mut self.new_channel, name.clone(), name.as_str());
                    }
                });
            if ui
                .add_enabled(
                    categories.contains(&self.new_channel),
                    egui::Button::new(tr("sounds.add_channel")),
                )
                .on_hover_text(tr("sounds.add_channel_hint"))
                .clicked()
            {
                actions.push(SoundAction::SetMix {
                    category: self.new_channel.clone(),
                });
            }
        });
    }
}

impl BrowserApp {
    /// Tools → Sound: opens the window.
    pub(super) fn open_sound_window(&mut self) {
        self.sound.open = true;
    }

    /// Opens the window on a page (a view of a sound table asks for it).
    pub(super) fn open_sound_page(&mut self, page: &str, mix: Option<String>) {
        self.sound.open = true;
        if let Some(page) = Page::from_slug(page) {
            self.sound.page = page;
        }
        if mix.is_some() {
            self.sound.mix = mix;
            self.sound.channels.clear();
        }
    }

    /// The Sound window; called every frame.
    pub(super) fn show_sound_window(&mut self, ctx: &egui::Context) {
        self.sound_preset();
        if !self.sound.open {
            return;
        }
        let Some(game) = self.game.clone() else {
            return;
        };
        // An edit, an undo or a redo anywhere: read the tables again.
        let now = self
            .workspace
            .as_ref()
            .map(|ws| (ws.head(), ws.operations().len()));
        if now != self.sound.seen {
            self.sound.seen = now;
            self.sound.invalidate();
            if let Some(name) = self.sound.selected.as_ref().map(|s| s.name.clone()) {
                let ws = self.workspace.as_ref();
                let _ = self.sound.select(&game, ws, &name);
            }
        }
        self.sound.error = None;
        let dir = self.workspace_dir();
        self.sound
            .ensure(&game, self.workspace.as_ref(), dir.as_deref());
        let mut open = true;
        let mut actions = Vec::new();
        egui::Window::new(tr("sounds.title"))
            .open(&mut open)
            .default_size([1040.0, 640.0])
            .resizable(true)
            .show(ctx, |ui| {
                actions = self.sound.ui(ui);
            });
        self.sound.open = open;
        if !open {
            #[cfg(feature = "audio")]
            {
                self.sound.playing = None;
            }
        }
        for action in actions {
            self.sound_action(action);
        }
    }

    /// Does what the Sound window asked for; an error is shown (and stops
    /// a run without input). Returns whether it worked.
    pub(crate) fn sound_action(&mut self, action: SoundAction) -> bool {
        match self.sound_do(action) {
            Ok(Some(text)) => {
                self.status = text;
                true
            }
            Ok(None) => true,
            Err(error) => {
                self.notify(Level::Error, trf("sounds.failed", &[("error", &error)]));
                false
            }
        }
    }

    fn sound_do(&mut self, action: SoundAction) -> Result<Option<String>, String> {
        let game = self.game.clone().ok_or(tr("edits.no_game"))?;
        let selected = || tr("sounds.pick_entry").to_owned();
        match action {
            SoundAction::Select(name) => {
                let ws = self.workspace.as_ref();
                self.sound.select(&game, ws, &name)?;
                Ok(None)
            }
            SoundAction::Stop => {
                #[cfg(feature = "audio")]
                {
                    self.sound.playing = None;
                }
                Ok(None)
            }
            SoundAction::Play => {
                #[cfg(feature = "audio")]
                {
                    let s = self.sound.selected.as_ref().ok_or_else(selected)?;
                    let sample = Sample::parse_snu(&s.data).map_err(|e| e.to_string())?;
                    let pcm = sample.decode().map_err(|e| e.to_string())?;
                    let playback = crate::audio_out::play(
                        &pcm,
                        u16::from(sample.header.channels),
                        sample.header.sample_rate,
                    )?;
                    self.sound.playing = Some(playback);
                }
                Ok(None)
            }
            SoundAction::Export(path) => {
                let s = self.sound.selected.as_ref().ok_or_else(selected)?;
                let wav = Sample::parse_snu(&s.data)
                    .and_then(|sample| sample.to_wav())
                    .map_err(|e| e.to_string())?;
                let stem = s
                    .name
                    .rsplit('\\')
                    .next()
                    .unwrap_or_default()
                    .split('.')
                    .next()
                    .unwrap_or("sound")
                    .to_owned();
                let path = match path {
                    Some(path) => path,
                    None => {
                        let mut dialog =
                            rfd::FileDialog::new().set_file_name(format!("{stem}.wav"));
                        if let Some(dir) = &self.settings.export_dirs.export {
                            dialog = dialog.set_directory(dir);
                        }
                        match dialog.save_file() {
                            Some(path) => path,
                            None => return Ok(None),
                        }
                    }
                };
                std::fs::write(&path, wav).map_err(|e| format!("{}: {e}", path.display()))?;
                Ok(Some(trf("sounds.exported", &[("file", &path.display())])))
            }
            SoundAction::Replace(path) => {
                let name = self
                    .sound
                    .selected
                    .as_ref()
                    .map(|s| s.name.clone())
                    .ok_or_else(selected)?;
                let path = match path {
                    Some(path) => path,
                    None => {
                        let mut dialog = rfd::FileDialog::new()
                            .set_title(trf("sounds.replace_title", &[("name", &name)]));
                        if let Some(dir) = &self.settings.export_dirs.import {
                            dialog = dialog.set_directory(dir);
                        }
                        match dialog.pick_file() {
                            Some(path) => path,
                            None => return Ok(None),
                        }
                    }
                };
                self.replace_archive_sound(&game, &name, &path).map(Some)
            }
            SoundAction::Subtitles => {
                let s = self.sound.selected.as_ref().ok_or_else(selected)?;
                let (name, texts) = (s.name.clone(), s.changed_texts());
                if texts.is_empty() {
                    return Err(tr("sounds.no_text_changes").to_owned());
                }
                let ws = self.workspace_mut()?;
                sound::set_subtitles(&game, ws, &name, &texts).map_err(|e| e.to_string())?;
                self.after_edit(None);
                let languages: Vec<&str> = texts.iter().map(|(l, _)| l.as_str()).collect();
                Ok(Some(trf(
                    "sounds.subtitles_set",
                    &[("name", &name), ("languages", &languages.join(" "))],
                )))
            }
            SoundAction::SetEvents => {
                let (names, edit) = self.sound.event_edit()?;
                let ws = self.workspace_mut()?;
                let report = sound::set_speech_events(&game, ws, &names, &edit)
                    .map_err(|e| e.to_string())?;
                self.after_edit(None);
                for name in &names {
                    self.sound
                        .typed_events
                        .insert(doctor_toolkit::hash::string_id(name), name.clone());
                }
                let new = report.events.iter().filter(|(_, new)| *new).count();
                Ok(Some(trf(
                    "sounds.events_set",
                    &[
                        ("names", &names.join(", ")),
                        ("new", &new),
                        ("stream", &report.stream),
                    ],
                )))
            }
            SoundAction::AddCategory => {
                let name = self.sound.new_category.trim().to_owned();
                let parent = self.sound.new_parent.clone();
                if name.is_empty() || parent.is_empty() {
                    return Err(tr("sounds.need_category").to_owned());
                }
                let ws = self.workspace_mut()?;
                let streams = sound::add_mix_category(&game, ws, &name, &parent)
                    .map_err(|e| e.to_string())?;
                self.after_edit(None);
                self.sound.new_category.clear();
                Ok(Some(trf(
                    "sounds.category_added",
                    &[
                        ("name", &name),
                        ("parent", &parent),
                        ("copies", &streams.len()),
                    ],
                )))
            }
            SoundAction::SetMix { category } => {
                let mix = self
                    .sound
                    .mix
                    .clone()
                    .ok_or_else(|| tr("sounds.pick_mix").to_owned())?;
                let hash = doctor_toolkit::hash::string_id(&category);
                let known = self
                    .sound
                    .chosen_mix()
                    .is_some_and(|(m, _)| m.channel(hash).is_some());
                let edit = if known || self.sound.channels.contains_key(&hash) {
                    self.sound.mix_edit(hash)?
                } else {
                    // A new channel: at full volume.
                    MixEdit {
                        volume: Some(1.0),
                        ..MixEdit::default()
                    }
                };
                let ws = self.workspace_mut()?;
                let streams =
                    sound::set_mix(&game, ws, &mix, &category, &edit).map_err(|e| e.to_string())?;
                self.after_edit(None);
                self.sound.channels.remove(&hash);
                Ok(Some(trf(
                    "sounds.mix_set",
                    &[
                        ("mix", &mix),
                        ("category", &category),
                        ("copies", &streams.len()),
                    ],
                )))
            }
        }
    }

    /// Replaces archive sound `name` with the sound file `path`.
    fn replace_archive_sound(
        &mut self,
        game: &GameInstance,
        name: &str,
        path: &Path,
    ) -> Result<String, String> {
        if let Some(dir) = path.parent() {
            self.settings.export_dirs.import = Some(dir.to_owned());
        }
        let pcm = doctor_toolkit::resource::video::import::read_audio_file(path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let ws = self.workspace_mut()?;
        let info = sound::replace_archive_sound(game, ws, name, &pcm).map_err(|e| e.to_string())?;
        self.after_edit(None);
        Ok(trf(
            "sounds.replaced",
            &[
                ("name", &name),
                ("seconds", &format!("{:.2}", info.seconds)),
                ("rate", &info.sample_rate),
            ],
        ))
    }

    /// `DOCTOR_TOOLKIT_GUI_SOUND`, once the game is open.
    fn sound_preset(&mut self) {
        if self.sound.preset_used || self.game.is_none() {
            return;
        }
        self.sound.preset_used = true;
        if let Ok(spec) = std::env::var("DOCTOR_TOOLKIT_GUI_SOUND") {
            self.run_sound_preset(&spec);
        }
    }

    /// Runs the preset `spec` of the Sound window: items separated by
    /// commas (a comma of a value as `\,` or in a JSON string, as in the
    /// wizard presets), done in order; the first one that fails stops the
    /// run.
    ///
    /// - `page=archives|banks|events|config|mixes`, `filter=<text>` (of the
    ///   page);
    /// - archives: `select=<entry>`, `export=<file.wav>`,
    ///   `replace=<sound file>`, `subtitle=<language>:<text>` (several;
    ///   recorded together by the next `apply`);
    /// - events: `event=<NAME>[+<NAME>]`, `cooldown=`, `priority=`,
    ///   `flags=`, `rank=`, then `apply`;
    /// - configuration: `category=<NAME>`, `parent=<NAME>`, then `apply`;
    /// - mixes: `mix=<name>`, `channel=<CATEGORY>`, `volume=`, `time=`,
    ///   `frequency=`, then `apply`.
    pub(crate) fn run_sound_preset(&mut self, spec: &str) {
        let Some(game) = self.game.clone() else {
            return;
        };
        let dir = self.workspace_dir();
        self.sound.open = true;
        let mut subtitles = false;
        let mut channel = String::new();
        for item in super::wizard::split_preset(spec) {
            if item.is_empty() {
                continue;
            }
            let (key, value) = super::wizard::preset_item(&item);
            let s = &mut self.sound;
            let ok = match key.as_str() {
                "page" => match Page::from_slug(&value) {
                    Some(page) => {
                        s.page = page;
                        true
                    }
                    None => {
                        self.preset_failed(&format!(
                            "DOCTOR_TOOLKIT_GUI_SOUND: unknown page {value}"
                        ));
                        false
                    }
                },
                "filter" => {
                    match s.page {
                        Page::Archives => s.filter = value,
                        Page::Banks => s.bank_filter = value,
                        _ => s.event_filter = value,
                    }
                    true
                }
                "select" => {
                    s.page = Page::Archives;
                    s.ensure(&game, self.workspace.as_ref(), dir.as_deref());
                    self.sound_action(SoundAction::Select(value))
                }
                "export" => self.sound_action(SoundAction::Export(Some(value.into()))),
                "replace" => self.sound_action(SoundAction::Replace(Some(value.into()))),
                "subtitle" => {
                    let (language, text) = value.split_once(':').unwrap_or((&value, ""));
                    let slot = s.selected.as_mut().and_then(|sel| {
                        sel.texts
                            .iter_mut()
                            .find(|(l, _)| l.eq_ignore_ascii_case(language))
                    });
                    match slot {
                        Some((_, old)) => {
                            text.clone_into(old);
                            subtitles = true;
                            true
                        }
                        None => {
                            self.preset_failed(&format!(
                                "DOCTOR_TOOLKIT_GUI_SOUND: no subtitle language {language} in \
                                 the selected sound"
                            ));
                            false
                        }
                    }
                }
                "event" => {
                    s.page = Page::Events;
                    s.event_names = value;
                    true
                }
                "cooldown" => {
                    s.event_cooldown = value;
                    true
                }
                "priority" => {
                    s.event_priority = value;
                    true
                }
                "flags" => {
                    s.event_flags = value;
                    true
                }
                "rank" => {
                    s.event_rank = value;
                    true
                }
                "category" => {
                    s.page = Page::Config;
                    s.new_category = value;
                    true
                }
                "parent" => {
                    s.new_parent = value;
                    true
                }
                "mix" => {
                    s.page = Page::Mixes;
                    s.mix = Some(value);
                    s.channels.clear();
                    true
                }
                "channel" => {
                    channel = value;
                    s.ensure(&game, self.workspace.as_ref(), dir.as_deref());
                    let hash = doctor_toolkit::hash::string_id(&channel);
                    let form = match s.chosen_mix().and_then(|(m, _)| m.channel(hash)) {
                        Some(c) => ChannelForm {
                            volume: c.volume().to_string(),
                            time: c.levels[LEVEL_TIMES[0]].to_string(),
                            frequency: c.frequency.to_string(),
                        },
                        // A new channel: at full volume unless said otherwise.
                        None => ChannelForm {
                            volume: "1".to_owned(),
                            ..ChannelForm::default()
                        },
                    };
                    s.channels.insert(hash, form);
                    true
                }
                "volume" | "time" | "frequency" => {
                    let hash = doctor_toolkit::hash::string_id(&channel);
                    match s.channels.get_mut(&hash) {
                        Some(form) => {
                            match key.as_str() {
                                "volume" => form.volume = value,
                                "time" => form.time = value,
                                _ => form.frequency = value,
                            }
                            true
                        }
                        None => {
                            self.preset_failed(&format!(
                                "DOCTOR_TOOLKIT_GUI_SOUND: {key} needs `channel=` first"
                            ));
                            false
                        }
                    }
                }
                "apply" => {
                    let action = match s.page {
                        Page::Archives if subtitles => Some(SoundAction::Subtitles),
                        Page::Events => Some(SoundAction::SetEvents),
                        Page::Config => Some(SoundAction::AddCategory),
                        Page::Mixes => Some(SoundAction::SetMix {
                            category: channel.clone(),
                        }),
                        _ => None,
                    };
                    subtitles = false;
                    match action {
                        Some(action) => self.sound_action(action),
                        None => {
                            self.preset_failed(
                                "DOCTOR_TOOLKIT_GUI_SOUND: nothing to apply on this page",
                            );
                            false
                        }
                    }
                }
                _ => {
                    self.preset_failed(&format!("DOCTOR_TOOLKIT_GUI_SOUND: unknown item {key}"));
                    false
                }
            };
            if !ok {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use doctor_toolkit::hash::string_id;
    use doctor_toolkit::testing as mini;

    use super::*;
    use crate::settings::{Settings, Store};

    struct Fixture {
        dir: PathBuf,
        ctx: egui::Context,
        app: BrowserApp,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// An editor without a window on the miniature game with sound.
    fn fixture(name: &str) -> Fixture {
        let dir = std::env::temp_dir().join(format!("dt-sound-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        mini::write_game(&dir).unwrap();
        mini::write_sound(&dir).unwrap();
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = BrowserApp::new(
            &cc,
            None,
            None,
            None,
            Settings::default(),
            Store { dir: None },
        );
        app.notes.log_to_file = false;
        // No preset of the environment.
        app.sound.preset_used = true;
        let game = Arc::new(GameInstance::open(&dir).unwrap());
        let (index, errors) = game.build_index(|_| {});
        assert!(errors.is_empty(), "{errors:?}");
        app.game = Some(game);
        app.index = Some(Arc::new(index));
        Fixture { dir, ctx, app }
    }

    fn frame(f: &mut Fixture) {
        use eframe::App as _;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 800.0),
            )),
            ..Default::default()
        };
        let mut eframe_frame = eframe::Frame::_new_kittest();
        let mut output = f.ctx.run_ui(input, |ui| f.app.ui(ui, &mut eframe_frame));
        output.textures_delta.clear();
    }

    fn operations(app: &BrowserApp) -> Vec<String> {
        app.workspace
            .as_ref()
            .map(|ws| {
                ws.operations()[..ws.head()]
                    .iter()
                    .map(|op| op.label.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn wav(dir: &Path) -> PathBuf {
        let pcm = mini::tone(1.0, 44_100);
        let path = dir.join("new.wav");
        let data =
            doctor_toolkit::resource::audio::wav(&pcm.samples, pcm.channels, pcm.sample_rate);
        std::fs::write(&path, data).unwrap();
        path
    }

    #[test]
    fn every_page_draws_and_the_command_opens_the_window() {
        let mut f = fixture("pages");
        assert!(f.app.enabled(super::super::Command::Sounds));
        f.app.pending.push(super::super::Command::Sounds);
        frame(&mut f);
        assert!(f.app.sound.open);
        for page in Page::ALL {
            f.app.sound.page = page;
            frame(&mut f);
            frame(&mut f);
            assert_eq!(f.app.sound.error, None, "{page:?}");
        }
        let s = &f.app.sound;
        assert_eq!(s.entries.as_ref().unwrap().len(), 2);
        assert_eq!(s.banks.as_ref().unwrap().rows.len(), 1);
        assert_eq!(s.events.as_ref().unwrap().rows.len(), 1);
        assert_eq!(s.configs.as_ref().unwrap()[0].1.categories.len(), 2);
        assert_eq!(s.mixes.as_ref().unwrap().len(), 1);
        // The filters.
        f.app.sound.page = Page::Archives;
        f.app.sound.filter = "music/".into();
        frame(&mut f);
        assert_eq!(f.app.sound.rows.len(), 1);
        f.app.sound.page = Page::Events;
        f.app.sound.event_filter = "nothing".into();
        frame(&mut f);
        assert!(f.app.sound.events.as_ref().unwrap().rows.is_empty());
        assert!(operations(&f.app).is_empty(), "looking records nothing");
    }

    #[test]
    fn an_archive_sound_is_exported_replaced_and_subtitled() {
        let mut f = fixture("archive");
        let dir = f.dir.clone();
        f.app.sound.open = true;
        frame(&mut f);
        assert!(
            f.app
                .sound_action(SoundAction::Select(mini::SPEECH_ENTRY.into()))
        );
        frame(&mut f);
        let selected = f.app.sound.selected.as_ref().unwrap();
        assert_eq!(selected.archive, "GODFATH0.viv");
        let info = selected.info.clone().unwrap();
        assert_eq!((info.sample_rate, info.channels), (32_000, 1));
        assert!(selected.changed_texts().is_empty());
        // Export to WAV.
        let out = dir.join("out.wav");
        assert!(f.app.sound_action(SoundAction::Export(Some(out.clone()))));
        assert_eq!(&std::fs::read(&out).unwrap()[..4], b"RIFF");
        // Replace with a longer sound: one operation, the view follows.
        let new = wav(&dir);
        assert!(f.app.sound_action(SoundAction::Replace(Some(new))));
        frame(&mut f);
        assert_eq!(operations(&f.app).len(), 1);
        let info = f.app.sound.selected.as_ref().unwrap().info.clone().unwrap();
        assert!((info.seconds - 1.0).abs() < 0.01, "{}", info.seconds);
        assert_eq!(info.sample_rate, 32_000, "converted to the old format");
        // The subtitle in one language.
        assert!(
            !f.app.sound_action(SoundAction::Subtitles),
            "nothing changed"
        );
        let selected = f.app.sound.selected.as_mut().unwrap();
        let en = selected.texts.iter_mut().find(|(l, _)| l == "EN").unwrap();
        en.1 = "Good evening.".into();
        assert!(f.app.sound_action(SoundAction::Subtitles));
        frame(&mut f);
        assert_eq!(operations(&f.app).len(), 2);
        let game = f.app.game.clone().unwrap();
        let data =
            sound::archive_sound_data(&game, f.app.workspace.as_ref(), mini::SPEECH_ENTRY).unwrap();
        let texts = sound::sound_info(&data).unwrap().subtitles.unwrap();
        assert!(texts.contains(&("EN".to_owned(), "Good evening.".to_owned())));
        // Undo: the window reads the sound again.
        f.app.pending.push(super::super::Command::Undo);
        frame(&mut f);
        frame(&mut f);
        let selected = f.app.sound.selected.as_ref().unwrap();
        assert!(
            selected
                .texts
                .contains(&("EN".to_owned(), "Hello there.".to_owned()))
        );
        // A sound without subtitles; a file that is not a sound.
        assert!(
            f.app
                .sound_action(SoundAction::Select(mini::MUSIC_ENTRY.into()))
        );
        assert!(f.app.sound.selected.as_ref().unwrap().texts.is_empty());
        frame(&mut f);
        assert!(
            !f.app
                .sound_action(SoundAction::Replace(Some(dir.join("missing.wav"))))
        );
        assert!(
            !f.app
                .sound_action(SoundAction::Select("no\\such.snu".into()))
        );
        assert_eq!(operations(&f.app).len(), 1);
    }

    #[test]
    fn events_categories_and_mixes_are_edited_from_the_forms() {
        let mut f = fixture("tables");
        f.app.sound.open = true;
        // A new record and a changed one.
        f.app.sound.page = Page::Events;
        f.app.sound.event_names = format!("{} DOCTOR_HELLO", mini::SPEECH_EVENT);
        f.app.sound.event_cooldown = "12.5".into();
        f.app.sound.event_flags = "0x80".into();
        assert!(f.app.sound_action(SoundAction::SetEvents));
        frame(&mut f);
        let events = f.app.sound.events.as_ref().unwrap();
        assert_eq!(events.table.events.len(), 2);
        let e = events.table.find(string_id("DOCTOR_HELLO")).unwrap();
        assert_eq!((e.cooldown, e.flags), (12.5, 0x80));
        f.app.sound.event_priority = "many".into();
        assert!(!f.app.sound_action(SoundAction::SetEvents), "not a number");
        f.app.sound.event_priority.clear();
        // A mix category.
        f.app.sound.page = Page::Config;
        f.app.sound.new_category = "DOCTOR_VOICE".into();
        assert!(!f.app.sound_action(SoundAction::AddCategory), "no parent");
        f.app.sound.new_parent = "DIALOG".into();
        assert!(f.app.sound_action(SoundAction::AddCategory));
        frame(&mut f);
        assert_eq!(
            f.app.sound.configs.as_ref().unwrap()[0].1.categories.len(),
            3
        );
        // The mix: a changed channel and a new one.
        f.app.sound.page = Page::Mixes;
        f.app.sound.mix = Some(mini::MIX_NAME.into());
        frame(&mut f);
        frame(&mut f);
        let dialog = string_id("DIALOG");
        let form = f
            .app
            .sound
            .channels
            .get_mut(&dialog)
            .expect("the form of the channel");
        assert_eq!(form.volume, "0.5");
        form.volume = "0.25".into();
        assert_eq!(
            f.app.sound.mix_edit(dialog).unwrap(),
            MixEdit {
                volume: Some(0.25),
                ..MixEdit::default()
            },
            "only what changed"
        );
        assert!(f.app.sound_action(SoundAction::SetMix {
            category: "DIALOG".into()
        }));
        assert!(f.app.sound_action(SoundAction::SetMix {
            category: "DOCTOR_VOICE".into()
        }));
        frame(&mut f);
        let (mix, copies) = f.app.sound.chosen_mix().unwrap();
        assert_eq!(copies, 1);
        assert_eq!(mix.channel(dialog).unwrap().volume(), 0.25);
        assert_eq!(
            mix.channel(string_id("DOCTOR_VOICE")).unwrap().volume(),
            1.0
        );
        assert_eq!(
            operations(&f.app),
            [
                format!("Speech events: {}, DOCTOR_HELLO", mini::SPEECH_EVENT),
                "New mix category DOCTOR_VOICE".to_owned(),
                format!("Mix {}: DIALOG", mini::MIX_NAME),
                format!("Mix {}: DOCTOR_VOICE", mini::MIX_NAME),
            ]
        );
        assert!(f.app.preset_error().is_some(), "the refusals were reported");
    }

    #[test]
    fn the_preset_drives_every_page_and_stops_at_the_first_error() {
        let mut f = fixture("preset");
        let dir = f.dir.clone();
        let new = wav(&dir);
        let out = dir.join("preset.wav");
        f.app.run_sound_preset(&format!(
            "select={},export={},replace={},subtitle=\"en:Well, hello.\",subtitle=es:Hola\\, amigo.,apply",
            mini::SPEECH_ENTRY.replace('\\', "/"),
            out.display(),
            new.display()
        ));
        assert_eq!(f.app.preset_error(), None);
        assert!(out.is_file());
        f.app
            .run_sound_preset("event=DOCTOR_HELLO,cooldown=3,priority=400,apply");
        f.app
            .run_sound_preset("category=DOCTOR_VOICE,parent=DIALOG,apply");
        f.app.run_sound_preset(&format!(
            "mix={},channel=DOCTOR_VOICE,volume=0.75,frequency=8000,apply,channel=DIALOG,time=2,apply",
            mini::MIX_NAME
        ));
        assert_eq!(f.app.preset_error(), None);
        assert_eq!(operations(&f.app).len(), 6);
        frame(&mut f);
        let game = f.app.game.clone().unwrap();
        let ws = f.app.workspace.as_ref().unwrap();
        let data = sound::archive_sound_data(&game, Some(ws), mini::SPEECH_ENTRY).unwrap();
        let texts = sound::sound_info(&data).unwrap().subtitles.unwrap();
        assert!(texts.contains(&("EN".to_owned(), "Well, hello.".to_owned())));
        assert!(texts.contains(&("ES".to_owned(), "Hola, amigo.".to_owned())));
        let mix = &sound::mixes(&game, ws).unwrap()[0].1;
        let voice = mix.channel(string_id("DOCTOR_VOICE")).unwrap();
        assert_eq!((voice.volume(), voice.frequency), (0.75, 8000));
        assert_eq!(
            mix.channel(string_id("DIALOG")).unwrap().levels[LEVEL_TIMES[0]],
            2.0
        );
        // The first failure stops the run: the category after it is not added.
        f.app.run_sound_preset(&format!(
            "mix={},channel=NOPE,volume=0.5,apply,category=AFTER,parent=DIALOG,apply",
            mini::MIX_NAME
        ));
        assert!(f.app.preset_error().is_some());
        assert_eq!(operations(&f.app).len(), 6);
        let mut f = fixture("preset-unknown");
        f.app.run_sound_preset("page=nowhere");
        assert!(f.app.preset_error().unwrap().contains("unknown page"));
    }

    #[test]
    fn the_views_of_the_tables_lead_to_the_window() {
        let mut f = fixture("views");
        let game = f.app.game.clone().unwrap();
        let stream = game.find_stream(mini::SOUND).unwrap();
        for resource in 0..4 {
            let loc = crate::nav::Location { stream, resource };
            let id = f.app.open_resource_tab(loc);
            frame(&mut f);
            frame(&mut f);
            let doc = &f.app.docs[&id];
            assert!(
                doc.views.iter().any(|(name, _)| *name == "sound_table"),
                "resource {resource}"
            );
        }
        let loc = crate::nav::Location {
            stream,
            resource: 3,
        };
        f.app.view_action(
            loc,
            crate::views::ViewAction::OpenSound {
                page: "mixes",
                mix: Some(mini::MIX_NAME.into()),
            },
        );
        frame(&mut f);
        frame(&mut f);
        assert!(f.app.sound.open);
        assert_eq!(f.app.sound.page, Page::Mixes);
        assert_eq!(f.app.sound.chosen_mix().unwrap().0.channels.len(), 1);
    }
}

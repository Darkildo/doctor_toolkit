//! New voice line: pick the speaker's chatter set, give a recording, the
//! speech event, a line name and the subtitles; the sound file, the speech
//! bank entry, the line and (for a new event) its record are recorded as one
//! operation ([`doctor_toolkit::voice::add_line`]).

use std::path::PathBuf;

use doctor_toolkit::resource::chv::{ChatterSet, TYPE_CHV, event_hash};
use doctor_toolkit::resource::smb::{SpeechBank, TYPE_SMB};
use doctor_toolkit::resource::speech::LANGUAGES;
use doctor_toolkit::resource::video::import::read_audio_file;
use doctor_toolkit::voice::{self, NewVoiceLine};
use doctor_toolkit::workspace::ResourceKey;
use doctor_toolkit_mods::i18n::{tr, trf};
use egui::RichText;

use super::{Start, Task, Tx};
use crate::app::BrowserApp;
use crate::nav::Location;

/// Sound files the file dialog offers; anything else goes through ffmpeg.
const SOUND_EXTENSIONS: &[&str] = &["wav", "flac", "ogg", "mp3", "m4a", "aiff"];
/// Events shown at most in the list of the speaker's events.
const SHOWN_EVENTS: usize = 200;

/// A chatter set to choose.
#[derive(Debug, Clone)]
pub(crate) struct Speaker {
    pub loc: Location,
    pub key: ResourceKey,
    /// Voice codes of its lines, the most frequent first (`dmnc`, `mob3`).
    pub voices: Vec<String>,
    /// Event names, sorted.
    pub events: Vec<String>,
    pub lines: usize,
}

impl Speaker {
    fn label(&self) -> String {
        let voice = if self.voices.is_empty() {
            "-".to_owned()
        } else {
            self.voices.join(", ")
        };
        format!(
            "{voice}  ·  {}  ·  {} / {}",
            self.key,
            self.events.len(),
            self.lines
        )
    }
}

pub(crate) struct Voice {
    pub speakers: Vec<Speaker>,
    pub filter: String,
    pub chosen: Option<usize>,
    pub file: String,
    pub event: String,
    pub name: String,
    /// Subtitle of every language column but the line name, in file order.
    pub subtitles: Vec<(String, String)>,
    pub weight: f32,
    pub only: bool,
    pub message: Option<String>,
}

impl Voice {
    pub fn new(app: &mut BrowserApp, start: &Start) -> Self {
        let mut speakers = Vec::new();
        let mut message = None;
        if let Some(game) = app.game.clone() {
            let found = voice::locate(&game, &[TYPE_CHV, TYPE_SMB]);
            let data = |app: &BrowserApp, s: usize, r: usize| {
                app.current_data(Location {
                    stream: s,
                    resource: r,
                })
            };
            // Voice codes come from the file names of the lines.
            let mut files = std::collections::HashMap::new();
            for &(_, s, r) in found.iter().filter(|f| f.0 == TYPE_SMB) {
                if let Ok(bank) = data(app, s, r).and_then(|d| parse(SpeechBank::parse(&d))) {
                    for e in bank.entries {
                        files.insert(e.id[0], e.file);
                    }
                }
            }
            for &(_, s, r) in found.iter().filter(|f| f.0 == TYPE_CHV) {
                let set = match data(app, s, r).and_then(|d| parse(ChatterSet::parse(&d))) {
                    Ok(set) => set,
                    Err(error) => {
                        message = Some(error);
                        continue;
                    }
                };
                let Ok(stream) = game.open_stream(s) else {
                    continue;
                };
                let Some(resource) = stream.layout().resources.get(r) else {
                    continue;
                };
                let key = ResourceKey::new(&game.streams()[s].stream_name(), &resource.header);
                let mut counts = std::collections::BTreeMap::<&str, usize>::new();
                for line in set.events.iter().flat_map(|e| &e.lines) {
                    if let Some(code) = files
                        .get(&line.sound())
                        .and_then(|f| f.strip_prefix("d_"))
                        .and_then(|f| f.split('_').next())
                    {
                        *counts.entry(code).or_default() += 1;
                    }
                }
                let mut counts: Vec<_> = counts.into_iter().collect();
                counts.sort_by_key(|&(code, n)| (std::cmp::Reverse(n), code));
                let mut events: Vec<String> = set.events.iter().map(|e| e.name.clone()).collect();
                events.sort();
                speakers.push(Speaker {
                    loc: Location {
                        stream: s,
                        resource: r,
                    },
                    key,
                    voices: counts
                        .iter()
                        .take(3)
                        .map(|&(code, _)| code.to_owned())
                        .collect(),
                    lines: set.events.iter().map(|e| e.lines.len()).sum(),
                    events,
                });
            }
        }
        let chosen = start
            .loc
            .and_then(|loc| speakers.iter().position(|s| s.loc == loc));
        Self {
            speakers,
            filter: String::new(),
            chosen,
            file: String::new(),
            event: String::new(),
            name: String::new(),
            subtitles: LANGUAGES
                .iter()
                .filter(|&&code| code != "SS" && code != "LL")
                .map(|&code| (code.to_owned(), String::new()))
                .collect(),
            weight: 1.0,
            only: false,
            message,
        }
    }

    pub fn preset(&mut self, key: &str, value: &str, _app: &mut BrowserApp) {
        match key {
            "chv" => {
                self.chosen = self.speakers.iter().position(|s| {
                    s.key.to_string().contains(value) || s.voices.iter().any(|v| v == value)
                });
            }
            "file" => self.file = value.to_owned(),
            "event" => self.event = value.to_owned(),
            "name" => self.name = value.to_owned(),
            "subtitle" => self.subtitles[0].1 = value.to_owned(),
            "only" => self.only = true,
            _ => {}
        }
    }

    fn speaker(&self) -> Option<&Speaker> {
        self.chosen.and_then(|i| self.speakers.get(i))
    }

    /// Whether the event is new for the chosen speaker.
    pub fn event_is_new(&self) -> bool {
        self.speaker().is_some_and(|s| {
            !s.events
                .iter()
                .any(|e| event_hash(e) == event_hash(&self.event))
        })
    }

    fn line(&self) -> Result<NewVoiceLine, String> {
        let speaker = self.speaker().ok_or(tr("wizard.voice.need_speaker"))?;
        Ok(NewVoiceLine {
            chv: speaker.key.clone(),
            event: self.event.trim().to_owned(),
            name: self.name.trim().to_owned(),
            subtitles: self
                .subtitles
                .iter()
                .filter(|(_, text)| !text.trim().is_empty())
                .cloned()
                .collect(),
            weight: self.weight,
            only: self.only,
            category: None,
        })
    }

    fn speaker_step(&mut self, ui: &mut egui::Ui) {
        ui.label(tr("wizard.voice.pick"));
        ui.horizontal(|ui| {
            ui.label(tr("wizard.filter"));
            ui.text_edit_singleline(&mut self.filter);
        });
        if let Some(message) = &self.message {
            ui.label(RichText::new(message).weak());
        }
        let filter = self.filter.to_lowercase();
        egui::ScrollArea::vertical()
            .id_salt("voice-speakers")
            .auto_shrink(false)
            .show(ui, |ui| {
                for (i, speaker) in self.speakers.iter().enumerate() {
                    let label = speaker.label();
                    if !filter.is_empty() && !label.to_lowercase().contains(&filter) {
                        continue;
                    }
                    if ui.selectable_label(self.chosen == Some(i), label).clicked() {
                        self.chosen = Some(i);
                    }
                }
            });
    }

    fn line_step(&mut self, ui: &mut egui::Ui) {
        egui::Grid::new("voice-line").num_columns(2).show(ui, |ui| {
            ui.label(tr("wizard.voice.file"));
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.file).desired_width(360.0));
                if ui.button(tr("wizard.voice.browse")).clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .add_filter(tr("wizard.voice.files_filter"), SOUND_EXTENSIONS)
                        .pick_file()
                {
                    self.file = path.display().to_string();
                }
            });
            ui.end_row();

            ui.label(tr("wizard.voice.event"));
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.event).desired_width(260.0));
                if !self.event.trim().is_empty() {
                    let key = if self.event_is_new() {
                        "wizard.voice.event_new"
                    } else {
                        "wizard.voice.event_known"
                    };
                    ui.label(RichText::new(tr(key)).weak());
                }
            });
            ui.end_row();

            ui.label(tr("wizard.voice.name"));
            ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(260.0))
                .on_hover_text(tr("wizard.voice.name_hint"));
            ui.end_row();

            for (code, text) in &mut self.subtitles {
                ui.label(trf("wizard.voice.subtitle", &[("language", code)]));
                ui.add(egui::TextEdit::singleline(text).desired_width(480.0));
                ui.end_row();
            }

            ui.label(tr("wizard.voice.weight"));
            ui.add(
                egui::DragValue::new(&mut self.weight)
                    .range(0.1..=10.0)
                    .speed(0.1),
            );
            ui.end_row();
        });
        ui.checkbox(&mut self.only, tr("wizard.voice.only"));
        ui.label(RichText::new(tr("wizard.voice.subtitle_hint")).weak());
        ui.separator();
        // The speaker's events: a click takes the name.
        let Some(speaker) = self.chosen.and_then(|i| self.speakers.get(i)) else {
            return;
        };
        ui.label(tr("wizard.voice.events"));
        let typed = self.event.trim().to_lowercase();
        let mut pick = None;
        egui::ScrollArea::vertical()
            .id_salt("voice-events")
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for event in speaker
                        .events
                        .iter()
                        .filter(|e| typed.is_empty() || e.to_lowercase().contains(&typed))
                        .take(SHOWN_EVENTS)
                    {
                        if ui.small_button(event).clicked() {
                            pick = Some(event.clone());
                        }
                    }
                });
            });
        if let Some(event) = pick {
            self.event = event;
        }
    }
}

fn parse<T>(result: doctor_toolkit::Result<T>) -> Result<T, String> {
    result.map_err(|e| e.to_string())
}

impl Task for Voice {
    fn steps(&self) -> &'static [&'static str] {
        &["wizard.voice.step.speaker", "wizard.voice.step.line"]
    }

    fn ui(&mut self, step: usize, ui: &mut egui::Ui, _app: &mut BrowserApp, _tx: &mut Tx) {
        match step {
            0 => self.speaker_step(ui),
            _ => self.line_step(ui),
        }
    }

    fn check(&self, step: usize, _app: &BrowserApp) -> Result<(), String> {
        if step == 0 {
            return match self.speaker() {
                Some(_) => Ok(()),
                None => Err(tr("wizard.voice.need_speaker").to_owned()),
            };
        }
        if !PathBuf::from(self.file.trim()).is_file() {
            return Err(tr("wizard.voice.need_file").to_owned());
        }
        voice::check_event(self.event.trim()).map_err(|e| e.to_string())?;
        voice::check_name(self.name.trim()).map_err(|e| e.to_string())?;
        Ok(())
    }

    fn summary(&self, _app: &BrowserApp) -> Vec<String> {
        let Some(speaker) = self.speaker() else {
            return Vec::new();
        };
        let name = self.name.trim();
        let mut out = vec![
            trf(
                "wizard.voice.summary.line",
                &[("name", &name), ("file", &self.file.trim())],
            ),
            trf(
                "wizard.voice.summary.speaker",
                &[("speaker", &speaker.label()), ("event", &self.event.trim())],
            ),
        ];
        if self.event_is_new() {
            out.push(tr("wizard.voice.summary.new_event").to_owned());
        }
        if self.only {
            out.push(tr("wizard.voice.summary.only").to_owned());
        }
        out.push(trf(
            "wizard.voice.summary.sound",
            &[("entry", &format!("d_{name}.exa.snu"))],
        ));
        for (code, text) in self.subtitles.iter().filter(|(_, t)| !t.trim().is_empty()) {
            out.push(format!("{code}: {text}"));
        }
        out
    }

    fn record(&mut self, app: &mut BrowserApp, _tx: &mut Tx) -> Result<String, String> {
        let line = self.line()?;
        let game = app.game.clone().ok_or(tr("edits.no_game"))?;
        let path = PathBuf::from(self.file.trim());
        let pcm = read_audio_file(&path).map_err(|e| e.to_string())?;
        let label = trf(
            "wizard.voice.label",
            &[("name", &line.name), ("event", &line.event)],
        );
        voice::add_line(&game, app.workspace_mut()?, &pcm, &line, &label)
            .map_err(|e| e.to_string())?;
        Ok(label)
    }
}

//! Views of the sound tables of a stream (stage 103): a speech bank
//! (`smb`), the speech event table (`get`), the audio configuration (`auc`)
//! and a mixer snapshot (`mix`). The view shows what the resource holds;
//! its button opens the page of the Sound window (Tools → Sound) where the
//! tables of the whole game are searched and edited.

use doctor_toolkit::resource::auc::{AudioConfig, TYPE_AUC};
use doctor_toolkit::resource::get::{SpeechEventTable, TYPE_GET};
use doctor_toolkit::resource::mix::{LEVEL_TIMES, Mix, TYPE_MIX};
use doctor_toolkit::resource::smb::{SpeechBank, TYPE_SMB};
use doctor_toolkit_mods::i18n::{tr, trf};
use egui::RichText;

use crate::views::{ResourceViewer, ViewAction, ViewContext, ViewEntry, ViewSource};

/// One sound table, as text.
pub struct SoundTableView {
    /// Page of the Sound window: `banks`, `events`, `config`, `mixes`.
    page: &'static str,
    /// The mix to choose on the page.
    mix: Option<String>,
    title: String,
    lines: Vec<String>,
    error: Option<String>,
}

pub const VIEW: ViewEntry = ViewEntry {
    id: "sound_table",
    load,
};

fn load(source: &mut ViewSource<'_>) -> Option<Box<dyn ResourceViewer>> {
    let page = match source.type_id {
        TYPE_SMB => "banks",
        TYPE_GET => "events",
        TYPE_AUC => "config",
        TYPE_MIX => "mixes",
        _ => return None,
    };
    let mut view = SoundTableView {
        page,
        mix: None,
        title: String::new(),
        lines: Vec::new(),
        error: None,
    };
    let filled = source
        .data(source.index)
        .and_then(|data| view.fill(source.type_id, &data).map_err(|e| e.to_string()));
    if let Err(error) = filled {
        view.error = Some(error);
    }
    Some(Box::new(view))
}

impl SoundTableView {
    fn fill(&mut self, type_id: u32, data: &[u8]) -> doctor_toolkit::Result<()> {
        match type_id {
            TYPE_SMB => {
                let bank = SpeechBank::parse(data)?;
                self.title = trf("sounds.view_bank", &[("n", &bank.entries.len())]);
                self.lines = bank
                    .entries
                    .iter()
                    .map(|e| {
                        format!(
                            "{:08x}  {:<28} flags {:#04x}  {}",
                            e.id[0],
                            e.file,
                            e.flags,
                            e.stream_path(None)
                        )
                    })
                    .collect();
            }
            TYPE_GET => {
                let table = SpeechEventTable::parse(data)?;
                self.title = trf("sounds.view_events", &[("n", &table.events.len())]);
                self.lines = table
                    .events
                    .iter()
                    .map(|e| {
                        format!(
                            "{:08x}  {:>8}  {:>8}  {:>4}  {:#010x}  {:>8}  {:>7}",
                            e.hash, e.cooldown, e.second, e.rank, e.flags, e.priority, e.range
                        )
                    })
                    .collect();
            }
            TYPE_AUC => {
                let config = AudioConfig::parse(data)?;
                self.title = trf(
                    "sounds.view_config",
                    &[
                        ("categories", &config.categories.len()),
                        ("pools", &config.pools.len()),
                        ("ducks", &config.ducks.len()),
                    ],
                );
                self.lines = config
                    .categories
                    .iter()
                    .map(|c| format!("{:08x}  {:<24} {}", c.hash(), c.name, c.parent))
                    .collect();
            }
            _ => {
                let mix = Mix::parse(data)?;
                self.title = trf(
                    "sounds.view_mix",
                    &[("name", &mix.name), ("n", &mix.channels.len())],
                );
                self.lines = mix
                    .channels
                    .iter()
                    .map(|c| {
                        format!(
                            "{:08x}  volume {:<5} time {:<4} {:>6} Hz",
                            c.category,
                            c.volume(),
                            c.levels[LEVEL_TIMES[0]],
                            c.frequency
                        )
                    })
                    .collect();
                self.mix = Some(mix.name);
            }
        }
        Ok(())
    }
}

impl ResourceViewer for SoundTableView {
    fn show(&mut self, ui: &mut egui::Ui, _cx: &mut ViewContext<'_>) -> Option<ViewAction> {
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
            return None;
        }
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            ui.strong(&self.title);
            if ui
                .button(tr("sounds.open_window"))
                .on_hover_text(tr("sounds.open_window_hint"))
                .clicked()
            {
                action = Some(ViewAction::OpenSound {
                    page: self.page,
                    mix: self.mix.clone(),
                });
            }
        });
        let row_height = ui.text_style_height(&egui::TextStyle::Monospace) + 2.0;
        egui::ScrollArea::both()
            .id_salt("sound_table_rows")
            .max_height(320.0)
            .auto_shrink([false, true])
            .show_rows(ui, row_height, self.lines.len(), |ui, range| {
                for line in &self.lines[range] {
                    ui.add(
                        egui::Label::new(RichText::new(line).monospace())
                            .extend()
                            .selectable(true),
                    );
                }
            });
        action
    }
}

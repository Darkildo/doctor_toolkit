//! Structure of an `sbk` sound bank: emitters → instrument tones →
//! variations → samples, with parameter editing and adding sounds.

use doctor_toolkit::resource::audio::Sample;
use doctor_toolkit::resource::names::emitter_names;
use doctor_toolkit::resource::sbk::{SoundBank, cmd, emx_name, flag, name_hash};
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::RichText;

/// What the structure panel asks the bank view to do.
pub enum StructureAction {
    /// Play sample `sample` at `pitch` times its rate.
    #[cfg_attr(not(feature = "audio"), allow(dead_code))]
    Play { sample: usize, pitch: f32 },
    /// Record the edited bank.
    Save { data: Vec<u8>, label: String },
}

/// The structure panel: the parsed bank and a draft with pending edits.
pub struct Structure {
    bank: SoundBank,
    draft: SoundBank,
    selected: usize,
    filter: String,
    /// Fixed pitch and velocity fields of the selected emitter.
    pitch: f32,
    velocity: i32,
    new_name: String,
}

impl Structure {
    pub fn new(bank: SoundBank) -> Self {
        let mut s = Self {
            draft: bank.clone(),
            bank,
            selected: 0,
            filter: String::new(),
            pitch: 1.0,
            velocity: 127,
            new_name: String::new(),
        };
        s.select(0);
        s
    }

    fn select(&mut self, emitter: usize) {
        self.selected = emitter;
        let Some(e) = self.draft.emitters.get(emitter) else {
            return;
        };
        let variations = &e.track.variations;
        self.pitch = variations
            .iter()
            .find_map(SoundBank::variation_pitch)
            .unwrap_or(1.0);
        self.velocity = variations
            .iter()
            .flat_map(|v| &v.commands)
            .find(|c| c.id == cmd::KEY_ON)
            .map_or(127, |c| c.arg32);
    }

    fn dirty(&self) -> bool {
        self.draft != self.bank
    }

    /// The format of an emitter's first sound, for new sounds.
    fn first_header(&self, emitter: usize) -> Option<doctor_toolkit::resource::audio::SnrHeader> {
        let ins = self.draft.instrument_of(emitter)?;
        let tone = self.draft.instruments[ins].tones.first()?;
        let sample = self.draft.samples.get(tone.sample as usize)?;
        Sample::parse_inline(&sample.bytes, 0)
            .ok()
            .map(|s| s.header)
    }

    /// Encodes audio files in the format of an emitter's first sound.
    fn encode(&self, emitter: usize, paths: &[std::path::PathBuf]) -> Result<Vec<Vec<u8>>, String> {
        use doctor_toolkit::resource::audio_encode::encode_like;
        let header = self
            .first_header(emitter)
            .ok_or_else(|| tr("sbk.no_format").to_owned())?;
        paths
            .iter()
            .map(|path| {
                let pcm = doctor_toolkit::resource::video::import::read_audio_file(path)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                encode_like(&pcm, &header)
                    .map(|(_, data)| data)
                    .map_err(|e| e.to_string())
            })
            .collect()
    }

    fn pick_sounds() -> Option<Vec<std::path::PathBuf>> {
        rfd::FileDialog::new()
            .set_title(tr("sbk.add_title"))
            .add_filter(
                tr("sound.filter"),
                &["wav", "wave", "mp3", "ogg", "flac", "aif", "aiff"],
            )
            .add_filter(tr("resource.all_files"), &["*"])
            .pick_files()
    }

    /// Problems of the draft that the saved bank does not have.
    fn new_problems(&self) -> Vec<String> {
        let old = self.bank.validate();
        self.draft
            .validate()
            .into_iter()
            .filter(|p| !old.contains(p))
            .collect()
    }

    fn save(&self, label: String) -> StructureAction {
        StructureAction::Save {
            data: self.draft.to_bytes(),
            label,
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, status: &mut String) -> Option<StructureAction> {
        let mut action = None;
        let named = (0..self.draft.emitters.len())
            .filter(|&i| self.draft.emitter_name(i).is_some())
            .count();
        ui.label(
            RichText::new(trf(
                "sbk.summary",
                &[
                    (
                        "emitters",
                        &trn("sbk.emitters", self.draft.emitters.len() as u64, &[]),
                    ),
                    ("named", &named),
                    ("samples", &self.draft.samples.len()),
                ],
            ))
            .weak(),
        );
        ui.horizontal(|ui| {
            ui.label(tr("sbk.filter"));
            ui.text_edit_singleline(&mut self.filter);
        });
        let filter = self.filter.to_ascii_lowercase();
        let mut select = None;
        egui::ScrollArea::vertical()
            .id_salt("sbk_tree")
            .max_height((ui.available_height() * 0.4).max(120.0))
            .show(ui, |ui| {
                for i in 0..self.draft.emitters.len() {
                    let label = self.draft.emitter_label(i);
                    if !filter.is_empty() && !label.contains(&filter) {
                        continue;
                    }
                    let e = &self.draft.emitters[i];
                    let head = format!(
                        "#{i} {label}  ({})",
                        trn("sbk.variations", e.track.variations.len() as u64, &[])
                    );
                    let response = egui::CollapsingHeader::new(
                        RichText::new(head)
                            .monospace()
                            .strong_if(i == self.selected),
                    )
                    .id_salt(("sbk_emitter", i))
                    .default_open(i == self.selected)
                    .show(ui, |ui| {
                        if let Some(a) = self.emitter_tree(ui, i) {
                            action = Some(a);
                        }
                    });
                    if response.header_response.clicked() {
                        select = Some(i);
                    }
                }
            });
        if let Some(i) = select {
            self.select(i);
        }
        ui.separator();
        if let Some(a) = self.editor(ui, status) {
            action = Some(a);
        }
        action
    }

    /// Children of an emitter in the tree: tones with their samples and
    /// variations with their commands.
    fn emitter_tree(&self, ui: &mut egui::Ui, emitter: usize) -> Option<StructureAction> {
        let mut action = None;
        let bank = &self.draft;
        let Some(ins) = bank.instrument_of(emitter) else {
            ui.label(RichText::new(tr("sbk.no_instrument")).weak());
            return None;
        };
        let tones = &bank.instruments[ins].tones;
        for (v, variation) in bank.emitters[emitter].track.variations.iter().enumerate() {
            let pitch = SoundBank::variation_pitch(variation).unwrap_or(1.0);
            ui.horizontal(|ui| {
                ui.monospace(trf("sbk.variation", &[("index", &v)]));
                for tone in SoundBank::variation_tones(variation) {
                    let Some(t) = tones.get(tone) else { continue };
                    let seconds = bank
                        .samples
                        .get(t.sample as usize)
                        .and_then(|s| Sample::parse_inline(&s.bytes, 0).ok())
                        .map_or(0.0, |s| s.duration());
                    if ui
                        .small_button(trf(
                            "sbk.tone",
                            &[
                                ("tone", &tone),
                                ("sample", &t.sample),
                                ("seconds", &format!("{seconds:.2}")),
                            ],
                        ))
                        .on_hover_text(tr("sbk.tone_hint"))
                        .clicked()
                    {
                        action = Some(StructureAction::Play {
                            sample: t.sample as usize,
                            pitch,
                        });
                    }
                }
                let commands: Vec<String> = variation
                    .commands
                    .iter()
                    .filter(|c| c.id != cmd::NONE && c.id != cmd::KEY_ON)
                    .map(|c| match c.id {
                        cmd::PITCH_BEND | cmd::MOD_PITCH => {
                            format!("{} {:.2}", cmd::name(c.id), c.arg32 as f32 / 65536.0)
                        }
                        _ => format!("+{}ms {}", c.delay, cmd::name(c.id)),
                    })
                    .collect();
                if !commands.is_empty() {
                    ui.label(RichText::new(commands.join(", ")).weak().small());
                }
            });
        }
        action
    }

    /// Parameters of the selected emitter, save, add sounds.
    fn editor(&mut self, ui: &mut egui::Ui, status: &mut String) -> Option<StructureAction> {
        let i = self.selected;
        let label = (i < self.draft.emitters.len()).then(|| self.draft.emitter_label(i))?;
        let mut action = None;
        ui.strong(trf("sbk.selected", &[("index", &i), ("name", &label)]));
        let e = &mut self.draft.emitters[i];
        egui::Grid::new("sbk_params")
            .num_columns(2)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                ui.label(tr("sbk.volume"));
                ui.add(egui::Slider::new(&mut e.track.volume, 0.0..=1.0));
                ui.end_row();
                ui.label(tr("sbk.pitch_range"));
                ui.add(egui::Slider::new(&mut e.track.pitch_range, 0.0..=1.0));
                ui.end_row();
                ui.label(tr("sbk.attn_range"));
                ui.add(egui::Slider::new(&mut e.track.attn_range, 0.0..=1.0));
                ui.end_row();
                ui.label(tr("sbk.falloff"));
                ui.add(
                    egui::DragValue::new(&mut e.falloff)
                        .range(0.01..=10000.0)
                        .speed(0.5),
                );
                ui.end_row();
                ui.label(tr("sbk.priority"));
                ui.add(egui::DragValue::new(&mut e.priority));
                ui.end_row();
                ui.label(tr("sbk.poly_max"));
                ui.add(egui::DragValue::new(&mut e.poly_max).range(1..=255));
                ui.end_row();
                ui.label(tr("sbk.select"))
                    .on_hover_text(tr("sbk.select_hint"));
                ui.add(egui::DragValue::new(&mut e.track.select));
                ui.end_row();
                ui.label(tr("sbk.mix"));
                ui.monospace(format!("{:08x}", e.mix_category));
                ui.end_row();
            });
        ui.horizontal_wrapped(|ui| {
            for (bit, key) in [
                (flag::THREE_D, "sbk.flag_3d"),
                (flag::DOPPLER, "sbk.flag_doppler"),
                (flag::REVERB_FROM_DISTANCE, "sbk.flag_reverb"),
            ] {
                let mut on = e.flags & bit != 0;
                if ui.checkbox(&mut on, tr(key)).changed() {
                    e.flags = if on { e.flags | bit } else { e.flags & !bit };
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label(tr("sbk.pitch"))
                .on_hover_text(tr("sbk.pitch_hint"));
            ui.add(
                egui::DragValue::new(&mut self.pitch)
                    .range(0.1..=4.0)
                    .speed(0.01),
            );
            ui.label(tr("sbk.velocity"));
            ui.add(egui::DragValue::new(&mut self.velocity).range(0..=127));
            if ui.button(tr("sbk.apply_pitch")).clicked() {
                let result = self
                    .draft
                    .set_pitch(i, self.pitch)
                    .and_then(|()| self.draft.set_velocity(i, self.velocity));
                if let Err(error) = result {
                    *status = error.to_string();
                }
            }
        });
        let problems = self.new_problems();
        for problem in &problems {
            ui.colored_label(egui::Color32::RED, problem);
        }
        ui.horizontal(|ui| {
            let dirty = self.dirty();
            if ui
                .add_enabled(
                    dirty && problems.is_empty(),
                    egui::Button::new(tr("sbk.save")),
                )
                .on_hover_text(tr("sbk.save_hint"))
                .clicked()
            {
                action = Some(self.save(trf("sbk.edit_label", &[("name", &label)])));
            }
            if ui
                .add_enabled(dirty, egui::Button::new(tr("sbk.discard")))
                .clicked()
            {
                self.draft = self.bank.clone();
                self.select(i);
            }
            if ui
                .button(tr("sbk.add_sound"))
                .on_hover_text(tr("sbk.add_sound_hint"))
                .clicked()
                && let Some(paths) = Self::pick_sounds()
            {
                let result = self.encode(i, &paths).and_then(|samples| {
                    for sample in samples {
                        self.draft
                            .add_variation(i, sample)
                            .map_err(|e| e.to_string())?;
                    }
                    Ok(())
                });
                match result {
                    Ok(()) => {
                        action = Some(self.save(trf(
                            "sbk.add_label",
                            &[("count", &paths.len()), ("name", &label)],
                        )))
                    }
                    Err(error) => {
                        self.draft = self.bank.clone();
                        *status = trf("sound.cannot_import", &[("error", &error)]);
                    }
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label(tr("sbk.new_name"));
            ui.text_edit_singleline(&mut self.new_name);
            let name = emx_name(&self.new_name);
            let hash = name_hash(&name);
            let taken = self.draft.emitters.iter().any(|e| e.template_id == hash)
                || emitter_names().get(hash).is_some();
            let valid = !self.new_name.trim().is_empty()
                && self
                    .new_name
                    .trim()
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_-.".contains(c));
            if taken {
                ui.colored_label(
                    egui::Color32::RED,
                    trf("sbk.name_taken", &[("name", &name)]),
                );
            } else if valid {
                ui.label(RichText::new(format!("{hash:08x}")).monospace().weak());
            }
            if ui
                .add_enabled(valid && !taken, egui::Button::new(tr("sbk.new_emitter")))
                .on_hover_text(tr("sbk.new_emitter_hint"))
                .clicked()
                && let Some(paths) = Self::pick_sounds()
            {
                let result = self.encode(i, &paths).and_then(|samples| {
                    self.draft
                        .add_emitter(&name, i, samples)
                        .map_err(|e| e.to_string())
                });
                match result {
                    Ok(n) => {
                        action = Some(
                            self.save(trf("sbk.new_label", &[("name", &name), ("index", &n)])),
                        );
                        self.new_name.clear();
                    }
                    Err(error) => {
                        self.draft = self.bank.clone();
                        *status = trf("sound.cannot_import", &[("error", &error)]);
                    }
                }
            }
        });
        action
    }
}

/// Bold text when `on`.
trait StrongIf {
    fn strong_if(self, on: bool) -> Self;
}

impl StrongIf for RichText {
    fn strong_if(self, on: bool) -> Self {
        if on { self.strong() } else { self }
    }
}

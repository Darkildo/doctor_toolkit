//! Detail views of rigs, fonts, sound banks and other smaller
//! resource types.

use doctor_toolkit::game::StreamFile;
use doctor_toolkit::resource::audio::{TYPE_SBK, parse_sbk};
use doctor_toolkit::resource::ffn::{Font, TYPE_FFN};
use doctor_toolkit::resource::hkx::{Packfile, TYPE_HKX};
use doctor_toolkit::resource::rcb::{Rig, Skeleton, TYPE_RCB, translation};
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::RichText;

use crate::collision_view::CollisionPreview;
use crate::texture_view::TexturePreview;

/// One sound of a bank, for display.
pub struct SoundRow {
    index: u32,
    channels: u8,
    sample_rate: u32,
    seconds: f64,
}

/// Sounds of an `sbk` bank with playback and WAV export.
pub struct SoundBankView {
    data: Vec<u8>,
    rows: Vec<SoundRow>,
    name: String,
    /// Emitters, variations and tones; `None` when the metadata does not parse.
    structure: Option<crate::sbk_view::Structure>,
    /// Show the structure (true) or the plain sample list.
    show_structure: bool,
    #[cfg(feature = "audio")]
    playing: Option<(usize, crate::audio_out::Playback)>,
}

impl SoundBankView {
    /// Interleaved PCM of sound `row`.
    fn decode(&self, row: usize) -> Result<(Vec<i16>, u16, u32), String> {
        let sounds = parse_sbk(&self.data).map_err(|e| e.to_string())?;
        let sample = &sounds.get(row).ok_or("no such sound")?.sample;
        let pcm = sample.decode().map_err(|e| e.to_string())?;
        Ok((
            pcm,
            sample.header.channels as u16,
            sample.header.sample_rate,
        ))
    }

    fn show(&mut self, ui: &mut egui::Ui, status: &mut String) -> Option<ResourceAction> {
        let total: f64 = self.rows.iter().map(|r| r.seconds).sum();
        ui.strong(trf(
            "sound.bank_info",
            &[
                ("sounds", &trn("count.sounds", self.rows.len() as u64, &[])),
                ("seconds", &format!("{total:.1}")),
            ],
        ));
        #[cfg(not(feature = "audio"))]
        ui.label(RichText::new(tr("sound.no_playback")).weak().small());
        #[cfg(feature = "audio")]
        if self.playing.as_ref().is_some_and(|(_, p)| p.finished()) {
            self.playing = None;
        }
        if self.structure.is_some() {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.show_structure, true, tr("sbk.tab_structure"));
                ui.selectable_value(&mut self.show_structure, false, tr("sbk.tab_samples"));
                #[cfg(feature = "audio")]
                if self.playing.is_some() && ui.small_button(tr("sbk.stop")).clicked() {
                    self.playing = None;
                }
            });
        }
        if self.show_structure
            && let Some(structure) = &mut self.structure
        {
            match structure.show(ui, status) {
                Some(crate::sbk_view::StructureAction::Save { data, label }) => {
                    return Some(ResourceAction::Save { data, label });
                }
                #[cfg(feature = "audio")]
                Some(crate::sbk_view::StructureAction::Play { sample, pitch }) => {
                    self.playing = None;
                    match self.decode(sample).and_then(|(pcm, channels, rate)| {
                        let rate = (rate as f32 * pitch).round().max(1.0) as u32;
                        crate::audio_out::play(&pcm, channels, rate)
                    }) {
                        Ok(playback) => self.playing = Some((sample, playback)),
                        Err(error) => *status = trf("sound.cannot_play", &[("error", &error)]),
                    }
                }
                #[cfg(not(feature = "audio"))]
                Some(crate::sbk_view::StructureAction::Play { .. }) => {}
                None => {}
            }
            return None;
        }
        let mut action = None;
        egui::ScrollArea::vertical()
            .id_salt("sbk_rows")
            .max_height(300.0)
            .show(ui, |ui| {
                for (i, r) in self.rows.iter().enumerate() {
                    ui.horizontal(|ui| {
                        #[cfg(feature = "audio")]
                        {
                            let playing = self.playing.as_ref().filter(|(row, _)| *row == i);
                            match playing {
                                Some((_, playback)) => {
                                    ui.ctx().request_repaint();
                                    if ui
                                        .small_button(trf(
                                            "sound.stop",
                                            &[(
                                                "percent",
                                                &format!("{:.0}", playback.progress() * 100.0),
                                            )],
                                        ))
                                        .clicked()
                                    {
                                        action = Some(SoundAction::Stop);
                                    }
                                }
                                None => {
                                    if ui.small_button(tr("sound.play")).clicked() {
                                        action = Some(SoundAction::Play(i));
                                    }
                                }
                            }
                        }
                        if ui
                            .small_button("WAV…")
                            .on_hover_text(tr("sound.save_wav_hint"))
                            .clicked()
                        {
                            action = Some(SoundAction::Save(i));
                        }
                        if ui
                            .small_button(tr("sound.import"))
                            .on_hover_text(tr("sound.import_hint"))
                            .clicked()
                        {
                            action = Some(SoundAction::Import(i));
                        }
                        ui.monospace(format!(
                            "{:4}  {} ch  {:5} Hz  {:6.2} s",
                            r.index, r.channels, r.sample_rate, r.seconds
                        ));
                    });
                }
            });
        match action {
            #[cfg(feature = "audio")]
            Some(SoundAction::Play(row)) => {
                self.playing = None;
                match self
                    .decode(row)
                    .and_then(|(pcm, channels, rate)| crate::audio_out::play(&pcm, channels, rate))
                {
                    Ok(playback) => self.playing = Some((row, playback)),
                    Err(error) => *status = trf("sound.cannot_play", &[("error", &error)]),
                }
            }
            #[cfg(feature = "audio")]
            Some(SoundAction::Stop) => self.playing = None,
            Some(SoundAction::Import(row)) => {
                let index = self.rows[row].index;
                if let Some(path) = rfd::FileDialog::new()
                    .set_title(trf("sound.replace_title", &[("index", &index)]))
                    .add_filter(
                        tr("sound.filter"),
                        &["wav", "wave", "mp3", "ogg", "flac", "aif", "aiff"],
                    )
                    .add_filter(tr("resource.all_files"), &["*"])
                    .pick_file()
                {
                    match self.import(row, &path) {
                        Ok(action) => return Some(action),
                        Err(error) => *status = trf("sound.cannot_import", &[("error", &error)]),
                    }
                }
            }
            Some(SoundAction::Save(row)) => {
                let index = self.rows[row].index;
                let wav = self.decode(row).map(|(pcm, channels, rate)| {
                    doctor_toolkit::resource::audio::wav(&pcm, channels, rate)
                });
                let file = format!("{}_{index:04}.wav", self.name);
                *status = match wav {
                    Err(error) => trf("sound.cannot_decode", &[("error", &error)]),
                    Ok(wav) => match rfd::FileDialog::new().set_file_name(&file).save_file() {
                        None => tr("resource.export_cancelled").to_owned(),
                        Some(path) => match std::fs::write(&path, wav) {
                            Ok(()) => trf("resource.saved", &[("path", &path.display())]),
                            Err(error) => trf(
                                "resource.cannot_save",
                                &[("path", &path.display()), ("error", &error)],
                            ),
                        },
                    },
                };
            }
            None => {}
        }
        None
    }
}

impl SoundBankView {
    /// The bank with sound `row` replaced by an audio file, converted to the
    /// sound's channels and rate. The bank is the workspace version, so
    /// earlier pending edits of the same bank are kept.
    fn import(&self, row: usize, path: &std::path::Path) -> Result<ResourceAction, String> {
        use doctor_toolkit::resource::audio_encode::{encode_like, replace_sbk_sound};
        let pcm = doctor_toolkit::resource::video::import::read_audio_file(path)
            .map_err(|e| e.to_string())?;
        let sounds = parse_sbk(&self.data).map_err(|e| e.to_string())?;
        let sound = sounds.get(row).ok_or("no such sound")?;
        let old = sound.sample.header;
        let (fitted, sample) = encode_like(&pcm, &old).map_err(|e| e.to_string())?;
        let data = replace_sbk_sound(&self.data, row, &sample).map_err(|e| e.to_string())?;
        let seconds = fitted.frames() as f64 / old.sample_rate as f64;
        Ok(ResourceAction::Save {
            data,
            label: trf(
                "sound.import_label",
                &[
                    ("index", &sound.index),
                    ("seconds", &format!("{seconds:.2}")),
                ],
            ),
        })
    }
}

/// What the user asked a resource view to do.
pub enum ResourceAction {
    /// Record new data for the resource.
    Save { data: Vec<u8>, label: String },
}

enum SoundAction {
    #[cfg(feature = "audio")]
    Play(usize),
    #[cfg(feature = "audio")]
    Stop,
    Save(usize),
    Import(usize),
}

/// The detail view of one resource.
pub enum ResourceView {
    Rig {
        rig: Rig,
        selected: usize,
    },
    Font {
        font: Box<Font>,
        atlas: Option<Box<TexturePreview>>,
    },
    SoundBank(Box<SoundBankView>),
    Havok(Box<CollisionPreview>),
    Error(String),
}

impl ResourceView {
    /// Loads resource `index` if its type has a view here, its data from
    /// `data` (the workspace version).
    pub fn load_with(
        stream: &StreamFile,
        index: usize,
        data: impl FnOnce(usize) -> Result<Vec<u8>, String>,
    ) -> Option<Self> {
        let type_id = stream.layout().resources[index].header.type_id;
        if ![TYPE_RCB, TYPE_FFN, TYPE_SBK, TYPE_HKX].contains(&type_id) {
            return None;
        }
        let data = match data(index) {
            Ok(data) => data,
            Err(error) => return Some(Self::Error(error)),
        };
        let view = match type_id {
            TYPE_RCB => Rig::parse(&data).map(|rig| Self::Rig { rig, selected: 0 }),
            TYPE_FFN => Font::parse(&data).map(|font| {
                let atlas = font
                    .find_atlas(stream.data(), stream.layout())
                    .map(|t| Box::new(TexturePreview::from_texture(t)));
                Self::Font {
                    font: Box::new(font),
                    atlas,
                }
            }),
            TYPE_SBK => parse_sbk(&data)
                .map(|sounds| {
                    sounds
                        .iter()
                        .map(|s| SoundRow {
                            index: s.index,
                            channels: s.sample.header.channels,
                            sample_rate: s.sample.header.sample_rate,
                            seconds: s.sample.duration(),
                        })
                        .collect::<Vec<_>>()
                })
                .map(|rows| {
                    let structure = doctor_toolkit::resource::sbk::SoundBank::parse(&data)
                        .ok()
                        .map(crate::sbk_view::Structure::new);
                    Self::SoundBank(Box::new(SoundBankView {
                        rows,
                        show_structure: structure.is_some(),
                        structure,
                        data,
                        name: format!("sbk_{index:05}"),
                        #[cfg(feature = "audio")]
                        playing: None,
                    }))
                }),
            TYPE_HKX => {
                Packfile::parse(&data).map(|p| Self::Havok(Box::new(CollisionPreview::new(p))))
            }
            _ => return None,
        };
        Some(view.unwrap_or_else(|error| Self::Error(error.to_string())))
    }

    pub fn show(&mut self, ui: &mut egui::Ui, status: &mut String) -> Option<ResourceAction> {
        if let Self::SoundBank(view) = self {
            return view.show(ui, status);
        }
        match self {
            Self::Error(error) => {
                ui.colored_label(egui::Color32::RED, error.as_str());
            }
            Self::Rig { rig, selected } => show_rig(ui, rig, selected),
            Self::Font { font, atlas } => {
                ui.horizontal_wrapped(|ui| {
                    ui.strong(&font.name);
                    ui.label(
                        RichText::new(trf(
                            "resource.font_info",
                            &[
                                (
                                    "glyphs",
                                    &trn("resource.glyphs", font.glyphs.len() as u64, &[]),
                                ),
                                (
                                    "pages",
                                    &font
                                        .pages
                                        .iter()
                                        .map(|p| format!("U+{p:02X}xx"))
                                        .collect::<Vec<_>>()
                                        .join(" "),
                                ),
                                ("kerning", &font.kerning_count),
                            ],
                        ))
                        .weak(),
                    );
                });
                let chars: String = font.glyphs.iter().filter_map(|g| g.character()).collect();
                ui.label(RichText::new(chars).monospace());
                match atlas {
                    Some(atlas) => {
                        atlas.show(ui, status);
                    }
                    None => {
                        ui.label(RichText::new(tr("resource.no_atlas")).weak());
                    }
                }
            }
            Self::Havok(view) => view.show(ui),
            Self::SoundBank(_) => {}
        }
        None
    }
}

fn show_rig(ui: &mut egui::Ui, rig: &Rig, selected: &mut usize) {
    ui.horizontal_wrapped(|ui| {
        ui.strong(trf("resource.rig", &[("version", &rig.version)]));
        ui.label(RichText::new(trn("resource.bone_sets", rig.bone_sets.len() as u64, &[])).weak());
        for (i, skeleton) in rig.skeletons.iter().enumerate() {
            let label = format!(
                "{} ({})",
                if skeleton.name.is_empty() {
                    format!("{:08x}", skeleton.name_hash)
                } else {
                    skeleton.name.clone()
                },
                trn("count.bones", skeleton.bones.len() as u64, &[])
            );
            ui.selectable_value(selected, i, label);
        }
    });
    let Some(skeleton) = rig.skeletons.get(*selected) else {
        return;
    };
    let named = skeleton.bones.iter().filter(|b| b.name.is_some()).count();
    ui.label(
        RichText::new(trf(
            "resource.bone_names",
            &[("named", &named), ("total", &skeleton.bones.len())],
        ))
        .weak(),
    );
    let positions: Vec<[f32; 3]> = skeleton.bind_globals().iter().map(translation).collect();
    egui::ScrollArea::vertical()
        .id_salt("rig_tree")
        .max_height(360.0)
        .show(ui, |ui| {
            for root in (0..skeleton.bones.len()).filter(|&i| skeleton.bones[i].parent.is_none()) {
                bone_tree(ui, skeleton, &positions, root);
            }
        });
}

fn bone_tree(ui: &mut egui::Ui, skeleton: &Skeleton, positions: &[[f32; 3]], bone: usize) {
    let b = &skeleton.bones[bone];
    let p = positions[bone];
    let text = format!(
        "{bone:3} {}  ({:.3}, {:.3}, {:.3})",
        b.display_name(),
        p[0],
        p[1],
        p[2]
    );
    let children: Vec<usize> = (bone + 1..skeleton.bones.len())
        .filter(|&c| skeleton.bones[c].parent == Some(bone))
        .collect();
    if children.is_empty() {
        ui.monospace(text);
    } else {
        egui::CollapsingHeader::new(RichText::new(text).monospace())
            .id_salt(("bone", bone))
            .default_open(bone < 3)
            .show(ui, |ui| {
                for child in children {
                    bone_tree(ui, skeleton, positions, child);
                }
            });
    }
}

// ------------------------------------------------------------ view registry

/// Rigs, fonts, sound banks (with import) and Havok packfiles.
pub const VIEW: crate::views::ViewEntry = crate::views::ViewEntry {
    id: "resource",
    load: load_view,
};

fn load_view(
    source: &mut crate::views::ViewSource<'_>,
) -> Option<Box<dyn crate::views::ResourceViewer>> {
    let src = &*source;
    ResourceView::load_with(src.stream, src.index, |i| src.data(i))
        .map(|v| Box::new(v) as Box<dyn crate::views::ResourceViewer>)
}

impl crate::views::ResourceViewer for ResourceView {
    fn show(
        &mut self,
        ui: &mut egui::Ui,
        cx: &mut crate::views::ViewContext<'_>,
    ) -> Option<crate::views::ViewAction> {
        ResourceView::show(self, ui, cx.status).map(|action| match action {
            ResourceAction::Save { data, label } => {
                crate::views::ViewAction::SaveData { data, label }
            }
        })
    }
}

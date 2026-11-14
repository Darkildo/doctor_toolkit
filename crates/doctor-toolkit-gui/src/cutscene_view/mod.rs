//! Cut-scene editor for `trinity_seq_master` shots (`docs/formats/cutscenes.md`):
//! the whole sequence the shot belongs to on a time line (shots, camera,
//! actors and their clips, effects, script commands, fades, sound tracks and
//! subtitles), a 3D preview at the play head and an inspector of the selected
//! item. Moving an item, swapping a clip, editing camera keys, commands or
//! subtitles records the shot (or the sound file) in the workspace, so undo
//! and redo work as for any edit. Play plays the sound tracks with the
//! picture (`sound.rs`).
//!
//! The preview shows the level around the cut-scene (`world.rs`, stage 61)
//! unless the World switch is off.
//!
//! `DOCTOR_TOOLKIT_GUI_CUTSCENE=<options>` presets the view for screenshots:
//! comma-separated `time=<s>`, `load` (characters), `free` (orbit camera),
//! `select=<shot>:<command>` or `select=sub:<line>`, `zoom=<pt per s>`,
//! `scroll=<s>`, `play`, `noworld` (actors only), `hour=<h>` (time of day),
//! `preview=<height share>` (0.3–0.9 of the view for the picture).

mod preview;
pub(crate) mod sound;
mod timeline;
pub(crate) mod world;

/// Whether a cut-scene preview is still loading its world (screenshots wait).
pub fn busy() -> bool {
    world::busy()
}

use std::any::Any;
use std::sync::Arc;

use doctor_toolkit::cutscene::{Cutscene, Source};
use doctor_toolkit::resource::trinity::{Command, CurveKey, FixedText, TYPE_TRINITY_SEQ_MASTER};
use doctor_toolkit::workspace::FileKey;
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::RichText;

use crate::views::{ResourceViewer, ViewAction, ViewContext, ViewEntry, ViewSource};

use preview::{Preview, Shared};
use sound::{Clock, SoundPlayer, TrackSource};
use timeline::{ItemId, TimelineState, Track};

pub const VIEW: ViewEntry = ViewEntry {
    id: "cutscene",
    load,
};

fn load(source: &mut ViewSource<'_>) -> Option<Box<dyn ResourceViewer>> {
    if source.type_id != TYPE_TRINITY_SEQ_MASTER {
        return None;
    }
    let cutscene = Cutscene::load(
        &Source::new(source.game, source.workspace),
        source.stream_index,
        Some(source.index),
    );
    // The sound tracks, as the workspace has them.
    let mut sounds = Vec::new();
    let mut sources = Vec::new();
    if let Ok(c) = &cutscene {
        let s = Source::new(source.game, source.workspace);
        for a in &c.audio {
            let key = FileKey::archive_entry(&a.entry);
            let data = s.file(&key).ok().map(Arc::new);
            if let (Some(data), Some(start)) = (&data, a.placement.start) {
                sources.push(TrackSource {
                    data: data.clone(),
                    start,
                    volume: a.placement.volume,
                });
            }
            sounds.push(data.map(|d| (key, d)));
        }
    }
    let mut view = CutsceneView {
        resource: source.index,
        tracks: cutscene.as_ref().map(timeline::tracks).unwrap_or_default(),
        cutscene: cutscene.map_err(|e| e.to_string()),
        sounds,
        sound: SoundPlayer::new(sources),
        timeline: TimelineState::default(),
        preview: Preview::new(source.game.root().to_owned(), source.gpu.cloned()),
        playing: false,
        draft: None,
        message: String::new(),
        preview_share: 0.5,
        zone_filter: String::new(),
        fps: 0.0,
    };
    view.preview.world = Arc::new(std::sync::Mutex::new(world::World::new(world::Edits::of(
        source.workspace,
    ))));
    if let Ok(c) = &view.cutscene
        && let Some(shot) = c.shots.iter().find(|s| s.resource == source.index)
    {
        view.timeline.playhead = shot.start;
        view.timeline.scroll = (shot.start - 1.0).max(0.0);
    }
    if let Ok(preset) = std::env::var("DOCTOR_TOOLKIT_GUI_CUTSCENE") {
        view.apply_preset(&preset);
    }
    Some(Box::new(view))
}

/// What a draft edit of the selected item holds.
#[derive(Clone)]
enum Draft {
    Command(ItemId, Command, f32),
    Subtitle(ItemId, u32, u32),
}

struct CutsceneView {
    /// The shot opened.
    resource: usize,
    cutscene: Result<Cutscene, String>,
    tracks: Vec<Track>,
    /// Data of the sound tracks (`None` when it cannot be read).
    sounds: Vec<Option<(FileKey, Arc<Vec<u8>>)>>,
    sound: SoundPlayer,
    timeline: TimelineState,
    preview: Preview,
    playing: bool,
    draft: Option<Draft>,
    message: String,
    /// Share of the view's height the picture takes.
    preview_share: f32,
    zone_filter: String,
    /// Frames per second while playing (smoothed).
    fps: f32,
}

/// What survives a reload after an edit.
struct State {
    timeline: TimelineState,
    shared: Shared,
    shot_camera: bool,
    characters: bool,
    orbit: crate::render::OrbitCamera,
    world: world::SharedWorld,
    hour: Option<f32>,
    preview_share: f32,
}

impl CutsceneView {
    fn apply_preset(&mut self, preset: &str) {
        for item in preset.split(',').map(str::trim) {
            let (name, value) = item.split_once('=').unwrap_or((item, ""));
            match name {
                "time" => self.timeline.playhead = value.parse().unwrap_or(0.0),
                "zoom" => self.timeline.zoom = value.parse().unwrap_or(self.timeline.zoom),
                "scroll" => self.timeline.scroll = value.parse().unwrap_or(0.0),
                "load" => self.preview.load(),
                "free" => self.preview.shot_camera = false,
                "play" => self.playing = true,
                "noworld" => self.set_world(false),
                "hour" => self.preview.hour = value.parse().ok(),
                "preview" => self.preview_share = value.parse().unwrap_or(0.5f32).clamp(0.3, 0.9),
                "select" => {
                    let parts: Vec<&str> = value.split(':').collect();
                    self.timeline.selected = match parts.as_slice() {
                        ["sub", line] => line.parse().ok().map(|line| ItemId::Subtitle {
                            track: self.subtitle_track().unwrap_or(0),
                            line,
                        }),
                        [shot, index] => shot
                            .parse()
                            .ok()
                            .zip(index.parse().ok())
                            .map(|(shot, index)| ItemId::Command { shot, index }),
                        _ => None,
                    };
                }
                _ => {}
            }
        }
    }

    fn set_world(&mut self, on: bool) {
        self.preview
            .world
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .enabled = on;
        self.preview.invalidate();
    }

    /// The world switch, the zones it shows, the time of day and how the
    /// load goes.
    fn world_bar(&mut self, ui: &mut egui::Ui, cutscene: &Cutscene) {
        let world = self.preview.world.clone();
        let mut w = world.lock().unwrap_or_else(|e| e.into_inner());
        let mut on = w.enabled;
        if ui
            .checkbox(&mut on, tr("cutscene.world"))
            .on_hover_text(tr("cutscene.world_hint"))
            .changed()
        {
            w.enabled = on;
            self.preview.invalidate();
        }
        if !on {
            return;
        }
        let ctx = ui.ctx().clone();
        ui.menu_button(trf("cutscene.zones", &[("count", &w.loaded.len())]), |ui| {
            ui.set_max_width(520.0);
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.zone_filter)
                        .hint_text(tr("cutscene.zones_filter"))
                        .desired_width(220.0),
                );
                if ui.button(tr("cutscene.zones_auto")).clicked() {
                    w.chosen = w.auto.clone();
                }
                if ui
                    .add_enabled(
                        w.chosen != w.loaded && !w.is_loading(),
                        egui::Button::new(tr("cutscene.zones_apply")),
                    )
                    .clicked()
                {
                    w.reload(&ctx);
                    self.preview.invalidate();
                }
            });
            ui.label(RichText::new(tr("cutscene.zones_hint")).weak());
            let filter = self.zone_filter.to_lowercase();
            let listed: Vec<(usize, String, &'static str, f32, bool)> = w
                .listed()
                .into_iter()
                .filter(|z| filter.is_empty() || z.name.contains(&filter))
                .take(300)
                .map(|z| {
                    (
                        z.stream,
                        z.name.clone(),
                        world::kind_name(z.kind),
                        w.distances.get(&z.stream).copied().unwrap_or(f32::MAX),
                        w.auto.contains(&z.stream),
                    )
                })
                .collect();
            egui::ScrollArea::vertical()
                .id_salt("cutscene zones")
                .max_height(360.0)
                .show(ui, |ui| {
                    for (stream, name, kind, distance, auto) in listed {
                        let mut checked = w.chosen.contains(&stream);
                        ui.horizontal(|ui| {
                            if ui.checkbox(&mut checked, &name).changed() {
                                if checked {
                                    w.chosen.insert(stream);
                                } else {
                                    w.chosen.remove(&stream);
                                }
                            }
                            let mut text = kind.to_owned();
                            if distance < f32::MAX {
                                text += &format!(", {distance:.0} m");
                            }
                            if auto {
                                text += ", auto";
                            }
                            ui.label(RichText::new(text).weak());
                        });
                    }
                });
        });
        // Time of day: the cut-scene's, or one chosen.
        let t = self.timeline.playhead;
        let (command, source) = doctor_toolkit::cutscene::world::time_at(cutscene, t);
        let automatic = preview::time_of_day(cutscene, t, w.mission_time);
        let why = match (command, source) {
            (Some(_), doctor_toolkit::cutscene::world::TimeSource::Command { shot, .. }) => trf(
                "cutscene.time_command",
                &[("shot", &cutscene.shots[shot].seq.source_name())],
            ),
            _ if w.mission_time.is_some() => tr("cutscene.time_mission").into(),
            _ => tr("cutscene.time_default").into(),
        };
        let current = match self.preview.hour {
            Some(h) => format!("{:02}:{:02}", h as u32, ((h.fract()) * 60.0) as u32),
            None => trf(
                "cutscene.time_auto",
                &[("time", &automatic.to_string()), ("why", &why)],
            ),
        };
        let before = self.preview.hour;
        egui::ComboBox::from_id_salt("cutscene time of day")
            .selected_text(current)
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut self.preview.hour,
                    None,
                    trf(
                        "cutscene.time_auto",
                        &[("time", &automatic.to_string()), ("why", &why)],
                    ),
                );
                for h in [0.0f32, 6.0, 9.0, 12.0, 16.0, 19.0, 22.0] {
                    ui.selectable_value(&mut self.preview.hour, Some(h), format!("{h:02}:00"));
                }
            })
            .response
            .on_hover_text(tr("cutscene.time_hint"));
        if before != self.preview.hour {
            self.preview.invalidate();
        }
        // How the load goes.
        if let Some(e) = &w.error {
            ui.colored_label(
                egui::Color32::RED,
                trf("cutscene.world_error", &[("error", e)]),
            );
        } else if w.is_loading() {
            ui.spinner();
            let text = match w.progress {
                Some((done, total)) => trf(
                    "cutscene.world_meshes",
                    &[("done", &done), ("total", &total)],
                ),
                None => tr("cutscene.world_zones").into(),
            };
            ui.label(RichText::new(text).weak());
        } else if w.is_ready() {
            let timings: Vec<String> = w
                .timings
                .iter()
                .map(|(name, d)| format!("{name} {:.2} s", d.as_secs_f32()))
                .collect();
            ui.label(RichText::new(format!("{} ({})", w.status, timings.join(", "))).weak());
        }
        if self.playing && self.fps > 0.0 {
            ui.label(
                RichText::new(trf("cutscene.fps", &[("fps", &format!("{:.0}", self.fps))])).weak(),
            );
        }
    }

    fn subtitle_track(&self) -> Option<usize> {
        let cutscene = self.cutscene.as_ref().ok()?;
        cutscene.audio.iter().position(|a| a.tob.is_some())
    }

    /// The edited shot as an action: the opened resource or another shot.
    fn save_shot(
        &self,
        cutscene: &Cutscene,
        s: usize,
        seq: &doctor_toolkit::resource::trinity::SeqMaster,
        label: String,
    ) -> Result<ViewAction, String> {
        let data = seq.to_bytes().map_err(|e| e.to_string())?;
        let resource = cutscene.shots[s].resource;
        if resource == self.resource {
            return Ok(ViewAction::SaveData { data, label });
        }
        // Another shot of the sequence: a resource of the same stream.
        let stream = cutscene.stream;
        let text = label.clone();
        Ok(ViewAction::Apply {
            label,
            run: Box::new(move |game, ws| {
                ws.set_resource(game, stream, resource, data, &text)
                    .map(|_| ())
            }),
        })
    }

    /// A subtitle table edit as an action.
    fn save_subtitle(
        &self,
        cutscene: &Cutscene,
        track: usize,
        line: usize,
        start_ms: u32,
        duration_ms: u32,
    ) -> Result<ViewAction, String> {
        let audio = &cutscene.audio[track];
        let sub = &audio.subtitles[line];
        let mut tob = audio.tob.clone().ok_or("no subtitle table")?;
        let group = &mut tob.groups[sub.group_index];
        group.entries[sub.entry].start_ms = start_ms;
        group.entries[sub.entry].duration_ms = duration_ms;
        group.entries.sort_by_key(|e| e.start_ms);
        let (key, snu) = self.sounds[track]
            .as_ref()
            .ok_or("the sound was not read")?;
        let data = tob.replace_in_snu(snu).map_err(|e| e.to_string())?;
        let label = trf(
            "cutscene.label_subtitle",
            &[
                ("line", &line),
                ("start", &format!("{:.3}", start_ms as f32 / 1000.0)),
            ],
        );
        let (key, text) = (key.clone(), label.clone());
        Ok(ViewAction::Apply {
            label,
            run: Box::new(move |game, ws| ws.set_file(game, &key, data, &text).map(|_| ())),
        })
    }

    /// Moves an item of the time line to `start` (seconds from the start of the cut-scene).
    fn move_item(&self, cutscene: &Cutscene, id: ItemId, start: f32) -> Option<ViewAction> {
        match id {
            ItemId::Command { shot, index } => {
                let s = &cutscene.shots[shot];
                let mut seq = s.seq.clone();
                let label = format!(
                    "{}: {}",
                    s.seq.source_name(),
                    seq.blocks[0].commands[index].label()
                );
                let local = (start - s.start).clamp(0.0, s.duration());
                seq.blocks[0].move_command(index, local).ok()?;
                self.save_shot(
                    cutscene,
                    shot,
                    &seq,
                    trf(
                        "cutscene.label_move",
                        &[("what", &label), ("time", &format!("{local:.3}"))],
                    ),
                )
                .ok()
            }
            ItemId::Subtitle { track, line } => {
                let sub = &cutscene.audio[track].subtitles[line];
                self.save_subtitle(
                    cutscene,
                    track,
                    line,
                    (start * 1000.0).round().max(0.0) as u32,
                    sub.duration_ms,
                )
                .ok()
            }
            _ => None,
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, cutscene: &Cutscene) {
        ui.horizontal_wrapped(|ui| {
            ui.strong(trf("cutscene.title", &[("name", &cutscene.name)]));
            ui.label(
                RichText::new(trf(
                    "cutscene.summary",
                    &[
                        (
                            "shots",
                            &trn("cutscene.shots", cutscene.shots.len() as u64, &[]),
                        ),
                        ("seconds", &format!("{:.1}", cutscene.duration())),
                    ],
                ))
                .weak(),
            );
            if let Some(seq) = &cutscene.sequence {
                ui.label(
                    RichText::new(trf(
                        "cutscene.messages",
                        &[
                            ("start", &seq.start_message.as_deref().unwrap_or("-")),
                            ("end", &seq.end_message.as_deref().unwrap_or("-")),
                        ],
                    ))
                    .weak(),
                );
            }
        });
        ui.horizontal(|ui| {
            let play = if self.playing {
                tr("cutscene.pause")
            } else {
                tr("cutscene.play")
            };
            if ui.button(play).clicked() {
                self.playing = !self.playing;
                if self.playing && self.timeline.playhead >= cutscene.duration() {
                    self.timeline.playhead = 0.0;
                }
            }
            if ui
                .button("|<")
                .on_hover_text(tr("cutscene.previous_shot"))
                .clicked()
                && let Some(s) = cutscene.shot_at(self.timeline.playhead)
            {
                let shot = &cutscene.shots[s];
                self.timeline.playhead = if self.timeline.playhead - shot.start > 0.2 || s == 0 {
                    shot.start
                } else {
                    cutscene.shots[s - 1].start
                };
            }
            if ui
                .button(">|")
                .on_hover_text(tr("cutscene.next_shot"))
                .clicked()
                && let Some(s) = cutscene.shot_at(self.timeline.playhead)
                && s + 1 < cutscene.shots.len()
            {
                self.timeline.playhead = cutscene.shots[s + 1].start;
            }
            ui.add(
                egui::DragValue::new(&mut self.timeline.playhead)
                    .range(0.0..=cutscene.duration())
                    .speed(0.02)
                    .suffix(tr("cutscene.unit_s")),
            );
            if let Some(s) = cutscene.shot_at(self.timeline.playhead) {
                ui.label(trf(
                    "cutscene.shot_at",
                    &[("shot", &cutscene.shots[s].seq.source_name())],
                ));
            }
            self.sound.controls(ui);
            ui.separator();
            ui.checkbox(&mut self.preview.shot_camera, tr("cutscene.shot_camera"));
            if !self.preview.is_loaded()
                && self.preview.has_gpu()
                && ui
                    .button(tr("cutscene.load_characters"))
                    .on_hover_text(tr("cutscene.load_characters_hint"))
                    .clicked()
            {
                self.preview.load();
            }
        });
        if self.preview.has_gpu() {
            ui.horizontal_wrapped(|ui| self.world_bar(ui, cutscene));
        }
    }

    fn inspector(&mut self, ui: &mut egui::Ui, cutscene: &Cutscene) -> Option<ViewAction> {
        let selected = self.timeline.selected?;
        let mut action = None;
        match selected {
            ItemId::Shot(s) | ItemId::Command { shot: s, .. } => {
                let shot = &cutscene.shots[s];
                ui.strong(shot.seq.source_name());
                ui.label(RichText::new(&shot.seq.path.text).weak().small());
                ui.label(trf(
                    "cutscene.shot_span",
                    &[
                        ("start", &format!("{:.3}", shot.start)),
                        ("end", &format!("{:.3}", shot.end())),
                    ],
                ));
                if let ItemId::Command { index, .. } = selected {
                    action = self.command_inspector(ui, cutscene, s, index);
                } else {
                    for p in self.preview.problems(cutscene, s) {
                        ui.colored_label(egui::Color32::YELLOW, p);
                    }
                }
            }
            ItemId::Subtitle { track, line } => {
                let sub = &cutscene.audio[track].subtitles[line];
                ui.strong(trf(
                    "cutscene.subtitle",
                    &[("line", &line), ("group", &sub.group)],
                ));
                ui.label(RichText::new(format!("{:08x}", sub.string)).monospace());
                if let Some(text) = &sub.text {
                    ui.label(text);
                }
                let fresh =
                    !matches!(&self.draft, Some(Draft::Subtitle(id, ..)) if *id == selected);
                if fresh {
                    self.draft = Some(Draft::Subtitle(selected, sub.start_ms, sub.duration_ms));
                }
                if let Some(Draft::Subtitle(_, start, duration)) = &mut self.draft {
                    egui::Grid::new("subtitle fields")
                        .num_columns(2)
                        .show(ui, |ui| {
                            ui.label(tr("cutscene.start_ms"));
                            ui.add(
                                egui::DragValue::new(start)
                                    .speed(10.0)
                                    .suffix(tr("cutscene.unit_ms")),
                            );
                            ui.end_row();
                            ui.label(tr("cutscene.duration_ms"));
                            ui.add(
                                egui::DragValue::new(duration)
                                    .speed(10.0)
                                    .suffix(tr("cutscene.unit_ms")),
                            );
                            ui.end_row();
                        });
                    let (start, duration) = (*start, *duration);
                    if ui.button(tr("cutscene.apply")).clicked() {
                        match self.save_subtitle(cutscene, track, line, start, duration) {
                            Ok(a) => action = Some(a),
                            Err(e) => self.message = e,
                        }
                    }
                }
            }
            ItemId::Sound(a) => {
                let audio = &cutscene.audio[a];
                ui.strong(&audio.entry);
                ui.label(trf(
                    "cutscene.sound",
                    &[
                        (
                            "seconds",
                            &format!("{:.2}", audio.duration_ms as f32 / 1000.0),
                        ),
                        (
                            "channels",
                            &trn("sound.channels", audio.channels as u64, &[]),
                        ),
                    ],
                ));
                let p = &audio.placement;
                let volume = format!("{:.0}", p.volume * 100.0);
                let text = match (p.start, &p.message) {
                    (Some(_), None) => trf("cutscene.sound_own", &[("volume", &volume)]),
                    (Some(start), Some(message)) => trf(
                        "cutscene.sound_cue",
                        &[
                            ("start", &format!("{start:.3}")),
                            ("message", message),
                            ("volume", &volume),
                        ],
                    ),
                    (None, message) => trf(
                        "cutscene.sound_outside",
                        &[("message", &message.as_deref().unwrap_or("?"))],
                    ),
                };
                ui.label(text);
                if audio.tob.is_some() {
                    ui.label(RichText::new(tr("cutscene.sound_hint")).weak().small());
                }
            }
        }
        action
    }

    fn command_inspector(
        &mut self,
        ui: &mut egui::Ui,
        cutscene: &Cutscene,
        s: usize,
        index: usize,
    ) -> Option<ViewAction> {
        let shot = &cutscene.shots[s];
        let block = shot.block();
        let command = block.commands.get(index)?;
        let time = block.times()[index];
        let id = ItemId::Command { shot: s, index };
        if !matches!(&self.draft, Some(Draft::Command(d, ..)) if *d == id) {
            self.draft = Some(Draft::Command(id, command.clone(), time));
        }
        let Some(Draft::Command(_, draft, draft_time)) = &mut self.draft else {
            return None;
        };
        ui.separator();
        ui.label(RichText::new(command.label()).strong());
        if let Some(actor) = command.actor().and_then(|g| shot.actor(&g)) {
            ui.label(
                RichText::new(format!(
                    "{} — {}{}",
                    actor.name,
                    actor.class.as_deref().unwrap_or("?"),
                    actor
                        .assembly
                        .as_deref()
                        .map(|a| format!(", {a} / {}", actor.set.as_deref().unwrap_or("-")))
                        .unwrap_or_default()
                ))
                .weak(),
            );
        }
        egui::Grid::new("command time")
            .num_columns(2)
            .show(ui, |ui| {
                ui.label(tr("cutscene.time_in_shot"));
                ui.add(
                    egui::DragValue::new(draft_time)
                        .range(0.0..=shot.duration())
                        .speed(0.01)
                        .suffix(tr("cutscene.unit_s")),
                );
                ui.end_row();
            });
        let mut changed = false;
        match &mut *draft {
            Command::ActorAnim { clip, speed, .. } => {
                egui::Grid::new("anim fields")
                    .num_columns(2)
                    .show(ui, |ui| {
                        ui.label(tr("cutscene.clip"));
                        egui::ComboBox::from_id_salt("cutscene clip")
                            .selected_text(&clip.text)
                            .height(300.0)
                            .show_ui(ui, |ui| {
                                for c in &shot.clips {
                                    let name =
                                        c.name.clone().unwrap_or_else(|| format!("{:08x}", c.hash));
                                    if ui
                                        .selectable_label(
                                            name.eq_ignore_ascii_case(&clip.text),
                                            format!(
                                                "{name} ({})",
                                                trn("count.frames", c.frames as u64, &[])
                                            ),
                                        )
                                        .clicked()
                                    {
                                        *clip = FixedText::new(name);
                                        changed = true;
                                    }
                                }
                            });
                        ui.end_row();
                        ui.label(tr("cutscene.speed"));
                        changed |= ui.add(egui::DragValue::new(speed).speed(0.01)).changed();
                        ui.end_row();
                    });
                if shot.clip(&clip.text).is_none() {
                    ui.colored_label(egui::Color32::YELLOW, tr("cutscene.clip_missing"));
                }
            }
            Command::CameraCurves { curves, .. } => {
                const CHANNELS: [&str; 6] = ["x", "y", "z", "rot x", "rot y", "rot z"];
                egui::ScrollArea::vertical()
                    .id_salt("camera keys")
                    .max_height(260.0)
                    .show(ui, |ui| {
                        for (c, keys) in curves.iter_mut().enumerate() {
                            egui::CollapsingHeader::new(format!(
                                "{} ({})",
                                CHANNELS[c],
                                keys.len()
                            ))
                            .id_salt(("camera channel", c))
                            .default_open(c < 3)
                            .show(ui, |ui| changed |= key_table(ui, keys, ("camera", c)));
                        }
                    });
            }
            Command::CameraFov { keys, .. } => {
                changed |= key_table(ui, keys, ("fov", 0));
            }
            Command::Script { kind, text, .. } => {
                egui::Grid::new("script fields")
                    .num_columns(2)
                    .show(ui, |ui| {
                        ui.label(tr("cutscene.kind"));
                        egui::ComboBox::from_id_salt("script kind")
                            .selected_text(script_kind(*kind))
                            .show_ui(ui, |ui| {
                                for k in 0..4u8 {
                                    changed |=
                                        ui.selectable_value(kind, k, script_kind(k)).changed();
                                }
                            });
                        ui.end_row();
                        ui.label(tr("cutscene.text"));
                        if ui.text_edit_singleline(&mut text.text).changed() {
                            text.slack = None;
                            changed = true;
                        }
                        ui.end_row();
                    });
            }
            Command::Fade {
                mode,
                seconds,
                color,
            } => {
                egui::Grid::new("fade fields")
                    .num_columns(2)
                    .show(ui, |ui| {
                        ui.label(tr("cutscene.mode"));
                        changed |= ui.add(egui::DragValue::new(mode).range(0..=1)).changed();
                        ui.end_row();
                        ui.label(tr("cutscene.seconds"));
                        changed |= ui
                            .add(egui::DragValue::new(seconds).speed(0.05).range(0.0..=60.0))
                            .changed();
                        ui.end_row();
                        ui.label(tr("cutscene.colour"));
                        changed |= ui.color_edit_button_rgba_unmultiplied(color).changed();
                        ui.end_row();
                    });
            }
            Command::VfxState { state, .. } => {
                ui.horizontal(|ui| {
                    ui.label(tr("cutscene.state"));
                    changed |= ui.add(egui::DragValue::new(state).range(0..=4)).changed();
                });
            }
            Command::ActorTransform {
                positions,
                rotations,
                ..
            } => {
                for (k, (p, r)) in positions.iter_mut().zip(rotations.iter_mut()).enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(trf(
                            "cutscene.at_seconds",
                            &[("time", &format!("{:.3}", p.time))],
                        ));
                        for v in &mut p.position {
                            changed |= ui.add(egui::DragValue::new(v).speed(0.01)).changed();
                        }
                        ui.label(
                            RichText::new(format!(
                                "q {:.3} {:.3} {:.3} {:.3}",
                                r.rotation[0], r.rotation[1], r.rotation[2], r.rotation[3]
                            ))
                            .weak()
                            .small(),
                        );
                    });
                    let _ = k;
                }
            }
            other => {
                let json = serde_json::to_string_pretty(&*other).unwrap_or_default();
                ui.label(RichText::new(json).monospace().small());
            }
        }
        let moved = (*draft_time - time).abs() > 1e-5;
        let edited = changed || *draft != *command;
        let mut apply = false;
        ui.horizontal(|ui| {
            apply = ui
                .add_enabled(edited || moved, egui::Button::new(tr("cutscene.apply")))
                .clicked();
            if ui
                .add_enabled(edited || moved, egui::Button::new(tr("cutscene.reset")))
                .clicked()
            {
                *draft = command.clone();
                *draft_time = time;
            }
        });
        if !apply {
            return None;
        }
        let (draft, draft_time) = (draft.clone(), *draft_time);
        let mut seq = shot.seq.clone();
        let label = command.label();
        seq.blocks[0].commands[index] = draft;
        if moved {
            let _ = seq.blocks[0].move_command(index, draft_time);
        }
        let label = trf(
            "cutscene.label_edit",
            &[("what", &label), ("shot", &shot.seq.source_name())],
        );
        match self.save_shot(cutscene, s, &seq, label) {
            Ok(a) => Some(a),
            Err(e) => {
                self.message = e;
                None
            }
        }
    }
}

fn script_kind(kind: u8) -> &'static str {
    match kind {
        0 => tr("cutscene.kind_time"),
        1 => tr("cutscene.kind_message"),
        2 => tr("cutscene.kind_shadows"),
        3 => tr("cutscene.kind_time_scale"),
        _ => "?",
    }
}

/// An editable table of curve keys; returns whether something changed.
fn key_table(ui: &mut egui::Ui, keys: &mut [CurveKey], salt: (&str, usize)) -> bool {
    let mut changed = false;
    egui::Grid::new(("curve keys", salt.0, salt.1))
        .num_columns(4)
        .striped(true)
        .show(ui, |ui| {
            ui.label(tr("cutscene.key_time"));
            ui.label(tr("cutscene.key_value"));
            ui.label(tr("cutscene.key_in"));
            ui.label(tr("cutscene.key_out"));
            ui.end_row();
            for k in keys.iter_mut() {
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut k.time)
                            .speed(0.01)
                            .max_decimals(3),
                    )
                    .changed();
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut k.value)
                            .speed(0.01)
                            .max_decimals(4),
                    )
                    .changed();
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut k.in_tangent)
                            .speed(0.001)
                            .max_decimals(4),
                    )
                    .changed();
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut k.out_tangent)
                            .speed(0.001)
                            .max_decimals(4),
                    )
                    .changed();
                ui.end_row();
            }
        });
    changed
}

impl ResourceViewer for CutsceneView {
    fn show(&mut self, ui: &mut egui::Ui, cx: &mut ViewContext<'_>) -> Option<ViewAction> {
        let cutscene = match std::mem::replace(&mut self.cutscene, Err(String::new())) {
            Ok(c) => c,
            Err(e) => {
                ui.colored_label(egui::Color32::RED, trf("cutscene.error", &[("error", &e)]));
                self.cutscene = Err(e);
                return None;
            }
        };
        let action = self.show_cutscene(ui, cx, &cutscene);
        self.cutscene = Ok(cutscene);
        action
    }

    fn hides_hex(&self) -> bool {
        true
    }

    fn state(&self) -> Option<Box<dyn Any>> {
        Some(Box::new(State {
            timeline: self.timeline.clone(),
            shared: self.preview.shared.clone(),
            shot_camera: self.preview.shot_camera,
            characters: self.preview.characters,
            orbit: self.preview.orbit,
            world: self.preview.world.clone(),
            hour: self.preview.hour,
            preview_share: self.preview_share,
        }))
    }

    fn restore(&mut self, state: Box<dyn Any>) {
        if let Ok(state) = state.downcast::<State>() {
            self.timeline = state.timeline;
            self.preview.shared = state.shared;
            self.preview.shot_camera = state.shot_camera;
            self.preview.characters = state.characters;
            self.preview.orbit = state.orbit;
            // The world loaded stays: an edit of a shot does not move zones.
            self.preview.world = state.world;
            self.preview.hour = state.hour;
            self.preview_share = state.preview_share;
            self.preview.invalidate();
        }
    }
}

impl CutsceneView {
    fn show_cutscene(
        &mut self,
        ui: &mut egui::Ui,
        cx: &mut ViewContext<'_>,
        cutscene: &Cutscene,
    ) -> Option<ViewAction> {
        if self.playing {
            let dt = ui.input(|i| i.unstable_dt);
            if dt > 0.0 {
                let fps = 1.0 / dt;
                self.fps = if self.fps > 0.0 {
                    self.fps * 0.9 + fps * 0.1
                } else {
                    fps
                };
            }
            match self.sound.clock() {
                Clock::Audio { time, playing } => {
                    self.timeline.playhead = time;
                    if !playing {
                        self.playing = false;
                    }
                }
                Clock::Waiting => {}
                Clock::Timer => {
                    let dt = ui.input(|i| i.stable_dt).min(0.1);
                    self.timeline.playhead += dt;
                }
            }
            if self.timeline.playhead >= cutscene.duration() {
                self.timeline.playhead = cutscene.duration();
                self.playing = false;
            }
            // Keep the play head in view.
            let width = (ui.available_width() - 170.0).max(50.0) / self.timeline.zoom;
            if self.timeline.playhead > self.timeline.scroll + width * 0.9 {
                self.timeline.scroll = self.timeline.playhead - width * 0.1;
            }
            ui.ctx().request_repaint();
        }
        // The play head as the clock left it: a change below is the user's.
        let synced = self.timeline.playhead;
        self.toolbar(ui, cutscene);
        if let Some(e) = self.preview.load_error() {
            ui.colored_label(egui::Color32::RED, e);
        }
        let mut action = None;
        let mut available = ui.available_size();
        if !available.y.is_finite() || available.y > 4000.0 {
            available.y = ui.ctx().content_rect().height() * 0.8;
        }
        let preview_height = (available.y * self.preview_share).clamp(160.0, available.x * 0.6);
        let inspector_width = 360.0f32.min(available.x * 0.4);
        let preview_width = (available.x - inspector_width - 16.0).max(120.0);
        ui.allocate_ui_with_layout(
            egui::vec2(available.x, preview_height),
            egui::Layout::left_to_right(egui::Align::Min),
            |ui| {
                ui.set_height(preview_height);
                ui.allocate_ui_with_layout(
                    egui::vec2(preview_width, preview_height),
                    egui::Layout::top_down(egui::Align::Center),
                    |ui| {
                        ui.set_width(preview_width);
                        self.preview.show(
                            ui,
                            cutscene,
                            self.timeline.playhead,
                            egui::vec2(preview_width, preview_height),
                        );
                    },
                );
                ui.add_space(8.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(inspector_width, preview_height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(inspector_width);
                        egui::ScrollArea::vertical()
                            .id_salt("cutscene inspector")
                            .max_height(preview_height)
                            .show(ui, |ui| {
                                if self.timeline.selected.is_none() {
                                    ui.label(RichText::new(tr("cutscene.select_hint")).weak());
                                }
                                action = self.inspector(ui, cutscene);
                                if !self.message.is_empty() {
                                    ui.colored_label(egui::Color32::YELLOW, &self.message);
                                }
                            });
                    },
                );
            },
        );
        ui.separator();
        let height = (available.y - preview_height - 90.0).max(140.0);
        let output = timeline::show(
            ui,
            &self.tracks,
            &mut self.timeline,
            cutscene.duration(),
            height,
        );
        if let Some((id, start)) = output.moved {
            action = self.move_item(cutscene, id, start);
        }
        if let Some((ItemId::Subtitle { track, line }, length)) = output.resized {
            let sub = &cutscene.audio[track].subtitles[line];
            match self.save_subtitle(
                cutscene,
                track,
                line,
                sub.start_ms,
                (length * 1000.0).round() as u32,
            ) {
                Ok(a) => action = Some(a),
                Err(e) => self.message = e,
            }
        }
        if let Some(ViewAction::SaveData { label, .. } | ViewAction::Apply { label, .. }) = &action
        {
            *cx.status = label.clone();
        }
        // Start, pause or resynchronise the sound.
        if self.playing != self.sound.in_session() {
            if self.playing {
                self.sound
                    .play(ui.ctx(), self.timeline.playhead, cutscene.duration());
                ui.ctx().request_repaint();
            } else {
                self.sound.pause();
            }
        } else if self.playing && (self.timeline.playhead - synced).abs() > 1e-4 {
            self.sound.seek(self.timeline.playhead);
        }
        action
    }
}

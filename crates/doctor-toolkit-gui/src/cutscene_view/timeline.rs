//! The time line of a cut-scene: tracks of clips and markers on a time
//! ruler, zoom and scroll, the play head; dragging moves commands and
//! subtitles, dragging the right edge of a subtitle changes its length.

use doctor_toolkit::cutscene::{Cutscene, command_span};
use doctor_toolkit::resource::trinity::Command;
use doctor_toolkit_mods::i18n::{tr, trf};
use egui::{Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

/// What an item of the time line stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemId {
    Shot(usize),
    Command { shot: usize, index: usize },
    Subtitle { track: usize, line: usize },
    Sound(usize),
}

/// A clip (or a marker when its length is 0) on a track.
#[derive(Debug, Clone)]
pub struct Item {
    pub id: ItemId,
    pub start: f32,
    pub length: f32,
    pub label: String,
    pub color: Color32,
    pub movable: bool,
    pub resizable: bool,
    /// Earliest and latest start allowed when dragged.
    pub bounds: (f32, f32),
}

/// A row of the time line.
#[derive(Debug, Clone)]
pub struct Track {
    pub name: String,
    pub items: Vec<Item>,
    /// Actor tracks fold under one header.
    pub actor: bool,
}

/// Zoom, scroll, play head and the drag in progress.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineState {
    /// Points per second.
    pub zoom: f32,
    /// Seconds at the left edge.
    pub scroll: f32,
    pub playhead: f32,
    pub selected: Option<ItemId>,
    pub show_actors: bool,
    drag: Option<Drag>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Drag {
    id: ItemId,
    resize: bool,
    origin_x: f32,
}

impl Default for TimelineState {
    fn default() -> Self {
        Self {
            zoom: 40.0,
            scroll: 0.0,
            playhead: 0.0,
            selected: None,
            show_actors: true,
            drag: None,
        }
    }
}

/// What the user did on the time line this frame.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Output {
    /// An item was dropped at a new start (seconds from the start).
    pub moved: Option<(ItemId, f32)>,
    /// A subtitle got a new length (seconds).
    pub resized: Option<(ItemId, f32)>,
}

const LABEL_WIDTH: f32 = 170.0;
const ROW: f32 = 20.0;
const RULER: f32 = 22.0;

fn colour(command: &Command) -> Color32 {
    match command {
        Command::ActorAnim { .. } => Color32::from_rgb(70, 130, 200),
        Command::ActorCreate { .. } | Command::ActorTransform { .. } => {
            Color32::from_rgb(110, 110, 160)
        }
        Command::CameraCurves { .. } => Color32::from_rgb(200, 110, 60),
        Command::CameraFov { .. } | Command::CameraCreate { .. } => Color32::from_rgb(180, 140, 70),
        Command::Script { .. } => Color32::from_rgb(140, 90, 170),
        Command::Fade { .. } => Color32::from_rgb(120, 120, 120),
        Command::VfxCreate { .. }
        | Command::VfxState { .. }
        | Command::VfxAttach { .. }
        | Command::VfxTransform { .. }
        | Command::VfxCurves { .. } => Color32::from_rgb(200, 80, 80),
        _ => Color32::from_rgb(90, 90, 90),
    }
}

/// The tracks of a cut-scene.
pub fn tracks(cutscene: &Cutscene) -> Vec<Track> {
    let mut shots = Track {
        name: tr("cutscene.track_shots").to_owned(),
        items: Vec::new(),
        actor: false,
    };
    let names = [
        "cutscene.track_camera",
        "cutscene.track_fov",
        "cutscene.track_effects",
        "cutscene.track_script",
        "cutscene.track_fades",
        "cutscene.track_other",
    ];
    let mut fixed: Vec<Track> = names
        .iter()
        .map(|n| Track {
            name: tr(n).to_owned(),
            items: Vec::new(),
            actor: false,
        })
        .collect();
    let mut actors: Vec<Track> = Vec::new();
    for (s, shot) in cutscene.shots.iter().enumerate() {
        shots.items.push(Item {
            id: ItemId::Shot(s),
            start: shot.start,
            length: shot.duration(),
            label: shot.seq.source_name().to_owned(),
            color: if s % 2 == 0 {
                Color32::from_rgb(60, 80, 70)
            } else {
                Color32::from_rgb(70, 90, 80)
            },
            movable: false,
            resizable: false,
            bounds: (shot.start, shot.start),
        });
        let block = shot.block();
        for (index, (command, time)) in block.commands.iter().zip(block.times()).enumerate() {
            if matches!(command, Command::Wait { .. }) {
                continue;
            }
            let (start, length) = command_span(shot, index, time);
            let mut label = command.label();
            if let Command::ActorAnim { clip, .. } = command {
                label = clip.text.clone();
                if shot.clip(&clip.text).is_none() {
                    label.push_str(" (?)");
                }
            }
            let item = Item {
                id: ItemId::Command { shot: s, index },
                start,
                length,
                label,
                color: colour(command),
                movable: true,
                resizable: false,
                bounds: (shot.start, shot.end()),
            };
            let row = match command {
                Command::CameraCreate { .. } | Command::CameraCurves { .. } => Some(0),
                Command::CameraFov { .. } => Some(1),
                Command::VfxCreate { .. }
                | Command::VfxTransform { .. }
                | Command::VfxState { .. }
                | Command::VfxAttach { .. }
                | Command::VfxCurves { .. } => Some(2),
                Command::Script { .. } => Some(3),
                Command::Fade { .. } => Some(4),
                _ => None,
            };
            if let Some(row) = row {
                fixed[row].items.push(item);
                continue;
            }
            let Some(guid) = command.actor() else {
                fixed[5].items.push(item);
                continue;
            };
            let name = shot
                .actor(&guid)
                .map_or_else(|| guid.to_string(), |a| a.name.clone());
            match actors.iter_mut().find(|t| t.name == name) {
                Some(t) => t.items.push(item),
                None => actors.push(Track {
                    name,
                    items: vec![item],
                    actor: true,
                }),
            }
        }
    }
    let mut out = vec![shots];
    let mut sounds = Vec::new();
    for (a, audio) in cutscene.audio.iter().enumerate() {
        let file = audio
            .entry
            .rsplit('\\')
            .next()
            .unwrap_or(&audio.entry)
            .to_owned();
        sounds.push(Track {
            name: file.clone(),
            items: vec![Item {
                id: ItemId::Sound(a),
                // A track the cut-scene does not start sits at 0, greyed.
                start: audio.placement.start.unwrap_or(0.0),
                length: audio.duration_ms as f32 / 1000.0,
                label: file,
                color: if audio.placement.start.is_some() {
                    Color32::from_rgb(60, 110, 90)
                } else {
                    Color32::from_gray(80)
                },
                movable: false,
                resizable: false,
                bounds: (0.0, 0.0),
            }],
            actor: false,
        });
        if !audio.subtitles.is_empty() {
            sounds.push(Track {
                name: tr("cutscene.track_subtitles").to_owned(),
                items: audio
                    .subtitles
                    .iter()
                    .enumerate()
                    .map(|(line, s)| Item {
                        id: ItemId::Subtitle { track: a, line },
                        start: s.start_ms as f32 / 1000.0,
                        length: s.duration_ms as f32 / 1000.0,
                        label: s
                            .text
                            .clone()
                            .unwrap_or_else(|| format!("{:08x}", s.string)),
                        color: if s.group == "titles" {
                            Color32::from_rgb(170, 150, 60)
                        } else {
                            Color32::from_rgb(90, 140, 60)
                        },
                        movable: true,
                        resizable: true,
                        bounds: (0.0, audio.duration_ms as f32 / 1000.0),
                    })
                    .collect(),
                actor: false,
            });
        }
    }
    // Subtitles first, then the sound tracks, the fixed rows and the actors.
    sounds.sort_by_key(|t| {
        t.items
            .first()
            .is_none_or(|i| !matches!(i.id, ItemId::Subtitle { .. }))
    });
    out.extend(sounds);
    out.extend(fixed.into_iter().filter(|t| !t.items.is_empty()));
    out.extend(actors);
    out
}

fn tick_step(zoom: f32) -> f32 {
    // About one labelled tick per 80 points.
    let raw = 80.0 / zoom.max(1e-3);
    [0.1f32, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0]
        .into_iter()
        .find(|&s| s >= raw)
        .unwrap_or(120.0)
}

/// Draws the time line and handles its input.
pub fn show(
    ui: &mut egui::Ui,
    tracks: &[Track],
    state: &mut TimelineState,
    duration: f32,
    height: f32,
) -> Output {
    let mut output = Output::default();
    // Zoom and scroll controls.
    ui.horizontal(|ui| {
        ui.label(tr("cutscene.zoom"));
        ui.add(
            egui::Slider::new(&mut state.zoom, 5.0..=600.0)
                .logarithmic(true)
                .show_value(false),
        );
        ui.label(tr("cutscene.scroll"));
        let width = (ui.available_width() - 20.0).max(60.0);
        ui.add_sized(
            [width, 18.0],
            egui::Slider::new(&mut state.scroll, 0.0..=duration.max(0.1)).show_value(false),
        );
    });
    let visible: Vec<&Track> = tracks
        .iter()
        .filter(|t| !t.actor || state.show_actors)
        .collect();
    let actor_count = tracks.iter().filter(|t| t.actor).count();
    egui::ScrollArea::vertical()
        .id_salt("cutscene timeline")
        .max_height(height)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            let width = ui.available_width().max(LABEL_WIDTH + 50.0);
            let rows = visible.len() as f32 + if actor_count > 0 { 1.0 } else { 0.0 };
            let total = Vec2::new(width, RULER + rows * ROW);
            let (rect, response) = ui.allocate_exact_size(total, Sense::click_and_drag());
            let painter = ui.painter_at(rect);
            let left = rect.min.x + LABEL_WIDTH;
            painter.rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);
            // Wheel: Ctrl zooms around the pointer, Shift (or a horizontal wheel) scrolls.
            if response.hovered() {
                let (scroll, zoom_delta, ctrl, shift, pointer) = ui.input(|i| {
                    (
                        i.smooth_scroll_delta,
                        i.zoom_delta(),
                        i.modifiers.ctrl,
                        i.modifiers.shift,
                        i.pointer.hover_pos(),
                    )
                });
                if (ctrl && scroll.y != 0.0) || zoom_delta != 1.0 {
                    let factor = if zoom_delta != 1.0 {
                        zoom_delta
                    } else {
                        (scroll.y * 0.003).exp()
                    };
                    let anchor =
                        pointer.map_or(state.scroll, |p| state.scroll + (p.x - left) / state.zoom);
                    state.zoom = (state.zoom * factor).clamp(5.0, 600.0);
                    state.scroll =
                        (anchor - (pointer.map_or(left, |p| p.x) - left) / state.zoom).max(0.0);
                    ui.ctx()
                        .input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
                } else if shift && scroll.y != 0.0 || scroll.x != 0.0 {
                    let dx = if scroll.x != 0.0 { scroll.x } else { scroll.y };
                    state.scroll = (state.scroll - dx / state.zoom).clamp(0.0, duration);
                    ui.ctx()
                        .input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
                }
            }
            let (zoom, scroll) = (state.zoom, state.scroll);
            let x_of = move |t: f32| left + (t - scroll) * zoom;
            let t_of = move |x: f32| scroll + (x - left) / zoom;
            // Ruler.
            let ruler = Rect::from_min_max(
                Pos2::new(left, rect.min.y),
                Pos2::new(rect.max.x, rect.min.y + RULER),
            );
            painter.rect_filled(ruler, 0.0, ui.visuals().faint_bg_color);
            let step = tick_step(state.zoom);
            let mut t = (state.scroll / step).floor() * step;
            let text = ui.visuals().text_color();
            while x_of(t) < rect.max.x {
                let x = x_of(t);
                if x >= left {
                    painter.line_segment(
                        [Pos2::new(x, ruler.max.y - 6.0), Pos2::new(x, rect.max.y)],
                        Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
                    );
                    painter.text(
                        Pos2::new(x + 2.0, ruler.min.y + 2.0),
                        egui::Align2::LEFT_TOP,
                        format!("{t:.1}"),
                        egui::FontId::monospace(11.0),
                        text,
                    );
                }
                t += step;
            }
            let ruler_response =
                ui.interact(ruler, ui.scope_id().with("ruler"), Sense::click_and_drag());
            if (ruler_response.clicked() || ruler_response.dragged())
                && let Some(p) = ruler_response.interact_pointer_pos()
            {
                state.playhead = t_of(p.x).clamp(0.0, duration);
            }
            // Rows.
            let mut y = rect.min.y + RULER;
            let font = egui::FontId::proportional(12.0);
            let pointer = ui.input(|i| i.pointer.interact_pos());
            let mut row = |ui: &mut egui::Ui, track: &Track, y: f32, output: &mut Output| {
                let label_rect =
                    Rect::from_min_size(Pos2::new(rect.min.x, y), Vec2::new(LABEL_WIDTH, ROW));
                painter.text(
                    label_rect.left_center() + Vec2::new(4.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    &track.name,
                    font.clone(),
                    text,
                );
                painter.line_segment(
                    [
                        Pos2::new(rect.min.x, y + ROW),
                        Pos2::new(rect.max.x, y + ROW),
                    ],
                    Stroke::new(1.0, ui.visuals().faint_bg_color),
                );
                for item in &track.items {
                    let mut start = item.start;
                    let mut length = item.length;
                    if let (Some(d), Some(p)) = (state.drag, pointer)
                        && d.id == item.id
                    {
                        let dt = (p.x - d.origin_x) / state.zoom;
                        if d.resize {
                            length = (item.length + dt).max(0.05);
                        } else {
                            start = (item.start + dt).clamp(item.bounds.0, item.bounds.1);
                        }
                    }
                    let x0 = x_of(start);
                    let x1 = if length > 0.0 {
                        x_of(start + length).max(x0 + 2.0)
                    } else {
                        x0 + 6.0
                    };
                    let x0 = if length > 0.0 { x0 } else { x0 - 3.0 };
                    if x1 < left || x0 > rect.max.x {
                        continue;
                    }
                    let r = Rect::from_min_max(
                        Pos2::new(x0.max(left), y + 2.0),
                        Pos2::new(x1.min(rect.max.x), y + ROW - 2.0),
                    );
                    let selected = state.selected == Some(item.id);
                    painter.rect_filled(r, CornerRadius::same(3), item.color);
                    if selected {
                        painter.rect_stroke(
                            r,
                            CornerRadius::same(3),
                            Stroke::new(2.0, Color32::WHITE),
                            StrokeKind::Inside,
                        );
                    }
                    if r.width() > 24.0 {
                        let clipped = painter.with_clip_rect(r.shrink(2.0));
                        clipped.text(
                            r.left_center() + Vec2::new(3.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            &item.label,
                            egui::FontId::proportional(11.0),
                            Color32::WHITE,
                        );
                    }
                    let response = ui
                        .interact(
                            r,
                            ui.scope_id().with(("timeline item", item.id)),
                            Sense::click_and_drag(),
                        )
                        .on_hover_text(trf(
                            "cutscene.item_hint",
                            &[
                                ("label", &item.label),
                                ("start", &format!("{:.3}", item.start)),
                                ("length", &format!("{:.3}", item.length)),
                            ],
                        ));
                    if response.clicked() {
                        state.selected = Some(item.id);
                    }
                    if response.drag_started()
                        && (item.movable || item.resizable)
                        && let Some(p) = response.interact_pointer_pos()
                    {
                        let resize = item.resizable && p.x > r.max.x - 6.0;
                        state.drag = Some(Drag {
                            id: item.id,
                            resize,
                            origin_x: p.x,
                        });
                        state.selected = Some(item.id);
                    }
                    if response.drag_stopped()
                        && let Some(d) = state.drag.take()
                        && d.id == item.id
                    {
                        if d.resize {
                            output.resized = Some((item.id, length));
                        } else if (start - item.start).abs() > 1e-4 {
                            output.moved = Some((item.id, start));
                        }
                    }
                }
            };
            for track in visible.iter().filter(|t| !t.actor) {
                row(ui, track, y, &mut output);
                y += ROW;
            }
            if actor_count > 0 {
                let header =
                    Rect::from_min_size(Pos2::new(rect.min.x, y), Vec2::new(LABEL_WIDTH, ROW));
                let label = format!(
                    "{} {} ({actor_count})",
                    if state.show_actors { "▼" } else { "▶" },
                    tr("cutscene.track_actors")
                );
                if ui
                    .interact(header, ui.scope_id().with("actors header"), Sense::click())
                    .clicked()
                {
                    state.show_actors = !state.show_actors;
                }
                painter.text(
                    header.left_center() + Vec2::new(4.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    label,
                    font.clone(),
                    text,
                );
                y += ROW;
                for track in visible.iter().filter(|t| t.actor) {
                    row(ui, track, y, &mut output);
                    y += ROW;
                }
            }
            // Clicking the empty part of the rows moves the play head.
            if response.clicked()
                && let Some(p) = response.interact_pointer_pos()
                && p.x > left
            {
                state.playhead = t_of(p.x).clamp(0.0, duration);
            }
            let x = x_of(state.playhead);
            if x >= left {
                painter.line_segment(
                    [Pos2::new(x, rect.min.y), Pos2::new(x, rect.max.y)],
                    Stroke::new(2.0, Color32::from_rgb(230, 60, 60)),
                );
            }
        });
    output
}

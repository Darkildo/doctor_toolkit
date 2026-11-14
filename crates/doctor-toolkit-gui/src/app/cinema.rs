//! A cut-scene edited in the level view (stage 62): the session the shell
//! keeps for it, the Timeline and Cut-scene Preview panels, and the edits
//! of its keys.
//!
//! "Open in the level view" (the cut-scene tab's bar, its Asset menu and
//! context menu, the Content Browser, the Level menu) loads the cut-scene,
//! puts it over a level tab ([`crate::level_view::cine::Layer`]) and loads
//! the zones the cut-scene preview would show (`world::choose_streams`, in
//! the background). The session has the play head: the Timeline panel
//! moves it and plays (with the sound of the cut-scene), the level view
//! and the Preview panel follow it, and a tab of the same cut-scene shares
//! it through [`SharedCursor`]. Edits — the gizmo on a camera or actor,
//! Details, "Key from the view", deleting a key — write the shot at the
//! play head into the workspace, one operation each (Ctrl+Z).

use std::sync::Arc;
use std::sync::mpsc::Receiver;

use doctor_toolkit::cutscene::edit;
use doctor_toolkit::cutscene::world::is_identity;
use doctor_toolkit::cutscene::{Cutscene, Source, camera_state};
use doctor_toolkit::resource::trinity::{Guid, SeqMaster};
use doctor_toolkit_mods::i18n::{tr, trf};
use egui::RichText;

use super::BrowserApp;
use super::dock::{DockOp, Pane};
use super::layouts::{Builtin, LayoutId};
use crate::cutscene_view::preview::Preview;
use crate::cutscene_view::sound::{Clock, SoundPlayer};
use crate::cutscene_view::timeline::{self, TimelineState, Track};
use crate::cutscene_view::{Cursor, Sounds, TimelineEdits, read_sounds, world};
use crate::level_view::cine::{CineAction, Item, Layer};
use crate::nav::Location;
use crate::render::math;

/// The zones of a cut-scene chosen in the background.
type ZoneChoice = Result<Result<Vec<usize>, String>, String>;

/// The cut-scene edited in the level view.
pub struct Cinema {
    /// The shot it was opened from.
    pub loc: Location,
    pub cutscene: Arc<Cutscene>,
    tracks: Vec<Track>,
    pub timeline: TimelineState,
    pub playing: bool,
    sound: SoundPlayer,
    sounds: Sounds,
    /// The level tab it is shown in.
    pub level: Option<u64>,
    zones: Option<Receiver<ZoneChoice>>,
    /// The picture of the shot camera (the Cut-scene Preview panel).
    preview: Option<Preview>,
    /// Edits of the Details panel not let go yet: item, play head, values.
    pub draft: Option<Draft>,
    /// The Preview panel loads the characters (they were loaded in the
    /// level view).
    preview_characters: bool,
    /// Video export: its settings and the render running.
    pub export: ExportSettings,
    exporting: Option<Exporting>,
}

/// What Export video writes.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportSettings {
    pub width: u32,
    pub height: u32,
    pub fps: f32,
    /// The shot at the play head only.
    pub shot_only: bool,
    pub characters: bool,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 720,
            fps: 30.0,
            shot_only: true,
            characters: true,
        }
    }
}

/// A video being rendered in the background.
struct Exporting {
    rx: Receiver<Result<Result<crate::cutscene_view::render::Done, String>, String>>,
    done: Arc<std::sync::atomic::AtomicUsize>,
    frames: usize,
    path: std::path::PathBuf,
}

/// Values Details edits for the camera or an actor at the play head.
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub item: Item,
    pub time: f32,
    /// The cut-scene the values are of (its address).
    pub of: usize,
    /// x, y, z and angles x, y, z in degrees.
    pub values: [f32; 6],
    /// Focal length, millimetres (the camera).
    pub focal: f32,
}

impl Cinema {
    /// Whether the Preview panel made its picture.
    #[cfg(test)]
    pub fn has_preview(&self) -> bool {
        self.preview.is_some()
    }

    fn duration(&self) -> f32 {
        self.cutscene.duration()
    }

    fn toggle_play(&mut self) {
        self.playing = !self.playing;
        if self.playing && self.timeline.playhead >= self.duration() - 1e-3 {
            self.timeline.playhead = 0.0;
        }
    }

    fn seek(&mut self, t: f32) {
        self.timeline.playhead = t.clamp(0.0, self.duration());
        if self.playing {
            self.sound.seek(self.timeline.playhead);
        }
    }
}

/// The camera values and focal length of the camera at `t`.
fn camera_values(cutscene: &Cutscene, t: f32) -> Option<([f32; 6], f32)> {
    let c = cutscene.camera_at(t)?;
    Some((c.values, c.fov))
}

impl BrowserApp {
    /// Opens the cut-scene of shot `loc` in a level tab.
    pub(super) fn open_cutscene_in_level(&mut self, loc: Location, ctx: &egui::Context) {
        let (Some(game), Some(index)) = (self.game.clone(), self.index.clone()) else {
            return;
        };
        let source = Source::new(&game, self.workspace.as_ref());
        let cutscene = match Cutscene::load(&source, loc.stream, Some(loc.resource)) {
            Ok(c) => Arc::new(c),
            Err(e) => {
                self.notify(
                    super::Level::Error,
                    trf("cutscene.error", &[("error", &e.to_string())]),
                );
                return;
            }
        };
        let (sounds, sources) = read_sounds(&source, &cutscene);
        let start = cutscene
            .shots
            .iter()
            .find(|s| s.resource == loc.resource)
            .map_or(0.0, |s| s.start);
        // The level tab: the one in front, else any, else a new one.
        let level = self
            .front_level()
            .or_else(|| self.levels.keys().copied().find(|&id| self.live(id)));
        let level = match level {
            Some(id) => {
                self.dock_ops.push(DockOp::Focus(id));
                id
            }
            None => {
                let id = self.new_pane_id();
                self.dock_ops.push(DockOp::Open(Pane::Level { id }));
                id
            }
        };
        self.ensure_level(level, ctx);
        // Another level showing the cut-scene before lets it go.
        for (&id, view) in &mut self.levels {
            if id != level {
                view.cine = None;
            }
        }
        if let Some(view) = self.levels.get_mut(&level) {
            let mut layer = Layer::new(cutscene.clone());
            layer.time = start;
            layer.status = tr("cine.choosing_zones").to_owned();
            view.cine = Some(layer);
        }
        let zones = {
            let (game, index, cutscene) = (game.clone(), index.clone(), cutscene.clone());
            crate::jobs::background(ctx, move || world::choose_streams(&game, &index, &cutscene))
        };
        let mut timeline = TimelineState::default();
        timeline.playhead = start;
        timeline.scroll = (start - 1.0).max(0.0);
        self.cine = Some(Cinema {
            loc,
            tracks: timeline::tracks(&cutscene),
            cutscene,
            timeline,
            playing: false,
            sound: SoundPlayer::new(sources),
            sounds,
            level: Some(level),
            zones: Some(zones),
            preview: None,
            draft: None,
            export: ExportSettings::default(),
            exporting: None,
            preview_characters: false,
        });
        // The Timeline panel: shown, or the Cinematics layout.
        let has_timeline = self
            .dock
            .as_ref()
            .is_some_and(|t| super::dock::find(t, |p| *p == Pane::Timeline).is_some())
            || self.drawers.contains(&Pane::Timeline);
        if has_timeline {
            self.dock_ops.push(DockOp::ShowPanel(Pane::Timeline));
        } else {
            self.dock_ops
                .push(DockOp::ApplyLayout(LayoutId::Builtin(Builtin::Cinematics)));
        }
        self.dock_ops.push(DockOp::Focus(level));
        self.status = trf("cine.opened", &[("name", &self.cine_name())]);
        if self.capture.is_some()
            && let Ok(spec) = std::env::var("DOCTOR_TOOLKIT_GUI_CINE")
        {
            self.cine_preset(level, &spec);
        }
    }

    /// `DOCTOR_TOOLKIT_GUI_CINE=time=<s>,characters,select=camera|<actor>,through,view=<eye>/<look>`
    /// for screenshots: the play head, the characters, what is selected,
    /// the view's camera (six numbers separated by `/`).
    fn cine_preset(&mut self, level: u64, spec: &str) {
        for item in spec.split(',').map(str::trim) {
            let (name, value) = item.split_once('=').unwrap_or((item, ""));
            let Some(cine) = &mut self.cine else { return };
            let layer = self.levels.get_mut(&level).and_then(|l| l.cine.as_mut());
            match name {
                "time" => {
                    cine.seek(value.parse().unwrap_or(0.0));
                    if let Some(layer) = layer {
                        layer.time = cine.timeline.playhead;
                    }
                }
                "characters" => {
                    cine.preview_characters = true;
                    if let Some(layer) = layer {
                        layer.want_characters = true;
                    }
                }
                "through" => {
                    if let Some(layer) = layer {
                        layer.through_shot = true;
                    }
                }
                "select" => {
                    let item = if value == "camera" {
                        Some(Item::Camera)
                    } else {
                        cine.cutscene
                            .shots
                            .iter()
                            .flat_map(|s| &s.actors)
                            .find(|a| a.name.eq_ignore_ascii_case(value))
                            .map(|a| Item::Actor(a.guid.0))
                    };
                    if let Some(level) = self.levels.get_mut(&level) {
                        level.cine_select(item);
                    }
                }
                "nomarkers" => {
                    if let Some(level) = self.levels.get_mut(&level) {
                        level.show_markers(false);
                    }
                }
                "behind" => {
                    // behind=<metres back>/<metres up>: over the shoulder of
                    // the shot camera, looking where it looks.
                    let v: Vec<f32> = value.split('/').filter_map(|x| x.parse().ok()).collect();
                    let (back, up) = (
                        v.first().copied().unwrap_or(3.0),
                        v.get(1).copied().unwrap_or(1.0),
                    );
                    let camera = self
                        .levels
                        .get(&level)
                        .and_then(|l| l.cine.as_ref())
                        .and_then(|l| l.shot_camera());
                    if let (Some(level), Some((m, _))) = (self.levels.get_mut(&level), camera) {
                        let f = math::normalize_or([-m[8], -m[9], -m[10]], [0.0, 0.0, -1.0]);
                        let eye = [m[12], m[13], m[14]];
                        let from = math::add(math::sub(eye, math::scale(f, back)), [0.0, up, 0.0]);
                        level.place_camera(from, math::add(eye, math::scale(f, 4.0)));
                    }
                }
                "view" => {
                    // view=<eye x>/<y>/<z>/<look x>/<y>/<z>
                    let v: Vec<f32> = value.split('/').filter_map(|x| x.parse().ok()).collect();
                    if let (Some(level), [a, b, c, d, e, f]) =
                        (self.levels.get_mut(&level), v.as_slice())
                    {
                        level.place_camera([*a, *b, *c], [*d, *e, *f]);
                    }
                }
                _ => eprintln!("DOCTOR_TOOLKIT_GUI_CINE: unknown item {name}"),
            }
        }
    }

    fn cine_name(&self) -> String {
        self.cine
            .as_ref()
            .map_or_else(String::new, |c| c.cutscene.name.clone())
    }

    /// Reads the cut-scene again after an edit, an undo or a redo.
    pub(super) fn reload_cine(&mut self) {
        let Some(game) = self.game.clone() else {
            return;
        };
        let Some(cine) = &mut self.cine else { return };
        let source = Source::new(&game, self.workspace.as_ref());
        let Ok(cutscene) = Cutscene::load(&source, cine.loc.stream, Some(cine.loc.resource)) else {
            return;
        };
        let cutscene = Arc::new(cutscene);
        let (sounds, sources) = read_sounds(&source, &cutscene);
        cine.tracks = timeline::tracks(&cutscene);
        cine.sounds = sounds;
        if !cine.playing {
            cine.sound = SoundPlayer::new(sources);
        }
        cine.cutscene = cutscene.clone();
        cine.draft = None;
        if let Some(p) = &mut cine.preview {
            p.invalidate();
        }
        if let Some(layer) = cine
            .level
            .and_then(|id| self.levels.get_mut(&id))
            .and_then(|l| l.cine.as_mut())
        {
            layer.cutscene = cutscene;
        }
    }

    /// Before the dock is drawn: the play head moves while playing, the
    /// tabs' requests come in, the level view follows.
    pub(super) fn cine_tick(&mut self, ctx: &egui::Context) {
        let Some(cine) = &mut self.cine else {
            *self.cine_cursor.lock().unwrap_or_else(|e| e.into_inner()) = None;
            return;
        };
        // The level was closed, or its cut-scene taken off.
        let shown = cine
            .level
            .and_then(|id| self.levels.get(&id))
            .is_some_and(|l| l.cine.is_some());
        if !shown && cine.level.is_some_and(|id| self.levels.contains_key(&id)) {
            cine.level = None;
        }
        // The zones of the cut-scene.
        if let Some(rx) = &cine.zones
            && let Ok(result) = rx.try_recv()
        {
            cine.zones = None;
            let level = cine.level.and_then(|id| self.levels.get_mut(&id));
            match result.and_then(|r| r) {
                Ok(streams) => {
                    if let Some(level) = level {
                        if let Some(layer) = &mut level.cine {
                            layer.status.clear();
                        }
                        level.load_streams(streams);
                    }
                }
                Err(e) => {
                    if let Some(layer) = level.and_then(|l| l.cine.as_mut()) {
                        layer.status.clear();
                    }
                    self.status = trf("cine.no_zones", &[("error", &e)]);
                }
            }
        }
        self.poll_export();
        let Some(cine) = &mut self.cine else { return };
        // What the tabs of the cut-scene asked for.
        {
            let mut cursor = self.cine_cursor.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(c) = cursor.as_mut() {
                if std::mem::take(&mut c.toggle) {
                    cine.toggle_play();
                }
                if std::mem::take(&mut c.moved) {
                    cine.seek(c.time);
                }
            }
        }
        // Playing: the sound (or the frame timer) moves the play head.
        if cine.playing {
            match cine.sound.clock() {
                Clock::Audio { time, playing } => {
                    cine.timeline.playhead = time;
                    if !playing {
                        cine.playing = false;
                    }
                }
                Clock::Waiting => {}
                Clock::Timer => {
                    let dt = ctx.input(|i| i.stable_dt).min(0.1);
                    cine.timeline.playhead += dt;
                }
            }
            if cine.timeline.playhead >= cine.duration() {
                cine.timeline.playhead = cine.duration();
                cine.playing = false;
            }
            ctx.request_repaint();
        }
        if cine.playing != cine.sound.in_session() {
            if cine.playing {
                cine.sound
                    .play(ctx, cine.timeline.playhead, cine.cutscene.duration());
            } else {
                cine.sound.pause();
            }
        }
        *self.cine_cursor.lock().unwrap_or_else(|e| e.into_inner()) = Some(Cursor {
            stream: cine.cutscene.stream,
            time: cine.timeline.playhead,
            playing: cine.playing,
            toggle: false,
            moved: false,
        });
        // The level view.
        let (time, playing) = (cine.timeline.playhead, cine.playing);
        let wants = cine
            .level
            .and_then(|id| self.levels.get_mut(&id))
            .and_then(|l| l.cine.as_mut())
            .map(|layer| {
                layer.time = time;
                layer.playing = playing;
                layer.want_characters && layer.characters.is_none()
            });
        if wants == Some(true) {
            self.cine_characters();
        }
    }

    /// Opens the characters of the cut-scene's actors for the level view.
    fn cine_characters(&mut self) {
        let (Some(game), Some(index)) = (self.game.clone(), self.index.clone()) else {
            return;
        };
        let Some(id) = self.cine.as_ref().and_then(|c| c.level) else {
            return;
        };
        if let Some(layer) = self.levels.get_mut(&id).and_then(|l| l.cine.as_mut()) {
            let _profile = crate::profile::scope("cut-scene: rigs and parts databases");
            let mut loaded = crate::cutscene_view::preview::Loaded::new(game, &index);
            loaded.salt = 1;
            layer.characters = Some(Ok(loaded));
            layer.want_characters = false;
        }
        if let Some(cine) = &mut self.cine {
            cine.preview_characters = true;
        }
    }

    /// After the dock: a tab moved the play head or asked to play.
    pub(super) fn cine_after_dock(&mut self) {
        let Some(cine) = &mut self.cine else { return };
        let mut cursor = self.cine_cursor.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = cursor.as_mut() {
            if std::mem::take(&mut c.toggle) {
                cine.toggle_play();
            }
            if std::mem::take(&mut c.moved) {
                cine.seek(c.time);
            }
        }
    }

    /// Carries out a command or edit of the cut-scene in level tab `tab`.
    pub(super) fn cine_action(&mut self, tab: u64, action: CineAction) {
        let Some(cine) = &mut self.cine else { return };
        match action {
            CineAction::Seek(t) => cine.seek(t),
            CineAction::Play => cine.toggle_play(),
            CineAction::Close => {
                if let Some(level) = self.levels.get_mut(&tab) {
                    level.cine = None;
                }
                if let Some(c) = &mut self.cine {
                    c.sound.pause();
                }
                self.cine = None;
            }
            CineAction::Transform { item, matrix } => self.cine_transform(item, matrix),
            CineAction::Camera(values) => {
                let label = self.cine_label(Item::Camera);
                self.cine_edit(label, move |seq, t| {
                    edit::set_camera(seq, t, values).map(|_| ())
                });
            }
            CineAction::Actor {
                guid,
                position,
                rotation,
            } => {
                let label = self.cine_label(Item::Actor(guid));
                self.cine_edit(label, move |seq, t| {
                    edit::set_actor(seq, Guid(guid), t, position, rotation).map(|_| ())
                });
            }
            CineAction::Focal(focal) => {
                let label = trf(
                    "cine.label_focal",
                    &[
                        ("mm", &format!("{focal:.1}")),
                        ("time", &format!("{:.2}", cine.timeline.playhead)),
                    ],
                );
                self.cine_edit(label, move |seq, t| {
                    edit::set_focal_length(seq, t, focal).map(|_| ())
                });
            }
            CineAction::KeyFromView { matrix, fov } => {
                let t = cine.timeline.playhead;
                let Some((now, _)) = camera_values(&cine.cutscene, t) else {
                    self.status = tr("cine.no_camera").to_owned();
                    return;
                };
                let local = self.cine_local(&matrix);
                let angles = edit::euler_of(&local, [now[3], now[4], now[5]]);
                let values = [
                    local[12], local[13], local[14], angles[0], angles[1], angles[2],
                ];
                let focal = edit::focal_length(fov);
                let label = trf("cine.label_from_view", &[("time", &format!("{t:.2}"))]);
                self.cine_edit(label, move |seq, t| {
                    edit::set_camera(seq, t, values)?;
                    // A shot without its own focal length keeps the default.
                    match edit::set_focal_length(seq, t, focal) {
                        Ok(_) | Err(doctor_toolkit::Error::InvalidArgument(_)) => Ok(()),
                        Err(e) => Err(e),
                    }
                });
            }
            CineAction::DeleteKey(item) => {
                let time = format!("{:.2}", cine.timeline.playhead);
                let label = trf(
                    "cine.label_delete",
                    &[("what", &self.cine_item_name(item)), ("time", &time)],
                );
                self.cine_edit(label, move |seq, t| {
                    edit::delete_key(seq, item.target(), t).map(|_| ())
                });
            }
        }
    }

    fn cine_item_name(&self, item: Item) -> String {
        let Some(cine) = &self.cine else {
            return String::new();
        };
        match item {
            Item::Camera => tr("cine.camera").to_owned(),
            Item::Actor(g) => cine
                .cutscene
                .shots
                .iter()
                .flat_map(|s| &s.actors)
                .find(|a| a.guid.0 == g)
                .map_or_else(|| "?".to_owned(), |a| a.name.clone()),
        }
    }

    /// "Key of the camera at 12.30 s (igc16_s03)".
    fn cine_label(&self, item: Item) -> String {
        let Some(cine) = &self.cine else {
            return String::new();
        };
        let t = cine.timeline.playhead;
        let shot = cine
            .cutscene
            .shot_at(t)
            .map(|s| cine.cutscene.shots[s].seq.source_name().to_owned())
            .unwrap_or_default();
        trf(
            "cine.label_key",
            &[
                ("what", &self.cine_item_name(item)),
                ("time", &format!("{t:.2}")),
                ("shot", &shot),
            ],
        )
    }

    /// A world matrix in the space of the keys of the shot at the play head.
    fn cine_local(&self, world: &[f32; 16]) -> [f32; 16] {
        let Some(cine) = &self.cine else {
            return *world;
        };
        let Some(s) = cine.cutscene.shot_at(cine.timeline.playhead) else {
            return *world;
        };
        let anchor = cine.cutscene.anchor(s);
        if is_identity(&anchor) {
            return *world;
        }
        let a: [f32; 16] = *anchor.as_flattened().first_chunk::<16>().expect("16");
        math::inverse(&a).map_or(*world, |inv| math::mul(&inv, world))
    }

    /// The gizmo let go of a camera or an actor.
    fn cine_transform(&mut self, item: Item, matrix: [f32; 16]) {
        let Some(cine) = &self.cine else { return };
        let t = cine.timeline.playhead;
        let Some(s) = cine.cutscene.shot_at(t) else {
            return;
        };
        let shot = &cine.cutscene.shots[s];
        let local_t = t - shot.start;
        let new = self.cine_local(&matrix);
        let label = self.cine_label(item);
        // What changed: the position, the rotation or both (keys of the
        // other stay as they are).
        let rotation_of = |m: &[f32; 16]| [m[0], m[1], m[2], m[4], m[5], m[6], m[8], m[9], m[10]];
        match item {
            Item::Camera => {
                let Some(c) = camera_state(shot, local_t) else {
                    return;
                };
                let old = c.matrix();
                let mut values = c.values;
                if (0..3).any(|i| (new[12 + i] - old[12 + i]).abs() > 1e-5) {
                    values[..3].copy_from_slice(&new[12..15]);
                }
                let turned = rotation_of(&new)
                    .iter()
                    .zip(rotation_of(&old))
                    .any(|(a, b)| (a - b).abs() > 1e-5);
                if turned {
                    let a = edit::euler_of(&new, [values[3], values[4], values[5]]);
                    values[3..].copy_from_slice(&a);
                }
                self.cine_edit(label, move |seq, t| {
                    edit::set_camera(seq, t, values).map(|_| ())
                });
            }
            Item::Actor(g) => {
                let Some(a) = doctor_toolkit::cutscene::actor_state(shot, &Guid(g), local_t) else {
                    return;
                };
                let old = a.matrix();
                let position = (0..3)
                    .any(|i| (new[12 + i] - old[12 + i]).abs() > 1e-5)
                    .then(|| [new[12], new[13], new[14]]);
                let rotation = rotation_of(&new)
                    .iter()
                    .zip(rotation_of(&old))
                    .any(|(a, b)| (a - b).abs() > 1e-5)
                    .then(|| edit::quaternion_of(&new));
                self.cine_edit(label, move |seq, t| {
                    edit::set_actor(seq, Guid(g), t, position, rotation).map(|_| ())
                });
            }
        }
    }

    /// Edits the shot at the play head (`edit` gets the time in the shot)
    /// and records it as one operation of the workspace.
    fn cine_edit(
        &mut self,
        label: String,
        edit: impl FnOnce(&mut SeqMaster, f32) -> doctor_toolkit::Result<()>,
    ) {
        let Some(game) = self.game.clone() else {
            return;
        };
        let Some(cine) = &self.cine else { return };
        let t = cine.timeline.playhead;
        let Some(s) = cine.cutscene.shot_at(t) else {
            return;
        };
        let shot = &cine.cutscene.shots[s];
        let (stream, resource) = (cine.cutscene.stream, shot.resource);
        let mut seq = shot.seq.clone();
        let result = edit(&mut seq, t - shot.start)
            .and_then(|()| seq.to_bytes())
            .map_err(|e| e.to_string())
            .and_then(|data| {
                let ws = self.workspace_mut()?;
                ws.set_resource(&game, stream, resource, data, &label)
                    .map_err(|e| e.to_string())
            });
        match result {
            Ok(_) => {
                self.status = trf("edits.recorded", &[("label", &label)]);
                self.after_edit(Some(stream));
            }
            Err(e) => self.notify(
                super::Level::Error,
                trf("edits.cannot_record", &[("what", &label), ("error", &e)]),
            ),
        }
    }

    // ------------------------------------------------------------ panels

    /// The Timeline panel: the time line of the cut-scene in the level view.
    pub(super) fn timeline_pane(&mut self, ui: &mut egui::Ui) {
        let Some(cine) = &mut self.cine else {
            ui.add_space(8.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(tr("cine.no_cutscene")).weak());
                ui.label(RichText::new(tr("cine.no_cutscene_hint")).weak().small());
            });
            return;
        };
        let cutscene = cine.cutscene.clone();
        let mut action = None;
        let mut export = None;
        ui.horizontal_wrapped(|ui| {
            ui.strong(trf("cutscene.title", &[("name", &cutscene.name)]));
            let label = if cine.playing {
                tr("cutscene.pause")
            } else {
                tr("cutscene.play")
            };
            if ui.button(label).clicked() {
                cine.toggle_play();
            }
            if ui
                .button("|<")
                .on_hover_text(tr("cutscene.previous_shot"))
                .clicked()
                && let Some(s) = cutscene.shot_at(cine.timeline.playhead)
            {
                let shot = &cutscene.shots[s];
                let t = if cine.timeline.playhead - shot.start > 0.2 || s == 0 {
                    shot.start
                } else {
                    cutscene.shots[s - 1].start
                };
                cine.seek(t);
            }
            if ui
                .button(">|")
                .on_hover_text(tr("cutscene.next_shot"))
                .clicked()
                && let Some(s) = cutscene.shot_at(cine.timeline.playhead)
                && s + 1 < cutscene.shots.len()
            {
                cine.seek(cutscene.shots[s + 1].start);
            }
            let mut t = cine.timeline.playhead;
            if ui
                .add(
                    egui::DragValue::new(&mut t)
                        .range(0.0..=cutscene.duration())
                        .speed(0.02)
                        .suffix(tr("cutscene.unit_s")),
                )
                .changed()
            {
                cine.seek(t);
            }
            if let Some(s) = cutscene.shot_at(cine.timeline.playhead) {
                ui.label(trf(
                    "cutscene.shot_at",
                    &[("shot", &cutscene.shots[s].seq.source_name())],
                ));
            }
            cine.sound.controls(ui);
            ui.separator();
            export = export_ui(ui, cine);
        });
        let before = cine.timeline.playhead;
        if cine.playing {
            // Keep the play head in view.
            let width = (ui.available_width() - 170.0).max(50.0) / cine.timeline.zoom;
            if cine.timeline.playhead > cine.timeline.scroll + width * 0.9 {
                cine.timeline.scroll = cine.timeline.playhead - width * 0.1;
            }
        }
        let height = (ui.available_height() - 30.0).max(80.0);
        let output = timeline::show(
            ui,
            &cine.tracks,
            &mut cine.timeline,
            cutscene.duration(),
            height,
        );
        if (cine.timeline.playhead - before).abs() > 1e-5 {
            let t = cine.timeline.playhead;
            cine.seek(t);
        }
        let edits = TimelineEdits {
            cutscene: &cutscene,
            opened: usize::MAX,
            sounds: &cine.sounds,
        };
        match edits.output(&output) {
            Ok(a) => action = a,
            Err(e) => self.status = e,
        }
        if let (Some(a), Some(loc)) = (action, self.cine.as_ref().map(|c| c.loc)) {
            self.view_action(loc, a);
        }
        if let Some(path) = export {
            self.start_export(path, ui.ctx());
        }
    }

    /// The Cut-scene Preview panel: the picture of the shot camera at the
    /// play head, 16:9, with the subtitle.
    pub(super) fn cutscene_preview_pane(&mut self, ui: &mut egui::Ui) {
        let Some(game) = self.game.clone() else {
            return;
        };
        let gpu = self.gpu.clone();
        let edits = world::Edits::of(self.workspace.as_ref());
        let Some(cine) = &mut self.cine else {
            ui.add_space(8.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(tr("cine.no_cutscene")).weak());
            });
            return;
        };
        let preview = cine.preview.get_or_insert_with(|| {
            let mut p = Preview::new(game.root().to_owned(), gpu);
            p.world = Arc::new(std::sync::Mutex::new(world::World::new(edits)));
            p
        });
        preview.shot_camera = true;
        if cine.preview_characters && !preview.is_loaded() && preview.has_gpu() {
            preview.load();
        }
        ui.horizontal(|ui| {
            if let Some(s) = cine.cutscene.shot_at(cine.timeline.playhead) {
                ui.label(
                    RichText::new(trf(
                        "cine.preview_of",
                        &[
                            ("shot", &cine.cutscene.shots[s].seq.source_name()),
                            ("time", &format!("{:.2}", cine.timeline.playhead)),
                        ],
                    ))
                    .weak(),
                );
            }
            if !preview.is_loaded()
                && preview.has_gpu()
                && ui
                    .small_button(tr("cutscene.load_characters"))
                    .on_hover_text(tr("cutscene.load_characters_hint"))
                    .clicked()
            {
                preview.load();
            }
        });
        let size = ui.available_size().max(egui::vec2(64.0, 36.0));
        ui.vertical_centered(|ui| {
            preview.show(ui, &cine.cutscene, cine.timeline.playhead, size);
        });
    }

    // ----------------------------------------------------------- Details

    /// Details of the camera or actor of the cut-scene selected in level
    /// `level`.
    pub(super) fn cutscene_details(&mut self, ui: &mut egui::Ui, level: u64) {
        let Some(item) = self.levels.get(&level).and_then(|l| l.cine_selected()) else {
            return;
        };
        let view_camera = self.levels.get(&level).map(|l| l.view_camera());
        let Some(cine) = &mut self.cine else { return };
        let cutscene = cine.cutscene.clone();
        let t = cine.timeline.playhead;
        let Some(s) = cutscene.shot_at(t) else { return };
        let shot = &cutscene.shots[s];
        let local = t - shot.start;
        let mut actions = Vec::new();
        let name = match item {
            Item::Camera => tr("cine.camera").to_owned(),
            Item::Actor(g) => shot
                .actor(&Guid(g))
                .map_or_else(|| "?".to_owned(), |a| a.name.clone()),
        };
        ui.heading(&name);
        if let Item::Actor(g) = item
            && let Some(a) = shot.actor(&Guid(g))
        {
            ui.label(
                RichText::new(format!(
                    "{}{}",
                    a.class.as_deref().unwrap_or("?"),
                    a.assembly
                        .as_deref()
                        .map(|x| format!(", {x}"))
                        .unwrap_or_default()
                ))
                .weak(),
            );
        }
        // Why a character is not drawn.
        if let Item::Actor(g) = item
            && let Some(Some(Ok(loaded))) = self
                .levels
                .get(&level)
                .and_then(|l| l.cine.as_ref())
                .map(|l| l.characters.as_ref())
        {
            let _ = g;
            for p in loaded.problems(&cutscene, s) {
                ui.colored_label(egui::Color32::YELLOW, p);
            }
        }
        let on_key = edit::key_at(shot.block(), item.target(), local);
        ui.label(trf(
            "cine.details_time",
            &[
                ("shot", &shot.seq.source_name()),
                ("time", &format!("{local:.3}")),
            ],
        ));
        ui.label(
            RichText::new(match on_key {
                Some(_) => tr("cine.on_key"),
                None => tr("cine.between_keys"),
            })
            .weak(),
        );
        // The values now, or the draft being edited.
        let of = Arc::as_ptr(&cutscene) as usize;
        let fresh = |values: [f32; 6], focal: f32| Draft {
            item,
            time: t,
            of,
            values,
            focal,
        };
        let current = match item {
            Item::Camera => camera_state(shot, local).map(|c| fresh(c.values, c.fov)),
            Item::Actor(g) => {
                doctor_toolkit::cutscene::actor_state(shot, &Guid(g), local).map(|a| {
                    let m = a.matrix();
                    let angles = edit::euler_of(&m, [0.0; 3]);
                    fresh(
                        [
                            a.position[0],
                            a.position[1],
                            a.position[2],
                            angles[0],
                            angles[1],
                            angles[2],
                        ],
                        0.0,
                    )
                })
            }
        };
        let Some(current) = current else {
            ui.label(RichText::new(tr("cine.not_in_shot")).weak());
            return;
        };
        let draft = match &cine.draft {
            Some(d) if d.item == item && d.time == t && d.of == of => d.clone(),
            _ => current.clone(),
        };
        let mut edited = draft.clone();
        let mut release = false;
        egui::CollapsingHeader::new(tr("details.transform"))
            .default_open(true)
            .show(ui, |ui| {
                egui::Grid::new("cine transform")
                    .num_columns(4)
                    .show(ui, |ui| {
                        for (row, (label, unit, speed)) in [
                            (tr("cine.position"), " m", 0.01),
                            (tr("cine.rotation"), "°", 0.2),
                        ]
                        .into_iter()
                        .enumerate()
                        {
                            ui.label(label);
                            for i in 0..3 {
                                let r = ui.add(
                                    egui::DragValue::new(&mut edited.values[row * 3 + i])
                                        .speed(speed)
                                        .max_decimals(3)
                                        .suffix(unit),
                                );
                                release |= r.drag_stopped() || r.lost_focus();
                            }
                            ui.end_row();
                        }
                    });
            });
        if item == Item::Camera {
            egui::CollapsingHeader::new(tr("cine.lens"))
                .default_open(true)
                .show(ui, |ui| {
                    egui::Grid::new("cine lens").num_columns(2).show(ui, |ui| {
                        ui.label(tr("cine.focal"));
                        let r = ui.add(
                            egui::DragValue::new(&mut edited.focal)
                                .range(1.0..=1000.0)
                                .speed(0.2)
                                .max_decimals(1)
                                .suffix(" mm"),
                        );
                        release |= r.drag_stopped() || r.lost_focus();
                        ui.end_row();
                        ui.label(tr("cine.fov"));
                        let mut fov = doctor_toolkit::cutscene::CameraState {
                            shot: 0,
                            camera: 0,
                            values: [0.0; 6],
                            fov: edited.focal,
                            aspect: 16.0 / 9.0,
                        }
                        .vertical_fov();
                        let r = ui
                            .add(
                                egui::DragValue::new(&mut fov)
                                    .range(1.0..=170.0)
                                    .speed(0.1)
                                    .max_decimals(2)
                                    .suffix("°"),
                            )
                            .on_hover_text(tr("cine.fov_hint"));
                        if r.changed() {
                            edited.focal = edit::focal_length(fov);
                        }
                        release |= r.drag_stopped() || r.lost_focus();
                        ui.end_row();
                    });
                });
        }
        if edited != current {
            cine.draft = Some(edited.clone());
        }
        if release && edited != current {
            cine.draft = None;
            match item {
                Item::Camera => {
                    if edited.values != current.values {
                        actions.push(CineAction::Camera(edited.values));
                    }
                    if (edited.focal - current.focal).abs() > 1e-4 {
                        actions.push(CineAction::Focal(edited.focal));
                    }
                }
                Item::Actor(guid) => {
                    let v = edited.values;
                    let moved = v[..3] != current.values[..3];
                    let turned = v[3..] != current.values[3..];
                    actions.push(CineAction::Actor {
                        guid,
                        position: moved.then(|| [v[0], v[1], v[2]]),
                        rotation: turned.then(|| {
                            doctor_toolkit::resource::trinity::euler_quaternion(v[3], v[4], v[5])
                        }),
                    });
                }
            }
        }
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            if ui
                .button(tr("cine.previous_key"))
                .on_hover_text(tr("cine.previous_key_hint"))
                .clicked()
                && let Some(k) = edit::next_key(shot, item.target(), local, false)
            {
                actions.push(CineAction::Seek(shot.start + k));
            }
            if ui
                .button(tr("cine.next_key"))
                .on_hover_text(tr("cine.next_key_hint"))
                .clicked()
                && let Some(k) = edit::next_key(shot, item.target(), local, true)
            {
                actions.push(CineAction::Seek(shot.start + k));
            }
            if ui
                .add_enabled(on_key.is_some(), egui::Button::new(tr("cine.delete_key")))
                .on_hover_text(tr("cine.delete_key_hint"))
                .clicked()
            {
                actions.push(CineAction::DeleteKey(item));
            }
            if item == Item::Camera
                && let Some((matrix, fov)) = view_camera
                && ui
                    .button(tr("cine.key_from_view"))
                    .on_hover_text(tr("cine.key_from_view_hint"))
                    .clicked()
            {
                actions.push(CineAction::KeyFromView { matrix, fov });
            }
        });
        let keys = edit::key_times(shot.block(), item.target());
        ui.label(
            RichText::new(trf(
                "cine.keys",
                &[(
                    "keys",
                    &keys
                        .iter()
                        .map(|k| format!("{k:.2}"))
                        .collect::<Vec<_>>()
                        .join(", "),
                )],
            ))
            .weak()
            .small(),
        );
        for a in actions {
            self.cine_action(level, a);
        }
    }

    /// Edits of the shot at the play head for a key made from the view
    /// (the command palette and the Level menu).
    pub(super) fn cine_key_from_view(&mut self) {
        let Some(level) = self.cine.as_ref().and_then(|c| c.level) else {
            return;
        };
        let Some((matrix, fov)) = self.levels.get(&level).map(|l| l.view_camera()) else {
            return;
        };
        self.cine_action(level, CineAction::KeyFromView { matrix, fov });
    }
}

/// The Export video menu of the Timeline: size, rate, what to render, and
/// the file (`.vp6` a game movie, `.mp4` through ffmpeg, no extension a
/// folder of PNG); the render's progress while it runs.
fn export_ui(ui: &mut egui::Ui, cine: &mut Cinema) -> Option<std::path::PathBuf> {
    if let Some(e) = &cine.exporting {
        ui.spinner();
        let done = e.done.load(std::sync::atomic::Ordering::Relaxed);
        ui.label(trf(
            "cine.exporting",
            &[("done", &done), ("frames", &e.frames)],
        ));
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(200));
        return None;
    }
    let mut chosen = None;
    ui.menu_button(tr("cine.export"), |ui| {
        let s = &mut cine.export;
        egui::Grid::new("cine export")
            .num_columns(2)
            .show(ui, |ui| {
                ui.label(tr("cine.export_size"));
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut s.width).range(64..=3840));
                    ui.label("×");
                    ui.add(egui::DragValue::new(&mut s.height).range(64..=2160));
                });
                ui.end_row();
                ui.label(tr("cine.export_fps"));
                ui.add(
                    egui::DragValue::new(&mut s.fps)
                        .range(1.0..=120.0)
                        .speed(0.1),
                );
                ui.end_row();
            });
        ui.checkbox(&mut s.shot_only, tr("cine.export_shot"));
        ui.checkbox(&mut s.characters, tr("cutscene.load_characters"));
        ui.label(RichText::new(tr("cine.export_hint")).weak().small());
        if ui.button(tr("cine.export_save")).clicked() {
            chosen = rfd::FileDialog::new()
                .set_file_name(format!("{}.vp6", cine.cutscene.name))
                .add_filter("VP6", &["vp6"])
                .add_filter("MP4", &["mp4"])
                .save_file();
            ui.close();
        }
    });
    chosen
}

impl BrowserApp {
    /// Starts rendering the cut-scene into `path` in the background.
    pub(super) fn start_export(&mut self, path: std::path::PathBuf, ctx: &egui::Context) {
        use crate::cutscene_view::render::{Job, outputs};
        let (Some(game), Some(gpu)) = (self.game.clone(), self.gpu.clone()) else {
            self.status = tr("cutscene.no_gpu").to_owned();
            return;
        };
        let Some(cine) = &self.cine else { return };
        let settings = cine.export.clone();
        let cutscene = cine.cutscene.clone();
        let (mut from, mut to) = (0.0, cutscene.duration());
        if settings.shot_only
            && let Some(s) = cutscene.shot_at(cine.timeline.playhead)
        {
            (from, to) = (cutscene.shots[s].start, cutscene.shots[s].end());
        }
        let source = Source::new(&game, self.workspace.as_ref());
        let audio = doctor_toolkit::cutscene::sound::mix(&source, &cutscene, from, to)
            .ok()
            .flatten();
        let (vp6, mp4, png_dir) = outputs(&path);
        let job = Job {
            root: game.root().to_path_buf(),
            edits: world::Edits::of(self.workspace.as_ref()),
            cutscene,
            audio,
            from,
            to,
            width: settings.width & !1,
            height: settings.height & !1,
            fps: settings.fps,
            world: true,
            characters: settings.characters,
            hour: None,
            png_dir,
            vp6,
            mp4,
        };
        let frames = job.frames();
        let done = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = done.clone();
        // A renderer of its own on the editor's device.
        let renderer = crate::render::Gpu::new(gpu.context());
        let rx = crate::jobs::background(ctx, move || job.run(renderer, &counter));
        if let Some(cine) = &mut self.cine {
            cine.exporting = Some(Exporting {
                rx,
                done,
                frames,
                path,
            });
        }
    }

    /// The render finished: say what it wrote.
    fn poll_export(&mut self) {
        let Some(cine) = &mut self.cine else { return };
        let Some(e) = &cine.exporting else { return };
        let Ok(result) = e.rx.try_recv() else { return };
        let path = e.path.display().to_string();
        cine.exporting = None;
        match result.and_then(|r| r) {
            Ok(done) => self.notify(
                super::Level::Info,
                trf(
                    "cine.exported",
                    &[("path", &path), ("summary", &done.summary())],
                ),
            ),
            Err(e) => self.notify(
                super::Level::Error,
                trf("cine.export_failed", &[("error", &e)]),
            ),
        }
    }
}

//! The movie player of the Movies window.
//!
//! A worker thread owns the VP6 decoder ([`MovieFrames`]): asked for a frame
//! it seeks (back to the key frame at or before it, then forward), converts
//! the picture to RGBA and sends it; while playing it goes on with the next
//! frames through a queue of [`QUEUE`] pictures, so it runs a few frames
//! ahead of the picture shown. Frames the play head has already passed are
//! decoded but not converted.
//!
//! The play head is driven like the cut-scene editor's: by the clock of the
//! sound output when the movie has sound and a device plays it
//! ([`SoundPlayer`]), else by the frame timer. The picture shown is the
//! newest decoded frame at or before the play head.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, TryRecvError, channel, sync_channel};
use std::time::Instant;

use doctor_toolkit::resource::video::{self, MovieFrames};
use doctor_toolkit_mods::i18n::{tr, trf};
use egui::{Color32, RichText};

use crate::cutscene_view::sound::{Clock, SoundPlayer, TrackSource};

/// Decoded pictures waiting in the queue.
const QUEUE: usize = 3;

enum Request {
    /// Show `frame`; then go on with the next frames when `play`.
    Show {
        generation: u64,
        frame: usize,
        play: bool,
    },
    /// Stop decoding ahead.
    Idle,
}

struct Decoded {
    generation: u64,
    index: usize,
    picture: Result<egui::ColorImage, String>,
    /// Mean decoding time of the frames decoded for this picture.
    decode_ms: f32,
}

/// The decoding thread and its channels. Dropping it ends the thread.
struct Worker {
    requests: Sender<Request>,
    frames: Receiver<Decoded>,
    /// The frame at the play head: earlier frames are not converted.
    head: Arc<AtomicUsize>,
}

impl Worker {
    fn start(ctx: &egui::Context, data: Arc<Vec<u8>>) -> Self {
        let (requests, rx) = channel();
        let (tx, frames) = sync_channel(QUEUE);
        let head = Arc::new(AtomicUsize::new(0));
        let ctx = ctx.clone();
        let shared = head.clone();
        std::thread::Builder::new()
            .name("movie decoder".into())
            .spawn(move || decode_loop(data, rx, tx, shared, ctx))
            .expect("spawn the movie decoder");
        Self {
            requests,
            frames,
            head,
        }
    }
}

/// The movie bytes shared with the window.
struct Shared(Arc<Vec<u8>>);

impl AsRef<[u8]> for Shared {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

fn decode_loop(
    data: Arc<Vec<u8>>,
    rx: Receiver<Request>,
    tx: SyncSender<Decoded>,
    head: Arc<AtomicUsize>,
    ctx: egui::Context,
) {
    let mut frames = match MovieFrames::new(Shared(data)) {
        Ok(f) => Some(f),
        Err(e) => {
            let _ = tx.send(Decoded {
                generation: 0,
                index: 0,
                picture: Err(e.to_string()),
                decode_ms: 0.0,
            });
            ctx.request_repaint();
            None
        }
    };
    // (generation, next frame, play)
    let mut job: Option<(u64, usize, bool)> = None;
    // Decoding time and frames decoded since the last picture sent.
    let mut spent = 0.0f32;
    let mut count = 0u64;
    loop {
        // The newest request wins; wait for one when there is nothing to do.
        let mut next = if job.is_none() {
            match rx.recv() {
                Ok(r) => Some(r),
                Err(_) => return,
            }
        } else {
            None
        };
        loop {
            match rx.try_recv() {
                Ok(r) => next = Some(r),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        match next {
            Some(Request::Show {
                generation,
                frame,
                play,
            }) => {
                job = Some((generation, frame, play));
                spent = 0.0;
                count = 0;
            }
            Some(Request::Idle) => job = None,
            None => {}
        }
        let (Some((generation, index, play)), Some(frames)) = (job, frames.as_mut()) else {
            job = None;
            continue;
        };
        if index >= frames.len() {
            job = None;
            continue;
        }
        let started = Instant::now();
        let before = frames.decoded();
        let result = frames.seek(index).map(|_| ());
        spent += started.elapsed().as_secs_f32() * 1000.0;
        count += frames.decoded() - before;
        let late = play && head.load(Ordering::Relaxed) > index && result.is_ok();
        job = (play && result.is_ok()).then_some((generation, index + 1, true));
        if late {
            continue;
        }
        let picture = match (result, frames.picture()) {
            (Ok(()), Some(pic)) => {
                let (w, h) = frames.display_size();
                let mut pixels = vec![Color32::BLACK; w * h];
                pic.rgba_to_slice(
                    w,
                    h,
                    frames.flipped(),
                    bytemuck::cast_slice_mut(&mut pixels),
                );
                Ok(egui::ColorImage::new([w, h], pixels))
            }
            (Err(e), _) => Err(e.to_string()),
            (Ok(()), None) => Err("no picture".to_owned()),
        };
        let message = Decoded {
            generation,
            index,
            picture,
            decode_ms: spent / count.max(1) as f32,
        };
        spent = 0.0;
        count = 0;
        if tx.send(message).is_err() {
            return;
        }
        ctx.request_repaint();
    }
}

/// Playback of one movie.
pub struct Player {
    pub rel_path: String,
    data: Arc<Vec<u8>>,
    count: usize,
    fps: f64,
    keys: Vec<bool>,
    worker: Worker,
    generation: u64,
    pending: VecDeque<Decoded>,
    texture: Option<egui::TextureHandle>,
    /// Frame in the texture.
    shown: Option<usize>,
    /// Frame at the play head.
    pub frame: usize,
    /// Play head in seconds.
    time: f64,
    playing: bool,
    sound: SoundPlayer,
    error: Option<String>,
    /// Mean decoding time per frame (ms), smoothed.
    decode_ms: f32,
    /// The frame asked of the worker while paused.
    asked: Option<usize>,
}

impl Player {
    /// Starts the decoder for a movie and shows `frame`.
    pub fn new(
        ctx: &egui::Context,
        rel_path: &str,
        data: Arc<Vec<u8>>,
        frame: usize,
        volume: f32,
        muted: bool,
    ) -> Self {
        let (count, fps, keys, has_audio) = match video::Movie::parse(&data) {
            Ok(m) => {
                let keys: Vec<bool> = m.frames().map(|(k, _)| k).collect();
                let fps = m.video.map_or(0.0, |v| v.fps());
                let fps = if fps.is_finite() && fps > 0.0 {
                    fps
                } else {
                    30000.0 / 1001.0
                };
                (keys.len(), fps, keys, m.audio.is_some())
            }
            Err(_) => (0, 30.0, Vec::new(), false),
        };
        let sources = if has_audio {
            vec![TrackSource {
                data: data.clone(),
                start: 0.0,
                volume: 1.0,
            }]
        } else {
            Vec::new()
        };
        let mut sound = SoundPlayer::with_decoder(sources, movie_audio);
        sound.volume = volume;
        sound.muted = muted;
        let mut player = Self {
            rel_path: rel_path.to_owned(),
            worker: Worker::start(ctx, data.clone()),
            data,
            count,
            fps,
            keys,
            generation: 0,
            pending: VecDeque::new(),
            texture: None,
            shown: None,
            frame: 0,
            time: 0.0,
            playing: false,
            sound,
            error: None,
            decode_ms: 0.0,
            asked: None,
        };
        player.seek(frame);
        player
    }

    pub fn data(&self) -> &Arc<Vec<u8>> {
        &self.data
    }

    pub fn volume(&self) -> (f32, bool) {
        (self.sound.volume, self.sound.muted)
    }

    /// Waiting for the picture of the play head or for the sound.
    pub fn busy(&self) -> bool {
        self.count > 0 && self.error.is_none() && !self.playing && self.shown != Some(self.frame)
    }

    fn duration(&self) -> f64 {
        self.count as f64 / self.fps
    }

    fn frame_at(&self, time: f64) -> usize {
        ((time * self.fps + 1e-6).floor().max(0.0) as usize).min(self.count.saturating_sub(1))
    }

    /// Asks the worker for `frame` (and the following ones when playing).
    fn request(&mut self, frame: usize, play: bool) {
        self.generation += 1;
        self.pending.clear();
        while self.worker.frames.try_recv().is_ok() {}
        self.asked = (!play).then_some(frame);
        let _ = self.worker.requests.send(Request::Show {
            generation: self.generation,
            frame,
            play,
        });
    }

    /// Moves the play head to `frame` (clamped).
    pub fn seek(&mut self, frame: usize) {
        if self.count == 0 {
            return;
        }
        let frame = frame.min(self.count - 1);
        self.frame = frame;
        self.time = frame as f64 / self.fps;
        self.worker.head.store(frame, Ordering::Relaxed);
        if self.playing {
            self.sound.seek(self.time as f32);
            self.request(frame, true);
        } else if self.shown != Some(frame) && self.asked != Some(frame) {
            self.request(frame, false);
        }
    }

    pub fn play(&mut self, ctx: &egui::Context) {
        if self.count == 0 || self.playing {
            return;
        }
        if self.frame + 1 >= self.count {
            self.frame = 0;
        }
        self.time = self.frame as f64 / self.fps;
        self.playing = true;
        self.sound
            .play(ctx, self.time as f32, self.duration() as f32);
        self.request(self.frame, true);
        ctx.request_repaint();
    }

    /// Stops at the picture shown.
    pub fn pause(&mut self) {
        if !self.playing {
            return;
        }
        self.playing = false;
        self.sound.pause();
        let _ = self.worker.requests.send(Request::Idle);
        self.generation += 1;
        self.pending.clear();
        let frame = self.shown.unwrap_or(self.frame);
        self.frame = frame;
        self.time = frame as f64 / self.fps;
        self.asked = None;
        if self.shown != Some(frame) {
            self.request(frame, false);
        }
    }

    pub fn toggle(&mut self, ctx: &egui::Context) {
        if self.playing {
            self.pause();
        } else {
            self.play(ctx);
        }
    }

    /// Advances the play head and takes decoded pictures; call every frame.
    pub fn update(&mut self, ctx: &egui::Context) {
        if self.playing {
            match self.sound.clock() {
                Clock::Audio { time, playing } => {
                    self.time = time as f64;
                    if !playing {
                        self.time = self.time.max(self.duration());
                    }
                }
                Clock::Waiting => {}
                Clock::Timer => {
                    let dt = ctx.input(|i| i.stable_dt).min(0.1) as f64;
                    self.time += dt;
                }
            }
            let end = self.time >= self.duration();
            self.frame = self.frame_at(self.time);
            self.worker.head.store(self.frame, Ordering::Relaxed);
            if end {
                self.frame = self.count.saturating_sub(1);
                self.playing = false;
                self.sound.pause();
            }
            ctx.request_repaint();
        }
        // Newest picture at or before the play head.
        let mut take = None;
        loop {
            if let Some(front) = self.pending.front() {
                if front.generation != self.generation {
                    self.pending.pop_front();
                    continue;
                }
                if front.index <= self.frame || front.picture.is_err() {
                    take = self.pending.pop_front();
                    continue;
                }
                break;
            }
            match self.worker.frames.try_recv() {
                Ok(d) => self.pending.push_back(d),
                Err(_) => break,
            }
        }
        if let Some(d) = take {
            self.decode_ms = if self.decode_ms == 0.0 {
                d.decode_ms
            } else {
                self.decode_ms * 0.9 + d.decode_ms * 0.1
            };
            match d.picture {
                Ok(image) => {
                    match &mut self.texture {
                        Some(t) => t.set(image, egui::TextureOptions::LINEAR),
                        None => {
                            self.texture = Some(ctx.load_texture(
                                "movie_frame",
                                image,
                                egui::TextureOptions::LINEAR,
                            ));
                        }
                    }
                    self.shown = Some(d.index);
                    self.error = None;
                }
                Err(e) => {
                    self.error = Some(e);
                    if self.playing {
                        self.playing = false;
                        self.sound.pause();
                    }
                }
            }
            if self.asked == Some(d.index) {
                self.asked = None;
            }
        }
        if !self.playing && self.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(30));
        }
    }

    /// The picture, scaled to fit `size` with its proportions; a double
    /// click returns true (full window).
    pub fn picture(&mut self, ui: &mut egui::Ui, size: egui::Vec2) -> bool {
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, Color32::BLACK);
        match &self.texture {
            Some(t) => {
                let [w, h] = t.size();
                let scale = (rect.width() / w as f32).min(rect.height() / h as f32);
                let shown = egui::vec2(w as f32 * scale, h as f32 * scale);
                let image = egui::Rect::from_center_size(rect.center(), shown);
                painter.image(
                    t.id(),
                    image,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            None if self.error.is_none() => {
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    tr("movies.decoding"),
                    egui::FontId::proportional(16.0),
                    Color32::GRAY,
                );
            }
            None => {}
        }
        if let Some(e) = &self.error {
            painter.text(
                rect.left_top() + egui::vec2(8.0, 8.0),
                egui::Align2::LEFT_TOP,
                trf("movies.error", &[("error", e)]),
                egui::FontId::proportional(14.0),
                Color32::LIGHT_RED,
            );
        }
        response.double_clicked()
    }

    /// Play/pause, frame steps, time line, time, volume. Returns true when the
    /// full-window button was pressed.
    pub fn controls(&mut self, ui: &mut egui::Ui, full: bool) -> bool {
        let mut toggle_full = false;
        if self.count == 0 {
            return false;
        }
        let ctx = ui.ctx().clone();
        // Keys: in the full window always, else while the pointer is over it.
        let keys = full || ui.ui_contains_pointer();
        if keys && !ctx.egui_wants_keyboard_input() {
            let (space, left, right, home, end) = ctx.input(|i| {
                (
                    i.key_pressed(egui::Key::Space),
                    i.key_pressed(egui::Key::ArrowLeft),
                    i.key_pressed(egui::Key::ArrowRight),
                    i.key_pressed(egui::Key::Home),
                    i.key_pressed(egui::Key::End),
                )
            });
            if space {
                self.toggle(&ctx);
            }
            if left {
                self.step(-1);
            }
            if right {
                self.step(1);
            }
            if home {
                self.seek(0);
            }
            if end {
                self.seek(self.count - 1);
            }
        }
        // Time line over the whole width.
        let mut frame = self.frame;
        let changed = ui
            .scope(|ui| {
                ui.spacing_mut().slider_width = ui.available_width() - 8.0;
                ui.add(
                    egui::Slider::new(&mut frame, 0..=self.count - 1)
                        .show_value(false)
                        .handle_shape(egui::style::HandleShape::Rect { aspect_ratio: 0.5 }),
                )
                .changed()
            })
            .inner;
        if changed {
            self.seek(frame);
        }
        ui.horizontal_wrapped(|ui| {
            let play = if self.playing {
                tr("movies.pause")
            } else {
                tr("movies.play")
            };
            if ui
                .add(egui::Button::new(play).min_size(egui::vec2(64.0, 0.0)))
                .on_hover_text(tr("movies.play_hint"))
                .clicked()
            {
                self.toggle(&ctx);
            }
            if ui.button("|<").on_hover_text(tr("movies.first")).clicked() {
                self.seek(0);
            }
            if ui
                .button("<")
                .on_hover_text(tr("movies.previous"))
                .clicked()
            {
                self.step(-1);
            }
            if ui.button(">").on_hover_text(tr("movies.next")).clicked() {
                self.step(1);
            }
            if ui.button(">|").on_hover_text(tr("movies.last")).clicked() {
                self.seek(self.count - 1);
            }
            ui.separator();
            ui.monospace(format!(
                "{} / {}",
                clock(self.frame as f64 / self.fps),
                clock(self.duration())
            ));
            let key = if self.keys.get(self.frame).copied().unwrap_or(false) {
                format!(" ({})", tr("movies.key"))
            } else {
                String::new()
            };
            ui.label(
                RichText::new(format!(
                    "{}{key}",
                    trf(
                        "movies.frame_of",
                        &[("frame", &self.frame), ("count", &self.count)]
                    )
                ))
                .weak(),
            );
            self.sound.controls(ui);
            ui.separator();
            let label = if full {
                tr("movies.full_exit")
            } else {
                tr("movies.full")
            };
            if ui
                .button(label)
                .on_hover_text(tr("movies.full_hint"))
                .clicked()
            {
                toggle_full = true;
            }
            if self.decode_ms > 0.0 {
                ui.label(
                    RichText::new(trf(
                        "movies.decode_time",
                        &[("ms", &format!("{:.1}", self.decode_ms))],
                    ))
                    .weak()
                    .small(),
                );
            }
        });
        toggle_full
    }

    /// One frame forward or back (pauses first).
    pub fn step(&mut self, delta: isize) {
        self.pause();
        let frame = self.frame.saturating_add_signed(delta);
        self.seek(frame.min(self.count.saturating_sub(1)));
    }

    /// Stops the sound (the window was closed).
    pub fn stop(&mut self) {
        self.pause();
    }
}

/// The audio track of a movie for [`SoundPlayer`].
fn movie_audio(data: &[u8]) -> doctor_toolkit::Result<video::input::Pcm> {
    video::decode_audio(data)?
        .ok_or_else(|| doctor_toolkit::Error::InvalidArgument("the movie has no audio".into()))
}

/// `m:ss.cc`.
fn clock(seconds: f64) -> String {
    let cs = (seconds.max(0.0) * 100.0).round() as u64;
    format!("{}:{:02}.{:02}", cs / 6000, cs / 100 % 60, cs % 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_text() {
        assert_eq!(clock(0.0), "0:00.00");
        assert_eq!(clock(65.432), "1:05.43");
        assert_eq!(clock(3600.0), "60:00.00");
    }
}

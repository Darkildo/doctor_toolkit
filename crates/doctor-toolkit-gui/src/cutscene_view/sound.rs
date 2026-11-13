//! The sound of the cut-scene editor: the tracks of the cut-scene mixed at
//! their starts and volumes (`doctor_toolkit::cutscene::sound`), with the
//! clock of the output driving the play head (cargo feature `audio`).
//!
//! The first Play opens the output device and decodes the tracks on a worker
//! thread (`jobs::background`); the play head waits until the sound is
//! ready. Then the view asks [`SoundPlayer::clock`] every frame: the time
//! being heard, never going back while playing, so the picture follows the
//! sound without drift. Without a device, without sound tracks or in a
//! build without `audio`, the clock is the frame timer as before.
//!
//! The Movies window plays a movie's own sound track with the same player
//! ([`SoundPlayer::with_decoder`]).

// Without `audio` only the controls and the timer remain.
#![cfg_attr(not(feature = "audio"), allow(dead_code))]

use std::sync::Arc;

use doctor_toolkit::resource::video::input::Pcm;
use doctor_toolkit_mods::i18n::{tr, trf};
use egui::RichText;

/// A track to play: its `.snu` data, start (seconds) and volume.
pub struct TrackSource {
    pub data: Arc<Vec<u8>>,
    pub start: f32,
    pub volume: f32,
}

/// What drives the play head this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Clock {
    /// The sound: the time heard, and whether it still plays (false at the end).
    Audio { time: f32, playing: bool },
    /// The sound is being prepared: hold the play head.
    Waiting,
    /// No sound: advance by the frame time.
    Timer,
}

#[cfg(feature = "audio")]
type Loaded = Result<Result<Vec<crate::audio_mix::MixTrack>, String>, String>;

/// Decodes the data of a [`TrackSource`] to PCM.
pub type Decode = fn(&[u8]) -> doctor_toolkit::Result<Pcm>;

pub struct SoundPlayer {
    sources: Vec<TrackSource>,
    decode: Decode,
    pub volume: f32,
    pub muted: bool,
    /// Why there is no sound.
    problem: Option<String>,
    /// Between `play` and `pause`.
    session: bool,
    /// The play head last given while playing.
    last: f32,
    #[cfg(feature = "audio")]
    output: Option<crate::audio_out::MixOutput>,
    #[cfg(feature = "audio")]
    loading: Option<std::sync::mpsc::Receiver<Loaded>>,
    /// The tracks are in the mixer.
    #[cfg(feature = "audio")]
    ready: bool,
}

impl SoundPlayer {
    /// Cut-scene sound tracks (`.snu`).
    pub fn new(sources: Vec<TrackSource>) -> Self {
        Self::with_decoder(sources, doctor_toolkit::cutscene::sound::decode)
    }

    /// Tracks in another format, decoded by `decode`.
    pub fn with_decoder(sources: Vec<TrackSource>, decode: Decode) -> Self {
        Self {
            sources,
            decode,
            volume: 1.0,
            muted: false,
            problem: None,
            session: false,
            last: 0.0,
            #[cfg(feature = "audio")]
            output: None,
            #[cfg(feature = "audio")]
            loading: None,
            #[cfg(feature = "audio")]
            ready: false,
        }
    }

    pub fn in_session(&self) -> bool {
        self.session
    }

    /// Starts playing at `at` seconds; playing stops by itself at `end`.
    pub fn play(&mut self, ctx: &egui::Context, at: f32, end: f32) {
        self.session = true;
        self.last = at;
        #[cfg(feature = "audio")]
        self.start_output(ctx, at, end);
        #[cfg(not(feature = "audio"))]
        let _ = (ctx, end);
    }

    pub fn pause(&mut self) {
        self.session = false;
        #[cfg(feature = "audio")]
        if let Some(output) = &self.output {
            output.state.set_playing(false);
        }
    }

    /// The play head was moved while playing.
    pub fn seek(&mut self, at: f32) {
        self.last = at;
        #[cfg(feature = "audio")]
        if let Some(output) = &self.output {
            output.state.seek(at as f64);
        }
    }

    /// What drives the play head; call every frame while playing.
    pub fn clock(&mut self) -> Clock {
        #[cfg(feature = "audio")]
        {
            self.poll();
            if self.loading.is_some() {
                return Clock::Waiting;
            }
            if let Some(output) = &self.output
                && self.ready
                && self.session
            {
                let time = (output.state.time() as f32).max(self.last);
                self.last = time;
                return Clock::Audio {
                    time,
                    playing: output.state.playing(),
                };
            }
        }
        Clock::Timer
    }

    #[cfg(feature = "audio")]
    fn start_output(&mut self, ctx: &egui::Context, at: f32, end: f32) {
        if self.sources.is_empty() || self.problem.is_some() {
            return;
        }
        if self.output.is_none() {
            match crate::audio_out::open_mix() {
                Ok(output) => self.output = Some(output),
                Err(e) => {
                    self.problem = Some(e);
                    return;
                }
            }
        }
        let Some(output) = &self.output else { return };
        let state = &output.state;
        state.set_end(end as f64);
        state.seek(at as f64);
        state.set_gain(self.gain());
        if self.ready {
            state.set_playing(true);
        } else if self.loading.is_none() {
            let sources: Vec<(Arc<Vec<u8>>, f32, f32)> = self
                .sources
                .iter()
                .map(|s| (s.data.clone(), s.start, s.volume))
                .collect();
            let (channels, rate) = (state.channels, state.rate);
            let decode = self.decode;
            self.loading = Some(crate::jobs::background(ctx, move || {
                prepare(&sources, decode, channels, rate)
            }));
        }
    }

    /// Takes the decoded tracks when the worker is done.
    #[cfg(feature = "audio")]
    fn poll(&mut self) {
        let Some(rx) = &self.loading else { return };
        let result = match rx.try_recv() {
            Ok(r) => r,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("worker lost".to_owned()),
        };
        self.loading = None;
        match result.and_then(|r| r) {
            Ok(tracks) => {
                if let Some(output) = &self.output {
                    let state = &output.state;
                    state.set_tracks(tracks);
                    self.ready = true;
                    if self.session {
                        // Where the play head is now (it may have been moved).
                        state.seek(self.last as f64);
                        state.set_playing(true);
                    }
                }
            }
            Err(e) => {
                self.problem = Some(e);
                self.output = None;
            }
        }
    }

    fn gain(&self) -> f32 {
        if self.muted { 0.0 } else { self.volume }
    }

    /// Volume, mute and what the sound is doing, for the play bar.
    pub fn controls(&mut self, ui: &mut egui::Ui) {
        if self.sources.is_empty() {
            return;
        }
        ui.separator();
        let mut changed = ui
            .checkbox(&mut self.muted, tr("cutscene.mute"))
            .on_hover_text(tr("cutscene.mute_hint"))
            .changed();
        changed |= ui
            .add_enabled(
                !self.muted,
                egui::Slider::new(&mut self.volume, 0.0..=2.0)
                    .show_value(false)
                    .text(tr("cutscene.volume")),
            )
            .on_hover_text(trf(
                "cutscene.volume_hint",
                &[("percent", &format!("{:.0}", self.volume * 100.0))],
            ))
            .changed();
        #[cfg(feature = "audio")]
        {
            if changed && let Some(output) = &self.output {
                output.state.set_gain(self.gain());
            }
            if self.loading.is_some() {
                ui.spinner();
                ui.label(RichText::new(tr("cutscene.sound_loading")).weak());
            }
        }
        #[cfg(not(feature = "audio"))]
        {
            let _ = changed;
            ui.label(
                RichText::new(tr("cutscene.sound_needs_feature"))
                    .weak()
                    .small(),
            );
        }
        if let Some(problem) = &self.problem {
            ui.label(
                RichText::new(trf("cutscene.sound_unavailable", &[("error", problem)]))
                    .color(egui::Color32::YELLOW)
                    .small(),
            );
        }
    }
}

/// Decodes the tracks and converts them to the output layout, in parallel.
#[cfg(feature = "audio")]
fn prepare(
    sources: &[(Arc<Vec<u8>>, f32, f32)],
    decode: Decode,
    channels: usize,
    rate: u32,
) -> Result<Vec<crate::audio_mix::MixTrack>, String> {
    use rayon::prelude::*;
    sources
        .par_iter()
        .map(|(data, start, volume)| {
            let pcm = decode(data).map_err(|e| e.to_string())?;
            Ok(crate::audio_mix::prepare(
                &pcm.samples,
                pcm.channels as usize,
                pcm.sample_rate,
                channels,
                rate,
                *start as f64,
                *volume,
            ))
        })
        .collect()
}

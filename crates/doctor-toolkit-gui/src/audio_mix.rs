//! Several sounds mixed on one time line, with the clock of the output
//! driving the picture (the cut-scene editor plays its sound tracks so).
//!
//! Each [`MixTrack`] is converted once to the channels and rate of the
//! output device and placed at an output frame. The audio callback renders
//! the frames from the current position ([`MixState::render`]); the interface
//! asks the state for the time being heard ([`MixState::time`]), seeks and
//! pauses. Nothing here talks to a device: `audio_out` connects the state to
//! one (cargo feature `audio`).

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A sound in the output's layout, placed on the time line.
#[derive(Debug, Clone, Default)]
pub struct MixTrack {
    /// Interleaved samples, output channels and rate.
    pub samples: Vec<f32>,
    /// Output frame at which the first sample plays (may be negative).
    pub start: i64,
    /// Volume multiplier.
    pub gain: f32,
}

/// The output frame of a time in seconds.
pub fn frame_at(seconds: f64, rate: u32) -> i64 {
    (seconds * rate as f64).round() as i64
}

/// Seconds of an output frame.
pub fn seconds_at(frame: i64, rate: u32) -> f64 {
    frame as f64 / rate.max(1) as f64
}

/// How `from` channels go to `to` channels: `to` rows of `from` gains.
///
/// Six channels are 5.1 in the order of the game's streamed sounds:
/// left, centre, right, left surround, right surround, LFE. To stereo they
/// fold as ITU-R BS.775 (centre and surrounds at −3 dB, LFE left out),
/// scaled so that no output exceeds full scale; to six or more output
/// channels they go to the device order (left, right, centre, LFE, left
/// surround, right surround). Mono is copied to the first two channels;
/// other layouts keep their first channels.
pub fn channel_matrix(from: usize, to: usize) -> Vec<Vec<f32>> {
    let mut m = vec![vec![0.0; from]; to];
    if from == 0 || to == 0 {
        return m;
    }
    const H: f32 = std::f32::consts::FRAC_1_SQRT_2;
    match (from, to) {
        (1, _) => {
            for row in m.iter_mut().take(2) {
                row[0] = 1.0;
            }
        }
        (_, 1) if from != 6 => {
            for gain in &mut m[0] {
                *gain = 1.0 / from as f32;
            }
        }
        (6, 1) => {
            let n = 1.0 / (2.0 + 3.0 * H);
            m[0] = vec![n * H, n, n * H, n * H, n * H, 0.0];
        }
        (6, t) if t < 6 => {
            let n = 1.0 / (1.0 + 2.0 * H);
            m[0] = vec![n, n * H, 0.0, n * H, 0.0, 0.0];
            m[1] = vec![0.0, n * H, n, 0.0, n * H, 0.0];
        }
        (6, _) => {
            // Device: L R C LFE Ls Rs.
            for (out, source) in [(0, 0), (1, 2), (2, 1), (3, 5), (4, 3), (5, 4)] {
                m[out][source] = 1.0;
            }
        }
        _ => {
            for (c, row) in m.iter_mut().enumerate().take(from) {
                row[c] = 1.0;
            }
        }
    }
    m
}

/// Converts interleaved 16-bit PCM to the output layout (channel matrix,
/// then linear resampling) and places it at `start` seconds with `gain`.
pub fn prepare(
    pcm: &[i16],
    channels: usize,
    rate: u32,
    out_channels: usize,
    out_rate: u32,
    start: f64,
    gain: f32,
) -> MixTrack {
    let channels = channels.max(1);
    let frames = pcm.len() / channels;
    let matrix = channel_matrix(channels, out_channels);
    // Channels first: fewer samples to resample.
    let mut folded = vec![0.0f32; frames * out_channels];
    for (f, frame) in pcm.chunks_exact(channels).enumerate() {
        for (o, row) in matrix.iter().enumerate() {
            let mut sum = 0.0;
            for (gain, &s) in row.iter().zip(frame) {
                sum += gain * s as f32;
            }
            folded[f * out_channels + o] = sum / 32768.0;
        }
    }
    let samples = if rate == out_rate || frames == 0 {
        folded
    } else {
        let out_frames = (frames as u64 * out_rate as u64 / rate.max(1) as u64) as usize;
        let step = rate as f64 / out_rate as f64;
        let mut out = Vec::with_capacity(out_frames * out_channels);
        for f in 0..out_frames {
            let t = f as f64 * step;
            let i = (t as usize).min(frames - 1);
            let j = (i + 1).min(frames - 1);
            let w = (t - i as f64) as f32;
            for c in 0..out_channels {
                let a = folded[i * out_channels + c];
                let b = folded[j * out_channels + c];
                out.push(a + (b - a) * w);
            }
        }
        out
    };
    MixTrack {
        samples,
        start: frame_at(start, out_rate),
        gain,
    }
}

/// Mixes the tracks into `out` (interleaved, `channels` per frame) from
/// output frame `position`, at `gain`, clipped to full scale.
pub fn mix(tracks: &[MixTrack], channels: usize, position: i64, gain: f32, out: &mut [f32]) {
    out.fill(0.0);
    let channels = channels.max(1);
    let frames = (out.len() / channels) as i64;
    for track in tracks {
        let len = (track.samples.len() / channels) as i64;
        // Overlap of [position, position + frames) with the track.
        let from = position.max(track.start);
        let to = (position + frames).min(track.start + len);
        if from >= to {
            continue;
        }
        let g = gain * track.gain;
        let src = &track.samples[((from - track.start) as usize) * channels..]
            [..((to - from) as usize) * channels];
        let dst = &mut out[((from - position) as usize) * channels..][..src.len()];
        for (d, s) in dst.iter_mut().zip(src) {
            *d += s * g;
        }
    }
    for s in out {
        *s = s.clamp(-1.0, 1.0);
    }
}

const NO_SEEK: i64 = i64::MIN;

/// Playback state shared by the interface and the audio callback.
pub struct MixState {
    pub channels: usize,
    pub rate: u32,
    tracks: Mutex<Arc<Vec<MixTrack>>>,
    /// Next frame to render.
    position: AtomicI64,
    /// Seek asked by the interface, taken by the next callback.
    seek: AtomicI64,
    /// Rendering stops at this frame.
    end: AtomicI64,
    playing: AtomicBool,
    /// Master gain (`f32` bits).
    gain: AtomicU32,
    /// First frame of the last buffer rendered while playing, when it was
    /// rendered (ns since `epoch`), its length and the output latency.
    clock_frame: AtomicI64,
    clock_stamp: AtomicU64,
    clock_frames: AtomicU64,
    latency_ns: AtomicU64,
    epoch: Instant,
}

impl MixState {
    pub fn new(channels: usize, rate: u32) -> Self {
        Self {
            channels: channels.max(1),
            rate: rate.max(1),
            tracks: Mutex::new(Arc::new(Vec::new())),
            position: AtomicI64::new(0),
            seek: AtomicI64::new(NO_SEEK),
            end: AtomicI64::new(i64::MAX),
            playing: AtomicBool::new(false),
            gain: AtomicU32::new(1.0f32.to_bits()),
            clock_frame: AtomicI64::new(0),
            clock_stamp: AtomicU64::new(0),
            clock_frames: AtomicU64::new(0),
            latency_ns: AtomicU64::new(0),
            epoch: Instant::now(),
        }
    }

    pub fn set_tracks(&self, tracks: Vec<MixTrack>) {
        *self.tracks.lock().unwrap_or_else(|e| e.into_inner()) = Arc::new(tracks);
    }

    /// Plays from `seconds` (the next callback takes it).
    pub fn seek(&self, seconds: f64) {
        self.seek
            .store(frame_at(seconds, self.rate).max(0), Ordering::Relaxed);
    }

    /// Stops rendering at `seconds` (the end of the cut-scene).
    pub fn set_end(&self, seconds: f64) {
        self.end
            .store(frame_at(seconds, self.rate), Ordering::Relaxed);
    }

    pub fn set_playing(&self, playing: bool) {
        self.playing.store(playing, Ordering::Relaxed);
    }

    /// Whether it plays: false after a pause or at the end.
    pub fn playing(&self) -> bool {
        self.playing.load(Ordering::Relaxed)
    }

    pub fn set_gain(&self, gain: f32) {
        self.gain.store(gain.to_bits(), Ordering::Relaxed);
    }

    fn now_ns(&self) -> u64 {
        self.epoch.elapsed().as_nanos() as u64
    }

    /// Time being heard, in seconds: the start of the last buffer plus the
    /// time since it was rendered (at most its length), less the output
    /// latency. A pending seek or a pause gives the position itself.
    pub fn time(&self) -> f64 {
        let seek = self.seek.load(Ordering::Relaxed);
        if seek != NO_SEEK {
            return seconds_at(seek, self.rate);
        }
        if !self.playing() {
            return seconds_at(self.position.load(Ordering::Relaxed), self.rate);
        }
        self.time_at(self.now_ns())
    }

    fn time_at(&self, now_ns: u64) -> f64 {
        let frame = self.clock_frame.load(Ordering::Relaxed);
        let stamp = self.clock_stamp.load(Ordering::Relaxed);
        let frames = self.clock_frames.load(Ordering::Relaxed);
        let latency = self.latency_ns.load(Ordering::Relaxed);
        let buffer = frames as f64 / self.rate as f64;
        let elapsed = now_ns.saturating_sub(stamp) as f64 * 1e-9;
        seconds_at(frame, self.rate) + elapsed.min(buffer) - latency as f64 * 1e-9
    }

    /// The audio callback: fills `out` (interleaved, [`Self::channels`] per
    /// frame) and advances the position. `latency` is the time from now
    /// until the buffer is heard, when the device tells it.
    pub fn render(&self, out: &mut [f32], latency: Option<Duration>) {
        self.render_at(out, latency, self.now_ns());
    }

    fn render_at(&self, out: &mut [f32], latency: Option<Duration>, now_ns: u64) {
        let seek = self.seek.swap(NO_SEEK, Ordering::Relaxed);
        if seek != NO_SEEK {
            self.position.store(seek, Ordering::Relaxed);
        }
        if !self.playing() {
            out.fill(0.0);
            return;
        }
        let position = self.position.load(Ordering::Relaxed);
        let frames = (out.len() / self.channels) as i64;
        // The interface may be replacing the tracks: silence for this buffer.
        match self.tracks.try_lock() {
            Ok(tracks) => {
                let tracks = Arc::clone(&tracks);
                let gain = f32::from_bits(self.gain.load(Ordering::Relaxed));
                mix(&tracks, self.channels, position, gain, out);
            }
            Err(_) => out.fill(0.0),
        }
        let end = self.end.load(Ordering::Relaxed);
        let mut next = position + frames;
        if next >= end {
            let keep = (end - position).clamp(0, frames) as usize * self.channels;
            out[keep..].fill(0.0);
            next = end.max(position);
            self.playing.store(false, Ordering::Relaxed);
        }
        self.position.store(next, Ordering::Relaxed);
        self.clock_frame.store(position, Ordering::Relaxed);
        self.clock_frames.store(frames as u64, Ordering::Relaxed);
        self.clock_stamp.store(now_ns, Ordering::Relaxed);
        if let Some(latency) = latency {
            self.latency_ns
                .store(latency.as_nanos() as u64, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(samples: Vec<f32>, start: i64, gain: f32) -> MixTrack {
        MixTrack {
            samples,
            start,
            gain,
        }
    }

    fn close(a: &[f32], b: &[f32]) {
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b) {
            assert!((x - y).abs() < 1e-6, "{a:?} != {b:?}");
        }
    }

    #[test]
    fn positions_become_frames() {
        assert_eq!(frame_at(0.0, 48000), 0);
        assert_eq!(frame_at(1.5, 48000), 72000);
        assert_eq!(frame_at(10.000_01, 44100), 441000);
        assert_eq!(frame_at(42.2, 32000), 1_350_400);
        assert_eq!(seconds_at(72000, 48000), 1.5);
        // A cut-scene position and back.
        for t in [0.25, 3.7, 125.1] {
            assert!((seconds_at(frame_at(t, 48000), 48000) - t).abs() < 1.0 / 48000.0);
        }
    }

    #[test]
    fn mixes_tracks_at_their_starts() {
        // Mono output; one track from frame 0, one from frame 3 at half volume.
        let tracks = [
            track(vec![0.1; 4], 0, 1.0),
            track(vec![0.4, 0.4, 0.4], 3, 0.5),
        ];
        let mut out = [9.0; 8];
        mix(&tracks, 1, 0, 1.0, &mut out);
        close(&out, &[0.1, 0.1, 0.1, 0.3, 0.2, 0.2, 0.0, 0.0]);
        // Starting in the middle: frames 2..6.
        let mut out = [0.0; 4];
        mix(&tracks, 1, 2, 1.0, &mut out);
        close(&out, &[0.1, 0.3, 0.2, 0.2]);
        // Before every track, and with a master gain.
        let mut out = [0.0; 2];
        mix(&tracks, 1, -2, 2.0, &mut out);
        close(&out, &[0.0, 0.0]);
        mix(&tracks, 1, -1, 2.0, &mut out);
        close(&out, &[0.0, 0.2]);
    }

    #[test]
    fn mixes_stereo_and_clips() {
        let tracks = [
            track(vec![0.5, -0.5, 0.75, -0.75], 1, 1.0),
            track(vec![0.75, -0.75], 2, 1.0),
        ];
        let mut out = [0.0; 6];
        mix(&tracks, 2, 0, 1.0, &mut out);
        assert_eq!(out, [0.0, 0.0, 0.5, -0.5, 1.0, -1.0]);
    }

    #[test]
    fn folds_5_1_to_stereo() {
        let m = channel_matrix(6, 2);
        // Centre equally on both sides, LFE dropped, nothing over full scale.
        assert_eq!(m[0][1], m[1][1]);
        assert_eq!(m[0][5], 0.0);
        assert!((m[0].iter().sum::<f32>() - 1.0).abs() < 1e-6);
        assert_eq!(m[0][2], 0.0, "right stays right");
        let track = prepare(&[0, 16384, 0, 0, 0, 0], 6, 48000, 2, 48000, 0.0, 1.0);
        assert_eq!(track.samples.len(), 2);
        assert_eq!(track.samples[0], track.samples[1]);
        assert!(track.samples[0] > 0.1);
        assert_eq!(channel_matrix(1, 2), [vec![1.0], vec![1.0]]);
        assert_eq!(channel_matrix(2, 1), [vec![0.5, 0.5]]);
        let to_51 = channel_matrix(6, 6);
        assert_eq!(to_51[2][1], 1.0, "centre to the device's centre");
    }

    #[test]
    fn resamples_and_places() {
        // 32 kHz mono ramp to 48 kHz stereo at 2 s.
        let pcm: Vec<i16> = (0..32).map(|i| i * 1000).collect();
        let t = prepare(&pcm, 1, 32000, 2, 48000, 2.0, 0.5);
        assert_eq!(t.start, 96000);
        assert_eq!(t.gain, 0.5);
        assert_eq!(t.samples.len(), 48 * 2);
        // Output frame 3 is source frame 2.
        assert!((t.samples[6] - 2000.0 / 32768.0).abs() < 1e-6);
        assert_eq!(t.samples[6], t.samples[7]);
        // Output frame 1 is two thirds of the way to source frame 1.
        assert!((t.samples[2] - 1000.0 * 2.0 / 3.0 / 32768.0).abs() < 1e-4);
    }

    #[test]
    fn the_callback_follows_seeks_pauses_and_the_end() {
        let state = MixState::new(1, 100);
        state.set_tracks(vec![track(
            (0..1000).map(|i| i as f32 / 1000.0).collect(),
            0,
            1.0,
        )]);
        state.set_end(5.0);
        let mut out = [0.0; 10];
        // Paused: silence, no advance.
        state.render_at(&mut out, None, 0);
        assert_eq!(out, [0.0; 10]);
        assert_eq!(state.time(), 0.0);
        // Seek to 2 s and play: frames 200..210.
        state.seek(2.0);
        assert_eq!(state.time(), 2.0, "a pending seek is the time");
        state.set_playing(true);
        state.render_at(&mut out, None, 1_000_000_000);
        assert_eq!(out[0], 0.2);
        assert!((out[9] - 0.209).abs() < 1e-6);
        // The clock: start of the buffer plus the time since, at most the buffer.
        assert_eq!(state.time_at(1_000_000_000), 2.0);
        assert!((state.time_at(1_050_000_000) - 2.05).abs() < 1e-9);
        assert!((state.time_at(2_000_000_000) - 2.1).abs() < 1e-9);
        // Latency puts the heard time back.
        state.render_at(&mut out, Some(Duration::from_millis(30)), 2_000_000_000);
        assert!((state.time_at(2_000_000_000) - (2.1 - 0.03)).abs() < 1e-9);
        // Pause keeps the position.
        state.set_playing(false);
        assert_eq!(state.time(), 2.2);
        // The end: the buffer is cut and playing stops.
        state.seek(4.95);
        state.set_playing(true);
        state.render_at(&mut out, None, 3_000_000_000);
        assert!((out[4] - 0.499).abs() < 1e-6);
        assert_eq!(out[5..], [0.0; 5]);
        assert!(!state.playing());
        assert_eq!(state.time(), 5.0);
    }
}

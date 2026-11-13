//! Sound playback through the default output device (cargo feature `audio`,
//! on by default).
//!
//! Opening the device never takes the window down: no device, a device that
//! refuses the stream or even a panic inside the audio backend comes back as
//! an error message, and the caller goes on without sound.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// A sound being played; dropping it stops playback.
pub struct Playback {
    _stream: cpal::Stream,
    position: Arc<AtomicUsize>,
    len: usize,
}

impl Playback {
    pub fn finished(&self) -> bool {
        self.position.load(Ordering::Relaxed) >= self.len
    }

    /// Played fraction, 0..1.
    pub fn progress(&self) -> f32 {
        (self.position.load(Ordering::Relaxed) as f32 / self.len.max(1) as f32).min(1.0)
    }
}

/// Runs `open`, turning a panic of the audio backend into an error.
fn guarded<T>(open: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(open)).unwrap_or_else(|payload| {
        let message = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".to_owned());
        Err(format!("the audio backend failed: {message}"))
    })
}

/// Starts playing interleaved 16-bit PCM.
pub fn play(samples: &[i16], channels: u16, sample_rate: u32) -> Result<Playback, String> {
    guarded(|| play_on_device(samples, channels, sample_rate))
}

fn play_on_device(samples: &[i16], channels: u16, sample_rate: u32) -> Result<Playback, String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("no audio output device")?;
    let supported = device
        .default_output_config()
        .map_err(|e| format!("cannot open the default output device ({e})"))?;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let buffer = Arc::new(convert(
        samples,
        channels.max(1) as usize,
        sample_rate.max(1),
        config.channels as usize,
        config.sample_rate.0,
    ));
    let position = Arc::new(AtomicUsize::new(0));
    let stream = match format {
        cpal::SampleFormat::F32 => build::<f32>(&device, &config, &buffer, &position),
        cpal::SampleFormat::I16 => build::<i16>(&device, &config, &buffer, &position),
        cpal::SampleFormat::U16 => build::<u16>(&device, &config, &buffer, &position),
        cpal::SampleFormat::I32 => build::<i32>(&device, &config, &buffer, &position),
        other => return Err(format!("unsupported output format {other}")),
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(Playback {
        _stream: stream,
        position,
        len: buffer.len(),
    })
}

fn build<T: cpal::SizedSample + cpal::FromSample<f32>>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    buffer: &Arc<Vec<f32>>,
    position: &Arc<AtomicUsize>,
) -> Result<cpal::Stream, cpal::BuildStreamError> {
    let buffer = buffer.clone();
    let position = position.clone();
    device.build_output_stream(
        config,
        move |out: &mut [T], _| {
            let start = position.fetch_add(out.len(), Ordering::Relaxed);
            for (i, sample) in out.iter_mut().enumerate() {
                let value = buffer.get(start + i).copied().unwrap_or(0.0);
                *sample = T::from_sample(value);
            }
        },
        |error| eprintln!("audio: {error}"),
        None,
    )
}

/// A mixer playing on the default output device; dropping it closes the
/// device.
pub struct MixOutput {
    _stream: cpal::Stream,
    pub state: Arc<crate::audio_mix::MixState>,
}

/// Opens the default output device for a mixer. It renders silence until
/// the state has tracks and plays; the tracks must be in the state's
/// channels and rate (the device's).
pub fn open_mix() -> Result<MixOutput, String> {
    guarded(open_mix_on_device)
}

fn open_mix_on_device() -> Result<MixOutput, String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("no audio output device")?;
    let supported = device
        .default_output_config()
        .map_err(|e| format!("cannot open the default output device ({e})"))?;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let state = Arc::new(crate::audio_mix::MixState::new(
        config.channels as usize,
        config.sample_rate.0,
    ));
    let stream = match format {
        cpal::SampleFormat::F32 => build_mix::<f32>(&device, &config, &state),
        cpal::SampleFormat::I16 => build_mix::<i16>(&device, &config, &state),
        cpal::SampleFormat::U16 => build_mix::<u16>(&device, &config, &state),
        cpal::SampleFormat::I32 => build_mix::<i32>(&device, &config, &state),
        other => return Err(format!("unsupported output format {other}")),
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(MixOutput {
        _stream: stream,
        state,
    })
}

fn build_mix<T: cpal::SizedSample + cpal::FromSample<f32>>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    state: &Arc<crate::audio_mix::MixState>,
) -> Result<cpal::Stream, cpal::BuildStreamError> {
    let state = state.clone();
    let mut buffer: Vec<f32> = Vec::new();
    device.build_output_stream(
        config,
        move |out: &mut [T], info: &cpal::OutputCallbackInfo| {
            buffer.resize(out.len(), 0.0);
            let stamp = info.timestamp();
            state.render(&mut buffer, stamp.playback.duration_since(&stamp.callback));
            for (o, s) in out.iter_mut().zip(&buffer) {
                *o = T::from_sample(*s);
            }
        },
        |error| eprintln!("audio: {error}"),
        None,
    )
}

/// Interleaved PCM resampled linearly to `out_rate` with `out_channels`
/// channels (mono is duplicated, extra channels are dropped or silent).
pub fn convert(
    samples: &[i16],
    channels: usize,
    rate: u32,
    out_channels: usize,
    out_rate: u32,
) -> Vec<f32> {
    let frames = samples.len() / channels;
    if frames == 0 || out_channels == 0 {
        return Vec::new();
    }
    let out_frames = (frames as u64 * out_rate as u64 / rate as u64).max(1) as usize;
    let step = rate as f64 / out_rate as f64;
    let at = |frame: usize, channel: usize| samples[frame * channels + channel] as f32 / 32768.0;
    let mut out = Vec::with_capacity(out_frames * out_channels);
    for f in 0..out_frames {
        let t = f as f64 * step;
        let i = (t as usize).min(frames - 1);
        let j = (i + 1).min(frames - 1);
        let w = (t - i as f64) as f32;
        for c in 0..out_channels {
            let source = if channels == 1 {
                0
            } else if c < channels {
                c
            } else {
                out.push(0.0);
                continue;
            };
            out.push(at(i, source) * (1.0 - w) + at(j, source) * w);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_rate_and_channels() {
        let mono = [0i16, 16384, -16384, 0];
        let out = convert(&mono, 1, 22050, 2, 44100);
        assert_eq!(out.len(), 8 * 2);
        assert_eq!(out[0], out[1]);
        assert_eq!(out[4], 0.5);
        assert_eq!(out[2], 0.25, "halfway between the first two samples");
        let stereo = [1000i16, -1000, 2000, -2000];
        let out = convert(&stereo, 2, 48000, 1, 48000);
        assert_eq!(out.len(), 2);
        assert!(out[0] > 0.0);
    }

    #[test]
    fn a_panic_of_the_backend_is_an_error() {
        let result: Result<(), String> = guarded(|| panic!("no card"));
        assert_eq!(result.unwrap_err(), "the audio backend failed: no card");
        assert_eq!(guarded(|| Ok(5)), Ok(5));
    }

    /// Opens the default output device for a mixer and lets it render a
    /// quiet tone for 0.3 s: the position must advance.
    #[test]
    #[ignore = "needs an audio output device"]
    fn mixes_on_the_default_device() {
        let output = open_mix().unwrap();
        let state = &output.state;
        let tone: Vec<i16> = (0..state.rate)
            .map(|i| ((i as f32 * 0.05).sin() * 300.0) as i16)
            .collect();
        state.set_tracks(vec![crate::audio_mix::prepare(
            &tone,
            1,
            state.rate,
            state.channels,
            state.rate,
            0.0,
            1.0,
        )]);
        state.set_end(0.3);
        state.seek(0.0);
        state.set_playing(true);
        let started = std::time::Instant::now();
        while state.playing() {
            assert!(started.elapsed().as_secs() < 5, "playback does not advance");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!((state.time() - 0.3).abs() < 1e-3);
    }

    /// Opens the default output device and plays 0.2 s of silence.
    #[test]
    #[ignore = "needs an audio output device"]
    fn plays_on_the_default_device() {
        let playback = play(&[0; 4410], 1, 22050).unwrap();
        let started = std::time::Instant::now();
        while !playback.finished() {
            assert!(started.elapsed().as_secs() < 5, "playback does not advance");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}

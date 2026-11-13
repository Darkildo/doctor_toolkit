//! Sound import commands: `sound-encode`, `sound-import`, `viv-list`.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Args;

use doctor_toolkit::container::big::{BigArchive, find_archives, normalize_name};
use doctor_toolkit::game::GameInstance;
use doctor_toolkit::resource::audio::{Sample, parse_sbk};
use doctor_toolkit::resource::audio_encode::{
    SampleSpec, build_inline, build_snu, encode_like, fit_pcm, quality, replace_sbk_sound,
};
use doctor_toolkit::resource::video::import::read_audio_file;
use doctor_toolkit::resource::video::input::Pcm;
use doctor_toolkit::workspace::{FileKey, ResourceKey};

use crate::workspace_cmd::{self, EditMode};

/// Writes a line to stdout and exits quietly when the pipe is closed.
macro_rules! out {
    ($($arg:tt)*) => {
        if writeln!(std::io::stdout(), $($arg)*).is_err() {
            std::process::exit(0);
        }
    };
}

#[derive(Args)]
pub struct EncodeArgs {
    /// Input sound: WAV, or any format ffmpeg reads (when it is installed)
    pub input: PathBuf,
    /// Output: `.snu` (streamed, as in the `.viv` archives) or `.snr` (SNR
    /// header with its SNS blocks, as in `sbk` banks)
    #[arg(short, long)]
    pub output: PathBuf,
    /// Take channels, rate and loop of this `.snu` file
    #[arg(long, conflicts_with_all = ["rate", "channels"])]
    pub like: Option<PathBuf>,
    /// Sample rate [default: the input's, at most 48000]
    #[arg(long)]
    pub rate: Option<u32>,
    /// Channels [default: the input's]
    #[arg(long)]
    pub channels: Option<u8>,
    /// Loop the whole sound
    #[arg(long = "loop")]
    pub looped: bool,
}

#[derive(Args)]
pub struct ImportArgs {
    pub game_dir: PathBuf,
    /// Sound to replace: a bank sound as `<sbk key>#<sound>` (for example
    /// `gf2/frontend:sbk:6d4bafbd#12`, the number as in `audio` file names),
    /// or a streamed file of the `.viv` archives
    /// (`audiostreams\e_screen\title_screen_01.exa.snu`, `/` works too)
    pub target: String,
    /// New sound: WAV, or any format ffmpeg reads; converted to the format of
    /// the sound it replaces (channels, sample rate, loop)
    pub input: PathBuf,
    #[command(flatten)]
    pub mode: EditMode,
}

#[derive(Args)]
pub struct VivListArgs {
    pub game_dir: PathBuf,
    /// Only entries whose name contains this text
    pub filter: Option<String>,
}

fn read_input(path: &Path) -> Result<Pcm> {
    let pcm = read_audio_file(path).with_context(|| format!("reading {}", path.display()))?;
    if pcm.frames() == 0 {
        bail!("{} holds no audio", path.display());
    }
    Ok(pcm)
}

/// `sound-encode`.
pub fn encode(args: &EncodeArgs) -> Result<()> {
    let pcm = read_input(&args.input)?;
    let snr = args
        .output
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("snr"));
    let (spec, template) = match &args.like {
        Some(path) => {
            let data = fs::read(path).with_context(|| path.display().to_string())?;
            let header = Sample::parse_snu(&data)?.header;
            (SampleSpec::like(&header), Some(data))
        }
        None => {
            let channels = args.channels.unwrap_or(pcm.channels.min(64) as u8);
            let spec = SampleSpec {
                channels,
                sample_rate: args.rate.unwrap_or(pcm.sample_rate.min(48000)),
                kind: if snr { 0 } else { 1 },
                loop_start: args.looped.then_some(0),
            };
            (spec, None)
        }
    };
    let fitted = fit_pcm(&pcm, spec.channels, spec.sample_rate);
    let data = if snr {
        build_inline(&fitted, &spec)?
    } else {
        build_snu(&fitted, &spec, template.as_deref())?
    };
    let decoded = if snr {
        Sample::parse_inline(&data, 0)?.decode()?
    } else {
        Sample::parse_snu(&data)?.decode()?
    };
    let q = quality(&fitted.samples, &decoded);
    fs::write(&args.output, &data)?;
    out!(
        "{}: {} Hz, {} channel(s), {:.2} s, {} bytes; round trip SNR {:.1} dB",
        args.output.display(),
        spec.sample_rate,
        spec.channels,
        fitted.frames() as f64 / spec.sample_rate as f64,
        data.len(),
        q.snr_db
    );
    Ok(())
}

/// `sound-import`.
pub fn import(args: &ImportArgs) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let pcm = read_input(&args.input)?;
    let mut ws = workspace_cmd::open_for_edit(&game, &args.mode)?;
    if let Some((key, sound)) = args.target.rsplit_once('#') {
        let key: ResourceKey = key.parse()?;
        let sound: u32 = sound.parse().context("the sound number after `#`")?;
        let stream_index = game
            .find_stream(&key.stream)
            .with_context(|| format!("no stream {}", key.stream))?;
        let stream = doctor_toolkit::game::StreamFile::open(&game.streams()[stream_index].path)?;
        let index = key
            .find_in(stream.layout())
            .with_context(|| format!("{key} is not in its stream"))?;
        let bank = ws.current_data(&game, stream_index, index)?;
        let sounds = parse_sbk(&bank)?;
        let Some(position) = sounds.iter().position(|s| s.index == sound) else {
            bail!(
                "{key} has no sound {sound} (sounds {})",
                sounds
                    .iter()
                    .map(|s| s.index.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        };
        let old = sounds[position].sample.header;
        let (fitted, sample) = encode_like(&pcm, &old)?;
        let new_bank = replace_sbk_sound(&bank, position, &sample)?;
        let label = format!(
            "Import sound {sound} of {} ({:.2} s)",
            key.stream,
            fitted.frames() as f64 / old.sample_rate as f64
        );
        ws.set_resource(&game, stream_index, index, new_bank, &label)?;
        out!(
            "sound {sound}: {} Hz, {} channel(s), {:.2} s (was {:.2} s)",
            old.sample_rate,
            old.channels,
            fitted.frames() as f64 / old.sample_rate as f64,
            old.sample_count as f64 / old.sample_rate as f64
        );
        workspace_cmd::finish_edit(&game, &mut ws, &args.mode, vec![key.stream.clone()])
    } else {
        let key = FileKey::archive_entry(&args.target);
        let original = ws.current_file(&game, &key)?;
        let data = doctor_toolkit::resource::audio_encode::encode_snu_like(&pcm, &original)?;
        let header = Sample::parse_snu(&data)?.header;
        let label = format!("Import sound {}", key.name());
        ws.set_file(&game, &key, data, &label)?;
        out!(
            "{}: {} Hz, {} channel(s), {:.2} s",
            key.name(),
            header.sample_rate,
            header.channels,
            header.sample_count as f64 / header.sample_rate as f64
        );
        workspace_cmd::finish_edit(
            &game,
            &mut ws,
            &args.mode,
            vec![doctor_toolkit::workspace::FILES.to_owned()],
        )
    }
}

/// `viv-list`.
pub fn viv_list(args: &VivListArgs) -> Result<()> {
    let filter = args.filter.as_deref().map(normalize_name);
    let mut count = 0usize;
    for path in find_archives(&args.game_dir)? {
        let archive = BigArchive::open(&path)?;
        let file = path.file_name().unwrap_or_default().to_string_lossy();
        for entry in &archive.entries {
            if filter
                .as_ref()
                .is_some_and(|f| !normalize_name(&entry.name).contains(f))
            {
                continue;
            }
            out!(
                "{file}  {:>10}  {:>9}  {}",
                entry.offset,
                entry.size,
                entry.name
            );
            count += 1;
        }
    }
    out!("{count} entr(ies)");
    Ok(())
}

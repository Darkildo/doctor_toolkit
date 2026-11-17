//! Sound bank structure commands: `sbk-info`, `sbk-edit`, `sound-add`.

use std::io::Write as _;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::resource::audio::Sample;
use doctor_toolkit::resource::sbk::{SoundBank, cmd};
use doctor_toolkit::sound_edit::{self, SbkEdit, SoundAdd};
use doctor_toolkit::workspace::{ResourceKey, Workspace};

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
pub struct InfoArgs {
    pub game_dir: PathBuf,
    /// The bank, as a resource key (`gf2/frontend:sbk:6d4bafbd`)
    pub key: String,
    /// Only this emitter: `.emx` name, hexadecimal hash or `#number`
    #[arg(long)]
    pub emitter: Option<String>,
    /// Print the whole structure as JSON
    #[arg(long)]
    pub json: bool,
    /// Workspace directory [default: <GAME_DIR>/_doctor_toolkit/workspace]
    #[arg(long, value_name = "DIR")]
    pub workspace_dir: Option<PathBuf>,
}

#[derive(Args)]
pub struct EditArgs {
    pub game_dir: PathBuf,
    /// The bank, as a resource key (`gf2/frontend:sbk:6d4bafbd`)
    pub key: String,
    /// The emitter: `.emx` name, hexadecimal hash or `#number`
    #[arg(long)]
    pub emitter: String,
    /// Track volume, 0–1
    #[arg(long)]
    pub volume: Option<f32>,
    /// Fixed pitch of every variation (1 = as recorded, 2 = an octave up);
    /// sets the pitch bend commands
    #[arg(long)]
    pub pitch: Option<f32>,
    /// Random pitch spread, 0–1
    #[arg(long)]
    pub pitch_range: Option<f32>,
    /// Random attenuation spread, 0–1
    #[arg(long)]
    pub attn_range: Option<f32>,
    /// Velocity of every key on, 0–127
    #[arg(long)]
    pub velocity: Option<i32>,
    /// Distance of full attenuation
    #[arg(long)]
    pub falloff: Option<f32>,
    /// Voice stealing priority, 0–255
    #[arg(long)]
    pub priority: Option<u8>,
    /// Most simultaneous performances
    #[arg(long)]
    pub poly_max: Option<u8>,
    /// Emitter flags (3D 0x01, doppler 0x02, reverb from distance 0x04,
    /// attenuation curves 0x08, 0x10, 0x20); decimal or 0x…
    #[arg(long, value_parser = parse_u32)]
    pub flags: Option<u32>,
    /// Variation choice: 0 explicit, other values let the game pick
    #[arg(long)]
    pub select: Option<u8>,
    /// Mixer category (hexadecimal hash, as other emitters use)
    #[arg(long, value_parser = parse_hex)]
    pub mix: Option<u32>,
    /// Remove this variation (its sample stays in the bank)
    #[arg(long)]
    pub remove_variation: Option<usize>,
    #[command(flatten)]
    pub mode: EditMode,
}

#[derive(Args)]
pub struct AddArgs {
    pub game_dir: PathBuf,
    /// The bank, as a resource key (`gf2/frontend:sbk:6d4bafbd`)
    pub key: String,
    /// Sounds to add: WAV, or any format ffmpeg reads
    #[arg(required = true)]
    pub inputs: Vec<PathBuf>,
    /// Add the sounds as new variations of this emitter (`.emx` name,
    /// hexadecimal hash or `#number`)
    #[arg(long, conflicts_with = "new")]
    pub emitter: Option<String>,
    /// Add a new emitter with this name (`.emx` is appended); the game plays
    /// it by this name (`PlayEmx` of scripts, `audio: play` of animations)
    #[arg(long, required_unless_present = "emitter")]
    pub new: Option<String>,
    /// For `--new`: copy the parameters of this emitter [default: the first]
    #[arg(long, requires = "new")]
    pub like: Option<String>,
    /// Sample rate of the new sounds [default: that of the emitter's first sound]
    #[arg(long)]
    pub rate: Option<u32>,
    /// Channels of the new sounds [default: that of the emitter's first sound]
    #[arg(long)]
    pub channels: Option<u8>,
    #[command(flatten)]
    pub mode: EditMode,
}

fn parse_u32(text: &str) -> Result<u32, String> {
    match text.strip_prefix("0x") {
        Some(hex) => u32::from_str_radix(hex, 16),
        None => text.parse(),
    }
    .map_err(|e| e.to_string())
}

fn parse_hex(text: &str) -> Result<u32, String> {
    u32::from_str_radix(text.trim_start_matches("0x"), 16).map_err(|e| e.to_string())
}

fn find_emitter(bank: &SoundBank, text: &str) -> Result<usize> {
    bank.find_emitter(text)
        .with_context(|| format!("the bank has no emitter {text}"))
}

/// `sbk-info`.
pub fn info(args: &InfoArgs) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let ws = Workspace::for_game(&game, args.workspace_dir.as_deref())?;
    let bank_ref = sound_edit::load_bank(&game, &ws, &args.key.parse()?)?;
    let bank = &bank_ref.bank;
    let only = args
        .emitter
        .as_deref()
        .map(|e| find_emitter(bank, e))
        .transpose()?;
    if args.json {
        let value = match only {
            Some(i) => serde_json::to_value(&bank.emitters[i])?,
            None => serde_json::to_value(bank)?,
        };
        out!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    if only.is_none() {
        out!(
            "{}: {} emitter(s), {} instrument(s), {} sample(s), session guid {:08x}",
            bank_ref.key,
            bank.emitters.len(),
            bank.instruments.len(),
            bank.samples.len(),
            bank.session.guid
        );
    }
    for i in only.map_or(0..bank.emitters.len(), |i| i..i + 1) {
        let e = &bank.emitters[i];
        let t = &e.track;
        out!(
            "#{i} {}  flags {:02x} priority {} poly {} falloff {} mix {:08x}{}",
            bank.emitter_label(i),
            e.flags,
            e.priority,
            e.poly_max,
            e.falloff,
            e.mix_category,
            if e.duck_category != 0 {
                format!(" duck {:08x}", e.duck_category)
            } else {
                String::new()
            }
        );
        out!(
            "    track: volume {} pitch range {} attn range {} select {} voices {} flags {:x}",
            t.volume,
            t.pitch_range,
            t.attn_range,
            t.select,
            t.voice_poly,
            t.flags
        );
        let instrument = bank.instrument_of(i);
        if let Some(ins) = instrument {
            for (k, tone) in bank.instruments[ins].tones.iter().enumerate() {
                let what = bank
                    .samples
                    .get(tone.sample as usize)
                    .and_then(|s| Sample::parse_inline(&s.bytes, 0).ok())
                    .map(|s| {
                        format!(
                            "{} Hz, {} ch, {:.2} s",
                            s.header.sample_rate,
                            s.header.channels,
                            s.duration()
                        )
                    })
                    .unwrap_or_else(|| "missing".into());
                out!("    tone {k}: sample {} ({what})", tone.sample);
            }
        } else {
            out!("    no instrument ({:08x})", t.instrument);
        }
        for (v, variation) in t.variations.iter().enumerate() {
            let commands: Vec<String> = variation
                .commands
                .iter()
                .filter(|c| c.id != cmd::NONE || c.delay != 0)
                .map(|c| match c.id {
                    cmd::KEY_ON => format!("+{}ms key on {} vel {}", c.delay, c.arg8, c.arg32),
                    cmd::KEY_OFF => format!("+{}ms key off {}", c.delay, c.arg8),
                    cmd::PITCH_BEND | cmd::MOD_PITCH => format!(
                        "+{}ms {} {:.3}",
                        c.delay,
                        cmd::name(c.id),
                        c.arg32 as f32 / 65536.0
                    ),
                    cmd::END => format!("+{}ms end (loop, skip {})", c.delay, c.arg8),
                    _ => format!("+{}ms {} {} {}", c.delay, cmd::name(c.id), c.arg8, c.arg32),
                })
                .collect();
            out!("    variation {v}: {}", commands.join(", "));
        }
    }
    for problem in bank.validate() {
        out!("problem: {problem}");
    }
    Ok(())
}

/// `sbk-edit`.
pub fn edit(args: &EditArgs) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let mut ws = workspace_cmd::open_for_edit(&game, &args.mode)?;
    let key: ResourceKey = args.key.parse()?;
    let label = sound_edit::edit_bank(
        &game,
        &mut ws,
        &key,
        &SbkEdit {
            emitter: args.emitter.clone(),
            volume: args.volume,
            pitch: args.pitch,
            pitch_range: args.pitch_range,
            attn_range: args.attn_range,
            velocity: args.velocity,
            falloff: args.falloff,
            priority: args.priority,
            poly_max: args.poly_max,
            flags: args.flags,
            select: args.select,
            mix: args.mix,
            remove_variation: args.remove_variation,
        },
    )?;
    out!("{label}");
    workspace_cmd::finish_edit(&game, &mut ws, &args.mode, vec![key.stream.clone()])
}

/// `sound-add`.
pub fn add(args: &AddArgs) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let mut ws = workspace_cmd::open_for_edit(&game, &args.mode)?;
    let key: ResourceKey = args.key.parse()?;
    let done = sound_edit::add_sounds(
        &game,
        &mut ws,
        &key,
        &SoundAdd {
            files: args.inputs.clone(),
            emitter: args.emitter.clone(),
            new: args.new.clone(),
            like: args.like.clone(),
            rate: args.rate,
            channels: args.channels,
        },
    )?;
    for s in &done.sounds {
        out!(
            "{}: {} Hz, {} channel(s), {:.2} s",
            s.file,
            s.sample_rate,
            s.channels,
            s.seconds
        );
    }
    out!("{}", done.operation);
    workspace_cmd::finish_edit(&game, &mut ws, &args.mode, vec![done.stream])
}

//! Sound bank structure commands: `sbk-info`, `sbk-edit`, `sound-add`.

use std::io::Write as _;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Args;

use doctor_toolkit::game::{GameInstance, StreamFile};
use doctor_toolkit::resource::audio::{Sample, TYPE_SBK};
use doctor_toolkit::resource::audio_encode::encode_like;
use doctor_toolkit::resource::sbk::{SoundBank, cmd, emx_name, name_hash};
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

/// A bank of the game, as the workspace has it.
struct BankRef {
    key: ResourceKey,
    stream_index: usize,
    index: usize,
    data: Vec<u8>,
}

fn load_bank(game: &GameInstance, ws: &Workspace, key: &str) -> Result<BankRef> {
    let key: ResourceKey = key.parse()?;
    if key.type_id != TYPE_SBK {
        bail!("{key} is not an sbk resource");
    }
    let stream_index = game
        .find_stream(&key.stream)
        .with_context(|| format!("no stream {}", key.stream))?;
    let stream = StreamFile::open(&game.streams()[stream_index].path)?;
    let index = key
        .find_in(stream.layout())
        .with_context(|| format!("{key} is not in its stream"))?;
    let data = ws.current_data(game, stream_index, index)?;
    Ok(BankRef {
        key,
        stream_index,
        index,
        data,
    })
}

fn find_emitter(bank: &SoundBank, text: &str) -> Result<usize> {
    bank.find_emitter(text)
        .with_context(|| format!("the bank has no emitter {text}"))
}

/// Saves an edited bank: refuses problems the original did not have.
fn save(
    game: &GameInstance,
    ws: &mut Workspace,
    bank_ref: &BankRef,
    before: &SoundBank,
    bank: &SoundBank,
    label: &str,
) -> Result<()> {
    let old = before.validate();
    let new: Vec<String> = bank
        .validate()
        .into_iter()
        .filter(|p| !old.contains(p))
        .collect();
    if !new.is_empty() {
        bail!("the edit breaks the bank:\n  {}", new.join("\n  "));
    }
    ws.set_resource(
        game,
        bank_ref.stream_index,
        bank_ref.index,
        bank.to_bytes(),
        label,
    )?;
    Ok(())
}

/// `sbk-info`.
pub fn info(args: &InfoArgs) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let ws = Workspace::for_game(&game, args.workspace_dir.as_deref())?;
    let bank_ref = load_bank(&game, &ws, &args.key)?;
    let bank = SoundBank::parse(&bank_ref.data)?;
    let only = args
        .emitter
        .as_deref()
        .map(|e| find_emitter(&bank, e))
        .transpose()?;
    if args.json {
        let value = match only {
            Some(i) => serde_json::to_value(&bank.emitters[i])?,
            None => serde_json::to_value(&bank)?,
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
    let bank_ref = load_bank(&game, &ws, &args.key)?;
    let before = SoundBank::parse(&bank_ref.data)?;
    let mut bank = before.clone();
    let i = find_emitter(&bank, &args.emitter)?;
    let mut changed = Vec::new();
    {
        let e = &mut bank.emitters[i];
        let mut set = |name: &str, done: bool| {
            if done {
                changed.push(name.to_owned());
            }
        };
        if let Some(v) = args.volume {
            e.track.volume = v;
        }
        set("volume", args.volume.is_some());
        if let Some(v) = args.pitch_range {
            e.track.pitch_range = v;
        }
        set("pitch range", args.pitch_range.is_some());
        if let Some(v) = args.attn_range {
            e.track.attn_range = v;
        }
        set("attenuation range", args.attn_range.is_some());
        if let Some(v) = args.falloff {
            if v.is_nan() || v <= 0.0 {
                bail!("falloff must be positive");
            }
            e.falloff = v;
        }
        set("falloff", args.falloff.is_some());
        if let Some(v) = args.priority {
            e.priority = v;
        }
        set("priority", args.priority.is_some());
        if let Some(v) = args.poly_max {
            if v == 0 {
                bail!("poly max must be at least 1");
            }
            e.poly_max = v;
        }
        set("poly max", args.poly_max.is_some());
        if let Some(v) = args.flags {
            e.flags = u8::try_from(v).context("flags are one byte")?;
        }
        set("flags", args.flags.is_some());
        if let Some(v) = args.select {
            e.track.select = v;
        }
        set("variation choice", args.select.is_some());
        if let Some(v) = args.mix {
            e.mix_category = v;
        }
        set("mixer category", args.mix.is_some());
    }
    if let Some(p) = args.pitch {
        bank.set_pitch(i, p)?;
        changed.push("pitch".into());
    }
    if let Some(v) = args.velocity {
        bank.set_velocity(i, v)?;
        changed.push("velocity".into());
    }
    if let Some(v) = args.remove_variation {
        bank.remove_variation(i, v)?;
        changed.push(format!("variation {v} removed"));
    }
    if changed.is_empty() {
        bail!("nothing to change: give --volume, --pitch, --velocity, … (see --help)");
    }
    let label = format!(
        "Edit sound {} of {}: {}",
        bank.emitter_label(i),
        bank_ref.key.stream,
        changed.join(", ")
    );
    save(&game, &mut ws, &bank_ref, &before, &bank, &label)?;
    out!("{label}");
    workspace_cmd::finish_edit(
        &game,
        &mut ws,
        &args.mode,
        vec![bank_ref.key.stream.clone()],
    )
}

/// `sound-add`.
pub fn add(args: &AddArgs) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let mut ws = workspace_cmd::open_for_edit(&game, &args.mode)?;
    let bank_ref = load_bank(&game, &ws, &args.key)?;
    let before = SoundBank::parse(&bank_ref.data)?;
    let mut bank = before.clone();
    if bank.emitters.is_empty() {
        bail!("the bank has no emitter to take the parameters from");
    }
    let template = match (&args.emitter, &args.like) {
        (Some(e), _) | (None, Some(e)) => find_emitter(&bank, e)?,
        (None, None) => 0,
    };
    // The format of the emitter's first sound.
    let header = bank
        .instrument_of(template)
        .and_then(|ins| bank.instruments[ins].tones.first())
        .and_then(|tone| bank.samples.get(tone.sample as usize))
        .and_then(|s| Sample::parse_inline(&s.bytes, 0).ok())
        .map(|s| s.header)
        .context("the emitter has no sound to take the format from")?;
    let mut header = header;
    if let Some(rate) = args.rate {
        header.sample_rate = rate;
    }
    if let Some(channels) = args.channels {
        header.channels = channels;
    }
    let mut encoded = Vec::new();
    for path in &args.inputs {
        let pcm = doctor_toolkit::resource::video::import::read_audio_file(path)
            .with_context(|| format!("reading {}", path.display()))?;
        if pcm.frames() == 0 {
            bail!("{} holds no audio", path.display());
        }
        let (fitted, data) = encode_like(&pcm, &header)?;
        out!(
            "{}: {} Hz, {} channel(s), {:.2} s",
            path.display(),
            header.sample_rate,
            header.channels,
            fitted.frames() as f64 / f64::from(header.sample_rate)
        );
        encoded.push(data);
    }
    let label = if let Some(name) = &args.new {
        let name = emx_name(name);
        let hash = name_hash(&name);
        // The game plays the first loaded bank that has the name.
        for other in other_banks(&game, &ws, &bank_ref)? {
            if other.1.emitters.iter().any(|e| e.template_id == hash) {
                bail!("{name} ({hash:08x}) is already an emitter of {}", other.0);
            }
        }
        let n = bank.add_emitter(&name, template, encoded)?;
        format!(
            "Add sound {name} ({hash:08x}, emitter #{n}, {} variation(s)) to {}",
            args.inputs.len(),
            bank_ref.key.stream
        )
    } else {
        let mut first = None;
        for data in encoded {
            let v = bank.add_variation(template, data)?;
            first.get_or_insert(v);
        }
        format!(
            "Add {} variation(s) to sound {} of {}",
            args.inputs.len(),
            bank.emitter_label(template),
            bank_ref.key.stream
        )
    };
    save(&game, &mut ws, &bank_ref, &before, &bank, &label)?;
    out!("{label}");
    workspace_cmd::finish_edit(
        &game,
        &mut ws,
        &args.mode,
        vec![bank_ref.key.stream.clone()],
    )
}

/// Every other `sbk` bank of the game, as the workspace has it.
fn other_banks(
    game: &GameInstance,
    ws: &Workspace,
    this: &BankRef,
) -> Result<Vec<(String, SoundBank)>> {
    let mut banks = Vec::new();
    for (stream_index, file) in game.streams().iter().enumerate() {
        let stream = StreamFile::open(&file.path)?;
        for (index, resource) in stream.layout().resources.iter().enumerate() {
            if resource.header.type_id != TYPE_SBK
                || (stream_index == this.stream_index && index == this.index)
            {
                continue;
            }
            let data = ws.current_data(game, stream_index, index)?;
            if let Ok(bank) = SoundBank::parse(&data) {
                banks.push((file.stream_name(), bank));
            }
        }
    }
    Ok(banks)
}

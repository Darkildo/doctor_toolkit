//! Voice commands: `voice-add`, `speech-bank`, `speech-events`,
//! `audio-config`, `mix`.

use std::io::Write as _;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Args;

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::hash::string_id;
use doctor_toolkit::resource::auc::AudioConfig;
use doctor_toolkit::resource::get::SpeechEvent;
use doctor_toolkit::resource::mix::{LEVEL_TIMES, Mix, TYPE_MIX};
use doctor_toolkit::resource::video::import::read_audio_file;
use doctor_toolkit::sound_edit::{self, MixEdit, SetSpeechEvents};
use doctor_toolkit::voice::{self, NewVoiceLine};
use doctor_toolkit::workspace::Workspace;

use crate::workspace_cmd::{self, EditMode};

/// Writes a line to stdout and exits quietly when the pipe is closed.
macro_rules! out {
    ($($arg:tt)*) => {
        if writeln!(std::io::stdout(), $($arg)*).is_err() {
            std::process::exit(0);
        }
    };
}

fn parse_u32(text: &str) -> Result<u32, String> {
    match text.strip_prefix("0x") {
        Some(hex) => u32::from_str_radix(hex, 16),
        None => text.parse(),
    }
    .map_err(|e| e.to_string())
}

fn pair(text: &str) -> Result<(String, String)> {
    let (a, b) = text
        .split_once('=')
        .with_context(|| format!("`{text}`: NAME=VALUE expected"))?;
    Ok((a.trim().to_owned(), b.to_owned()))
}

fn stream_name(game: &GameInstance, stream: usize) -> String {
    game.streams()[stream].stream_name()
}

#[derive(Args)]
pub struct AddArgs {
    pub game_dir: PathBuf,
    /// The chatter set of the speaker, as a resource key
    /// (`gf2/globals_global:chv:1a2b3c4d`; `voice-add GAME --speakers`
    /// lists them)
    #[arg(required_unless_present = "speakers")]
    pub chv: Option<String>,
    /// The recording: WAV, or any format ffmpeg reads; converted to the
    /// format of the speaker's lines (mono, 32 kHz)
    #[arg(required_unless_present = "speakers")]
    pub input: Option<PathBuf>,
    /// Speech event the line belongs to (`F2F_ENJOY_FLIGHT`); a new name
    /// makes a new event with its record in the event table
    #[arg(long, required_unless_present = "speakers")]
    pub event: Option<String>,
    /// Line name: lower-case letters, digits and `_`
    /// (`doc1_xxx_0000001`); the file becomes `d_<name>.exa.snu`
    #[arg(long, required_unless_present = "speakers")]
    pub name: Option<String>,
    /// Subtitle of a language (`en`, `cs`, `nl`, `pl`, `ru`, `es`), e.g.
    /// `--subtitle en="Hello."` (repeatable); other languages get the
    /// English text
    #[arg(long, value_name = "LANG=TEXT")]
    pub subtitle: Vec<String>,
    /// Weight among the lines of the event
    #[arg(long, default_value_t = 1.0)]
    pub weight: f32,
    /// Remove the other lines of the event: it always says the new one
    #[arg(long)]
    pub only: bool,
    /// Mix category by name (`DIALOG`, `CHATTER`) [default: that of the
    /// speaker's lines]
    #[arg(long)]
    pub category: Option<String>,
    /// List the chatter sets of the game (optionally those with this event)
    /// and exit
    #[arg(long, conflicts_with_all = ["chv", "input"])]
    pub speakers: bool,
    #[command(flatten)]
    pub mode: EditMode,
}

fn speakers(game: &GameInstance, ws: &Workspace, event: Option<&str>) -> Result<()> {
    let all = sound_edit::speakers(game, ws, event)?;
    for s in &all {
        out!(
            "{}  {} event(s), {} line(s)  voice: {}",
            s.key,
            s.events,
            s.lines,
            if s.voices.is_empty() {
                "-".to_owned()
            } else {
                s.voices.join(", ")
            }
        );
    }
    out!("{} chatter set(s)", all.len());
    Ok(())
}

/// `voice-add`.
pub fn add(args: &AddArgs) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    if args.speakers {
        let ws = Workspace::for_game(&game, args.mode.workspace_dir.as_deref())?;
        return speakers(&game, &ws, args.event.as_deref());
    }
    let (Some(chv), Some(input), Some(event), Some(name)) =
        (&args.chv, &args.input, &args.event, &args.name)
    else {
        bail!("the chatter set, the recording, --event and --name are needed");
    };
    let pcm = read_audio_file(input).with_context(|| format!("reading {}", input.display()))?;
    let line = NewVoiceLine {
        chv: chv.parse()?,
        event: event.clone(),
        name: name.clone(),
        subtitles: args
            .subtitle
            .iter()
            .map(|s| pair(s))
            .collect::<Result<_>>()?,
        weight: args.weight,
        only: args.only,
        category: args.category.clone(),
    };
    let mut ws = workspace_cmd::open_for_edit(&game, &args.mode)?;
    let label = format!("New voice line {name} ({event})");
    let report = voice::add_line(&game, &mut ws, &pcm, &line, &label)?;
    out!(
        "{}: sound {:08x}, {} Hz, {} channel(s), {:.2} s",
        report.entry,
        report.sound,
        report.sample_rate,
        report.channels,
        report.seconds
    );
    out!(
        "speech bank {}: new entry {}{}",
        report.bank,
        report.file,
        report
            .model
            .as_ref()
            .map(|m| format!(" (settings of {m})"))
            .unwrap_or_default()
    );
    out!(
        "event {event}: {}{}",
        if report.new_event {
            "new in the chatter set"
        } else {
            "one more line"
        },
        if report.new_record {
            ", new record in the event table"
        } else {
            ""
        }
    );
    workspace_cmd::finish_edit(&game, &mut ws, &args.mode, report.streams)
}

#[derive(Args)]
pub struct BankArgs {
    pub game_dir: PathBuf,
    /// Only entries whose file name contains this text, or whose sound id
    /// is this hexadecimal number
    pub filter: Option<String>,
    /// Print the entries as JSON
    #[arg(long)]
    pub json: bool,
    /// Workspace directory [default: <GAME_DIR>/_doctor_toolkit/workspace]
    #[arg(long, value_name = "DIR")]
    pub workspace_dir: Option<PathBuf>,
}

/// `speech-bank`.
pub fn bank(args: &BankArgs) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let ws = Workspace::for_game(&game, args.workspace_dir.as_deref())?;
    let config = configs(&game, &ws)?.into_iter().next().map(|(_, c)| c);
    let name = |hash: u32| match config.as_ref().and_then(|c| c.name_of(hash)) {
        Some(name) => name.to_owned(),
        None if hash == 0 => "-".to_owned(),
        None => format!("{hash:08x}"),
    };
    let filter = args.filter.as_ref().map(|f| f.to_lowercase());
    let id = filter
        .as_deref()
        .and_then(|f| u32::from_str_radix(f.trim_start_matches("0x"), 16).ok());
    let mut json = Vec::new();
    for ((s, _), bank) in voice::banks(&game, &ws)? {
        let stream = stream_name(&game, s);
        let shown: Vec<_> = bank
            .entries
            .iter()
            .filter(|e| match &filter {
                Some(f) => e.file.to_lowercase().contains(f) || Some(e.id[0]) == id,
                None => true,
            })
            .collect();
        if args.json {
            for e in shown {
                let mut value = serde_json::to_value(e)?;
                value["stream"] = stream.clone().into();
                value["path"] = e.stream_path(None).into();
                json.push(value);
            }
            continue;
        }
        if filter.is_none() {
            out!("{stream}: {} entr(ies)", bank.entries.len());
            continue;
        }
        for e in shown {
            out!(
                "{stream}  {:08x}  {}  flags {:#04x}  {} / {} / {}  {:?}  {}",
                e.id[0],
                e.file,
                e.flags,
                name(e.category),
                name(e.category2),
                name(e.duck),
                e.settings,
                e.stream_path(None)
            );
        }
    }
    if args.json {
        out!("{}", serde_json::to_string_pretty(&json)?);
    }
    Ok(())
}

#[derive(Args)]
pub struct EventsArgs {
    pub game_dir: PathBuf,
    /// Event names to show (all records when none is given and nothing is
    /// set)
    pub events: Vec<String>,
    /// Add a record for each named event that has none (the common values
    /// of the game), or change the named records with `--cooldown`,
    /// `--priority`, `--flags`, `--rank`
    #[arg(long)]
    pub set: bool,
    /// Seconds before the event may be said again
    #[arg(long)]
    pub cooldown: Option<f32>,
    /// Priority (300–500 in the game)
    #[arg(long)]
    pub priority: Option<u32>,
    /// Flags, decimal or 0x…
    #[arg(long, value_parser = parse_u32)]
    pub flags: Option<u32>,
    /// Rank, 0–4
    #[arg(long)]
    pub rank: Option<i32>,
    /// Print the records as JSON
    #[arg(long)]
    pub json: bool,
    #[command(flatten)]
    pub mode: EditMode,
}

/// `speech-events`.
pub fn events(args: &EventsArgs) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    if !args.set {
        let ws = Workspace::for_game(&game, args.mode.workspace_dir.as_deref())?;
        let (_, table) = voice::event_table(&game, &ws)?;
        let wanted: Vec<u32> = args.events.iter().map(|n| string_id(n)).collect();
        let shown: Vec<&SpeechEvent> = table
            .events
            .iter()
            .filter(|e| wanted.is_empty() || wanted.contains(&e.hash))
            .collect();
        if args.json {
            out!("{}", serde_json::to_string_pretty(&shown)?);
            return Ok(());
        }
        let names = sound_edit::event_names(&game, &ws)?;
        for e in &shown {
            let name = args
                .events
                .iter()
                .find(|n| string_id(n) == e.hash)
                .cloned()
                .or_else(|| names.get(&e.hash).cloned())
                .unwrap_or_default();
            out!(
                "{:08x}  cooldown {:>6}  second {:>6}  rank {}  flags {:#08x}  priority {:>3}  \
                 range {:>5}  {name}",
                e.hash,
                e.cooldown,
                e.second,
                e.rank,
                e.flags,
                e.priority,
                e.range
            );
        }
        for name in &args.events {
            if table.find(string_id(name)).is_none() {
                out!("{name}: no record");
            }
        }
        out!("{} of {} record(s)", shown.len(), table.events.len());
        return Ok(());
    }
    if args.events.is_empty() {
        bail!("--set needs event names");
    }
    let mut ws = workspace_cmd::open_for_edit(&game, &args.mode)?;
    let (done, stream) = sound_edit::set_speech_events(
        &game,
        &mut ws,
        &SetSpeechEvents {
            events: args.events.clone(),
            cooldown: args.cooldown,
            priority: args.priority,
            flags: args.flags,
            rank: args.rank,
        },
    )?;
    for (name, new) in &done {
        out!("{name}: {}", if *new { "new record" } else { "changed" });
    }
    workspace_cmd::finish_edit(&game, &mut ws, &args.mode, vec![stream])
}

fn configs(game: &GameInstance, ws: &Workspace) -> Result<Vec<((usize, usize), AudioConfig)>> {
    Ok(sound_edit::audio_configs(game, ws)?)
}

#[derive(Args)]
pub struct ConfigArgs {
    pub game_dir: PathBuf,
    /// Add a mix category with this name to every `auc` of the game
    #[arg(long, value_name = "NAME", requires = "parent")]
    pub add_category: Option<String>,
    /// Parent of the new category (`DIALOG`, `MUSIC`, `SFX` or any other)
    #[arg(long)]
    pub parent: Option<String>,
    /// Print the configuration as JSON
    #[arg(long)]
    pub json: bool,
    #[command(flatten)]
    pub mode: EditMode,
}

/// `audio-config`.
pub fn config(args: &ConfigArgs) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let Some(new) = &args.add_category else {
        let ws = Workspace::for_game(&game, args.mode.workspace_dir.as_deref())?;
        let all = configs(&game, &ws)?;
        let Some((_, config)) = all.first() else {
            bail!("the game has no `auc` resource");
        };
        if args.json {
            out!("{}", serde_json::to_string_pretty(config)?);
            return Ok(());
        }
        for ((s, _), other) in &all {
            out!(
                "{}{}",
                stream_name(&game, *s),
                if other == config {
                    ""
                } else {
                    " (differs from the first)"
                }
            );
        }
        out!("categories ({}):", config.categories.len());
        for c in &config.categories {
            out!("  {:08x}  {:<24} parent {}", c.hash(), c.name, c.parent);
        }
        out!("bank pools ({}):", config.pools.len());
        for p in &config.pools {
            out!("  {:<16} size {:>5}  kind {}", p.name, p.size, p.kind);
        }
        out!("ducks ({}):", config.ducks.len());
        for d in &config.ducks {
            out!(
                "  {:08x}  {:<24} {} / {}  mix {}",
                string_id(&d.name),
                d.name,
                d.levels[0],
                d.levels[1],
                d.mix
            );
        }
        return Ok(());
    };
    let parent = args.parent.as_deref().unwrap_or_default();
    let mut ws = workspace_cmd::open_for_edit(&game, &args.mode)?;
    let streams = sound_edit::add_mix_category(&game, &mut ws, new, parent)?;
    out!(
        "{new} ({:08x}) under {parent}: {} configuration(s)",
        string_id(new),
        streams.len()
    );
    workspace_cmd::finish_edit(&game, &mut ws, &args.mode, streams)
}

#[derive(Args)]
pub struct MixArgs {
    pub game_dir: PathBuf,
    /// Mix name (`occlusion.mix`); all mixes are listed without it
    pub name: Option<String>,
    /// The category to change or add (`DIALOG`); needs a value option
    #[arg(long)]
    pub category: Option<String>,
    /// Volume of the category in the mix, 0–1
    #[arg(long, requires = "category")]
    pub volume: Option<f32>,
    /// Low-pass frequency, Hz (96000 leaves the sound as it is)
    #[arg(long, requires = "category")]
    pub frequency: Option<u32>,
    /// The two times of the channel, seconds
    #[arg(long, requires = "category")]
    pub time: Option<f32>,
    /// Print the mix as JSON
    #[arg(long)]
    pub json: bool,
    #[command(flatten)]
    pub mode: EditMode,
}

/// `mix`.
pub fn mix(args: &MixArgs) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let editing = args.volume.is_some() || args.frequency.is_some() || args.time.is_some();
    let mut ws = if editing {
        workspace_cmd::open_for_edit(&game, &args.mode)?
    } else {
        Workspace::for_game(&game, args.mode.workspace_dir.as_deref())?
    };
    let config = configs(&game, &ws)?.into_iter().next().map(|(_, c)| c);
    let mut found = Vec::new();
    for (_, s, r) in voice::locate(&game, &[TYPE_MIX]) {
        let mix = Mix::parse(&ws.current_data(&game, s, r)?)?;
        match &args.name {
            Some(name) => {
                let stored = doctor_toolkit::resource::mix::stored_name(name);
                if mix.name.eq_ignore_ascii_case(stored) {
                    found.push(((s, r), mix));
                }
            }
            None => out!(
                "{:<34} {:>2} channel(s)  {}",
                mix.name,
                mix.channels.len(),
                stream_name(&game, s)
            ),
        }
    }
    let Some(name) = &args.name else {
        return Ok(());
    };
    if found.is_empty() {
        bail!("no mix {name}");
    }
    if !editing {
        let (_, mix) = &found[0];
        if args.json {
            out!("{}", serde_json::to_string_pretty(mix)?);
            return Ok(());
        }
        out!(
            "{} ({} cop(ies)): {} channel(s)",
            mix.name,
            found.len(),
            mix.channels.len()
        );
        for c in &mix.channels {
            let label = config
                .as_ref()
                .and_then(|a| a.name_of(c.category))
                .map_or_else(|| format!("{:08x}", c.category), str::to_owned);
            out!(
                "  {label:<24} volume {:<5} time {:<4} {:>6} Hz  {:?}",
                c.volume(),
                c.levels[LEVEL_TIMES[0]],
                c.frequency,
                c.levels
            );
        }
        return Ok(());
    }
    let category = args.category.as_deref().unwrap_or_default();
    let streams = sound_edit::edit_mix(
        &game,
        &mut ws,
        &MixEdit {
            name: name.clone(),
            category: category.to_owned(),
            volume: args.volume,
            frequency: args.frequency,
            time: args.time,
        },
    )?;
    out!("{name}: {category} changed in {} cop(ies)", streams.len());
    workspace_cmd::finish_edit(&game, &mut ws, &args.mode, streams)
}

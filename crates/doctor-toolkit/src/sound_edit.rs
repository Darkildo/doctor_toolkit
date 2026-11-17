//! Edits of the sound data as operations of the workspace: parameters of a
//! sound of a bank (`sbk`), new variations and emitters, records of the
//! speech event table (`get`), mix categories (`auc`) and channels of mixer
//! snapshots (`mix`).
//!
//! Each function is the whole action — read the resource as the workspace
//! has it, change it, check it, record one undoable operation — so that the
//! command line (`sbk-edit`, `sound-add`, `speech-events`, `audio-config`,
//! `mix`), the MCP server (`sbk_edit`, `sound_add`, `speech`, `mix`) and
//! the editor do the same thing.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::game::{GameInstance, StreamFile};
use crate::hash::string_id;
use crate::resource::auc::{AudioConfig, TYPE_AUC};
use crate::resource::audio::{Sample, TYPE_SBK};
use crate::resource::audio_encode::encode_like;
use crate::resource::chv::{ChatterSet, TYPE_CHV};
use crate::resource::get::SpeechEvent;
use crate::resource::mix::{LEVEL_TIMES, LEVEL_VOLUME, Mix, MixChannel, TYPE_MIX, stored_name};
use crate::resource::sbk::{SoundBank, emx_name, name_hash};
use crate::resource::video::import::read_audio_file;
use crate::voice;
use crate::workspace::{ResourceEdit, ResourceKey, StorageChoice, Workspace};
use crate::{Error, Result};

fn invalid<T>(text: impl Into<String>) -> Result<T> {
    Err(Error::InvalidArgument(text.into()))
}

fn stream_name(game: &GameInstance, stream: usize) -> String {
    game.streams()[stream].stream_name()
}

// ------------------------------------------------------------ sound banks

/// A bank of the game, as the workspace has it.
pub struct BankRef {
    pub key: ResourceKey,
    pub stream: usize,
    pub resource: usize,
    pub bank: SoundBank,
}

/// The `sbk` bank with resource key `key`.
pub fn load_bank(game: &GameInstance, ws: &Workspace, key: &ResourceKey) -> Result<BankRef> {
    if key.type_id != TYPE_SBK {
        return invalid(format!("{key} is not an sbk resource"));
    }
    let stream = game
        .find_stream(&key.stream)
        .ok_or_else(|| Error::InvalidArgument(format!("no stream {}", key.stream)))?;
    let file = StreamFile::open(&game.streams()[stream].path)?;
    let resource = key
        .find_in(file.layout())
        .ok_or_else(|| Error::InvalidArgument(format!("{key} is not in its stream")))?;
    let bank = SoundBank::parse(&ws.current_data(game, stream, resource)?)?;
    Ok(BankRef {
        key: key.clone(),
        stream,
        resource,
        bank,
    })
}

fn find_emitter(bank: &SoundBank, text: &str) -> Result<usize> {
    bank.find_emitter(text)
        .ok_or_else(|| Error::InvalidArgument(format!("the bank has no emitter {text}")))
}

/// Records an edited bank; refuses problems the bank did not have before.
fn save_bank(
    game: &GameInstance,
    ws: &mut Workspace,
    at: &BankRef,
    bank: &SoundBank,
    label: &str,
) -> Result<()> {
    let old = at.bank.validate();
    let new: Vec<String> = bank
        .validate()
        .into_iter()
        .filter(|p| !old.contains(p))
        .collect();
    if !new.is_empty() {
        return invalid(format!("the edit breaks the bank:\n  {}", new.join("\n  ")));
    }
    ws.set_resource(game, at.stream, at.resource, bank.to_bytes(), label)?;
    Ok(())
}

/// New parameters of one sound (emitter) of a bank; what is left out stays.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SbkEdit {
    /// The emitter: `.emx` name, hexadecimal hash or `#number`.
    pub emitter: String,
    /// Track volume, 0–1.
    #[serde(default)]
    pub volume: Option<f32>,
    /// Fixed pitch of every variation (1 = as recorded, 2 = an octave up).
    #[serde(default)]
    pub pitch: Option<f32>,
    /// Random pitch spread, 0–1.
    #[serde(default)]
    pub pitch_range: Option<f32>,
    /// Random attenuation spread, 0–1.
    #[serde(default)]
    pub attn_range: Option<f32>,
    /// Velocity of every key on, 0–127.
    #[serde(default)]
    pub velocity: Option<i32>,
    /// Distance of full attenuation, metres.
    #[serde(default)]
    pub falloff: Option<f32>,
    /// Voice stealing priority, 0–255.
    #[serde(default)]
    pub priority: Option<u8>,
    /// Most simultaneous performances.
    #[serde(default)]
    pub poly_max: Option<u8>,
    /// Emitter flags (3D 0x01, doppler 0x02, reverb from distance 0x04,
    /// attenuation curves 0x08, 0x10, 0x20).
    #[serde(default)]
    pub flags: Option<u32>,
    /// Variation choice: 0 explicit, other values let the game pick.
    #[serde(default)]
    pub select: Option<u8>,
    /// Mixer category: the hash other emitters use.
    #[serde(default)]
    pub mix: Option<u32>,
    /// Remove this variation (its sample stays in the bank).
    #[serde(default)]
    pub remove_variation: Option<usize>,
}

/// Changes parameters of one sound of the bank `key`: one undoable
/// operation. Returns its label.
pub fn edit_bank(
    game: &GameInstance,
    ws: &mut Workspace,
    key: &ResourceKey,
    edit: &SbkEdit,
) -> Result<String> {
    let at = load_bank(game, ws, key)?;
    let mut bank = at.bank.clone();
    let i = find_emitter(&bank, &edit.emitter)?;
    let mut changed: Vec<String> = Vec::new();
    {
        let e = &mut bank.emitters[i];
        if let Some(v) = edit.volume {
            e.track.volume = v;
            changed.push("volume".into());
        }
        if let Some(v) = edit.pitch_range {
            e.track.pitch_range = v;
            changed.push("pitch range".into());
        }
        if let Some(v) = edit.attn_range {
            e.track.attn_range = v;
            changed.push("attenuation range".into());
        }
        if let Some(v) = edit.falloff {
            if v.is_nan() || v <= 0.0 {
                return invalid("falloff must be positive");
            }
            e.falloff = v;
            changed.push("falloff".into());
        }
        if let Some(v) = edit.priority {
            e.priority = v;
            changed.push("priority".into());
        }
        if let Some(v) = edit.poly_max {
            if v == 0 {
                return invalid("poly max must be at least 1");
            }
            e.poly_max = v;
            changed.push("poly max".into());
        }
        if let Some(v) = edit.flags {
            e.flags =
                u8::try_from(v).map_err(|_| Error::InvalidArgument("flags are one byte".into()))?;
            changed.push("flags".into());
        }
        if let Some(v) = edit.select {
            e.track.select = v;
            changed.push("variation choice".into());
        }
        if let Some(v) = edit.mix {
            e.mix_category = v;
            changed.push("mixer category".into());
        }
    }
    if let Some(p) = edit.pitch {
        bank.set_pitch(i, p)?;
        changed.push("pitch".into());
    }
    if let Some(v) = edit.velocity {
        bank.set_velocity(i, v)?;
        changed.push("velocity".into());
    }
    if let Some(v) = edit.remove_variation {
        bank.remove_variation(i, v)?;
        changed.push(format!("variation {v} removed"));
    }
    if changed.is_empty() {
        return invalid("nothing to change: give a volume, a pitch, a velocity, …");
    }
    let label = format!(
        "Edit sound {} of {}: {}",
        bank.emitter_label(i),
        at.key.stream,
        changed.join(", ")
    );
    save_bank(game, ws, &at, &bank, &label)?;
    Ok(label)
}

/// New sounds of a bank: variations of an emitter, or a new emitter.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SoundAdd {
    /// Sound files (absolute paths): WAV, or any format ffmpeg reads. Each
    /// becomes one variation.
    pub files: Vec<PathBuf>,
    /// Add them as new variations of this emitter (`.emx` name, hexadecimal
    /// hash or `#number`).
    #[serde(default)]
    pub emitter: Option<String>,
    /// Or: add a new emitter with this name (`.emx` is appended); the game
    /// plays it by this name (`PlayEmx` of scripts, `audio: play` of
    /// animations).
    #[serde(default)]
    pub new: Option<String>,
    /// With `new`: copy the parameters of this emitter [default: the first].
    #[serde(default)]
    pub like: Option<String>,
    /// Sample rate of the new sounds [default: that of the emitter's first sound].
    #[serde(default)]
    pub rate: Option<u32>,
    /// Channels of the new sounds [default: that of the emitter's first sound].
    #[serde(default)]
    pub channels: Option<u8>,
}

/// One encoded file of [`add_sounds`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EncodedSound {
    pub file: String,
    pub sample_rate: u32,
    pub channels: u8,
    pub seconds: f64,
}

/// What [`add_sounds`] did.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AddedSounds {
    /// Label of the operation.
    pub operation: String,
    pub sounds: Vec<EncodedSound>,
    /// Name and hash of the new emitter.
    pub new_emitter: Option<(String, String)>,
    pub stream: String,
}

/// Every other `sbk` bank of the game, as the workspace has it.
fn other_banks(
    game: &GameInstance,
    ws: &Workspace,
    this: &BankRef,
) -> Result<Vec<(String, SoundBank)>> {
    let mut banks = Vec::new();
    for (_, stream, resource) in voice::locate(game, &[TYPE_SBK]) {
        if stream == this.stream && resource == this.resource {
            continue;
        }
        let data = ws.current_data(game, stream, resource)?;
        if let Ok(bank) = SoundBank::parse(&data) {
            banks.push((stream_name(game, stream), bank));
        }
    }
    Ok(banks)
}

/// Adds sound files to the bank `key` as variations of an emitter or as a
/// new emitter: one undoable operation.
pub fn add_sounds(
    game: &GameInstance,
    ws: &mut Workspace,
    key: &ResourceKey,
    add: &SoundAdd,
) -> Result<AddedSounds> {
    if add.files.is_empty() {
        return invalid("give at least one sound file");
    }
    if add.emitter.is_some() == add.new.is_some() {
        return invalid("give either the emitter to add variations to or the name of a new one");
    }
    let at = load_bank(game, ws, key)?;
    let mut bank = at.bank.clone();
    if bank.emitters.is_empty() {
        return invalid("the bank has no emitter to take the parameters from");
    }
    let template = match (&add.emitter, &add.like) {
        (Some(e), _) | (None, Some(e)) => find_emitter(&bank, e)?,
        (None, None) => 0,
    };
    // The format of the emitter's first sound.
    let mut header = bank
        .instrument_of(template)
        .and_then(|ins| bank.instruments[ins].tones.first())
        .and_then(|tone| bank.samples.get(tone.sample as usize))
        .and_then(|s| Sample::parse_inline(&s.bytes, 0).ok())
        .map(|s| s.header)
        .ok_or_else(|| {
            Error::InvalidArgument("the emitter has no sound to take the format from".into())
        })?;
    if let Some(rate) = add.rate {
        header.sample_rate = rate;
    }
    if let Some(channels) = add.channels {
        header.channels = channels;
    }
    let mut encoded = Vec::new();
    let mut sounds = Vec::new();
    for path in &add.files {
        let pcm = read_audio_file(path)
            .map_err(|e| Error::InvalidArgument(format!("reading {}: {e}", path.display())))?;
        if pcm.frames() == 0 {
            return invalid(format!("{} holds no audio", path.display()));
        }
        let (fitted, data) = encode_like(&pcm, &header)?;
        sounds.push(EncodedSound {
            file: path.display().to_string(),
            sample_rate: header.sample_rate,
            channels: header.channels,
            seconds: fitted.frames() as f64 / f64::from(header.sample_rate),
        });
        encoded.push(data);
    }
    let mut new_emitter = None;
    let operation = if let Some(name) = &add.new {
        let name = emx_name(name);
        let hash = name_hash(&name);
        // The game plays the first loaded bank that has the name.
        for (stream, other) in other_banks(game, ws, &at)? {
            if other.emitters.iter().any(|e| e.template_id == hash) {
                return invalid(format!(
                    "{name} ({hash:08x}) is already an emitter of {stream}"
                ));
            }
        }
        let n = bank.add_emitter(&name, template, encoded)?;
        new_emitter = Some((name.clone(), format!("{hash:08x}")));
        format!(
            "Add sound {name} ({hash:08x}, emitter #{n}, {} variation(s)) to {}",
            add.files.len(),
            at.key.stream
        )
    } else {
        for data in encoded {
            bank.add_variation(template, data)?;
        }
        format!(
            "Add {} variation(s) to sound {} of {}",
            add.files.len(),
            bank.emitter_label(template),
            at.key.stream
        )
    };
    save_bank(game, ws, &at, &bank, &operation)?;
    Ok(AddedSounds {
        operation,
        sounds,
        new_emitter,
        stream: at.key.stream.clone(),
    })
}

// ------------------------------------------------------------ speech

/// A chatter set of the game: who can say voice lines.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Speaker {
    /// Resource key of the chatter set (`chv`).
    pub key: String,
    pub events: usize,
    pub lines: usize,
    /// Voice codes of most of its lines (`mob3`), most common first.
    pub voices: Vec<String>,
}

/// The chatter sets of the game, optionally only those with `event`.
pub fn speakers(game: &GameInstance, ws: &Workspace, event: Option<&str>) -> Result<Vec<Speaker>> {
    let wanted = event.map(string_id);
    // Voice codes come from the file names of the lines (`d_mob3_…` → mob3).
    let mut files = HashMap::new();
    for (_, bank) in voice::banks(game, ws)? {
        for e in bank.entries {
            files.insert(e.id[0], e.file);
        }
    }
    let mut out = Vec::new();
    for (_, s, r) in voice::locate(game, &[TYPE_CHV]) {
        let set = ChatterSet::parse(&ws.current_data(game, s, r)?)?;
        if wanted.is_some_and(|h| set.event(h).is_none()) {
            continue;
        }
        let lines: usize = set.events.iter().map(|e| e.lines.len()).sum();
        let mut voices = BTreeMap::<&str, usize>::new();
        for line in set.events.iter().flat_map(|e| &e.lines) {
            if let Some(code) = files
                .get(&line.sound())
                .and_then(|f| f.strip_prefix("d_"))
                .and_then(|f| f.split('_').next())
            {
                *voices.entry(code).or_default() += 1;
            }
        }
        let mut voices: Vec<_> = voices.into_iter().collect();
        voices.sort_by_key(|&(code, n)| (std::cmp::Reverse(n), code));
        let file = StreamFile::open(&game.streams()[s].path)?;
        let key = ResourceKey::new(&stream_name(game, s), &file.layout().resources[r].header);
        out.push(Speaker {
            key: key.to_string(),
            events: set.events.len(),
            lines,
            voices: voices
                .iter()
                .take(3)
                .map(|&(code, _)| code.to_owned())
                .collect(),
        });
    }
    Ok(out)
}

/// Names of the speech events, which only the chatter sets store.
pub fn event_names(game: &GameInstance, ws: &Workspace) -> Result<HashMap<u32, String>> {
    let mut names = HashMap::new();
    for (_, s, r) in voice::locate(game, &[TYPE_CHV]) {
        for event in ChatterSet::parse(&ws.current_data(game, s, r)?)?.events {
            names.insert(event.hash, event.name);
        }
    }
    Ok(names)
}

/// Records of the speech event table to add or change.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SetSpeechEvents {
    /// Event names (`F2F_ENJOY_FLIGHT`). An event without a record gets one
    /// with the common values of the game.
    pub events: Vec<String>,
    /// Seconds before the event may be said again.
    #[serde(default)]
    pub cooldown: Option<f32>,
    /// Priority (300–500 in the game).
    #[serde(default)]
    pub priority: Option<u32>,
    /// Flags.
    #[serde(default)]
    pub flags: Option<u32>,
    /// Rank, 0–4.
    #[serde(default)]
    pub rank: Option<i32>,
}

/// Adds or changes records of the speech event table: one undoable
/// operation. Returns `(event, whether its record is new)` and the stream.
pub fn set_speech_events(
    game: &GameInstance,
    ws: &mut Workspace,
    set: &SetSpeechEvents,
) -> Result<(Vec<(String, bool)>, String)> {
    if set.events.is_empty() {
        return invalid("give event names");
    }
    let ((s, r), mut table) = voice::event_table(game, ws)?;
    let mut done = Vec::new();
    for name in &set.events {
        voice::check_event(name)?;
        let mut event = table
            .find(string_id(name))
            .cloned()
            .unwrap_or_else(|| SpeechEvent::new(name));
        if let Some(v) = set.cooldown {
            event.cooldown = v;
        }
        if let Some(v) = set.priority {
            event.priority = v;
        }
        if let Some(v) = set.flags {
            event.flags = v;
        }
        if let Some(v) = set.rank {
            event.rank = v;
        }
        done.push((name.clone(), table.insert(event)));
    }
    if let Some(problem) = table.validate().first() {
        return invalid(format!("the edit breaks the table: {problem}"));
    }
    let label = format!("Speech events: {}", set.events.join(", "));
    ws.set_resource(game, s, r, table.to_bytes()?, &label)?;
    Ok((done, stream_name(game, s)))
}

// ------------------------------------------------------------ mixer

/// The audio configurations (`auc`) of the game as the workspace has them.
pub fn audio_configs(
    game: &GameInstance,
    ws: &Workspace,
) -> Result<Vec<((usize, usize), AudioConfig)>> {
    voice::locate(game, &[TYPE_AUC])
        .into_iter()
        .map(|(_, s, r)| Ok(((s, r), AudioConfig::parse(&ws.current_data(game, s, r)?)?)))
        .collect()
}

/// Adds mix category `name` under `parent` to every audio configuration of
/// the game: one undoable operation. Returns the streams changed.
pub fn add_mix_category(
    game: &GameInstance,
    ws: &mut Workspace,
    name: &str,
    parent: &str,
) -> Result<Vec<String>> {
    let mut edits = Vec::new();
    let mut streams = Vec::new();
    for ((s, r), mut config) in audio_configs(game, ws)? {
        config.add_category(name, parent)?;
        if let Some(problem) = config.validate().first() {
            return invalid(format!("the edit breaks the configuration: {problem}"));
        }
        edits.push((
            s,
            vec![ResourceEdit {
                index: r,
                data: config.to_bytes()?,
                storage: StorageChoice::Keep,
            }],
        ));
        streams.push(stream_name(game, s));
    }
    if edits.is_empty() {
        return invalid("the game has no `auc` resource");
    }
    ws.change_resources(game, &format!("New mix category {name}"), &edits, &[])?;
    Ok(streams)
}

/// Every mixer snapshot (`mix`) of the game as the workspace has it.
pub fn mixes(game: &GameInstance, ws: &Workspace) -> Result<Vec<((usize, usize), Mix)>> {
    voice::locate(game, &[TYPE_MIX])
        .into_iter()
        .map(|(_, s, r)| Ok(((s, r), Mix::parse(&ws.current_data(game, s, r)?)?)))
        .collect()
}

/// A channel of a mixer snapshot to change or add.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MixEdit {
    /// Name of the snapshot (`occlusion.mix`).
    pub name: String,
    /// The mix category to change or add (`DIALOG`).
    pub category: String,
    /// Volume of the category in the snapshot, 0–1.
    #[serde(default)]
    pub volume: Option<f32>,
    /// Low-pass frequency, Hz (96000 leaves the sound as it is).
    #[serde(default)]
    pub frequency: Option<u32>,
    /// The two times of the channel, seconds.
    #[serde(default)]
    pub time: Option<f32>,
}

/// Changes a channel of every copy of a mixer snapshot (a category the
/// snapshot does not have gets a channel): one undoable operation. Returns
/// the streams changed.
pub fn edit_mix(game: &GameInstance, ws: &mut Workspace, edit: &MixEdit) -> Result<Vec<String>> {
    if edit.volume.is_none() && edit.frequency.is_none() && edit.time.is_none() {
        return invalid("nothing to change: give a volume, a frequency or a time");
    }
    let hash = string_id(&edit.category);
    if let Some((_, config)) = audio_configs(game, ws)?.first()
        && config.name_of(hash).is_none()
    {
        return invalid(format!(
            "no mix category {} (see the audio configuration)",
            edit.category
        ));
    }
    let stored = stored_name(&edit.name);
    let mut edits = Vec::new();
    let mut streams = Vec::new();
    for ((s, r), mut mix) in mixes(game, ws)? {
        if !mix.name.eq_ignore_ascii_case(stored) {
            continue;
        }
        if mix.channel(hash).is_none() {
            // A new channel takes the levels of the first one, at full volume.
            let mut levels = mix
                .channels
                .first()
                .map_or([-1.0, -1.0, 1.0, 1.0, 0.0, -1.0, 0.0, 0.0], |c| c.levels);
            levels[LEVEL_VOLUME] = 1.0;
            mix.channels.push(MixChannel {
                category: hash,
                levels,
                frequency: 96_000,
            });
        }
        let channel = mix.channel_mut(hash).expect("the channel was just added");
        if let Some(v) = edit.volume {
            if !(0.0..=1.0).contains(&v) {
                return invalid("the volume must be 0..1");
            }
            channel.levels[LEVEL_VOLUME] = v;
        }
        if let Some(v) = edit.frequency {
            channel.frequency = v;
        }
        if let Some(v) = edit.time {
            for k in LEVEL_TIMES {
                channel.levels[k] = v;
            }
        }
        edits.push((
            s,
            vec![ResourceEdit {
                index: r,
                data: mix.to_bytes()?,
                storage: StorageChoice::Keep,
            }],
        ));
        streams.push(stream_name(game, s));
    }
    if edits.is_empty() {
        return invalid(format!("no mix {}", edit.name));
    }
    let label = format!("Mix {}: {}", edit.name, edit.category);
    ws.change_resources(game, &label, &edits, &[])?;
    Ok(streams)
}

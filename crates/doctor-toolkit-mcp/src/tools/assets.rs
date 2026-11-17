//! Sound, animation and interface data (stage 104): parameters and new
//! sounds of sound banks, the speech tables, the mixer, clips, events and
//! node graphs of animation banks, the parts of Flash interface files. The
//! operations are functions of the library (`doctor_toolkit::{sound_edit,
//! anim_edit, apx_edit}`) that the command line calls too; each edit is one
//! undoable operation of the workspace answered with its diff.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use doctor_toolkit::anim_edit::{self, ImportAnimation};
use doctor_toolkit::apx_edit::{self, ApxEdit};
use doctor_toolkit::game::GameAssets;
use doctor_toolkit::hash::string_id;
use doctor_toolkit::resource::anim::ClipEvent;
use doctor_toolkit::resource::anim_graph::GraphJson;
use doctor_toolkit::resource::mix::LEVEL_TIMES;
pub use doctor_toolkit::sound_edit::SetSpeechEvents;
use doctor_toolkit::sound_edit::{self, MixEdit, SbkEdit, SoundAdd};
use doctor_toolkit::voice;
use doctor_toolkit::workspace::{ResourceKey, Workspace};

use super::create::{game_resource, input_file, last_label};
use super::edit::finish;
use super::wizards::resource_key;
use super::{Output, clamp};
use crate::state::{ToolError, ToolResult, Toolkit, fail};

// ------------------------------------------------------------ parameters

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct SbkEditParams {
    /// The sound bank: a resource key (`gf2/frontend:sbk:6d4bafbd`),
    /// `<stream>#<index>` or a one-result search query.
    pub bank: String,
    #[serde(flatten)]
    pub edit: SbkEdit,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct SoundAddParams {
    /// The sound bank: a resource key (`gf2/frontend:sbk:6d4bafbd`),
    /// `<stream>#<index>` or a one-result search query.
    pub bank: String,
    #[serde(flatten)]
    pub add: SoundAdd,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct SpeechParams {
    /// What to show: `events` (default: the records of the speech event
    /// table), `banks` (entries of the speech banks) or `speakers` (the
    /// chatter sets, who can say voice lines).
    #[serde(default)]
    pub show: Option<String>,
    /// `events`: only these events by name. `speakers`: only chatter sets
    /// with the first of these events.
    #[serde(default)]
    pub events: Vec<String>,
    /// `banks`: only entries whose file name contains this text, or whose
    /// sound id is this hexadecimal number. `events`: only events whose
    /// name contains this text.
    #[serde(default)]
    pub filter: Option<String>,
    /// Read the game files only, without the edits of the workspace.
    #[serde(default)]
    pub original: Option<bool>,
    /// Skip this many rows (paging).
    #[serde(default)]
    pub offset: Option<usize>,
    /// At most this many rows (default 100, at most 1000).
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct MixParams {
    /// A mixer snapshot by name (`occlusion.mix`): its channels. Without
    /// it: the mix categories, bank pools and ducks of the audio
    /// configuration and the list of snapshots.
    #[serde(default)]
    pub name: Option<String>,
    /// Read the game files only, without the edits of the workspace.
    #[serde(default)]
    pub original: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct SetMixParams {
    /// Change a channel of a mixer snapshot: its name, the category and
    /// the values to set (a category the snapshot does not have gets a
    /// channel).
    #[serde(default)]
    pub channel: Option<MixEdit>,
    /// Or: add a mix category with this name to every audio configuration
    /// of the game. Goes with `parent`.
    #[serde(default)]
    pub add_category: Option<String>,
    /// Parent of the new category (`DIALOG`, `MUSIC`, `SFX` or any other).
    #[serde(default)]
    pub parent: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ApxEditParams {
    /// The interface file: a resource key of type `apx`, `<stream>#<index>`
    /// or a one-result search query (resource_json shows its textures and
    /// text references, export writes its parts).
    pub apx: String,
    #[serde(flatten)]
    pub edit: ApxEdit,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ImportAnimationParams {
    /// The animation bank: a resource key (`gf2/globals_global:bnk:c0e2841d`),
    /// `<stream>#<index>` or a one-result search query.
    pub bank: String,
    #[serde(flatten)]
    pub import: ImportAnimation,
    /// Only say what would be done.
    #[serde(default)]
    pub dry_run: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ClipEventsParams {
    /// The animation bank: a resource key, `<stream>#<index>` or a
    /// one-result search query.
    pub bank: String,
    /// The clip: its name or hexadecimal hash.
    pub clip: String,
    /// The new list of events, each `{"frame": ticks (2 per clip frame),
    /// "name": ..., ...}` as this tool shows them; without it the events
    /// of the clip are only shown.
    #[serde(default)]
    pub events: Option<Vec<Value>>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct AnimationGraphParams {
    /// The animation bank: a resource key, `<stream>#<index>` or a
    /// one-result search query.
    pub bank: String,
    /// Only this entry (name): its node tree as JSON. Without it the
    /// entries are listed by name.
    #[serde(default)]
    pub entry: Option<String>,
    /// Write the whole graph as JSON (the form set_animation_graph reads)
    /// into this file of the output folder (a name without extension).
    #[serde(default)]
    pub save_as: Option<String>,
    /// Read the game files only, without the edits of the workspace.
    #[serde(default)]
    pub original: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct SetAnimationGraphParams {
    /// The animation bank: a resource key, `<stream>#<index>` or a
    /// one-result search query.
    pub bank: String,
    /// The whole graph: `{"entries": [...]}` as animation_graph writes it.
    #[serde(default)]
    pub graph: Option<Value>,
    /// Or: a JSON file with the graph (an absolute path).
    #[serde(default)]
    pub file: Option<String>,
}

// ------------------------------------------------------------ helpers

/// The workspace for reading: the saved one, or an empty one in memory.
fn read_workspace(kit: &Toolkit, original: bool) -> ToolResult<Workspace> {
    match kit.workspace_unless(original)? {
        Some(ws) => Ok(ws),
        None => Ok(Workspace::for_game(&kit.game, Some(&kit.workspace_dir))?),
    }
}

fn stream_name(kit: &Toolkit, stream: usize) -> String {
    kit.game.streams()[stream].stream_name()
}

fn paged(
    rows: Vec<Value>,
    offset: Option<usize>,
    limit: Option<usize>,
) -> (usize, usize, Vec<Value>) {
    let offset = offset.unwrap_or(0);
    let limit = clamp(limit, 100, 1000);
    let total = rows.len();
    (
        total,
        offset,
        rows.into_iter().skip(offset).take(limit).collect(),
    )
}

// ------------------------------------------------------------ sound banks

pub fn sbk_edit(kit: &Toolkit, p: SbkEditParams) -> ToolResult<Output> {
    let mut access = kit.write_workspace(&format!("sbk_edit: {}", p.edit.emitter))?;
    let ws = &mut access.ws;
    let key = resource_key(kit, ws, &p.bank)?;
    let since = ws.head();
    let label = sound_edit::edit_bank(&kit.game, ws, &key, &p.edit)?;
    finish(
        ws,
        since,
        &label,
        json!({ "bank": key.to_string(), "emitter": p.edit.emitter }),
    )
}

pub fn sound_add(kit: &Toolkit, p: SoundAddParams) -> ToolResult<Output> {
    for file in &p.add.files {
        input_file(&file.display().to_string())?;
    }
    let mut access = kit.write_workspace(&format!("sound_add: {}", p.bank))?;
    let ws = &mut access.ws;
    let key = resource_key(kit, ws, &p.bank)?;
    let since = ws.head();
    let done = sound_edit::add_sounds(&kit.game, ws, &key, &p.add)?;
    let label = done.operation.clone();
    finish(ws, since, &label, serde_json::to_value(&done)?)
}

// ------------------------------------------------------------ speech

pub fn speech(kit: &Toolkit, p: SpeechParams) -> ToolResult<Output> {
    let ws = read_workspace(kit, p.original.unwrap_or(false))?;
    let game = &kit.game;
    match p.show.as_deref().unwrap_or("events") {
        "speakers" => {
            let rows: Vec<Value> =
                sound_edit::speakers(game, &ws, p.events.first().map(String::as_str))?
                    .iter()
                    .map(|s| serde_json::to_value(s).unwrap_or(Value::Null))
                    .collect();
            let (total, offset, rows) = paged(rows, p.offset, p.limit);
            Ok(Output::Json(json!({
                "total": total,
                "offset": offset,
                "speakers": rows,
                "next": "voice_add takes the key of a speaker",
            })))
        }
        "banks" => {
            let config = sound_edit::audio_configs(game, &ws)?
                .into_iter()
                .next()
                .map(|(_, c)| c);
            let name = |hash: u32| match config.as_ref().and_then(|c| c.name_of(hash)) {
                Some(name) => name.to_owned(),
                None if hash == 0 => "-".to_owned(),
                None => format!("{hash:08x}"),
            };
            let filter = p.filter.as_ref().map(|f| f.to_lowercase());
            let id = filter
                .as_deref()
                .and_then(|f| u32::from_str_radix(f.trim_start_matches("0x"), 16).ok());
            let mut summary = Vec::new();
            let mut rows = Vec::new();
            for ((s, _), bank) in voice::banks(game, &ws)? {
                let stream = stream_name(kit, s);
                summary.push(json!({ "stream": stream, "entries": bank.entries.len() }));
                if filter.is_none() {
                    continue;
                }
                for e in &bank.entries {
                    if !(e
                        .file
                        .to_lowercase()
                        .contains(filter.as_deref().unwrap_or_default())
                        || Some(e.id[0]) == id)
                    {
                        continue;
                    }
                    rows.push(json!({
                        "stream": stream,
                        "sound": format!("{:08x}", e.id[0]),
                        "file": e.file,
                        "flags": e.flags,
                        "category": name(e.category),
                        "category2": name(e.category2),
                        "duck": name(e.duck),
                        "settings": e.settings,
                        "path": e.stream_path(None),
                    }));
                }
            }
            let (total, offset, rows) = paged(rows, p.offset, p.limit);
            Ok(Output::Json(json!({
                "banks": summary,
                "total": total,
                "offset": offset,
                "entries": rows,
                "note": "`filter` (a part of a file name or a sound id) lists entries",
            })))
        }
        "events" => {
            let (_, table) = voice::event_table(game, &ws)?;
            let names = sound_edit::event_names(game, &ws)?;
            let wanted: Vec<u32> = p.events.iter().map(|n| string_id(n)).collect();
            let filter = p.filter.as_ref().map(|f| f.to_lowercase());
            let rows: Vec<Value> = table
                .events
                .iter()
                .filter(|e| wanted.is_empty() || wanted.contains(&e.hash))
                .map(|e| {
                    let name = p
                        .events
                        .iter()
                        .find(|n| string_id(n) == e.hash)
                        .cloned()
                        .or_else(|| names.get(&e.hash).cloned())
                        .unwrap_or_default();
                    (name, e)
                })
                .filter(|(name, _)| {
                    filter
                        .as_deref()
                        .is_none_or(|f| name.to_lowercase().contains(f))
                })
                .map(|(name, e)| {
                    json!({
                        "name": name,
                        "hash": format!("{:08x}", e.hash),
                        "cooldown": e.cooldown,
                        "second": e.second,
                        "rank": e.rank,
                        "flags": e.flags,
                        "priority": e.priority,
                        "range": e.range,
                    })
                })
                .collect();
            let missing: Vec<&String> = p
                .events
                .iter()
                .filter(|n| table.find(string_id(n)).is_none())
                .collect();
            let (total, offset, rows) = paged(rows, p.offset, p.limit);
            Ok(Output::Json(json!({
                "records": table.events.len(),
                "total": total,
                "offset": offset,
                "events": rows,
                "no_record": missing,
                "next": "set_speech_events adds a record or changes cool-down, priority, flags, rank",
            })))
        }
        other => fail(format!(
            "`show`: `{other}` is not one of events, banks, speakers"
        )),
    }
}

pub fn set_speech_events(kit: &Toolkit, p: SetSpeechEvents) -> ToolResult<Output> {
    let mut access = kit.write_workspace(&format!("set_speech_events: {}", p.events.join(", ")))?;
    let ws = &mut access.ws;
    let since = ws.head();
    let (done, stream) = sound_edit::set_speech_events(&kit.game, ws, &p)?;
    let label = last_label(ws);
    finish(
        ws,
        since,
        &label,
        json!({
            "stream": stream,
            "events": done
                .iter()
                .map(|(name, new)| json!({ "name": name, "new_record": new }))
                .collect::<Vec<_>>(),
        }),
    )
}

// ------------------------------------------------------------ mixer

pub fn mix(kit: &Toolkit, p: MixParams) -> ToolResult<Output> {
    let ws = read_workspace(kit, p.original.unwrap_or(false))?;
    let game = &kit.game;
    let configs = sound_edit::audio_configs(game, &ws)?;
    let config = configs.first().map(|(_, c)| c);
    let mixes = sound_edit::mixes(game, &ws)?;
    let Some(name) = &p.name else {
        let Some(config) = config else {
            return fail("the game has no audio configuration (`auc`)");
        };
        return Ok(Output::Json(json!({
            "configurations": configs
                .iter()
                .map(|((s, _), other)| json!({
                    "stream": stream_name(kit, *s),
                    "differs_from_the_first": other != config,
                }))
                .collect::<Vec<_>>(),
            "categories": config
                .categories
                .iter()
                .map(|c| json!({
                    "name": c.name,
                    "hash": format!("{:08x}", c.hash()),
                    "parent": c.parent,
                }))
                .collect::<Vec<_>>(),
            "bank_pools": config.pools,
            "ducks": config.ducks,
            "mixes": mixes
                .iter()
                .map(|((s, _), m)| json!({
                    "name": m.name,
                    "channels": m.channels.len(),
                    "stream": stream_name(kit, *s),
                }))
                .collect::<Vec<_>>(),
            "next": "`name` shows the channels of a snapshot; set_mix changes them",
        })));
    };
    let stored = doctor_toolkit::resource::mix::stored_name(name);
    let found: Vec<_> = mixes
        .iter()
        .filter(|(_, m)| m.name.eq_ignore_ascii_case(stored))
        .collect();
    let Some((_, first)) = found.first() else {
        return fail(format!("no mix {name}; `mix` without a name lists them"));
    };
    Ok(Output::Json(json!({
        "name": first.name,
        "copies": found
            .iter()
            .map(|((s, _), _)| stream_name(kit, *s))
            .collect::<Vec<_>>(),
        "channels": first
            .channels
            .iter()
            .map(|c| json!({
                "category": config
                    .and_then(|a| a.name_of(c.category))
                    .map_or_else(|| format!("{:08x}", c.category), str::to_owned),
                "volume": c.volume(),
                "time": c.levels[LEVEL_TIMES[0]],
                "frequency": c.frequency,
                "levels": c.levels,
            }))
            .collect::<Vec<_>>(),
    })))
}

pub fn set_mix(kit: &Toolkit, p: SetMixParams) -> ToolResult<Output> {
    match (p.channel, p.add_category) {
        (Some(edit), None) => {
            let mut access =
                kit.write_workspace(&format!("set_mix: {} of {}", edit.category, edit.name))?;
            let ws = &mut access.ws;
            let since = ws.head();
            let streams = sound_edit::edit_mix(&kit.game, ws, &edit)?;
            let label = last_label(ws);
            finish(
                ws,
                since,
                &label,
                json!({ "mix": edit.name, "category": edit.category, "copies_changed": streams }),
            )
        }
        (None, Some(name)) => {
            let Some(parent) = p.parent else {
                return fail("`add_category` goes with `parent`");
            };
            let mut access = kit.write_workspace(&format!("set_mix: new category {name}"))?;
            let ws = &mut access.ws;
            let since = ws.head();
            let streams = sound_edit::add_mix_category(&kit.game, ws, &name, &parent)?;
            let label = last_label(ws);
            finish(
                ws,
                since,
                &label,
                json!({
                    "category": name,
                    "hash": format!("{:08x}", string_id(&name)),
                    "parent": parent,
                    "configurations_changed": streams,
                }),
            )
        }
        _ => fail(
            "give `channel` (a channel of a snapshot to change) or `add_category` with `parent`",
        ),
    }
}

// ------------------------------------------------------------ interface files

pub fn apx_edit(kit: &Toolkit, p: ApxEditParams) -> ToolResult<Output> {
    for file in [&p.edit.image, &p.edit.apt, &p.edit.constants]
        .into_iter()
        .flatten()
    {
        input_file(&file.display().to_string())?;
    }
    let at = game_resource(kit, &p.apx)?;
    let mut access = kit.write_workspace(&format!("apx_edit: {}", p.apx))?;
    let ws = &mut access.ws;
    let since = ws.head();
    let done = apx_edit::edit(&kit.game, ws, at.stream, at.resource, &p.edit)?;
    let label = done.operation.clone().unwrap_or_default();
    finish(ws, since, &label, serde_json::to_value(&done)?)
}

// ------------------------------------------------------------ animations

fn bank_key(kit: &Toolkit, ws: &Workspace, text: &str) -> ToolResult<ResourceKey> {
    let key = resource_key(kit, ws, text)?;
    anim_edit::find_bank(&kit.game, &key)?;
    Ok(key)
}

pub fn import_animation(kit: &Toolkit, p: ImportAnimationParams) -> ToolResult<Output> {
    input_file(&p.import.file.display().to_string())?;
    let dry_run = p.dry_run.unwrap_or(false);
    let mut assets = GameAssets::new(kit.game.clone(), kit.resources());
    let rigs = assets.rigs().to_vec();
    let mut access = kit.write_workspace(&format!(
        "import_animation: {} into {}",
        p.import.file.display(),
        p.bank
    ))?;
    let ws = &mut access.ws;
    let key = bank_key(kit, ws, &p.bank)?;
    let since = ws.head();
    let done = anim_edit::import_animation(&kit.game, ws, &rigs, &key, &p.import, dry_run)?;
    let value = serde_json::to_value(&done)?;
    if dry_run {
        return Ok(Output::Json(json!({ "dry_run": true, "would": value })));
    }
    let label = done.operation.clone().unwrap_or_default();
    finish(ws, since, &label, value)
}

pub fn clip_events(kit: &Toolkit, p: ClipEventsParams) -> ToolResult<Output> {
    let Some(events) = p.events else {
        let ws = read_workspace(kit, false)?;
        let key = bank_key(kit, &ws, &p.bank)?;
        let (name, events) = anim_edit::clip_events(&kit.game, &ws, &key, &p.clip)?;
        return Ok(Output::Json(json!({
            "bank": key.to_string(),
            "clip": name,
            "events": events,
            "note": "`frame` is in ticks: 2 per clip frame, 59.94 per second; \
                     give `events` (the whole new list) to replace them",
        })));
    };
    let events: Vec<ClipEvent> = events
        .into_iter()
        .map(serde_json::from_value)
        .collect::<Result<_, _>>()
        .map_err(|e| {
            ToolError(format!(
                "`events`: {e}; give events as this tool shows them"
            ))
        })?;
    let mut access = kit.write_workspace(&format!("clip_events: {}", p.clip))?;
    let ws = &mut access.ws;
    let key = bank_key(kit, ws, &p.bank)?;
    let since = ws.head();
    let (name, label) = anim_edit::set_clip_events(&kit.game, ws, &key, &p.clip, &events)?;
    finish(
        ws,
        since,
        label.as_deref().unwrap_or_default(),
        json!({ "bank": key.to_string(), "clip": name, "events": events.len() }),
    )
}

pub fn animation_graph(kit: &Toolkit, p: AnimationGraphParams) -> ToolResult<Output> {
    let ws = read_workspace(kit, p.original.unwrap_or(false))?;
    let key = bank_key(kit, &ws, &p.bank)?;
    let whole = anim_edit::graph(&kit.game, &ws, &key, None)?;
    let mut out = json!({
        "bank": key.to_string(),
        "entries": whole.entries,
        "nodes": whole.nodes,
        "slots": whole.slots,
        "issues": whole.issues,
    });
    if let Some(name) = &p.save_as {
        let path = kit.output_path(&format!("{}.json", name.trim()))?;
        let mut text = serde_json::to_string_pretty(&whole.graph)?;
        text.push('\n');
        std::fs::write(&path, text)
            .map_err(|e| ToolError(format!("cannot write {}: {e}", path.display())))?;
        out["saved"] = json!(path);
    }
    match &p.entry {
        Some(entry) => {
            out["graph"] = anim_edit::graph(&kit.game, &ws, &key, Some(entry))?.graph;
        }
        None => {
            out["entry_names"] = json!(
                whole.graph["entries"]
                    .as_array()
                    .map(|list| list.iter().map(|e| e["name"].clone()).collect::<Vec<_>>())
            );
            out["next"] = json!(
                "`entry` shows the node tree of one entry; `save_as` writes the whole graph \
                 as JSON; set_animation_graph replaces it"
            );
        }
    }
    Ok(Output::Json(out))
}

pub fn set_animation_graph(kit: &Toolkit, p: SetAnimationGraphParams) -> ToolResult<Output> {
    let value: Value = match (p.graph, p.file) {
        (Some(graph), None) => graph,
        (None, Some(file)) => {
            let path = input_file(&file)?;
            serde_json::from_slice(
                &std::fs::read(&path)
                    .map_err(|e| ToolError(format!("cannot read {}: {e}", path.display())))?,
            )
            .map_err(|e| ToolError(format!("{}: {e}", path.display())))?
        }
        _ => return fail("give `graph` (the JSON) or `file` (a JSON file), one of them"),
    };
    let graph: GraphJson = serde_json::from_value(value).map_err(|e| {
        ToolError(format!(
            "not an animation graph ({e}): give it as animation_graph writes it"
        ))
    })?;
    let mut access = kit.write_workspace(&format!("set_animation_graph: {}", p.bank))?;
    let ws = &mut access.ws;
    let key = bank_key(kit, ws, &p.bank)?;
    let since = ws.head();
    let done = anim_edit::set_graph(&kit.game, ws, &key, &graph)?;
    let label = done.operation.clone().unwrap_or_default();
    finish(ws, since, &label, serde_json::to_value(&done)?)
}

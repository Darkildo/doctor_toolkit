//! Edits of animation banks (`bnk`) as operations of the workspace: import
//! a clip from an animation file (replace one or add one), replace the
//! events of a clip, replace the node graph.
//!
//! Each function is the whole action — read the bank as the workspace has
//! it, change it, check it, record one undoable operation — so that the
//! command line (`anim import`, `anim events`, `anim graph`), the MCP
//! server (`import_animation`, `clip_events`, `animation_graph`) and the
//! editor do the same thing.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::game::GameInstance;
use crate::import::anim::{
    ClipTarget, ImportOptions, ImportSummary, RestPose, import_clip, read_animations,
};
use crate::resource::anim::{AnimBank, ClipEvent, TYPE_BNK};
use crate::resource::anim_encode::EncodeOptions;
use crate::resource::anim_graph::{AnimGraph, GraphJson, Issue, Severity};
use crate::resource::anim_image::BankImage;
use crate::resource::rcb::Rig;
use crate::workspace::{ResourceKey, Workspace};
use crate::{Error, Result};

fn invalid<T>(text: impl Into<String>) -> Result<T> {
    Err(Error::InvalidArgument(text.into()))
}

/// The stream index and resource index of the bank `key`.
pub fn find_bank(game: &GameInstance, key: &ResourceKey) -> Result<(usize, usize)> {
    if key.type_id != TYPE_BNK {
        return invalid(format!("{key} is not an animation bank (bnk)"));
    }
    let stream = game
        .find_stream(&key.stream)
        .ok_or_else(|| Error::InvalidArgument(format!("stream `{}` not found", key.stream)))?;
    let file = game.open_stream(stream)?;
    let index = key
        .find_in(file.layout())
        .ok_or_else(|| Error::InvalidArgument(format!("{key} is not in its stream")))?;
    Ok((stream, index))
}

/// Index of the clip named `text` (name or hexadecimal hash).
pub fn find_clip(bank: &AnimBank, text: &str) -> Result<usize> {
    bank.clips
        .iter()
        .position(|c| {
            c.name
                .as_deref()
                .is_some_and(|n| n.eq_ignore_ascii_case(text))
                || u32::from_str_radix(text.trim_start_matches("0x"), 16) == Ok(c.hash)
        })
        .or_else(|| {
            let hash = crate::hash::sdbm(text.to_ascii_lowercase().as_bytes());
            bank.clips.iter().position(|c| c.hash == hash)
        })
        .ok_or_else(|| Error::InvalidArgument(format!("the bank has no clip `{text}`")))
}

fn default_scheme() -> u32 {
    4
}
fn default_angle() -> f32 {
    0.002
}
fn default_shift() -> f32 {
    0.0005
}

/// An animation file to import into a bank.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ImportAnimation {
    /// The animation file (an absolute path): .glb, .gltf, .fbx, .usd,
    /// .usda, .usdc or .usdz.
    pub file: PathBuf,
    /// The clip to replace (name or hexadecimal hash), or the name of the
    /// new clip with `add`.
    pub clip: String,
    /// Add a new clip (and a named entry that plays it) instead of
    /// replacing one.
    #[serde(default)]
    pub add: bool,
    /// With `add`: do not add a named entry.
    #[serde(default)]
    pub no_entry: bool,
    /// With `add`: priority of the entry (a later bank replaces an entry of
    /// the same name only with a higher one).
    #[serde(default)]
    pub priority: u32,
    /// With `add`: copy root motion, looping and bone set from this clip
    /// [default: the first clip of the bank].
    #[serde(default)]
    pub like: Option<String>,
    /// With `add`: bone set hash, hexadecimal.
    #[serde(default)]
    pub bone_set: Option<String>,
    /// The animation of the file to take: its name or number [default: the first].
    #[serde(default)]
    pub animation: Option<String>,
    /// Make the clip loop (true) or play once (false) [default: as the
    /// clip it replaces or is modelled on].
    #[serde(default)]
    pub looping: Option<bool>,
    /// Game bone names → joint names of the file, for joints named differently.
    #[serde(default)]
    pub map: HashMap<String, String>,
    /// How the file's rest pose meets the skeleton: `auto` (default),
    /// `as-is` (made from the game skeleton), `axes` (the same bones with
    /// other axes, as Blender's FBX) or `retarget` (another rig).
    #[serde(default)]
    pub rest: Option<String>,
    /// Compression scheme: 4 (default, as most game clips), 1 or 3.
    #[serde(default = "default_scheme")]
    pub scheme: u32,
    /// Largest rotation error, radians (default 0.002).
    #[serde(default = "default_angle")]
    pub max_angle: f32,
    /// Largest translation error, metres (default 0.0005).
    #[serde(default = "default_shift")]
    pub max_shift: f32,
}

/// What [`import_animation`] did.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ImportedAnimation {
    /// Name of the animation of the file that was taken.
    pub source: String,
    pub summary: ImportSummary,
    /// Label of the operation; `None` for a dry run or when nothing changed.
    pub operation: Option<String>,
    pub stream: String,
}

/// Imports an animation into the bank `key`: one undoable operation, or
/// only the summary with `dry_run`. `rigs` are the rigs of the game
/// ([`crate::game::GameAssets::rigs`]).
pub fn import_animation(
    game: &GameInstance,
    ws: &mut Workspace,
    rigs: &[Rig],
    key: &ResourceKey,
    import: &ImportAnimation,
    dry_run: bool,
) -> Result<ImportedAnimation> {
    let (stream, index) = find_bank(game, key)?;
    let data = ws.current_data(game, stream, index)?;
    let bank = AnimBank::parse(&data)?;
    let animations = read_animations(&import.file).map_err(|e| {
        Error::InvalidArgument(format!("cannot read {}: {e}", import.file.display()))
    })?;
    let source = match &import.animation {
        None => animations.first(),
        Some(a) => match a.parse::<usize>() {
            Ok(i) => animations.get(i),
            Err(_) => animations.iter().find(|x| x.name.eq_ignore_ascii_case(a)),
        },
    }
    .ok_or_else(|| {
        let names: Vec<&str> = animations.iter().map(|a| a.name.as_str()).collect();
        Error::InvalidArgument(format!(
            "no animation `{}` in the file (it has {names:?})",
            import.animation.as_deref().unwrap_or("0")
        ))
    })?;
    let target = if import.add {
        ClipTarget::Add {
            name: import.clip.clone(),
            entry: !import.no_entry,
            priority: import.priority,
            like: import
                .like
                .as_deref()
                .map(|l| find_clip(&bank, l))
                .transpose()?,
        }
    } else {
        ClipTarget::Replace(find_clip(&bank, &import.clip)?)
    };
    let options = ImportOptions {
        encode: EncodeOptions {
            scheme: import.scheme,
            max_angle: import.max_angle,
            max_translation: import.max_shift,
        },
        looping: import.looping,
        bone_set: import
            .bone_set
            .as_deref()
            .map(|h| u32::from_str_radix(h.trim_start_matches("0x"), 16))
            .transpose()
            .map_err(|_| Error::InvalidArgument("the bone set is a hexadecimal hash".into()))?,
        map: import.map.clone(),
        rest: match import.rest.as_deref().unwrap_or("auto") {
            "auto" => RestPose::Auto,
            "as-is" => RestPose::AsIs,
            "axes" => RestPose::Axes,
            "retarget" => RestPose::Retarget,
            other => {
                return invalid(format!(
                    "`{other}`: the rest pose is auto, as-is, axes or retarget"
                ));
            }
        },
    };
    let (out, summary) = import_clip(&data, rigs, source, &target, &options)?;
    let mut operation = None;
    if !dry_run {
        let label = format!("Import animation {} into {key}", summary.clip);
        if ws.set_resource(game, stream, index, out, &label)? {
            operation = Some(label);
        }
    }
    Ok(ImportedAnimation {
        source: source.name.clone(),
        summary,
        operation,
        stream: game.streams()[stream].stream_name(),
    })
}

/// The events of clip `clip` of the bank `key` as the workspace has it.
pub fn clip_events(
    game: &GameInstance,
    ws: &Workspace,
    key: &ResourceKey,
    clip: &str,
) -> Result<(String, Vec<ClipEvent>)> {
    let (stream, index) = find_bank(game, key)?;
    let bank = AnimBank::parse(&ws.current_data(game, stream, index)?)?;
    let clip = find_clip(&bank, clip)?;
    Ok((
        bank.clips[clip].display_name(),
        bank.clips[clip].events.clone(),
    ))
}

/// Replaces the events of clip `clip` of the bank `key`: one undoable
/// operation. Returns the name of the clip and the label of the operation
/// (`None` when the clip already has these events).
pub fn set_clip_events(
    game: &GameInstance,
    ws: &mut Workspace,
    key: &ResourceKey,
    clip: &str,
    events: &[ClipEvent],
) -> Result<(String, Option<String>)> {
    let (stream, index) = find_bank(game, key)?;
    let data = ws.current_data(game, stream, index)?;
    let bank = AnimBank::parse(&data)?;
    let clip = find_clip(&bank, clip)?;
    let mut image = BankImage::parse(&data)?;
    image.set_events(clip, events)?;
    let name = bank.clips[clip].display_name();
    let label = format!("Edit events of {name} in {key}");
    let changed = ws.set_resource(game, stream, index, image.encode()?, &label)?;
    Ok((name, changed.then_some(label)))
}

/// The node graph of a bank.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GraphView {
    pub entries: usize,
    pub nodes: usize,
    /// Node slots of the bank.
    pub slots: usize,
    /// The graph in the form [`set_graph`] reads; only `entry` when one was asked for.
    pub graph: serde_json::Value,
    pub issues: Vec<Issue>,
}

/// The node graph of the bank `key` as the workspace has it, checked;
/// `entry` leaves only that entry in the JSON.
pub fn graph(
    game: &GameInstance,
    ws: &Workspace,
    key: &ResourceKey,
    entry: Option<&str>,
) -> Result<GraphView> {
    let (stream, index) = find_bank(game, key)?;
    let data = ws.current_data(game, stream, index)?;
    let image = BankImage::parse(&data)?;
    let mut g = AnimGraph::parse(&image)?;
    let stats = g.stats();
    let issues = g.check(&image);
    if let Some(name) = entry {
        let e = g
            .entries
            .iter()
            .position(|e| e.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| Error::InvalidArgument(format!("the bank has no entry `{name}`")))?;
        g.entries = vec![g.entries[e].clone()];
    }
    Ok(GraphView {
        entries: stats.entries,
        nodes: stats.nodes,
        slots: image.node_count as usize,
        graph: serde_json::to_value(g.to_json(&image))
            .map_err(|e| Error::InvalidArgument(e.to_string()))?,
        issues,
    })
}

/// What [`set_graph`] did.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GraphSet {
    pub entries: usize,
    pub nodes: usize,
    pub issues: Vec<Issue>,
    /// Label of the operation; `None` when nothing changed.
    pub operation: Option<String>,
    pub stream: String,
}

/// Replaces the node graph of the bank `key` with `json` (the whole graph,
/// as [`graph`] gives it): one undoable operation. A graph with errors is
/// refused with the list of its problems.
pub fn set_graph(
    game: &GameInstance,
    ws: &mut Workspace,
    key: &ResourceKey,
    json: &GraphJson,
) -> Result<GraphSet> {
    let (stream, index) = find_bank(game, key)?;
    let data = ws.current_data(game, stream, index)?;
    let g = AnimGraph::from_json(json)?;
    let mut image = BankImage::parse(&data)?;
    let issues = g.check(&image);
    if issues.iter().any(|i| i.severity == Severity::Error) {
        let lines: Vec<String> = issues
            .iter()
            .map(|i| format!("{:?}: node {:?}: {}", i.severity, i.node, i.text))
            .collect();
        return invalid(format!(
            "the graph has errors; nothing recorded:\n{}",
            lines.join("\n")
        ));
    }
    g.store(&mut image)?;
    let stats = g.stats();
    let label = format!("Edit the animation graph of {key}");
    let changed = ws.set_resource(game, stream, index, image.encode()?, &label)?;
    Ok(GraphSet {
        entries: stats.entries,
        nodes: stats.nodes,
        issues,
        operation: changed.then_some(label),
        stream: game.streams()[stream].stream_name(),
    })
}

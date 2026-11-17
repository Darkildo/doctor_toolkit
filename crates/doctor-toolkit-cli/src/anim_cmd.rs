//! Character animations (`bnk` banks): list clips, export them as glTF,
//! FBX or USD animations on the rig skeleton or on an assembled character,
//! import animations into banks, edit clip events and check the encoder.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use rayon::prelude::*;

use doctor_toolkit::anim_edit::{self, ImportAnimation};
use doctor_toolkit::character::{Character, MeshLocator, parse_selection};
use doctor_toolkit::export::gltf::{SceneAnimation, SceneModel};
use doctor_toolkit::game::{GameAssets, GameInstance};
use doctor_toolkit::resource::anim::{
    AnimBank, ClipEvent, FRAME_RATE, TYPE_BNK, bone_set, find_skeleton,
};
use doctor_toolkit::resource::anim_encode::{EncodeOptions, reencode};
use doctor_toolkit::resource::anim_graph::{self, AnimGraph, GraphJson};
use doctor_toolkit::resource::anim_image::BankImage;
use doctor_toolkit::resource::partsdb::PartsDb;
use doctor_toolkit::scene::{self, ModelFormat};
use doctor_toolkit::workspace::ResourceKey;

use crate::workspace_cmd::{self, EditMode};

macro_rules! out {
    ($($arg:tt)*) => {
        if writeln!(std::io::stdout(), $($arg)*).is_err() {
            std::process::exit(0);
        }
    };
}

const TYPE_PARTSDB: u32 = doctor_toolkit::hash::sdbm(b"partsdb");

/// Encoder settings shared by the commands that encode clips.
#[derive(clap::Args, Clone)]
pub struct EncodeArgs {
    /// Compression scheme: 4 (as most game clips), 1 or 3
    #[arg(long, default_value_t = 4)]
    scheme: u32,
    /// Largest rotation error, radians
    #[arg(long, default_value_t = 0.002)]
    max_angle: f32,
    /// Largest translation error, metres
    #[arg(long, default_value_t = 0.0005)]
    max_shift: f32,
}

impl EncodeArgs {
    fn options(&self) -> EncodeOptions {
        EncodeOptions {
            scheme: self.scheme,
            max_angle: self.max_angle,
            max_translation: self.max_shift,
        }
    }
}

#[derive(Subcommand)]
pub enum AnimCommand {
    /// List the animation banks of a stream, their clips, named entries and clip events
    List {
        game_dir: PathBuf,
        /// Stream, e.g. `gf2/globals_global`
        stream: String,
        /// Only this bank (resource index)
        bank: Option<usize>,
    },
    /// Export clips of a bank as animations (glTF, FBX, USD), on the rig's skeleton or on a character
    Export {
        game_dir: PathBuf,
        stream: String,
        /// Bank resource index (see `anim list`)
        bank: usize,
        /// Clip name or hexadecimal hash (repeatable); default: every clip
        #[arg(long = "clip")]
        clips: Vec<String>,
        /// Dress the skeleton: parts database name, e.g. `player`
        #[arg(long, requires = "set")]
        partsdb: Option<String>,
        /// Set of the parts database, e.g. `_all`
        #[arg(long)]
        set: Option<String>,
        /// Stream of the parts database
        #[arg(long, default_value = "gf2/globalfolder/characters/allcharacters")]
        partsdb_stream: String,
        /// Output file (.glb, .fbx, .usd, .usda, .usdc, .usdz)
        #[arg(short, long)]
        output: PathBuf,
        /// File format (default: from the output extension, else glb)
        #[arg(long)]
        format: Option<ModelFormat>,
    },
    /// Import an animation (glTF, FBX, USD) into a bank: replace a clip or add a new one
    Import(ImportArgs),
    /// Print the events of a clip as JSON, or replace them from a JSON file
    Events {
        game_dir: PathBuf,
        /// The bank: a resource key such as `gf2/globals_global:bnk:c0e2841d` (see `anim list`)
        bank: String,
        /// Clip name or hexadecimal hash
        #[arg(long)]
        clip: String,
        /// Replace the events with the list in this JSON file (as printed)
        #[arg(long, value_name = "FILE")]
        set: Option<PathBuf>,
        #[command(flatten)]
        mode: EditMode,
    },
    /// Decode every clip, encode it again and report the error and the size
    EncodeTest {
        game_dir: PathBuf,
        /// Only streams whose name contains this text
        #[arg(long)]
        stream: Option<String>,
        #[command(flatten)]
        encode: EncodeArgs,
        /// Print the summary as JSON
        #[arg(long)]
        json: bool,
    },
    /// Print the node graph of a bank (entries and their node trees), check it, or replace it from JSON
    Graph {
        game_dir: PathBuf,
        /// The bank: a resource key such as `gf2/globals_global:bnk:c0e2841d` (see `anim list`)
        bank: String,
        /// Only this entry (name)
        #[arg(long)]
        entry: Option<String>,
        /// Print the graph as JSON (the form `--set` reads)
        #[arg(long, conflicts_with = "set")]
        json: bool,
        /// Replace the graph with the one in this JSON file (as printed by `--json`)
        #[arg(long, value_name = "FILE")]
        set: Option<PathBuf>,
        #[command(flatten)]
        mode: EditMode,
    },
    /// Print the node types of animation graphs and the game's animation controls
    NodeTypes {
        /// As JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(clap::Args)]
pub struct ImportArgs {
    game_dir: PathBuf,
    /// The bank: a resource key such as `gf2/globals_global:bnk:c0e2841d` (see `anim list`)
    bank: String,
    /// Animation file (.glb, .gltf, .fbx, .usd, .usda, .usdc, .usdz)
    file: PathBuf,
    /// Clip to replace (name or hexadecimal hash), or the name of the new clip with --add
    #[arg(long)]
    clip: String,
    /// Replace the clip (the default)
    #[arg(long, conflicts_with = "add")]
    replace: bool,
    /// Add a new clip and a named entry that plays it
    #[arg(long)]
    add: bool,
    /// With --add: do not add a named entry
    #[arg(long, requires = "add")]
    no_entry: bool,
    /// With --add: priority of the entry (a later bank replaces an entry of the same name only with a higher one)
    #[arg(long, default_value_t = 0, requires = "add")]
    priority: u32,
    /// With --add: copy root motion, looping and bone set from this clip (default: the first)
    #[arg(long, requires = "add")]
    like: Option<String>,
    /// With --add: bone set hash (hexadecimal)
    #[arg(long, requires = "add")]
    bone_set: Option<String>,
    /// Animation of the file to take (name or index); default: the first
    #[arg(long)]
    animation: Option<String>,
    /// Make the clip loop
    #[arg(long = "loop", conflicts_with = "no_loop")]
    looping: bool,
    /// Make the clip play once
    #[arg(long)]
    no_loop: bool,
    /// JSON object mapping game bone names to joint names of the file
    #[arg(long, value_name = "FILE")]
    map: Option<PathBuf>,
    /// How the file's rest pose meets the skeleton: auto, as-is (made from the
    /// game skeleton), axes (the same bones with other axes, as Blender's FBX)
    /// or retarget (another rig)
    #[arg(long, default_value = "auto", value_parser = ["auto", "as-is", "axes", "retarget"])]
    rest: String,
    #[command(flatten)]
    encode: EncodeArgs,
    /// Print what would be done without recording anything
    #[arg(long)]
    dry_run: bool,
    /// Print the summary as JSON
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    mode: EditMode,
}

pub fn run(command: AnimCommand) -> Result<()> {
    match command {
        AnimCommand::List {
            game_dir,
            stream,
            bank,
        } => list(&game_dir, &stream, bank),
        AnimCommand::Export {
            game_dir,
            stream,
            bank,
            clips,
            partsdb,
            set,
            partsdb_stream,
            output,
            format,
        } => export(
            &game_dir,
            &stream,
            bank,
            &clips,
            partsdb.as_deref().zip(set.as_deref()),
            &partsdb_stream,
            &output,
            format,
        ),
        AnimCommand::Import(args) => import(args),
        AnimCommand::Events {
            game_dir,
            bank,
            clip,
            set,
            mode,
        } => events(&game_dir, &bank, &clip, set.as_deref(), &mode),
        AnimCommand::EncodeTest {
            game_dir,
            stream,
            encode,
            json,
        } => encode_test(&game_dir, stream.as_deref(), &encode.options(), json),
        AnimCommand::Graph {
            game_dir,
            bank,
            entry,
            json,
            set,
            mode,
        } => graph(
            &game_dir,
            &bank,
            entry.as_deref(),
            json,
            set.as_deref(),
            &mode,
        ),
        AnimCommand::NodeTypes { json } => node_types(json),
    }
}

/// The stream index and resource index of the bank `key`.
fn find_bank(game: &GameInstance, key: &str) -> Result<(usize, usize, ResourceKey)> {
    let key: ResourceKey = key
        .parse()
        .with_context(|| format!("`{key}` is not a resource key (stream:bnk:id)"))?;
    let (stream, index) = anim_edit::find_bank(game, &key)?;
    Ok((stream, index, key))
}

/// Index of the clip named `text` (name or hexadecimal hash).
fn find_clip(bank: &AnimBank, text: &str) -> Result<usize> {
    Ok(anim_edit::find_clip(bank, text)?)
}

fn import(args: ImportArgs) -> Result<()> {
    let game = Arc::new(GameInstance::open(&args.game_dir)?);
    let mut ws = workspace_cmd::open_for_edit(&game, &args.mode)?;
    let (_, _, key) = find_bank(&game, &args.bank)?;
    let map: HashMap<String, String> = match &args.map {
        Some(p) => serde_json::from_slice(
            &std::fs::read(p).with_context(|| format!("cannot read {}", p.display()))?,
        )
        .with_context(|| format!("{}: expected a JSON object of names", p.display()))?,
        None => HashMap::new(),
    };
    let (resources, _) = game.build_index(|_| {});
    let mut assets = GameAssets::new(game.clone(), &resources);
    let rigs = assets.rigs().to_vec();
    let done = anim_edit::import_animation(
        &game,
        &mut ws,
        &rigs,
        &key,
        &ImportAnimation {
            file: args.file.clone(),
            clip: args.clip.clone(),
            add: args.add,
            no_entry: args.no_entry,
            priority: args.priority,
            like: args.like.clone(),
            bone_set: args.bone_set.clone(),
            animation: args.animation.clone(),
            looping: if args.looping {
                Some(true)
            } else if args.no_loop {
                Some(false)
            } else {
                None
            },
            map,
            rest: Some(args.rest.clone()),
            scheme: args.encode.scheme,
            max_angle: args.encode.max_angle,
            max_shift: args.encode.max_shift,
        },
        args.dry_run,
    )?;
    let summary = &done.summary;
    if args.json {
        out!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        out!(
            "{} clip {} ({:08x}) of {key} from `{}`",
            summary.mode,
            summary.clip,
            summary.clip_hash,
            done.source
        );
        out!(
            "  {} frames ({:.2} s at {FRAME_RATE} fps), {}, bone set {:08x}: {} bone(s), {} from the file",
            summary.frames,
            summary.frames.saturating_sub(1) as f32 / FRAME_RATE,
            if summary.looping { "looping" } else { "once" },
            summary.bone_set,
            summary.bones,
            summary.mapped.len()
        );
        out!("  rest pose: {}", summary.rest);
        if (summary.scale - 1.0).abs() > 1e-3 {
            out!("  translations scaled by {:.3}", summary.scale);
        }
        out!(
            "  scheme {}: {} bytes of streams (was {}), error ≤ {:.5} rad, {:.6} m; bank {} → {} bytes",
            summary.scheme,
            summary.streams_after,
            summary.streams_before,
            summary.max_angle,
            summary.max_translation,
            summary.bank_before,
            summary.bank_after
        );
        if let Some(entry) = &summary.entry {
            out!("  named entry `{entry}` plays the clip");
        }
        if !summary.unused.is_empty() {
            out!(
                "  joints of the file not used: {}",
                summary.unused.join(", ")
            );
        }
        for w in &summary.warnings {
            out!("  warning: {w}");
        }
    }
    if args.dry_run {
        return Ok(());
    }
    if done.operation.is_none() {
        out!("nothing changed");
        return Ok(());
    }
    workspace_cmd::finish_edit(&game, &mut ws, &args.mode, vec![done.stream])
}

fn events(
    game_dir: &std::path::Path,
    bank_key: &str,
    clip: &str,
    set: Option<&std::path::Path>,
    mode: &EditMode,
) -> Result<()> {
    let game = GameInstance::open(game_dir)?;
    let (stream, index, key) = find_bank(&game, bank_key)?;
    let Some(path) = set else {
        let data = match doctor_toolkit::workspace::Workspace::for_game(
            &game,
            mode.workspace_dir.as_deref(),
        ) {
            Ok(ws) => ws.current_data(&game, stream, index)?,
            Err(_) => game.open_stream(stream)?.resource_data(index)?,
        };
        let bank = AnimBank::parse(&data)?;
        let clip = find_clip(&bank, clip)?;
        out!(
            "{}",
            serde_json::to_string_pretty(&bank.clips[clip].events)?
        );
        return Ok(());
    };
    let mut ws = workspace_cmd::open_for_edit(&game, mode)?;
    let list: Vec<ClipEvent> = serde_json::from_slice(
        &std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?,
    )
    .with_context(|| format!("{}: expected a list of events", path.display()))?;
    let (name, operation) = anim_edit::set_clip_events(&game, &mut ws, &key, clip, &list)?;
    out!("{name}: {} event(s)", list.len());
    if operation.is_none() {
        out!("nothing changed");
        return Ok(());
    }
    let name = game.streams()[stream].stream_name();
    workspace_cmd::finish_edit(&game, &mut ws, mode, vec![name])
}

fn encode_test(
    game_dir: &std::path::Path,
    filter: Option<&str>,
    options: &EncodeOptions,
    json: bool,
) -> Result<()> {
    let game = Arc::new(GameInstance::open(game_dir)?);
    let (resources, _) = game.build_index(|_| {});
    let mut assets = GameAssets::new(game.clone(), &resources);
    let rigs = assets.rigs().to_vec();
    #[derive(Default)]
    struct Totals {
        clips: usize,
        failed: usize,
        original_bytes: usize,
        encoded_bytes: usize,
        max_angle: f32,
        max_shift: f32,
        mean_angle: f64,
        mean_shift: f64,
        codecs: [usize; 16],
    }
    let totals = std::sync::Mutex::new(Totals::default());
    game.streams()
        .par_iter()
        .filter(|f| filter.is_none_or(|t| f.rel_path.contains(t)))
        .for_each(|file| {
            let Ok(stream) = doctor_toolkit::game::StreamFile::open(&file.path) else {
                return;
            };
            for (index, r) in stream.layout().resources.iter().enumerate() {
                if r.header.type_id != TYPE_BNK {
                    continue;
                }
                let Ok(data) = stream.resource_data(index) else {
                    continue;
                };
                let Ok(bank) = AnimBank::parse(&data) else {
                    continue;
                };
                for clip in &bank.clips {
                    let Some(bones) = bone_set(&rigs, clip.bone_set) else {
                        continue;
                    };
                    let result = reencode(clip, &data, bones, options);
                    let mut t = totals.lock().unwrap();
                    match result {
                        Ok(r) => {
                            t.clips += 1;
                            t.original_bytes += r.original_size;
                            t.encoded_bytes += r.encoded_size;
                            t.max_angle = t.max_angle.max(r.max_angle);
                            t.max_shift = t.max_shift.max(r.max_translation);
                            t.mean_angle += r.max_angle as f64;
                            t.mean_shift += r.max_translation as f64;
                            for (a, b) in t.codecs.iter_mut().zip(r.codecs) {
                                *a += b;
                            }
                        }
                        Err(e) => {
                            t.failed += 1;
                            drop(t);
                            out!("{} #{index} {}: {e}", file.rel_path, clip.display_name());
                        }
                    }
                }
            }
        });
    let mut t = totals.into_inner().unwrap();
    if t.clips > 0 {
        t.mean_angle /= t.clips as f64;
        t.mean_shift /= t.clips as f64;
    }
    if json {
        let value = serde_json::json!({
            "clips": t.clips,
            "failed": t.failed,
            "original_bytes": t.original_bytes,
            "encoded_bytes": t.encoded_bytes,
            "max_angle": t.max_angle,
            "max_shift": t.max_shift,
            "mean_angle": t.mean_angle,
            "mean_shift": t.mean_shift,
            "codecs": t.codecs,
        });
        out!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    out!(
        "{} clip(s) re-encoded (scheme {}, budget {} rad, {} m), {} failed",
        t.clips,
        options.scheme,
        options.max_angle,
        options.max_translation,
        t.failed
    );
    out!(
        "  streams: {} bytes in the game, {} re-encoded ({:.1} %)",
        t.original_bytes,
        t.encoded_bytes,
        100.0 * t.encoded_bytes as f64 / t.original_bytes.max(1) as f64
    );
    out!(
        "  largest error per clip: mean {:.5} rad / {:.6} m, max {:.5} rad / {:.6} m",
        t.mean_angle,
        t.mean_shift,
        t.max_angle,
        t.max_shift
    );
    let names = [
        "const",
        "linear",
        "quadratic",
        "cubic",
        "f32",
        "segments",
        "u8",
        "u16",
        "",
        "",
        "",
        "",
        "zero",
        "one",
        "bits",
        "",
    ];
    let used: Vec<String> = t
        .codecs
        .iter()
        .enumerate()
        .filter(|(_, n)| **n > 0)
        .map(|(c, n)| format!("{} {n}", names[c]))
        .collect();
    out!("  codecs: {}", used.join(", "));
    if t.failed > 0 {
        bail!("{} clip(s) failed", t.failed);
    }
    Ok(())
}

fn list(game_dir: &std::path::Path, stream_name: &str, only: Option<usize>) -> Result<()> {
    let game = GameInstance::open(game_dir)?;
    let stream = game
        .find_stream(stream_name)
        .with_context(|| format!("stream `{stream_name}` not found"))?;
    let file = game.open_stream(stream)?;
    for (index, r) in file.layout().resources.iter().enumerate() {
        if r.header.type_id != TYPE_BNK || only.is_some_and(|o| o != index) {
            continue;
        }
        let data = file.resource_data(index)?;
        let bank = AnimBank::parse(&data)?;
        let key = ResourceKey::new(&game.streams()[stream].stream_name(), &r.header);
        out!(
            "{index:6}  bank {:08x} ({key}), skeleton {:08x}, {} clip(s)",
            bank.hash,
            bank.skeleton,
            bank.clips.len()
        );
        let image = BankImage::parse(&data).ok();
        if let Some(image) = &image {
            for entry in image.entry_list() {
                let clips: Vec<String> = entry
                    .clips
                    .iter()
                    .map(|&c| bank.clips[c].display_name())
                    .collect();
                out!(
                    "        entry {:<42} priority {}  plays {}",
                    entry.name,
                    entry.priority.map_or("-".into(), |p| p.to_string()),
                    clips.join(", ")
                );
            }
        }
        for (k, clip) in bank.clips.iter().enumerate() {
            let looping = image.as_ref().is_some_and(|i| i.clips[k].looping());
            out!(
                "        {:<48} {:5} frames  bone set {:08x}  scheme {}{}",
                clip.display_name(),
                clip.frames,
                clip.bone_set,
                clip.scheme,
                if looping { "  loop" } else { "" }
            );
            for event in &clip.events {
                let args: Vec<String> = event
                    .args
                    .iter()
                    .map(|a| match a {
                        doctor_toolkit::resource::anim::EventArg::Int(v) => v.to_string(),
                        doctor_toolkit::resource::anim::EventArg::Float(v) => v.to_string(),
                        doctor_toolkit::resource::anim::EventArg::Text(t) => format!("{t:?}"),
                    })
                    .collect();
                out!(
                    "            frame {:6.1}  {} {}",
                    event.frame,
                    event.name,
                    args.join(" ")
                );
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn export(
    game_dir: &std::path::Path,
    stream_name: &str,
    index: usize,
    wanted: &[String],
    character: Option<(&str, &str)>,
    partsdb_stream: &str,
    output: &std::path::Path,
    format: Option<ModelFormat>,
) -> Result<()> {
    let format = format
        .or_else(|| ModelFormat::from_path(output))
        .unwrap_or(ModelFormat::Glb);
    if !format.has_skeletons() {
        bail!("{format} keeps no skeleton or animation; use glb, fbx or usd");
    }
    let game = Arc::new(GameInstance::open(game_dir)?);
    let stream = game
        .find_stream(stream_name)
        .with_context(|| format!("stream `{stream_name}` not found"))?;
    let file = game.open_stream(stream)?;
    let resource = file
        .layout()
        .resources
        .get(index)
        .with_context(|| format!("no resource {index} in {stream_name}"))?;
    if resource.header.type_id != TYPE_BNK {
        bail!("resource {index} of {stream_name} is not an animation bank (bnk)");
    }
    let data = file.resource_data(index)?;
    let bank = AnimBank::parse(&data)?;
    let (resources, _) = game.build_index(|_| {});
    let mut assets = GameAssets::new(game.clone(), &resources);
    let rigs = assets.rigs().to_vec();
    let skeleton = find_skeleton(&rigs, bank.skeleton)
        .with_context(|| format!("no rig has skeleton {:08x}", bank.skeleton))?;
    let mut animations = Vec::new();
    for clip in &bank.clips {
        let selected = wanted.is_empty()
            || wanted.iter().any(|w| {
                clip.name
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(w))
                    || u32::from_str_radix(w.trim_start_matches("0x"), 16) == Ok(clip.hash)
            });
        if !selected {
            continue;
        }
        let Some(bones) = bone_set(&rigs, clip.bone_set) else {
            out!(
                "  {}: bone set {:08x} not found",
                clip.display_name(),
                clip.bone_set
            );
            continue;
        };
        match clip.decode(&data, bones) {
            Ok(tracks) => {
                out!(
                    "  {}: {} frames, {} bones",
                    clip.display_name(),
                    clip.frames,
                    tracks.len()
                );
                animations.push(SceneAnimation {
                    name: clip.display_name(),
                    skeleton,
                    tracks,
                    frame_rate: FRAME_RATE,
                });
            }
            Err(e) => out!("  {}: {e}", clip.display_name()),
        }
    }
    if animations.is_empty() {
        bail!("no clip decoded");
    }
    let mut dressed = None;
    if let Some((db_name, set_name)) = character {
        let db_stream = game
            .find_stream(partsdb_stream)
            .with_context(|| format!("stream `{partsdb_stream}` not found"))?;
        let db_file = game.open_stream(db_stream)?;
        let mut db = None;
        for (i, r) in db_file.layout().resources.iter().enumerate() {
            if r.header.type_id == TYPE_PARTSDB {
                let parsed = PartsDb::parse(&db_file.resource_data(i)?)?;
                if parsed.name.eq_ignore_ascii_case(db_name) {
                    db = Some(parsed);
                    break;
                }
            }
        }
        let db = db.with_context(|| format!("no parts database `{db_name}`"))?;
        let set = db
            .set_index(set_name)
            .with_context(|| format!("no set `{set_name}`"))?;
        let selection = parse_selection(&db, set, &[])?;
        let meshes = MeshLocator::new(&resources);
        dressed = Some(Character::load(
            &game, &meshes, db_stream, &db, set, &selection, 0,
        )?);
    }
    let models: Vec<SceneModel<'_>> = dressed
        .as_ref()
        .map(|c| {
            c.parts
                .iter()
                .map(|p| SceneModel {
                    model: &p.model,
                    name: p.name.clone(),
                    materials: p.materials.clone(),
                    morphs: c.morphs.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    let override_skeleton = models.is_empty().then_some(skeleton);
    let built = scene::game::build(&models, override_skeleton, &animations, &mut assets)?;
    let written = scene::write_file(&built, output, Some(format))
        .with_context(|| format!("cannot write {}", output.display()))?;
    let bytes: u64 = written
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .sum();
    out!(
        "{}: {} animation(s), {} file(s), {bytes} bytes",
        output.display(),
        animations.len(),
        written.len()
    );
    Ok(())
}

/// Text of parameter words typed by `fields`, with control names.
fn words_text(words: &[u32], fields: Option<&'static [anim_graph::Field]>, kind: u8) -> String {
    use anim_graph::FieldType;
    words
        .iter()
        .enumerate()
        .map(|(i, &w)| {
            let field = anim_graph::field(fields, i);
            let value = match field.ty {
                FieldType::U32 if w > 0xFFFF => format!("{w:#010x}"),
                FieldType::U32 => w.to_string(),
                FieldType::F32 => format!("{}", f32::from_bits(w)),
                FieldType::Pair => format!("{}/{}", w & 0xFFFF, w >> 16),
            };
            let control = match (kind & 0x3F, i) {
                (0x05, 0) | (0x21, _) => anim_graph::control_name(w),
                (0x06, 1) => anim_graph::control_name(w & 0xFFFF),
                _ => None,
            };
            match control {
                Some(c) => format!("{}={value} ({c})", field.name),
                None => format!("{}={value}", field.name),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn print_node(g: &AnimGraph, bank: &AnimBank, node: usize, parent: Option<u8>, depth: usize) {
    let n = &g.nodes[node];
    let info = anim_graph::kind_info(n.kind);
    let mut line = format!(
        "{:indent$}#{} [{:02x} {}]",
        "",
        n.number,
        n.kind,
        info.map_or("?", |k| k.label),
        indent = 2 * depth
    );
    if let Some(c) = n.clip {
        let name = bank
            .clips
            .get(c as usize)
            .map_or_else(|| "?".into(), |c| c.display_name());
        line += &format!(" clip {c} {name}");
    }
    if let Some(p) = &n.params {
        line += &format!(" {}", words_text(p, info.and_then(|k| k.params), n.kind));
    }
    if let Some(x) = &n.extra {
        let fields = parent
            .and_then(anim_graph::kind_info)
            .and_then(|k| k.child_extra);
        line += &format!(" | {}", words_text(x, fields, 0));
    }
    out!("{line}");
    for &c in &n.children {
        print_node(g, bank, c, Some(n.kind), depth + 1);
    }
}

fn graph(
    game_dir: &std::path::Path,
    bank_key: &str,
    entry: Option<&str>,
    json: bool,
    set: Option<&std::path::Path>,
    mode: &EditMode,
) -> Result<()> {
    let game = GameInstance::open(game_dir)?;
    let (stream, index, key) = find_bank(&game, bank_key)?;
    let Some(path) = set else {
        let data = match doctor_toolkit::workspace::Workspace::for_game(
            &game,
            mode.workspace_dir.as_deref(),
        ) {
            Ok(ws) => ws.current_data(&game, stream, index)?,
            Err(_) => game.open_stream(stream)?.resource_data(index)?,
        };
        let image = BankImage::parse(&data)?;
        let bank = AnimBank::parse(&data)?;
        let mut g = AnimGraph::parse(&image)?;
        let stats = g.stats();
        let issues = g.check(&image);
        if let Some(name) = entry {
            let e = g
                .entries
                .iter()
                .position(|e| e.name.eq_ignore_ascii_case(name))
                .with_context(|| format!("the bank has no entry `{name}`"))?;
            g.entries = vec![g.entries[e].clone()];
        }
        if json {
            out!("{}", serde_json::to_string_pretty(&g.to_json(&image))?);
            return Ok(());
        }
        out!(
            "{key}: {} entries, {} nodes, {} slots",
            stats.entries,
            stats.nodes,
            image.node_count
        );
        for e in &g.entries {
            out!("{}", e.name);
            print_node(&g, &bank, e.root, None, 1);
        }
        for issue in issues {
            out!(
                "{:?}: node {}: {}",
                issue.severity,
                issue
                    .node
                    .map_or_else(|| "-".into(), |n| g.nodes[n].number.to_string()),
                issue.text
            );
        }
        return Ok(());
    };
    if entry.is_some() {
        bail!("--set replaces the whole graph; --entry only filters printing");
    }
    let mut ws = workspace_cmd::open_for_edit(&game, mode)?;
    let json: GraphJson = serde_json::from_slice(
        &std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?,
    )
    .with_context(|| {
        format!(
            "{}: expected a graph as printed by `anim graph --json`",
            path.display()
        )
    })?;
    let done = anim_edit::set_graph(&game, &mut ws, &key, &json)?;
    for issue in &done.issues {
        out!(
            "{:?}: node {:?}: {}",
            issue.severity,
            issue.node,
            issue.text
        );
    }
    out!("{} entries, {} nodes", done.entries, done.nodes);
    if done.operation.is_none() {
        out!("nothing changed");
        return Ok(());
    }
    workspace_cmd::finish_edit(&game, &mut ws, mode, vec![done.stream])
}

fn node_types(json: bool) -> Result<()> {
    if json {
        out!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "kinds": anim_graph::kinds(),
                "controls": anim_graph::CONTROLS.to_vec(),
            }))?
        );
        return Ok(());
    }
    for k in anim_graph::kinds() {
        let arity = match k.arity {
            anim_graph::Arity::Clip => "clip".to_owned(),
            anim_graph::Arity::Exactly(n) => n.to_string(),
            anim_graph::Arity::Range(a, b) => format!("{a}-{b}"),
        };
        out!(
            "{:#04x}  {:<20} children {:<5} {}  (exe {:#010x})",
            k.kind,
            k.label,
            arity,
            k.help,
            k.sample_fn
        );
        for f in k.params.into_iter().flatten() {
            out!("        {:<16} {:?}  {}", f.name, f.ty, f.help);
        }
        for f in k.child_extra.into_iter().flatten() {
            out!("        child extra {:<6} {:?}  {}", f.name, f.ty, f.help);
        }
    }
    out!("controls:");
    for (i, c) in anim_graph::CONTROLS.iter().enumerate() {
        out!("  {i:>2}  {c}");
    }
    Ok(())
}

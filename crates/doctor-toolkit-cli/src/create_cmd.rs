//! Commands that create resources and streams: `add-resource`,
//! `clone-resource`, `add-stream`, `remove-stream`, `check-refs`.

use std::fs;
use std::io::Write as _;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Args;

use doctor_toolkit::container::ustream::{ResourceHeader, Storage};
use doctor_toolkit::create::{self, CloneOptions, IdForm, Remap};
use doctor_toolkit::game::GameInstance;
use doctor_toolkit::resource::names::type_tags;
use doctor_toolkit::workspace::{
    NewResourceEdit, ResourceEdit, ResourceKey, StorageChoice, Workspace,
};

use crate::workspace_cmd::{self, EditMode};

macro_rules! out {
    ($($arg:tt)*) => {
        if writeln!(std::io::stdout(), $($arg)*).is_err() {
            std::process::exit(0);
        }
    };
}

#[derive(Args)]
pub struct AddResource {
    game_dir: PathBuf,
    /// Stream name, e.g. `gf2/frontend`; a stream of the game or one made with `add-stream`
    stream: String,
    /// Resource type: a tag such as `lh2` or a hexadecimal id
    #[arg(long = "type")]
    type_tag: String,
    /// File with the raw data
    #[arg(long)]
    data: PathBuf,
    /// Ids: one hexadecimal id (repeated four times) or four separated by dots
    /// [default: a new unused id derived from the stream, type and file name]
    #[arg(long)]
    ids: Option<String>,
    /// Place it before this resource index of the stream [default: at the end]
    #[arg(long)]
    before: Option<usize>,
    /// Memory policy name or hexadecimal hash [default: the one of the type]
    #[arg(long)]
    policy: Option<String>,
    /// Load alignment [default: the one of the type]
    #[arg(long)]
    alignment: Option<u32>,
    /// Store RefPack-compressed
    #[arg(long)]
    compress: bool,
    #[command(flatten)]
    mode: EditMode,
}

#[derive(Args)]
pub struct CloneResource {
    game_dir: PathBuf,
    /// Stream that holds the resources, e.g. `gf2/frontend`
    stream: String,
    /// Index of a resource to clone (repeatable). Texture header and data,
    /// mesh and meshvolatile are always cloned together
    #[arg(long = "index", required = true)]
    indices: Vec<usize>,
    /// Put the clones into this stream instead [default: the same stream, after the sources]
    #[arg(long)]
    to: Option<String>,
    /// Text the new ids are derived from [default: the stream name]
    #[arg(long)]
    seed: Option<String>,
    /// Keep the references between the clones pointing at the originals
    #[arg(long)]
    no_rewrite: bool,
    /// Point this resource of the stream at the clones (repeatable): a mesh
    /// at cloned materials, a material at cloned textures, a metamodel or
    /// simgroup at cloned resources
    #[arg(long = "retarget", value_name = "INDEX")]
    retarget: Vec<usize>,
    #[command(flatten)]
    mode: EditMode,
}

#[derive(Args)]
pub struct AddStream {
    game_dir: PathBuf,
    /// Name of the new stream, e.g. `gf2/doctor/props` (file `gf2/doctor/props.str`)
    name: String,
    /// Stream it depends on, e.g. `gf2/frontend`
    #[arg(long)]
    parent: String,
    /// Make it a parented stream of the parent (table-of-contents flags
    /// `0,0,5,0`, as the game's mission streams). Such a stream loads with
    /// its parent only after an `EnableDisableStreams` object with its hash
    /// got its enable message (stage 45: copy one with `level add
    /// --template-stream`, set `EnableDisableStreams.2` to the hash `toc`
    /// prints and `.0` to a message that is sent). Otherwise it is loaded
    /// when game data asks for its hash (`StreamSet`)
    #[arg(long)]
    with_parent: bool,
    /// Table of contents to list it in, relative path, optionally with
    /// another parent there: `gf2/globals_global.toc=gf2/globals_global`
    /// (repeatable) [default: every table that lists the parent]
    #[arg(long = "toc", value_name = "TOC[=PARENT]")]
    tocs: Vec<String>,
    /// 32 hexadecimal digits [default: derived from the name, with an unused hash]
    #[arg(long)]
    guid: Option<String>,
    #[command(flatten)]
    mode: EditMode,
}

#[derive(Args)]
pub struct RemoveStream {
    game_dir: PathBuf,
    name: String,
    #[command(flatten)]
    mode: EditMode,
}

#[derive(Args)]
pub struct CheckRefs {
    game_dir: PathBuf,
    /// Stream names (repeatable) [default: every stream]
    streams: Vec<String>,
}

fn parse_type(text: &str) -> Result<u32> {
    Ok(create::parse_type(text)?)
}

fn parse_ids(text: &str) -> Result<[u32; 4]> {
    Ok(create::parse_ids(text)?)
}

fn find_stream(game: &GameInstance, name: &str) -> Result<usize> {
    game.find_stream(name)
        .with_context(|| format!("stream `{name}` not found in {}", game.root().display()))
}

fn used_ids(game: &GameInstance, ws: &Workspace) -> create::UsedIds {
    let (index, _) = game.build_index(|_| {});
    ws.used_ids(&index)
}

pub fn add_resource(args: AddResource) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let mut ws = workspace_cmd::open_for_edit(&game, &args.mode)?;
    let stream = doctor_toolkit::workspace::key::normalize_stream(&args.stream);
    let type_id = parse_type(&args.type_tag)?;
    let data =
        fs::read(&args.data).with_context(|| format!("cannot read {}", args.data.display()))?;
    let ids = match &args.ids {
        Some(text) => parse_ids(text)?,
        None => {
            let mut used = used_ids(&game, &ws);
            let seed = format!("{stream}:{type_id:08x}:{}", args.data.display());
            used.new_ids(IdForm::for_type(type_id), type_id, &[0; 4], &seed)
        }
    };
    let defaults = create::type_defaults(type_id);
    let mem_policy = match &args.policy {
        None => defaults.mem_policy,
        Some(text) => create::parse_mem_policy(text)?,
    };
    let before = match args.before {
        None => None,
        Some(index) => {
            let file = game.open_stream(find_stream(&game, &stream)?)?;
            let resource = file
                .layout()
                .resources
                .get(index)
                .with_context(|| format!("{stream} has no resource {index}"))?;
            Some(ResourceKey::new(&stream, &resource.header))
        }
    };
    let header = ResourceHeader {
        mem_policy,
        alignment: args.alignment.unwrap_or(defaults.alignment),
        type_id,
        ids,
        data_size: 0,
    };
    let key = ResourceKey {
        stream: stream.clone(),
        type_id,
        ids,
    };
    let label = format!("Add {key} from {}", args.data.display());
    ws.add_resources(
        &game,
        &label,
        &[NewResourceEdit {
            stream: stream.clone(),
            header,
            data,
            storage: if args.compress {
                StorageChoice::Refpack
            } else {
                StorageChoice::Raw
            },
            before,
        }],
    )?;
    out!("added {key}");
    workspace_cmd::finish_edit(&game, &mut ws, &args.mode, vec![stream])
}

pub fn clone_resource(args: CloneResource) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let mut ws = workspace_cmd::open_for_edit(&game, &args.mode)?;
    let source_index = find_stream(&game, &args.stream)?;
    let source = game.streams()[source_index].stream_name();
    let target = args.to.as_deref().map_or_else(
        || source.clone(),
        doctor_toolkit::workspace::key::normalize_stream,
    );
    let file = game.open_stream(source_index)?;
    let mut used = used_ids(&game, &ws);
    let options = CloneOptions {
        seed: args.seed.clone().unwrap_or_else(|| target.clone()),
        rewrite_references: !args.no_rewrite,
        storage: None,
    };
    let (clones, remap) = create::clone_resources(&file, &args.indices, &mut used, &options)?;
    // In the same stream, the clones follow their sources.
    let before = if target == source {
        let last = clones.iter().filter_map(|c| c.source).max().unwrap_or(0);
        file.layout()
            .resources
            .get(last + 1)
            .map(|r| ResourceKey::new(&source, &r.header))
    } else {
        None
    };
    let additions: Vec<NewResourceEdit> = clones
        .iter()
        .map(|c| NewResourceEdit {
            stream: target.clone(),
            header: c.header,
            data: c.data.clone(),
            storage: match c.storage {
                Storage::Raw => StorageChoice::Raw,
                Storage::RefPack => StorageChoice::Refpack,
            },
            before: before.clone(),
        })
        .collect();
    let label = format!(
        "Clone {} resource(s) of {source} into {target}",
        additions.len()
    );
    ws.add_resources(&game, &label, &additions)?;
    for clone in &clones {
        let from = clone
            .source
            .map(|i| ResourceKey::new(&source, &file.layout().resources[i].header));
        let to = ResourceKey::new(&target, &clone.header);
        match from {
            Some(from) => out!("{from} -> {to}"),
            None => out!("{to}"),
        }
    }
    for (old, new) in &remap.names {
        out!("name hash {old:08x} -> {new:08x}");
    }
    let mut streams = vec![target.clone()];
    if !args.retarget.is_empty() {
        retarget(&game, &mut ws, source_index, &args.retarget, &remap)?;
        streams.push(source);
    }
    drop(file);
    streams.dedup();
    workspace_cmd::finish_edit(&game, &mut ws, &args.mode, streams)
}

fn retarget(
    game: &GameInstance,
    ws: &mut Workspace,
    stream_index: usize,
    indices: &[usize],
    remap: &Remap,
) -> Result<()> {
    let file = game.open_stream(stream_index)?;
    let name = game.streams()[stream_index].stream_name();
    let mut edits = Vec::new();
    for &index in indices {
        let resource = file
            .layout()
            .resources
            .get(index)
            .with_context(|| format!("{name} has no resource {index}"))?;
        let key = ResourceKey::new(&name, &resource.header);
        let mut data = match ws.data(&key)? {
            Some(data) => data,
            None => file.resource_data(index)?,
        };
        let changed = create::rewrite_references(resource.header.type_id, &mut data, remap)?;
        out!("{key}: {changed} reference(s) point at the clones");
        if changed > 0 {
            edits.push(ResourceEdit {
                index,
                data,
                storage: StorageChoice::Keep,
            });
        }
    }
    drop(file);
    if !edits.is_empty() {
        ws.set_resources(
            game,
            &format!("Point {} resource(s) of {name} at clones", edits.len()),
            stream_index,
            &edits,
        )?;
    }
    Ok(())
}

pub fn add_stream(args: AddStream) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let mut ws = workspace_cmd::open_for_edit(&game, &args.mode)?;
    let stream = create::new_stream(
        &game,
        &ws,
        &args.name,
        &args.parent,
        args.with_parent,
        &args.tocs,
        args.guid.as_deref(),
    )?;
    let (name, guid_text, hash, tocs) = (
        stream.name.clone(),
        stream.guid.clone(),
        stream.hash,
        stream.tocs.clone(),
    );
    ws.add_stream(&game, stream)?;
    out!(
        "new stream {name}: GUID {guid_text}, hash {hash:08x}, listed in {}",
        tocs.join(", ")
    );
    workspace_cmd::finish_edit(&game, &mut ws, &args.mode, vec![name])
}

pub fn remove_stream(args: RemoveStream) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let mut ws = workspace_cmd::open_for_edit(&game, &args.mode)?;
    let name = doctor_toolkit::workspace::key::normalize_stream(&args.name);
    if !ws.remove_stream(&name)? {
        bail!("{name} is not a stream made by the workspace");
    }
    out!("removed {name}");
    workspace_cmd::finish_edit(&game, &mut ws, &args.mode, vec![name])
}

pub fn check_refs(args: CheckRefs) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let indices: Vec<usize> = if args.streams.is_empty() {
        (0..game.streams().len()).collect()
    } else {
        args.streams
            .iter()
            .map(|s| find_stream(&game, s))
            .collect::<Result<_>>()?
    };
    let mut total = 0;
    let mut checker = create::ReferenceChecker::new(&game)?;
    for index in indices {
        let problems = checker.check(index)?;
        for p in &problems {
            out!(
                "{}#{} ({}): unresolved {:?}",
                p.stream,
                p.resource,
                type_tags().display(p.type_id),
                p.target
            );
        }
        total += problems.len();
    }
    out!("{total} unresolved reference(s)");
    Ok(())
}

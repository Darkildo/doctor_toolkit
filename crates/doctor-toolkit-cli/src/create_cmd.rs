//! Commands that create resources and streams: `add-resource`,
//! `clone-resource`, `add-stream`, `add-simgroup`, `remove-stream`,
//! `check-refs`, `world-grid`. What they do is in
//! [`doctor_toolkit::zone_ops`], shared with the editor.

use std::io::Write as _;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};

use doctor_toolkit::create;
use doctor_toolkit::game::GameInstance;
use doctor_toolkit::level::world_grid::{self, ZoneCell};
use doctor_toolkit::resource::names::type_tags;
use doctor_toolkit::resource::wgi;
use doctor_toolkit::workspace::Workspace;
use doctor_toolkit::zone_ops;

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
    /// With `--with-parent`: do not mark the stream as disabled by default
    /// (flags `0,0,1,0`), so the game opens it with every load of the
    /// parent and unloads it with the parent. With a world zone as the
    /// parent the new stream comes and goes as the player approaches and
    /// leaves, without `EnableDisableStreams` (stage 84)
    #[arg(long, requires = "with_parent")]
    auto_load: bool,
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

#[derive(Args)]
pub struct CheckEdits {
    game_dir: PathBuf,
    /// Only changes whose key contains this text
    #[arg(long)]
    filter: Option<String>,
    /// Mod project whose state is checked too
    #[arg(long, value_name = "DIR")]
    project: Option<PathBuf>,
    /// Workspace directory [default: <GAME_DIR>/_doctor_toolkit/workspace]
    #[arg(long, value_name = "DIR")]
    workspace: Option<PathBuf>,
    /// Print the report as JSON
    #[arg(long)]
    json: bool,
}

fn find_stream(game: &GameInstance, name: &str) -> Result<usize> {
    game.find_stream(name)
        .with_context(|| format!("stream `{name}` not found in {}", game.root().display()))
}

pub fn add_resource(args: AddResource) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let mut ws = workspace_cmd::open_for_edit(&game, &args.mode)?;
    // A new stream of the workspace is a stream like the others.
    let game = ws.stage(&game)?;
    let (index, _) = game.build_index(|_| {});
    let key = zone_ops::add_resource(
        &game,
        &index,
        &mut ws,
        &zone_ops::AddResourceParams {
            stream: args.stream.clone(),
            type_tag: args.type_tag.clone(),
            file: args.data.clone(),
            ids: args.ids.clone(),
            before: args.before,
            policy: args.policy.clone(),
            alignment: args.alignment,
            compress: args.compress,
        },
    )?;
    out!("added {key}");
    let stream = key.stream.clone();
    workspace_cmd::finish_edit(&game, &mut ws, &args.mode, vec![stream])
}

pub fn clone_resource(args: CloneResource) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let mut ws = workspace_cmd::open_for_edit(&game, &args.mode)?;
    let game = ws.stage(&game)?;
    let (index, _) = game.build_index(|_| {});
    let done = zone_ops::clone_resources(
        &game,
        &index,
        &mut ws,
        &zone_ops::CloneParams {
            stream: args.stream.clone(),
            indices: args.indices.clone(),
            to: args.to.clone(),
            seed: args.seed.clone(),
            keep_references: args.no_rewrite,
            retarget: args.retarget.clone(),
        },
    )?;
    for (from, to) in &done.clones {
        match from {
            Some(from) => out!("{from} -> {to}"),
            None => out!("{to}"),
        }
    }
    for (old, new) in &done.remap.names {
        out!("name hash {old:08x} -> {new:08x}");
    }
    for (key, changed) in &done.retargeted {
        out!("{key}: {changed} reference(s) retargeted");
    }
    let mut streams = vec![done.target.clone()];
    if !args.retarget.is_empty() {
        streams.push(done.source.clone());
    }
    streams.dedup();
    workspace_cmd::finish_edit(&game, &mut ws, &args.mode, streams)
}

#[derive(Args)]
pub struct AddSimGroup {
    game_dir: PathBuf,
    /// Stream the SimGroup goes into: a stream of the game or one made with `add-stream`
    stream: String,
    /// Copy the objects of this SimGroup, `STREAM#INDEX` (new GUIDs; links
    /// between the copies follow them) [default: an empty SimGroup]
    #[arg(long, value_name = "STREAM#INDEX")]
    copy_of: Option<String>,
    /// With `--copy-of`: only this object (repeatable) [default: every object]
    #[arg(long = "object", value_name = "INDEX", requires = "copy_of")]
    objects: Vec<usize>,
    #[command(flatten)]
    mode: EditMode,
}

pub fn add_simgroup(args: AddSimGroup) -> Result<()> {
    let (game, index) =
        crate::level_cmd::open_index_with(&args.game_dir, args.mode.workspace_dir.as_deref())?;
    let mut ws = workspace_cmd::open_for_edit(&game, &args.mode)?;
    let taken = crate::level_cmd::all_guids(&game, &index);
    let made = zone_ops::add_simgroup(
        &game,
        &index,
        &mut ws,
        &zone_ops::NewGroupParams {
            stream: args.stream.clone(),
            copy_of: args.copy_of.clone(),
            objects: (!args.objects.is_empty()).then(|| args.objects.clone()),
        },
        &|g| taken.contains(g),
    )?;
    out!(
        "new SimGroup {} with {} object(s){}",
        made.key,
        made.objects,
        if made.rewired > 0 {
            format!(", {} reference(s) between them rewired", made.rewired)
        } else {
            String::new()
        }
    );
    // Its index once the stream is staged again: what `level add` takes.
    let staged = ws.stage(&game)?;
    if let Some(stream) = staged.find_stream(&made.key.stream)
        && let Some(resource) = made.key.find_in(staged.open_stream(stream)?.layout())
        && ws.new_stream(&made.key.stream).is_some()
    {
        out!("resource {resource} of {}", made.key.stream);
    }
    let stream = made.key.stream.clone();
    workspace_cmd::finish_edit(&game, &mut ws, &args.mode, vec![stream])
}

pub fn add_stream(args: AddStream) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let mut ws = workspace_cmd::open_for_edit(&game, &args.mode)?;
    let stream = zone_ops::add_stream(
        &game,
        &mut ws,
        &zone_ops::NewStreamParams {
            name: args.name.clone(),
            parent: args.parent.clone(),
            load: match (args.with_parent, args.auto_load) {
                (true, true) => zone_ops::StreamLoad::AutoLoad,
                (true, false) => zone_ops::StreamLoad::WithParent,
                _ => zone_ops::StreamLoad::OnDemand,
            },
            tocs: args.tocs.clone(),
            guid: args.guid.clone(),
        },
    )?;
    let (name, guid_text, hash, tocs) = (stream.name, stream.guid, stream.hash, stream.tocs);
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
    // With the new streams of the workspace, so that a new zone is checked
    // before it is written into the game.
    let game = workspace_cmd::staged(GameInstance::open(&args.game_dir)?, None);
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
                "{}#{} ({}): {}",
                p.stream,
                p.resource,
                type_tags().display(p.type_id),
                p.what()
            );
        }
        total += problems.len();
    }
    out!("{total} unresolved reference(s)");
    Ok(())
}

/// `check-edits`: the check of the workspace's edits (`doctor_toolkit::validate`).
pub fn check_edits(args: CheckEdits) -> Result<()> {
    use doctor_toolkit::validate as v;
    let game = GameInstance::open(&args.game_dir)?;
    let dir = args
        .workspace
        .clone()
        .unwrap_or_else(|| Workspace::default_dir(game.root()));
    if !Workspace::exists(&dir) {
        if args.json {
            out!(
                "{}",
                serde_json::json!({ "ok": true, "note": "there is no workspace: nothing to check" })
            );
        } else {
            out!("there is no workspace: nothing to check");
        }
        return Ok(());
    }
    let ws = Workspace::open(&dir)?;
    let (index, _) = game.build_index(|_| {});
    let params = v::Params {
        filter: args.filter,
        project: args.project,
    };
    let report = v::validate(&game, &index, &ws, &params)?;
    if args.json {
        out!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        let line = |tag: &str, f: &v::Finding| {
            let place = f.place();
            if place.is_empty() {
                out!("{tag}: {}", f.what);
            } else {
                out!("{tag}: {place}: {}", f.what);
            }
        };
        for f in &report.errors {
            line("error", f);
        }
        for f in &report.warnings {
            line("warning", f);
        }
        for name in &report.new_messages {
            out!("new message: {name}");
        }
        out!(
            "{} change(s) checked ({} pending, {} applied): {} error(s), {} warning(s)",
            report.checked_changes,
            report.pending,
            report.applied,
            report.errors.len(),
            report.warnings.len()
        );
        if report.game_running {
            out!("the game is running: it does not see edits written now");
        }
    }
    if !report.ok {
        bail!("the edits have {} error(s)", report.errors.len());
    }
    Ok(())
}

#[derive(Subcommand)]
pub enum WorldGridCommand {
    /// List the world grids of a level: streams, cells and their boxes
    Info(WorldGridInfo),
    /// Add a stream to a world grid: the game loads it when the player
    /// comes near and unloads it when the player leaves (recorded in the
    /// workspace)
    ///
    /// The stream is one made with `add-stream` (or any stream with
    /// `--guid`). With the default `--lod high` the game loads it within
    /// about 180 m of the centre of the box.
    Add(WorldGridAdd),
    /// Take the cell of a stream out of the world grids of a level
    /// (recorded in the workspace)
    Remove(WorldGridRemove),
}

#[derive(Args)]
pub struct WorldGridInfo {
    game_dir: PathBuf,
    /// Root stream of the level, e.g. `gf2/queens`
    level: String,
    /// List every cell
    #[arg(long)]
    cells: bool,
    /// Print the grids as JSON
    #[arg(long)]
    json: bool,
    /// Workspace directory [default: <GAME_DIR>/_doctor_toolkit/workspace]
    #[arg(long, value_name = "DIR")]
    workspace_dir: Option<PathBuf>,
}

#[derive(Args)]
pub struct WorldGridAdd {
    game_dir: PathBuf,
    /// Root stream of the level, e.g. `gf2/queens`
    level: String,
    /// Stream to load by the position of the player, e.g. `gf2/doctor/island`
    stream: String,
    /// Lowest corner of the box of the zone, `X,Y,Z` in metres
    #[arg(long, value_name = "X,Y,Z", allow_hyphen_values = true)]
    min: String,
    /// Highest corner of the box
    #[arg(long, value_name = "X,Y,Z", allow_hyphen_values = true)]
    max: String,
    /// Grid to add it to: `high` (loaded within about 180 m), `medium`
    /// (450 m), `low` (1800 m), `map` (the grid of the Don's View map)
    #[arg(long, default_value = "high", value_parser = ["high", "medium", "low", "map"])]
    lod: String,
    /// Name `ZoneStreamEnable` objects find the cell by
    #[arg(long)]
    name: Option<String>,
    /// Leave the cell disabled until the player enters the box of a
    /// `ZoneStreamEnable` object with this `--name` (cell kind 5, as the
    /// game's interiors)
    #[arg(long, requires = "name")]
    on_enable: bool,
    /// GUID of the stream, 32 hexadecimal digits [default: the GUID of the
    /// stream made with `add-stream`]
    #[arg(long)]
    guid: Option<String>,
    #[command(flatten)]
    mode: EditMode,
}

#[derive(Args)]
pub struct WorldGridRemove {
    game_dir: PathBuf,
    level: String,
    /// Stream name, or its table-of-contents hash as 8 hexadecimal digits
    stream: String,
    #[command(flatten)]
    mode: EditMode,
}

fn parse_point(text: &str) -> Result<[f32; 3]> {
    let parts: Vec<f32> = text
        .split(',')
        .map(|p| p.trim().parse::<f32>())
        .collect::<std::result::Result<_, _>>()
        .with_context(|| format!("`{text}` is not X,Y,Z"))?;
    <[f32; 3]>::try_from(parts).map_err(|_| anyhow::anyhow!("`{text}` is not X,Y,Z"))
}

use world_grid::lod_name;

pub fn world_grid(command: WorldGridCommand) -> Result<()> {
    match command {
        WorldGridCommand::Info(args) => world_grid_info(args),
        WorldGridCommand::Add(args) => world_grid_add(args),
        WorldGridCommand::Remove(args) => world_grid_remove(args),
    }
}

fn world_grid_info(args: WorldGridInfo) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let ws = Workspace::for_game(&game, args.workspace_dir.as_deref())?;
    let grids = world_grid::level_grids(&game, &ws, &args.level)?;
    if grids.is_empty() {
        bail!("{} has no world grid (`wgi`)", args.level);
    }
    if args.json {
        let list: Vec<_> = grids
            .iter()
            .map(|g| {
                serde_json::json!({
                    "resource": g.resource,
                    "id": format!("{:08x}", g.id),
                    "grid": g.grid,
                })
            })
            .collect();
        out!("{}", serde_json::to_string_pretty(&list)?);
        return Ok(());
    }
    let names = zone_ops::stream_names(&game, &ws)?;
    let name_of = |hash: u32| {
        names
            .get(&hash)
            .cloned()
            .unwrap_or_else(|| format!("{hash:08x}"))
    };
    for g in &grids {
        let grid = &g.grid;
        out!(
            "#{} wgi {:08x}: {} cell(s), level of detail {}, {} stream(s) in the table, {} pool word(s)",
            g.resource,
            g.id,
            grid.cells.len(),
            grid.main_lod_mask().map_or("none".to_owned(), lod_name),
            grid.streams.len(),
            grid.pool.len()
        );
        if !args.cells {
            continue;
        }
        for (index, cell) in grid.cells.iter().enumerate() {
            let needs: Vec<String> = grid
                .list(&cell.needs)
                .iter()
                .map(|&s| name_of(grid.streams[s as usize].hash()))
                .collect();
            out!(
                "  {index:4} {}  kind {} flags {:#04x} lod {}  centre {:.1},{:.1},{:.1} r {:.1}{}{}",
                name_of(cell.stream_hash.0),
                cell.kind,
                cell.flags,
                lod_name(cell.lod_mask),
                cell.centre[0],
                cell.centre[1],
                cell.centre[2],
                cell.radius,
                if cell.name_hash.0 != 0 {
                    format!("  name {}", cell.name_hash)
                } else {
                    String::new()
                },
                if needs.is_empty() {
                    String::new()
                } else {
                    format!("  needs {}", needs.join(", "))
                }
            );
        }
    }
    Ok(())
}

fn world_grid_add(args: WorldGridAdd) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let mut ws = workspace_cmd::open_for_edit(&game, &args.mode)?;
    let lod_mask = match args.lod.as_str() {
        "high" => wgi::lod::HIGH,
        "medium" => wgi::lod::MEDIUM,
        "low" => wgi::lod::LOW,
        _ => wgi::lod::MAP,
    };
    let cell = ZoneCell {
        stream: args.stream.clone(),
        guid: args.guid.clone(),
        box_min: parse_point(&args.min)?,
        box_max: parse_point(&args.max)?,
        lod_mask,
        kind: if args.on_enable { 5 } else { wgi::KIND_WORLD },
        flags: 0,
        name: args.name.clone(),
    };
    let added = world_grid::add_zone_cell(&game, &mut ws, &args.level, &cell)?;
    out!(
        "cell {} of world grid #{} of {}: stream {} ({:08x}){}",
        added.cell,
        added.resource,
        args.level,
        args.stream,
        added.hash,
        added
            .needs
            .as_ref()
            .map_or(String::new(), |n| format!(", needs {n}"))
    );
    let level = doctor_toolkit::workspace::key::normalize_stream(&args.level);
    workspace_cmd::finish_edit(&game, &mut ws, &args.mode, vec![level])
}

fn world_grid_remove(args: WorldGridRemove) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let mut ws = workspace_cmd::open_for_edit(&game, &args.mode)?;
    let hash = zone_ops::remove_zone_cell(&game, &mut ws, &args.level, &args.stream)?;
    out!("removed the cell of {} ({hash:08x})", args.stream);
    let level = doctor_toolkit::workspace::key::normalize_stream(&args.level);
    workspace_cmd::finish_edit(&game, &mut ws, &args.mode, vec![level])
}

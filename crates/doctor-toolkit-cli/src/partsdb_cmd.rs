//! Parts databases (`partsdb`): show the tree, add parts and variants to
//! sets, and assemble a character into a glTF file.

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Args, Subcommand};

use doctor_toolkit::character::{Character, MeshLocator, parse_selection};
use doctor_toolkit::game::{GameAssets, GameInstance};
use doctor_toolkit::hash::sdbm;
use doctor_toolkit::outfit::{self, AllowVariant, NewPart};
use doctor_toolkit::resource::partsdb::PartsDb;

use crate::logic_cmd::export_data;
use crate::workspace_cmd::{self, EditMode};

macro_rules! out {
    ($($arg:tt)*) => {
        if writeln!(std::io::stdout(), $($arg)*).is_err() {
            std::process::exit(0);
        }
    };
}

const TYPE_PARTSDB: u32 = sdbm(b"partsdb");

/// Which database: stream and name (or resource index).
#[derive(Args, Clone)]
pub struct DbArgs {
    pub game_dir: PathBuf,
    /// Stream, e.g. `gf2/globalfolder/characters/allcharacters`
    pub stream: String,
    /// Database name (`player`, `wrk_male`) or resource index
    pub db: String,
}

#[derive(Subcommand)]
pub enum PartsdbCommand {
    /// List the databases of a stream
    List { game_dir: PathBuf, stream: String },
    /// Print a database as a tree: groups, sets, slots, variants, parts and meshes
    Show {
        #[command(flatten)]
        db: DbArgs,
        /// Only sets whose name contains this text
        #[arg(long)]
        set: Option<String>,
        /// Print the whole database as JSON (the form `logic-import` reads)
        #[arg(long)]
        json: bool,
        /// Workspace directory [default: <GAME_DIR>/_doctor_toolkit/workspace]
        #[arg(long, value_name = "DIR")]
        workspace_dir: Option<PathBuf>,
    },
    /// Add a part (one model in its levels of detail) and a variant with it,
    /// and let sets use the variant
    AddPart {
        #[command(flatten)]
        db: DbArgs,
        /// Name of the new part
        #[arg(long)]
        name: String,
        /// Slot of the part, e.g. `upper_body`, `a_hat`
        #[arg(long)]
        slot: String,
        /// Mesh of each level of detail: resource id (hexadecimal) or model
        /// name in the stream; repeat for `_lod1`, `_lod2`
        #[arg(long = "mesh", required = true)]
        meshes: Vec<String>,
        /// Culling tag of the part, e.g. `hatCulled` (repeatable)
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// Name of the new variant [default: the part name]
        #[arg(long)]
        variant: Option<String>,
        /// Sets that may use the variant (name, repeatable)
        #[arg(long = "set")]
        sets: Vec<String>,
        /// Every set of the database may use the variant
        #[arg(long, conflicts_with = "sets")]
        all_sets: bool,
        /// Make the variant the only choice of its slot in those sets
        #[arg(long)]
        only: bool,
        #[command(flatten)]
        mode: EditMode,
    },
    /// Let sets use an existing variant in a slot
    Allow {
        #[command(flatten)]
        db: DbArgs,
        /// Variant name
        variant: String,
        /// Slot [default: the slot of the variant's first part]
        #[arg(long)]
        slot: Option<String>,
        /// Sets (name, repeatable); none = every set
        #[arg(long = "set")]
        sets: Vec<String>,
        /// Make the variant the only choice of the slot
        #[arg(long)]
        only: bool,
        #[command(flatten)]
        mode: EditMode,
    },
    /// Assemble a set into one character and write it as binary glTF
    Character {
        #[command(flatten)]
        db: DbArgs,
        /// Set name or index
        set: String,
        /// Choices: `SLOT=VARIANT`, `PRESET=ITEM` (names or indices), `tag=TAG`
        #[arg(long = "choose", value_name = "KEY=VALUE")]
        choices: Vec<String>,
        /// Level of detail (characters) or damage state (vehicles), from 0
        #[arg(long, default_value_t = 0)]
        lod: usize,
        /// Output .glb file
        #[arg(short, long)]
        output: PathBuf,
        /// Workspace directory [default: <GAME_DIR>/_doctor_toolkit/workspace]
        #[arg(long, value_name = "DIR")]
        workspace_dir: Option<PathBuf>,
    },
}

impl PartsdbCommand {
    pub fn may_write_game(&self) -> bool {
        match self {
            Self::AddPart { mode, .. } | Self::Allow { mode, .. } => mode.direct,
            _ => false,
        }
    }
}

pub fn run(command: PartsdbCommand) -> Result<()> {
    match command {
        PartsdbCommand::List { game_dir, stream } => list(&game_dir, &stream),
        PartsdbCommand::Show {
            db,
            set,
            json,
            workspace_dir,
        } => show(&db, workspace_dir.as_deref(), set.as_deref(), json),
        PartsdbCommand::AddPart {
            db,
            name,
            slot,
            meshes,
            tags,
            variant,
            sets,
            all_sets,
            only,
            mode,
        } => add_part(
            &db,
            NewPart {
                name,
                slot,
                meshes,
                tags,
                variant,
                // `--all-sets` and no `--set` both mean every set.
                sets: if all_sets { Vec::new() } else { sets },
                only,
            },
            &mode,
        ),
        PartsdbCommand::Allow {
            db,
            variant,
            slot,
            sets,
            only,
            mode,
        } => allow(
            &db,
            &AllowVariant {
                variant,
                slot,
                sets,
                only,
            },
            &mode,
        ),
        PartsdbCommand::Character {
            db,
            set,
            choices,
            lod,
            output,
            workspace_dir,
        } => character(&db, workspace_dir.as_deref(), &set, &choices, lod, &output),
    }
}

struct Opened {
    game: GameInstance,
    stream: usize,
    resource: usize,
    db: PartsDb,
}

fn open(args: &DbArgs, workspace_dir: Option<&std::path::Path>) -> Result<Opened> {
    let game = GameInstance::open(&args.game_dir)?;
    let stream = game
        .find_stream(&args.stream)
        .with_context(|| format!("stream `{}` not found", args.stream))?;
    let file = game.open_stream(stream)?;
    let ws = doctor_toolkit::workspace::Workspace::for_game(&game, workspace_dir).ok();
    let mut found = None;
    for (index, r) in file.layout().resources.iter().enumerate() {
        if r.header.type_id != TYPE_PARTSDB {
            continue;
        }
        let data = export_data(ws.as_ref(), &args.stream, &file, index)?;
        let db = PartsDb::parse(&data)?;
        if args.db.parse::<usize>() == Ok(index) || db.name.eq_ignore_ascii_case(&args.db) {
            found = Some((index, db));
            break;
        }
    }
    let (resource, db) =
        found.with_context(|| format!("no parts database `{}` in {}", args.db, args.stream))?;
    Ok(Opened {
        game,
        stream,
        resource,
        db,
    })
}

fn list(game_dir: &std::path::Path, stream_name: &str) -> Result<()> {
    let game = GameInstance::open(game_dir)?;
    let stream = game
        .find_stream(stream_name)
        .with_context(|| format!("stream `{stream_name}` not found"))?;
    let file = game.open_stream(stream)?;
    for (index, r) in file.layout().resources.iter().enumerate() {
        if r.header.type_id == TYPE_PARTSDB {
            let db = PartsDb::parse(&file.resource_data(index)?)?;
            out!(
                "{index:6}  {:<28} {:4} parts {:4} variants {:4} sets {:3} slots",
                db.name,
                db.parts.len(),
                db.variants.len(),
                db.sets.len(),
                db.slots.len()
            );
        }
    }
    Ok(())
}

fn show(
    args: &DbArgs,
    workspace_dir: Option<&std::path::Path>,
    filter: Option<&str>,
    json: bool,
) -> Result<()> {
    let o = open(args, workspace_dir)?;
    let db = &o.db;
    if json {
        out!("{}", serde_json::to_string_pretty(db)?);
        return Ok(());
    }
    let names = outfit::mesh_names(&o.game.open_stream(o.stream)?);
    let kind = match db.kind {
        2 => "characters",
        4 => "vehicle",
        _ => "?",
    };
    out!(
        "{} ({kind}): {} slots, {} parts, {} variants, {} presets, {} blenders, {} sets, {} groups",
        db.name,
        db.slots.len(),
        db.parts.len(),
        db.variants.len(),
        db.presets.len(),
        db.blenders.len(),
        db.sets.len(),
        db.groups.len()
    );
    for g in &db.groups {
        let sets: Vec<&str> = g.sets.iter().map(|&s| db.sets[s].name.as_str()).collect();
        out!("group {}: {}", g.name, sets.join(", "));
    }
    for set in &db.sets {
        if filter.is_some_and(|f| !set.name.contains(f)) {
            continue;
        }
        out!("set {}", set.name);
        for c in &set.choices {
            out!("  {}", db.slots[c.slot]);
            for &v in &c.variants {
                let variant = &db.variants[v];
                out!("    {}", variant.name);
                for &p in &variant.parts {
                    let part = &db.parts[p as usize];
                    let meshes: Vec<String> = part
                        .mesh_ids()
                        .map(|id| match names.get(&id) {
                            Some(n) => format!("{id:08x} {n}"),
                            None => format!("{id:08x} (not in this stream)"),
                        })
                        .collect();
                    let tags = if part.tags.is_empty() {
                        String::new()
                    } else {
                        format!(" [{}]", part.tags.join(", "))
                    };
                    out!("      {}{tags}: {}", part.name, meshes.join("; "));
                }
            }
        }
        for p in &set.presets {
            let preset = &db.presets[p.preset];
            let items: Vec<String> = p
                .items
                .iter()
                .map(|&i| {
                    let item = &preset.items[i];
                    format!("{} ({} swaps)", item.name, item.swaps.len())
                })
                .collect();
            out!("  preset {}: {}", preset.name, items.join(", "));
        }
        for m in &set.materials {
            for c in &m.materials {
                out!(
                    "  part {} material {}: {} candidates",
                    db.parts[m.part].name,
                    c.material,
                    c.candidates.len()
                );
            }
        }
    }
    Ok(())
}

fn add_part(args: &DbArgs, add: NewPart, mode: &EditMode) -> Result<()> {
    let o = open(args, mode.workspace_dir.as_deref())?;
    let mut ws = workspace_cmd::open_for_edit(&o.game, mode)?;
    let done = outfit::record_part(&o.game, &mut ws, (o.stream, o.resource), &add)?;
    for id in &done.meshes_not_in_stream {
        out!("note: mesh {id} is not in the stream's game files (new in the workspace?)");
    }
    out!(
        "part {} `{}` in slot {}, variant {} `{}`, {} set(s) changed",
        done.part.unwrap_or_default(),
        add.name,
        done.slot,
        done.variant,
        done.variant_name,
        done.sets_changed
    );
    workspace_cmd::finish_edit(&o.game, &mut ws, mode, vec![args.stream.clone()])
}

fn allow(args: &DbArgs, params: &AllowVariant, mode: &EditMode) -> Result<()> {
    let o = open(args, mode.workspace_dir.as_deref())?;
    let mut ws = workspace_cmd::open_for_edit(&o.game, mode)?;
    let done = outfit::record_allow(&o.game, &mut ws, (o.stream, o.resource), params)?;
    if done.sets_changed == 0 {
        out!("no change");
        return Ok(());
    }
    out!("{} set(s) changed", done.sets_changed);
    workspace_cmd::finish_edit(&o.game, &mut ws, mode, vec![args.stream.clone()])
}

fn character(
    args: &DbArgs,
    workspace_dir: Option<&std::path::Path>,
    set: &str,
    choices: &[String],
    lod: usize,
    output: &std::path::Path,
) -> Result<()> {
    let o = open(args, workspace_dir)?;
    let set =
        o.db.set_index(set)
            .or_else(|| set.parse().ok().filter(|&i: &usize| i < o.db.sets.len()))
            .with_context(|| format!("no set `{set}`"))?;
    let selection = parse_selection(&o.db, set, choices)?;
    let game = Arc::new(o.game);
    let (index, _) = game.build_index(|_| {});
    let meshes = MeshLocator::new(&index);
    let character = Character::load(&game, &meshes, o.stream, &o.db, set, &selection, lod)?;
    for p in &character.parts {
        let swaps = if p.materials.is_empty() {
            String::new()
        } else {
            format!(", {} material(s) swapped", p.materials.len())
        };
        out!(
            "  {} -> {} ({:08x}){swaps}",
            p.name,
            p.model.name,
            p.mesh_id
        );
    }
    for &p in &character.assembly.culled {
        out!("  {} hidden by a culling tag", o.db.parts[p].name);
    }
    for (name, id) in &character.missing {
        out!("  {name}: mesh {id:08x} not found");
    }
    for (name, w) in &character.assembly.morphs {
        if *w != 0.0 {
            out!("  morph {name} = {w}");
        }
    }
    let mut assets = GameAssets::new(game.clone(), &index);
    let glb = character.to_glb(&mut assets)?;
    std::fs::write(output, &glb).with_context(|| format!("cannot write {}", output.display()))?;
    out!(
        "{}: {} part(s), {} bytes",
        output.display(),
        character.parts.len(),
        glb.len()
    );
    Ok(())
}

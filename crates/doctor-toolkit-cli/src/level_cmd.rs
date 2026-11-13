//! Level commands: zones, coverage of the 3D view, and object edits
//! (duplicate, remove, add with an existing or cloned model).

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};

use doctor_toolkit::game::{GameInstance, ResourceIndex};
use doctor_toolkit::import::collision;
use doctor_toolkit::level::edit::{self, ModelChoice};
use doctor_toolkit::level::{
    Catalog, Coverage, Level, LevelSource, Location, Lookup, Overrides, Scope,
};
use doctor_toolkit::resource::hkx::collision::CollisionKind;
use doctor_toolkit::resource::simgroup::Guid;

use crate::workspace_cmd::{self, EditMode};

macro_rules! out {
    ($($arg:tt)*) => {
        if writeln!(std::io::stdout(), $($arg)*).is_err() {
            std::process::exit(0);
        }
    };
}

#[derive(Subcommand)]
pub enum LevelCommand {
    /// List the zones (streams with SimGroups) under a path, e.g. `gf2/miami/local/bonita`
    List {
        game_dir: PathBuf,
        /// Only zones whose name starts with this path
        prefix: Option<String>,
    },
    /// What the 3D level view can show: objects, models, meshes, scope
    ///
    /// Each argument is a zone or a directory of zones; they are loaded
    /// together as one level. `--each` reports every zone separately,
    /// `--levels` every level (`gf2/miami`, `mp_mall/mp_mall`, …) as a whole.
    Info {
        game_dir: PathBuf,
        zones: Vec<String>,
        /// Report each zone separately
        #[arg(long)]
        each: bool,
        /// Report every level of the game, all its zones loaded together
        #[arg(long, conflicts_with_all = ["zones", "each"])]
        levels: bool,
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// List the placed objects of zones with their models and meshes
    Objects {
        game_dir: PathBuf,
        zones: Vec<String>,
        /// Only objects whose class or name contains this text
        #[arg(long)]
        filter: Option<String>,
        /// Only objects within `--radius` of this point `X,Y,Z`, nearest first
        #[arg(long, value_name = "X,Y,Z", allow_hyphen_values = true)]
        near: Option<String>,
        /// Radius for `--near` in metres
        #[arg(long, default_value_t = 30.0)]
        radius: f32,
    },
    /// Duplicate an object of a SimGroup (the copy shares its attribute packets)
    Duplicate {
        #[command(flatten)]
        object: ObjectArgs,
        /// Move the copy by `X,Y,Z`
        #[arg(long, value_name = "X,Y,Z", allow_hyphen_values = true)]
        offset: Option<String>,
        #[command(flatten)]
        mode: EditMode,
    },
    /// Remove an object of a SimGroup
    Remove {
        #[command(flatten)]
        object: ObjectArgs,
        #[command(flatten)]
        mode: EditMode,
    },
    /// Add an object: a copy of a template object with another model and position
    ///
    /// The model is an existing metamodel (`--model`, it must be in the
    /// SimGroup's stream or an ancestor) or a clone of one made in the
    /// SimGroup's stream (`--clone-model STREAM:INDEX`, with the meshes of
    /// its own stream). New objects have the collision of their model; with
    /// `--collision KIND` the clone gets new collision of its own.
    Add {
        #[command(flatten)]
        object: ObjectArgs,
        /// Stream of the template object when it is in another SimGroup
        #[arg(long, value_name = "STREAM", requires = "template_resource")]
        template_stream: Option<String>,
        #[arg(long, value_name = "INDEX")]
        template_resource: Option<usize>,
        /// Show this metamodel (hexadecimal id)
        #[arg(long, value_name = "ID", conflicts_with = "clone_model")]
        model: Option<String>,
        /// Show a clone of the metamodel `STREAM:INDEX`
        #[arg(long, value_name = "STREAM:INDEX")]
        clone_model: Option<String>,
        /// Give the clone new collision built from its meshes: `mesh`,
        /// `hull` or `box`
        #[arg(long, value_name = "KIND", requires = "clone_model")]
        collision: Option<CollisionKind>,
        /// Build that collision from this glTF file instead of the clone's meshes
        #[arg(long, value_name = "FILE", requires = "collision")]
        collision_from: Option<PathBuf>,
        /// Position `X,Y,Z` (Y is up)
        #[arg(long, value_name = "X,Y,Z", allow_hyphen_values = true)]
        position: Option<String>,
        /// Add the object even if its class cannot show the model (the game
        /// may crash, see docs/levels.md)
        #[arg(long)]
        force: bool,
        #[command(flatten)]
        mode: EditMode,
    },
}

/// An object of a SimGroup resource.
#[derive(Args)]
pub struct ObjectArgs {
    pub game_dir: PathBuf,
    /// Stream name, e.g. `gf2/miami/local/bonita/high/terrain06`
    pub stream: String,
    /// Index of the SimGroup resource in the stream
    pub resource: usize,
    /// Index of the object (for `add`: the template object)
    pub object: usize,
}

pub fn run(command: LevelCommand) -> Result<()> {
    match command {
        LevelCommand::List { game_dir, prefix } => list(&game_dir, prefix.as_deref()),
        LevelCommand::Info {
            game_dir,
            zones,
            each,
            levels,
            json,
        } => info(&game_dir, &zones, each, levels, json),
        LevelCommand::Objects {
            game_dir,
            zones,
            filter,
            near,
            radius,
        } => {
            let near = near.as_deref().map(parse_floats).transpose()?;
            objects(
                &game_dir,
                &zones,
                filter.as_deref(),
                near.map(|p| (p, radius)),
            )
        }
        LevelCommand::Duplicate {
            object,
            offset,
            mode,
        } => {
            let offset = offset.as_deref().map(parse_floats).transpose()?;
            let (game, index, group) = open_group(&object)?;
            let mut ws = workspace_cmd::open_for_edit(&game, &mode)?;
            let taken = all_guids(&game, &index);
            let (copy, guid) = edit::duplicate(
                &game,
                &mut ws,
                group,
                object.object,
                offset.unwrap_or_default(),
                &|g: &Guid| taken.contains(g),
            )?;
            out!("object {} duplicated as {copy} ({guid})", object.object);
            workspace_cmd::finish_edit(&game, &mut ws, &mode, vec![object.stream.clone()])
        }
        LevelCommand::Remove { object, mode } => {
            let (game, _, group) = open_group(&object)?;
            let mut ws = workspace_cmd::open_for_edit(&game, &mode)?;
            let dangling = edit::remove(&game, &mut ws, group, object.object)?;
            out!("object {} removed", object.object);
            if dangling > 0 {
                out!("warning: {dangling} attribute(s) of other objects still refer to it");
            }
            workspace_cmd::finish_edit(&game, &mut ws, &mode, vec![object.stream.clone()])
        }
        LevelCommand::Add {
            object,
            template_stream,
            template_resource,
            model,
            clone_model,
            collision,
            collision_from,
            position,
            force,
            mode,
        } => {
            let (game, index, group) = open_group(&object)?;
            let template_group = match (template_stream, template_resource) {
                (Some(s), Some(r)) => Location {
                    stream: find_stream(&game, &s)?,
                    resource: r,
                },
                _ => group,
            };
            let model = match (model, clone_model) {
                (Some(id), _) => ModelChoice::Existing(
                    u32::from_str_radix(id.trim_start_matches("0x"), 16)
                        .with_context(|| format!("`{id}` is not a hexadecimal id"))?,
                ),
                (None, Some(at)) => {
                    let (s, r) = at
                        .rsplit_once(':')
                        .with_context(|| format!("`{at}`: STREAM:INDEX expected"))?;
                    let at = Location {
                        stream: find_stream(&game, s)?,
                        resource: r
                            .parse()
                            .with_context(|| format!("`{r}` is not an index"))?,
                    };
                    match collision {
                        Some(kind) => {
                            let mesh = match &collision_from {
                                Some(path) => {
                                    let model = doctor_toolkit::scene::read_file(path)
                                        .with_context(|| {
                                            format!("cannot read {}", path.display())
                                        })?;
                                    Some(std::sync::Arc::new(collision::mesh_from_gltf(&model)))
                                }
                                None => None,
                            };
                            ModelChoice::CloneWithCollision(at, kind, mesh)
                        }
                        None => ModelChoice::Clone(at),
                    }
                }
                (None, None) => ModelChoice::Template,
            };
            let position = position.as_deref().map(parse_floats).transpose()?;
            let mut ws = workspace_cmd::open_for_edit(&game, &mode)?;
            let taken = all_guids(&game, &index);
            let added = edit::add(
                &game,
                &index,
                &mut ws,
                group,
                (template_group, object.object),
                model,
                position,
                &|g: &Guid| taken.contains(g),
                !force,
            )?;
            for key in &added.cloned {
                out!("cloned {key}");
            }
            out!(
                "added object {} ({}){}",
                added.object,
                added.guid,
                added
                    .model
                    .map_or_else(String::new, |m| format!(" showing metamodel {m:08x}"))
            );
            if collision.is_none() {
                out!("note: the new object has the collision of its model (see docs/levels.md)");
            }
            workspace_cmd::finish_edit(&game, &mut ws, &mode, vec![object.stream.clone()])
        }
    }
}

fn find_stream(game: &GameInstance, name: &str) -> Result<usize> {
    game.find_stream(name)
        .with_context(|| format!("stream `{name}` not found"))
}

fn parse_floats(text: &str) -> Result<[f32; 3]> {
    let parts: Vec<&str> = text.split(',').map(str::trim).collect();
    if parts.len() != 3 {
        bail!("`{text}`: X,Y,Z expected");
    }
    let mut out = [0.0; 3];
    for (o, p) in out.iter_mut().zip(parts) {
        *o = p
            .parse()
            .with_context(|| format!("`{p}` is not a number"))?;
    }
    Ok(out)
}

fn open_index(game_dir: &std::path::Path) -> Result<(GameInstance, ResourceIndex)> {
    let game = GameInstance::open(game_dir)?;
    let (index, errors) = game.build_index(|_| {});
    if let Some((stream, error)) = errors.first() {
        eprintln!(
            "warning: {} unreadable stream(s), first {}: {error}",
            errors.len(),
            game.streams()[*stream].rel_path
        );
    }
    Ok((game, index))
}

fn open_group(args: &ObjectArgs) -> Result<(GameInstance, ResourceIndex, Location)> {
    let (game, index) = open_index(&args.game_dir)?;
    let group = Location {
        stream: find_stream(&game, &args.stream)?,
        resource: args.resource,
    };
    Ok((game, index, group))
}

/// GUIDs of every SimGroup object of the game, so that new GUIDs are unique.
fn all_guids(game: &GameInstance, index: &ResourceIndex) -> std::collections::HashSet<Guid> {
    use rayon::prelude::*;
    let simgroup = doctor_toolkit::level::TYPE_SIMGROUP;
    let mut by_stream: std::collections::HashMap<usize, Vec<usize>> = Default::default();
    for e in index
        .entries
        .iter()
        .filter(|e| e.header.type_id == simgroup)
    {
        by_stream
            .entry(e.stream as usize)
            .or_default()
            .push(e.resource as usize);
    }
    by_stream
        .par_iter()
        .flat_map_iter(|(&s, resources)| {
            let stream = game.open_stream(s).ok();
            resources
                .iter()
                .filter_map(|&r| stream.as_ref()?.resource_data(r).ok())
                .filter_map(|d| doctor_toolkit::resource::simgroup::SimGroup::parse(&d).ok())
                .flat_map(|g| g.objects.into_iter().map(|o| o.guid))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn list(game_dir: &std::path::Path, prefix: Option<&str>) -> Result<()> {
    let (game, index) = open_index(game_dir)?;
    let catalog = Catalog::new(&game, &index);
    let zones: Vec<_> = match prefix {
        Some(p) => catalog.under(p.trim_end_matches('/')).collect(),
        None => catalog.zones.iter().collect(),
    };
    out!("{:>9} {:>6} {:>6}  zone", "size", "meshes", "models");
    for z in &zones {
        out!(
            "{:>9} {:>6} {:>6}  {}",
            format!("{:.1}M", z.bytes as f64 / 1048576.0),
            z.meshes,
            z.metamodels,
            z.name
        );
    }
    out!("{} zone(s)", zones.len());
    Ok(())
}

fn info(
    game_dir: &std::path::Path,
    zones: &[String],
    each: bool,
    levels: bool,
    json: bool,
) -> Result<()> {
    let started = Instant::now();
    let (game, index) = open_index(game_dir)?;
    let game = Arc::new(game);
    let catalog = Catalog::new(&game, &index);
    let lookup = Arc::new(Lookup::new(&index));
    let parents = game.stream_parents()?;
    let indexed = started.elapsed();
    let mut sets: Vec<(String, Vec<usize>)> = Vec::new();
    if levels {
        for level in catalog.levels() {
            sets.push((
                level.to_owned(),
                catalog.under(level).map(|z| z.stream).collect(),
            ));
        }
    } else {
        if zones.is_empty() {
            bail!("name a zone or a directory of zones, or pass --levels");
        }
        let mut all = Vec::new();
        for z in zones {
            let found: Vec<_> = catalog.under(z.trim_end_matches('/')).collect();
            if found.is_empty() {
                bail!("no zone at `{z}` (see `level list`)");
            }
            if each {
                for zone in found {
                    sets.push((zone.name.clone(), vec![zone.stream]));
                }
            } else {
                all.extend(found.iter().map(|z| z.stream));
            }
        }
        if !each {
            sets.push((zones.join(" + "), all));
        }
    }
    let mut total = Coverage::default();
    let mut rows = Vec::new();
    for (name, streams) in sets {
        let t = Instant::now();
        let scope = Scope::new(&game, &parents, &streams);
        let source = Arc::new(LevelSource::new(
            game.clone(),
            lookup.clone(),
            Overrides::default(),
            scope,
        ));
        let level = Level::load(source, &streams)?;
        let c = level.coverage();
        let elapsed = t.elapsed();
        add(&mut total, &c);
        if json {
            rows.push(
                serde_json::json!({ "level": name, "coverage": c, "load_ms": elapsed.as_millis() }),
            );
        } else {
            print_coverage(&name, &c, elapsed);
        }
    }
    if json {
        out!("{}", serde_json::to_string_pretty(&rows)?);
    } else if levels || each {
        print_coverage("total", &total, started.elapsed());
    }
    eprintln!(
        "index {:.1} s, total {:.1} s",
        indexed.as_secs_f64(),
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

fn add(total: &mut Coverage, c: &Coverage) {
    total.streams += c.streams;
    total.simgroups += c.simgroups;
    total.objects += c.objects;
    total.placed += c.placed;
    total.with_model += c.with_model;
    total.rendered += c.rendered;
    total.rendered_placed += c.rendered_placed;
    total.model_missing += c.model_missing;
    total.model_out_of_scope += c.model_out_of_scope;
    total.model_without_mesh += c.model_without_mesh;
    total.models += c.models;
    total.meshes += c.meshes;
    total.meshes_missing += c.meshes_missing;
    total.meshes_out_of_scope += c.meshes_out_of_scope;
    total.instances += c.instances;
    total.trees += c.trees;
    total.points += c.points;
}

fn print_coverage(name: &str, c: &Coverage, elapsed: std::time::Duration) {
    let pct = |a: usize, b: usize| {
        if b == 0 {
            0.0
        } else {
            100.0 * a as f64 / b as f64
        }
    };
    out!("{name}");
    out!(
        "  {} zone(s), {} in scope, {} SimGroup(s), loaded in {} ms",
        c.streams,
        c.scope_streams,
        c.simgroups,
        elapsed.as_millis()
    );
    out!(
        "  objects {}: placed {}, with a model {}, drawn {} ({:.1} % of models)",
        c.objects,
        c.placed,
        c.with_model,
        c.rendered,
        pct(c.rendered, c.with_model)
    );
    out!(
        "  models {}: missing {}, out of scope {}, without a mesh {}",
        c.models,
        c.model_missing,
        c.model_out_of_scope,
        c.model_without_mesh
    );
    out!(
        "  meshes {}: missing {}, out of scope {}; instances {}, trees {}, points {}",
        c.meshes,
        c.meshes_missing,
        c.meshes_out_of_scope,
        c.instances,
        c.trees,
        c.points
    );
}

fn objects(
    game_dir: &std::path::Path,
    zones: &[String],
    filter: Option<&str>,
    near: Option<([f32; 3], f32)>,
) -> Result<()> {
    let (game, index) = open_index(game_dir)?;
    let game = Arc::new(game);
    let catalog = Catalog::new(&game, &index);
    let mut streams = Vec::new();
    for z in zones {
        let found: Vec<usize> = catalog
            .under(z.trim_end_matches('/'))
            .map(|z| z.stream)
            .collect();
        if found.is_empty() {
            bail!("no zone at `{z}` (see `level list`)");
        }
        streams.extend(found);
    }
    let parents = game.stream_parents()?;
    let scope = Scope::new(&game, &parents, &streams);
    let source = Arc::new(LevelSource::new(
        game.clone(),
        Arc::new(Lookup::new(&index)),
        Overrides::default(),
        scope,
    ));
    let level = Level::load(source.clone(), &streams)?;
    let filter = filter.map(str::to_lowercase);
    let mut names: std::collections::HashMap<u32, String> = Default::default();
    let mut listed: Vec<(usize, &doctor_toolkit::level::LevelObject, f32)> = Vec::new();
    for (g, group) in level.groups.iter().enumerate() {
        for o in &group.objects {
            let distance = match (near, o.transform) {
                (Some((p, _)), Some(m)) if o.has_matrix => {
                    let d: f32 = (0..3).map(|i| (m[3][i] - p[i]).powi(2)).sum();
                    d.sqrt()
                }
                (Some(_), _) => continue,
                (None, _) => 0.0,
            };
            if near.is_some_and(|(_, r)| distance > r) {
                continue;
            }
            listed.push((g, o, distance));
        }
    }
    if near.is_some() {
        listed.sort_by(|a, b| a.2.total_cmp(&b.2));
    }
    for (g, o, distance) in listed {
        let group = &level.groups[g];
        let stream = game.streams()[group.location.stream].stream_name();
        {
            let class = o.class_name.unwrap_or("?");
            if let Some(f) = &filter
                && !class.to_lowercase().contains(f)
                && !o.name.to_lowercase().contains(f)
            {
                continue;
            }
            let at = match o.transform {
                Some(m) if o.has_matrix => format!("{:.1} {:.1} {:.1}", m[3][0], m[3][1], m[3][2]),
                Some(_) => "origin".to_owned(),
                None => "-".to_owned(),
            };
            let mut line = format!(
                "{g}:{} {stream} #{} {class} {:?} at {at}",
                o.index, group.location.resource, o.name
            );
            if near.is_some() {
                line += &format!(" ({distance:.1} m)");
            }
            if let Some(id) = o.model {
                let m = &level.models[&id];
                let meshes: Vec<String> = m
                    .desc
                    .pieces
                    .iter()
                    .map(|p| {
                        let name = names
                            .entry(p.mesh)
                            .or_insert_with(|| {
                                source
                                    .mesh(p.mesh)
                                    .map(|m| m.name)
                                    .unwrap_or_else(|_| format!("{:08x} (not found)", p.mesh))
                            })
                            .clone();
                        match p.lod {
                            Some([a, b]) => format!("{name} [{a}..{b})"),
                            None => name,
                        }
                    })
                    .collect();
                line += &format!(" model {id:08x}: {}", meshes.join(", "));
                if !m.desc.trees.is_empty() {
                    line += &format!(", {} tree(s)", m.desc.trees.len());
                }
            }
            out!("{line}");
        }
    }
    Ok(())
}

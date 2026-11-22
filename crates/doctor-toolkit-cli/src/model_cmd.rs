//! `model-import`: a model file (glTF, FBX, OBJ, USD) into the game;
//! `model-info`: what a model file holds.

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::Args;

use doctor_toolkit::game::{GameAssets, GameInstance};
use doctor_toolkit::import::model;
use doctor_toolkit::resource::hkx::collision::CollisionKind;
use doctor_toolkit::resource::logic::TYPE_METAMODEL;
use doctor_toolkit::resource::mesh::{self, TYPE_MESH};
use doctor_toolkit::scene;

use crate::workspace_cmd::{self, EditMode};

macro_rules! out {
    ($($arg:tt)*) => {
        if writeln!(std::io::stdout(), $($arg)*).is_err() {
            std::process::exit(0);
        }
    };
}

#[derive(Args)]
pub struct ModelImport {
    game_dir: PathBuf,
    /// Stream that holds the model, e.g. `gf2/miami/local/danny/high/building_004`
    stream: String,
    /// The model to replace: a resource index (mesh or metamodel), an id
    /// (`mesh:aa5c8185`, `metamodel:c06e926c`; it stays the same when the
    /// stream gets new resources, the index does not) or a mesh model name
    model: String,
    /// The model to import: .glb, .gltf, .fbx, .obj, .usd, .usda, .usdc or .usdz
    file: PathBuf,
    /// Create a new model set with this name (clones of the metamodel and
    /// meshes under new ids) instead of replacing the model in place
    #[arg(long, value_name = "NAME")]
    new: Option<String>,
    /// Point this resource of the stream (SimGroup, metamodel) at the new
    /// set (repeatable; needs --new)
    #[arg(long = "retarget", value_name = "INDEX", requires = "new")]
    retarget: Vec<usize>,
    /// With --new: put the new set into this stream (a stream of the game
    /// or one made with `add-stream`) instead of the model's own
    #[arg(long, value_name = "STREAM", requires = "new")]
    to: Option<String>,
    /// Vertex format of every draw record (0–12) instead of the one the
    /// replaced model uses for the same kind of part
    #[arg(long)]
    format: Option<u8>,
    /// Also build new collision from the imported geometry and point the
    /// metamodel at it: `mesh` (triangles with MOPP), `hull` or `box`
    #[arg(long, value_name = "KIND")]
    collision: Option<CollisionKind>,
    /// Replace the model only in this stream. By default every stream with
    /// a copy of a replaced mesh (the same ids) gets the new one too: the
    /// game takes the mesh from whichever of them it loaded
    #[arg(long, conflicts_with = "new")]
    this_stream_only: bool,
    /// Print what would be done without recording anything
    #[arg(long)]
    dry_run: bool,
    /// Print the summary as JSON
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    mode: EditMode,
}

pub fn model_import(args: ModelImport) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let mut ws = workspace_cmd::open_for_edit(&game, &args.mode)?;
    // The model or the target stream may be new in the workspace.
    let game = Arc::new(ws.stage(&game)?);
    let stream = game
        .find_stream(&args.stream)
        .with_context(|| format!("stream `{}` not found", args.stream))?;
    let resource = find_model(&game, stream, &args.model)?;
    let gltf = scene::read_file(&args.file)
        .with_context(|| format!("cannot read {}", args.file.display()))?;
    let (index, _) = game.build_index(|_| {});
    let mut assets = GameAssets::new(game.clone(), &index);
    let mut plan = model::plan(
        &game,
        &ws,
        &index,
        &mut assets,
        stream,
        resource,
        &gltf,
        &model::ImportOptions {
            new_name: args.new.clone(),
            retarget: args.retarget.clone(),
            format: args.format,
            collision: args.collision,
            this_stream_only: args.this_stream_only,
            keep_copies: Vec::new(),
        },
    )?;
    if let Some(to) = &args.to {
        let to = doctor_toolkit::zone_ops::any_stream(&game, &ws, to)?;
        plan.put_into(&game, &ws, &to)?;
    }
    let s = &plan.summary;
    if args.json {
        out!("{}", serde_json::to_string_pretty(s)?);
    } else {
        out!("{} ({} mode)", plan.label, s.mode);
        for p in &s.parts {
            out!(
                "  part {}: {} vertices, {} triangles, {} records, material {}",
                p.name,
                p.vertices,
                p.triangles,
                p.kind,
                p.material
            );
        }
        out!("  draw records: {}", s.records);
        if let Some(skeleton) = &s.skeleton {
            out!("  skeleton {skeleton}, {} bone(s) mapped", s.bones_mapped);
        }
        out!(
            "  materials: {} reused, {} new{}; {} texture(s) imported",
            s.materials_reused,
            s.materials_new,
            match s.materials_updated {
                0 => String::new(),
                n => format!(", {n} kept with new textures"),
            },
            s.textures_imported
        );
        for m in &s.meshes {
            out!("  mesh {m}");
        }
        if !s.copies.is_empty() {
            out!(
                "  also in {} other stream(s) with a copy of the model:",
                s.copies.len()
            );
            for c in &s.copies {
                out!("    {c}");
            }
        }
        for a in &s.added {
            out!("  add {a}");
        }
        for r in &s.retargeted {
            out!("  retarget {r}");
        }
        if let Some(c) = &s.collision {
            out!(
                "  collision: {:?}, {} triangles, {} MOPP bytes, {} bytes",
                c.kind,
                c.triangles,
                c.mopp_bytes,
                c.size
            );
        }
        for w in &s.warnings {
            out!("  warning: {w}");
        }
    }
    if args.dry_run {
        return Ok(());
    }
    if plan.commit(&mut ws, &game)?.is_none() {
        out!("nothing changed");
        return Ok(());
    }
    let mut streams = vec![game.streams()[stream].stream_name()];
    if let Some(to) = &args.to {
        streams.push(doctor_toolkit::workspace::key::normalize_stream(to));
    }
    workspace_cmd::finish_edit(&game, &mut ws, &args.mode, streams)
}

/// A resource index, the id of a mesh or metamodel (`aa5c8185`,
/// `mesh:aa5c8185`, `metamodel:c06e926c`: unlike the index it stays the same
/// when resources are added to the stream), or the mesh whose model name is
/// `text`.
fn find_model(game: &GameInstance, stream: usize, text: &str) -> Result<usize> {
    let file = game.open_stream(stream)?;
    let resources = &file.layout().resources;
    let (kind, rest) = match text.split_once(':') {
        Some(("mesh", id)) => (Some(TYPE_MESH), id),
        Some(("metamodel", id)) => (Some(TYPE_METAMODEL), id),
        _ => (None, text),
    };
    let id = (rest.len() == 8)
        .then(|| u32::from_str_radix(rest, 16).ok())
        .flatten();
    if kind.is_none()
        && let Ok(index) = text.trim_start_matches('#').parse::<usize>()
    {
        if index < resources.len() {
            return Ok(index);
        }
        if id.is_none() {
            bail!("the stream has {} resources", resources.len());
        }
    }
    if let Some(id) = id {
        let found: Vec<usize> = resources
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                r.header.ids[3] == id
                    && match kind {
                        Some(kind) => r.header.type_id == kind,
                        None => matches!(r.header.type_id, TYPE_MESH | TYPE_METAMODEL),
                    }
            })
            .map(|(i, _)| i)
            .collect();
        match found.as_slice() {
            [one] => return Ok(*one),
            [] if kind.is_some() => bail!("no `{text}` in the stream"),
            [] => {}
            many => bail!(
                "a mesh and a metamodel have the id {id:08x} (resources {many:?}): \
                 write `mesh:{id:08x}` or `metamodel:{id:08x}`"
            ),
        }
    } else if kind.is_some() {
        bail!("`{text}`: the id is 8 hexadecimal digits");
    }
    let mut found = Vec::new();
    for (i, r) in resources.iter().enumerate() {
        if r.header.type_id == TYPE_MESH
            && let Ok(model) = mesh::load(&file, i)
            && model.name.eq_ignore_ascii_case(text)
        {
            found.push(i);
        }
    }
    match found.as_slice() {
        [one] => Ok(*one),
        [] => bail!(
            "no mesh named `{text}` in the stream; give a resource index or an id \
             (`mesh:aa5c8185`, `metamodel:c06e926c`)"
        ),
        many => bail!(
            "several meshes named `{text}`: {}; give one of these ids",
            many.iter()
                .map(|&i| format!("mesh:{:08x} (resource {i})", resources[i].header.ids[3]))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

#[derive(Args)]
pub struct ModelInfo {
    /// Model file: .glb, .gltf, .fbx, .obj, .usd, .usda, .usdc or .usdz
    file: PathBuf,
    /// Print the summary as JSON
    #[arg(long)]
    json: bool,
}

/// Prints the parts, materials, joints and clips of a model file as the
/// importers read it (in the game's space: Y up, metres).
pub fn model_info(args: ModelInfo) -> Result<()> {
    let scene = scene::read_file(&args.file)
        .with_context(|| format!("cannot read {}", args.file.display()))?;
    if args.json {
        let parts: Vec<serde_json::Value> = scene
            .parts
            .iter()
            .map(|p| {
                serde_json::json!({
                    "name": p.name,
                    "object": p.object,
                    "material": p.material.map(|m| scene.materials[m].name.clone()),
                    "vertices": p.geometry.positions.len(),
                    "triangles": p.geometry.triangles.len(),
                    "skinned": p.geometry.skin.is_some(),
                })
            })
            .collect();
        let materials: Vec<serde_json::Value> = scene
            .materials
            .iter()
            .map(|m| {
                serde_json::json!({
                    "name": m.name,
                    "base_color": m.base_color.as_ref().map(|i| i.name.clone()),
                    "normal": m.normal.as_ref().map(|i| i.name.clone()),
                    "specular": m.specular.as_ref().map(|i| i.name.clone()),
                    "factor": m.base_color_factor,
                })
            })
            .collect();
        let joints: Vec<serde_json::Value> = scene
            .joints
            .iter()
            .map(|j| serde_json::json!({ "name": j.name, "parent": j.parent }))
            .collect();
        let clips: Vec<serde_json::Value> = scene
            .clips
            .iter()
            .map(|c| {
                serde_json::json!({
                    "name": c.name,
                    "frames": c.frames(),
                    "frame_rate": c.frame_rate,
                    "tracks": c.tracks.len(),
                })
            })
            .collect();
        out!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "name": scene.name,
                "parts": parts,
                "materials": materials,
                "joints": joints,
                "clips": clips,
                "warnings": scene.warnings,
            }))?
        );
        return Ok(());
    }
    out!("{}: model `{}`", args.file.display(), scene.name);
    for p in &scene.parts {
        let g = &p.geometry;
        out!(
            "  part {} (object {}): {} vertices, {} triangles{}{}",
            p.name,
            p.object,
            g.positions.len(),
            g.triangles.len(),
            p.material
                .map(|m| format!(", material {}", scene.materials[m].name))
                .unwrap_or_default(),
            if g.skin.is_some() { ", skinned" } else { "" }
        );
    }
    for m in &scene.materials {
        let image =
            |i: &Option<scene::SceneImage>| i.as_ref().map_or("-".to_owned(), |i| i.name.clone());
        out!(
            "  material {}: base {}, normal {}, specular {}",
            m.name,
            image(&m.base_color),
            image(&m.normal),
            image(&m.specular)
        );
    }
    if !scene.joints.is_empty() {
        let names: Vec<&str> = scene.joints.iter().map(|j| j.name.as_str()).collect();
        out!("  {} joint(s): {}", names.len(), names.join(", "));
    }
    for c in &scene.clips {
        out!(
            "  clip {}: {} frames at {} fps, {} joint track(s)",
            c.name,
            c.frames(),
            c.frame_rate,
            c.tracks.len()
        );
    }
    for w in &scene.warnings {
        out!("  warning: {w}");
    }
    Ok(())
}

//! `collision-export` and `collision-build`: Havok collision (`hkx`).

use std::fs;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::{Args, ValueEnum};

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::import::collision;
use doctor_toolkit::resource::hkx::collision::CollisionKind;
use doctor_toolkit::resource::hkx::geometry::physics_mesh;
use doctor_toolkit::resource::hkx::physics::Physics;
use doctor_toolkit::resource::hkx::{Packfile, TYPE_HKX};

use crate::workspace_cmd::{self, EditMode};

macro_rules! out {
    ($($arg:tt)*) => {
        if writeln!(std::io::stdout(), $($arg)*).is_err() {
            std::process::exit(0);
        }
    };
}

#[derive(Clone, Copy, ValueEnum)]
pub enum MeshFormat {
    Obj,
    Glb,
}

#[derive(Args)]
pub struct CollisionExport {
    game_dir: PathBuf,
    /// Output directory: `<output>/<stream path>/<index>_hkx_<id>.obj` (or
    /// `.glb`) with the triangles of every body in model space, and `.json`
    /// with bodies, shapes and EA materials
    #[arg(short, long)]
    output: PathBuf,
    /// Only this stream
    #[arg(long)]
    stream: Option<String>,
    /// Only this resource of the stream
    #[arg(long, requires = "stream")]
    index: Option<usize>,
    #[arg(long, value_enum, default_value = "obj")]
    format: MeshFormat,
}

pub fn collision_export(args: CollisionExport) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let streams: Vec<usize> = match &args.stream {
        Some(s) => vec![
            game.find_stream(s)
                .with_context(|| format!("stream `{s}` not found"))?,
        ],
        None => (0..game.streams().len()).collect(),
    };
    let (mut count, mut triangles, mut failed) = (0usize, 0usize, 0usize);
    for stream_index in streams {
        let file = &game.streams()[stream_index];
        let stream = game.open_stream(stream_index)?;
        for (i, r) in stream.layout().resources.iter().enumerate() {
            if r.header.type_id != TYPE_HKX || args.index.is_some_and(|x| x != i) {
                continue;
            }
            let data = stream.resource_data(i)?;
            let id = r.header.ids[3];
            let physics = match Physics::parse(&data) {
                Ok(p) => p,
                Err(e) => {
                    out!("{} #{i}: {e}", file.rel_path);
                    failed += 1;
                    continue;
                }
            };
            let mesh = physics_mesh(&physics);
            let dir = args.output.join(file.rel_path.trim_end_matches(".str"));
            fs::create_dir_all(&dir)?;
            let base = dir.join(format!("{i:05}_hkx_{id:08x}"));
            let name = format!("{id:08x}");
            match args.format {
                MeshFormat::Obj => fs::write(base.with_extension("obj"), mesh.to_obj(&name))?,
                MeshFormat::Glb => fs::write(base.with_extension("glb"), mesh.to_glb(&name))?,
            }
            fs::write(
                base.with_extension("json"),
                serde_json::to_string_pretty(&physics)? + "\n",
            )?;
            count += 1;
            triangles += mesh.triangles.len();
        }
    }
    out!("exported {count} collision packfile(s), {triangles} triangle(s); {failed} not read");
    Ok(())
}

#[derive(Args)]
pub struct CollisionBuild {
    game_dir: PathBuf,
    /// Stream that holds the metamodel
    stream: String,
    /// Resource index of the metamodel; its collision parts will point at the new `hkx`
    metamodel: usize,
    /// Geometry: a model file (.glb, .gltf, .fbx, .obj, .usd, .usda, .usdc,
    /// .usdz); default: the first mesh the
    /// metamodel shows, as the workspace has it
    #[arg(long, value_name = "FILE")]
    from: Option<PathBuf>,
    /// `mesh` (triangles with MOPP, static), `hull` (convex hull) or `box`
    #[arg(long, default_value = "mesh")]
    kind: CollisionKind,
    /// Also save the new packfile to this file
    #[arg(long, value_name = "FILE")]
    save: Option<PathBuf>,
    /// Print what would be done without recording anything
    #[arg(long)]
    dry_run: bool,
    /// Print the summary as JSON
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    mode: EditMode,
}

pub fn collision_build(args: CollisionBuild) -> Result<()> {
    let game = Arc::new(GameInstance::open(&args.game_dir)?);
    let mut ws = workspace_cmd::open_for_edit(&game, &args.mode)?;
    let stream = game
        .find_stream(&args.stream)
        .with_context(|| format!("stream `{}` not found", args.stream))?;
    let mesh = match &args.from {
        Some(path) => {
            let model = doctor_toolkit::scene::read_file(path)
                .with_context(|| format!("cannot read {}", path.display()))?;
            collision::mesh_from_gltf(&model)
        }
        None => collision::mesh_from_metamodel(&game, &ws, stream, args.metamodel)?,
    };
    if mesh.triangles.is_empty() {
        bail!("the geometry has no triangles");
    }
    let (index, _) = game.build_index(|_| {});
    let plan = collision::plan(&game, &ws, &index, stream, args.metamodel, &mesh, args.kind)?;
    let r = &plan.report;
    if args.json {
        out!("{}", serde_json::to_string_pretty(r)?);
    } else {
        let c = &r.collision;
        out!("{}", plan.label);
        out!(
            "  {} vertices, {} triangles ({} degenerate dropped), MOPP {} bytes, packfile {} bytes",
            c.vertices,
            c.triangles,
            c.degenerate,
            c.mopp_bytes,
            c.size
        );
        out!("  bounds {:?} .. {:?}", c.bounds_min, c.bounds_max);
        out!("  EA material {:08x?}", c.material);
        out!("  add {}", r.added);
        out!(
            "  metamodel {} ({} reference(s))",
            r.metamodel,
            r.references
        );
    }
    if let Some(path) = &args.save {
        fs::write(path, &plan.hkx)?;
        // Check what was written.
        Packfile::parse(&plan.hkx)?;
    }
    if args.dry_run {
        return Ok(());
    }
    if plan.commit(&mut ws, &game)?.is_none() {
        out!("nothing changed");
        return Ok(());
    }
    let name = game.streams()[stream].stream_name();
    workspace_cmd::finish_edit(&game, &mut ws, &args.mode, vec![name])
}

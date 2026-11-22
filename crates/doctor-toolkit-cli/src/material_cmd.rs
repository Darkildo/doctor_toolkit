//! `material`: the parameters of a material resource, shown or set.

use std::io::Write as _;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Args;

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::resource::material::{self, Material, ParamEdit, ParamValue, TYPE_MATERIAL};

use crate::workspace_cmd::{self, EditMode};

macro_rules! out {
    ($($arg:tt)*) => {
        if writeln!(std::io::stdout(), $($arg)*).is_err() {
            std::process::exit(0);
        }
    };
}

#[derive(Args)]
pub struct MaterialArgs {
    game_dir: PathBuf,
    /// Stream of the material, e.g. `gf2/globalfolder/characters/allcharacters`
    stream: String,
    /// The material: its resource index in the stream, or its name
    material: String,
    /// Set a parameter: `NAME=N,N,N` for numbers (as many as the parameter
    /// holds; a tint colour `g_TintColor0` has four) or `NAME=tex:HASH` for
    /// the name hash of a texture; may be repeated (one operation)
    #[arg(long, value_name = "NAME=VALUE")]
    set: Vec<String>,
    /// Change the material only in this stream. By default every copy of
    /// the material (every material of the game with the same name hash)
    /// gets the edit: the game uses the copy it loaded first
    #[arg(long, requires = "set")]
    this_stream_only: bool,
    #[command(flatten)]
    mode: EditMode,
}

/// Index of the material `name` (an index or a material name) in `stream`.
fn find(
    game: &GameInstance,
    ws: &doctor_toolkit::workspace::Workspace,
    stream: usize,
    name: &str,
) -> Result<usize> {
    let file = game.open_stream(stream)?;
    let resources = &file.layout().resources;
    if let Ok(index) = name.parse::<usize>() {
        let r = resources
            .get(index)
            .with_context(|| format!("the stream has no resource {index}"))?;
        if r.header.type_id != TYPE_MATERIAL {
            bail!("resource {index} is not a material");
        }
        return Ok(index);
    }
    let mut found = Vec::new();
    for (index, r) in resources.iter().enumerate() {
        if r.header.type_id != TYPE_MATERIAL {
            continue;
        }
        let data = ws.current_data(game, stream, index)?;
        if Material::parse(&data).is_ok_and(|m| m.name.eq_ignore_ascii_case(name)) {
            found.push(index);
        }
    }
    match found.as_slice() {
        [index] => Ok(*index),
        [] => bail!("the stream has no material `{name}`"),
        many => bail!(
            "{} materials are named `{name}`: give the index ({many:?})",
            many.len()
        ),
    }
}

pub fn material(args: MaterialArgs) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let mut ws = workspace_cmd::open_for_edit(&game, &args.mode)?;
    let stream = game
        .find_stream(&args.stream)
        .with_context(|| format!("stream `{}` not found", args.stream))?;
    let index = find(&game, &ws, stream, &args.material)?;
    if !args.set.is_empty() {
        let edits = args
            .set
            .iter()
            .map(|s| ParamEdit::parse(s))
            .collect::<doctor_toolkit::Result<Vec<_>>>()?;
        let done = material::set_params(
            &game,
            &mut ws,
            stream,
            index,
            &edits,
            !args.this_stream_only,
        )?;
        for skipped in &done.skipped {
            out!("warning: copy left as it is: {skipped}");
        }
        if !done.changed {
            out!(
                "nothing changed: {} already has these values ({} cop(y/ies))",
                done.material,
                done.copies.len()
            );
            return Ok(());
        }
        out!("{} ({})", done.label, done.resource);
        out!(
            "{} of {} cop(y/ies) of the material changed",
            done.changed_copies.len(),
            done.copies.len()
        );
        let mut streams: Vec<String> = done
            .changed_copies
            .iter()
            .filter_map(|k| k.parse::<doctor_toolkit::workspace::ResourceKey>().ok())
            .map(|k| k.stream)
            .collect();
        streams.sort();
        streams.dedup();
        return workspace_cmd::finish_edit(&game, &mut ws, &args.mode, streams);
    }
    let m = Material::parse(&ws.current_data(&game, stream, index)?)?;
    out!(
        "material {} ({:08x}), flags {:#x}, {} parameter(s)",
        m.name,
        m.name_hash,
        m.flags,
        m.params.len()
    );
    // A parameter is listed once per shader pass that takes it; `--set`
    // sets it in all of them.
    for (p, passes) in m.unique_params() {
        let value = match &p.value {
            ParamValue::Floats(v) => v
                .iter()
                .map(|f| format!("{f}"))
                .collect::<Vec<_>>()
                .join(","),
            ParamValue::Texture(hash) => format!("tex:{hash:08x}"),
            ParamValue::Unknown(kind) => format!("(type {kind})"),
        };
        let passes = if passes > 1 {
            format!("  x{passes}")
        } else {
            String::new()
        };
        if p.source.is_empty() || p.source == "(null)" {
            out!("  {:<28} {value}{passes}", p.name);
        } else {
            out!("  {:<28} {value}{passes}  ({})", p.name, p.source);
        }
    }
    Ok(())
}

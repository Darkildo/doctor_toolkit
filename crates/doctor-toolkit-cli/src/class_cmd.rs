//! Classes of native mods (stage 80): `class-schema` checks the class file
//! of a mod, `simgroup-add` puts a new object of a class into a SimGroup.

use std::io::Write as _;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Args;

use doctor_toolkit::game_types::custom;
use doctor_toolkit::level::Location;
use doctor_toolkit::logic_graph::schema::LogicSchema;
use doctor_toolkit::resource::simgroup::Guid;

use crate::level_cmd;
use crate::workspace_cmd::{self, EditMode};

macro_rules! out {
    ($($arg:tt)*) => {
        if writeln!(std::io::stdout(), $($arg)*).is_err() {
            std::process::exit(0);
        }
    };
}

/// Arguments of `class-schema`.
#[derive(Args)]
pub struct ClassSchema {
    /// Class files of native mods (TOML, see docs/analysis/asi-platform.md)
    #[arg(required = true)]
    pub files: Vec<PathBuf>,
    /// Print the classes as JSON
    #[arg(long)]
    pub json: bool,
}

/// Checks class files and prints what they describe.
pub fn class_schema(args: &ClassSchema) -> Result<()> {
    let engine = LogicSchema::engine();
    let mut all = Vec::new();
    for file in &args.files {
        let text = std::fs::read_to_string(file)
            .with_context(|| format!("cannot read {}", file.display()))?;
        let classes =
            custom::parse(&text, &engine).with_context(|| format!("{}", file.display()))?;
        for class in classes {
            if all
                .iter()
                .any(|c: &custom::CustomClass| c.hash == class.hash)
            {
                bail!("class `{}` is in two files", class.name);
            }
            all.push(class);
        }
    }
    if args.json {
        let classes: Vec<_> = all
            .iter()
            .map(|c| {
                serde_json::json!({
                    "name": c.name,
                    "hash": format!("{:08x}", c.hash),
                    "category": c.category.name(),
                    "summary": c.summary,
                    "attr_count": c.attr_count,
                    "layout": c.layout(&engine).iter()
                        .map(|(h, n)| format!("{h:08x}:{n}")).collect::<Vec<_>>(),
                    "attrs": c.attrs.iter().map(|a| serde_json::json!({
                        "i": a.index,
                        "name": a.name,
                        "pin": a.pin,
                        "kind": a.kind,
                        "delay": a.delay,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        out!("{}", serde_json::to_string_pretty(&classes)?);
        return Ok(());
    }
    for c in &all {
        out!(
            "{} (hash {:08x}, {}): {} attribute(s){}",
            c.name,
            c.hash,
            c.category.name(),
            c.attr_count,
            if c.summary.is_empty() {
                String::new()
            } else {
                format!(" — {}", c.summary)
            }
        );
        for a in &c.attrs {
            out!(
                "  {:3} {:<24} {:<6} {:?}",
                a.index,
                a.name,
                format!("{:?}", a.pin).to_lowercase(),
                a.kind
            );
        }
    }
    out!(
        "{} class(es); set {}={} to use them in every command",
        all.len(),
        custom::ENV,
        std::env::join_paths(&args.files)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    );
    Ok(())
}

/// Arguments of `simgroup-add`.
#[derive(Args)]
pub struct SimGroupAdd {
    pub game_dir: PathBuf,
    /// Stream name, e.g. `gf2/cuba/local`
    pub stream: String,
    /// Index of the SimGroup resource in the stream (see `list`)
    pub resource: usize,
    /// Class of the new object: a name or a hexadecimal hash. Classes of
    /// native mods are known when DOCTOR_TOOLKIT_CLASSES names their files
    pub class: String,
    #[command(flatten)]
    pub mode: EditMode,
}

/// Adds an object of a class with empty packets of the schema's layout.
pub fn simgroup_add(args: &SimGroupAdd) -> Result<()> {
    for e in custom::load_errors() {
        eprintln!(
            "warning: {}: {} is not used: {}",
            custom::ENV,
            e.file.display(),
            e.error
        );
    }
    let schema = LogicSchema::gf2();
    let class = doctor_toolkit::resource::names::sim_classes()
        .resolve(&args.class)
        .filter(|&h| schema.layout(h).is_some())
        .with_context(|| {
            format!(
                "the schema has no layout of class `{}` (a class of a native mod? set {})",
                args.class,
                custom::ENV
            )
        })?;
    let (game, index) =
        level_cmd::open_index_with(&args.game_dir, args.mode.workspace_dir.as_deref())?;
    let group = Location {
        stream: level_cmd::find_stream(&game, &args.stream)?,
        resource: args.resource,
    };
    let mut ws = workspace_cmd::open_for_edit(&game, &args.mode)?;
    let taken = level_cmd::all_guids(&game, &index);
    let (object, guid) = doctor_toolkit::level::edit::add_empty_at(
        &game,
        &index,
        &mut ws,
        &group.into(),
        class,
        None,
        &|g: &Guid| taken.contains(g),
    )?;
    out!(
        "added object {object} ({guid}) of class {}",
        schema.class_name(class).unwrap_or(&args.class)
    );
    workspace_cmd::finish_edit(&game, &mut ws, &args.mode, vec![args.stream.clone()])
}

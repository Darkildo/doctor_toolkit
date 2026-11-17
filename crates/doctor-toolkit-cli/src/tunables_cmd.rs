//! `tunables`: the curated list of game parameters — show them with their
//! current values, change them in every copy of the attribute (what the
//! Game parameters wizard of the editor and the MCP tools `tunables` and
//! `set_tunables` do).

use std::io::Write as _;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Args;

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::tunables::{self, SetTunable};
use doctor_toolkit::workspace::Workspace;

use crate::workspace_cmd::{self, EditMode};

/// Writes a line to stdout and exits quietly when the pipe is closed.
macro_rules! out {
    ($($arg:tt)*) => {
        if writeln!(std::io::stdout(), $($arg)*).is_err() {
            std::process::exit(0);
        }
    };
}

#[derive(Args)]
pub struct TunablesArgs {
    pub game_dir: PathBuf,
    /// Show one parameter with the value of each of its objects
    pub id: Option<String>,
    /// Only this category (weapons, health, player, crew, economy, police,
    /// vehicles, world)
    #[arg(long)]
    pub category: Option<String>,
    /// Set a parameter in every object that has it: `ID=VALUE` (repeatable)
    #[arg(long = "set", value_name = "ID=VALUE")]
    pub set: Vec<String>,
    /// Multiply the value of every object of a parameter: `ID=FACTOR`
    /// (repeatable; whole numbers are rounded)
    #[arg(long = "multiply", value_name = "ID=FACTOR")]
    pub multiply: Vec<String>,
    /// With `--set` and `--multiply`: only print what would change
    #[arg(long)]
    pub dry_run: bool,
    #[command(flatten)]
    pub mode: EditMode,
}

impl TunablesArgs {
    pub fn may_write_game(&self) -> bool {
        self.mode.direct && !(self.set.is_empty() && self.multiply.is_empty())
    }
}

fn pair(text: &str) -> Result<(String, f64)> {
    let (id, value) = text
        .split_once('=')
        .with_context(|| format!("`{text}`: ID=NUMBER expected"))?;
    let id = id.trim().to_owned();
    let t = tunables::by_id(&id)?;
    // `true` and `false` for switches, as the wizard takes them.
    let value = match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" => 1.0,
        "false" | "no" | "off" => 0.0,
        v => v
            .parse()
            .with_context(|| format!("{}: `{v}` is not a number", t.id))?,
    };
    Ok((id, value))
}

pub fn run(args: &TunablesArgs) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let mut changes = Vec::new();
    for text in &args.set {
        let (id, value) = pair(text)?;
        changes.push(SetTunable {
            id,
            value: Some(value),
            multiply: None,
        });
    }
    for text in &args.multiply {
        let (id, factor) = pair(text)?;
        changes.push(SetTunable {
            id,
            value: None,
            multiply: Some(factor),
        });
    }
    if changes.is_empty() {
        let ws = Workspace::for_game(&game, args.mode.workspace_dir.as_deref())?;
        return show(&game, &ws, args);
    }
    if args.dry_run {
        let ws = Workspace::for_game(&game, args.mode.workspace_dir.as_deref())?;
        let planned = tunables::changes(&game, Some(&ws), &changes)?;
        let (_, diff) = tunables::plan(&game, Some(&ws), &planned)?;
        print_diff(&diff);
        return Ok(());
    }
    let mut ws = workspace_cmd::open_for_edit(&game, &args.mode)?;
    let (diff, label) = tunables::set(&game, &mut ws, &changes)?;
    print_diff(&diff);
    let Some(label) = label else {
        out!("no change");
        return Ok(());
    };
    out!("{label}");
    let mut streams: Vec<String> = changes
        .iter()
        .filter_map(|c| tunables::by_id(&c.id).ok())
        .map(|t| t.resource.stream.clone())
        .collect();
    streams.sort();
    streams.dedup();
    workspace_cmd::finish_edit(&game, &mut ws, &args.mode, streams)
}

fn print_diff(diff: &[tunables::DiffLine]) {
    for d in diff {
        out!("{}  {}: {} -> {}", d.id, d.object, d.old, d.new);
    }
    out!("{} value(s) change", diff.len());
}

fn show(game: &GameInstance, ws: &Workspace, args: &TunablesArgs) -> Result<()> {
    if let Some(id) = &args.id {
        let t = tunables::by_id(id)?;
        out!("{}  [{}]  {}", t.id, t.category, t.label);
        if !t.description.is_empty() {
            out!("  {}", t.description);
        }
        out!(
            "  {} attribute {} of {}",
            t.class_name(),
            t.attribute,
            t.resource
        );
        if t.min.is_some() || t.max.is_some() {
            out!(
                "  bounds: {} .. {}",
                t.min.map_or_else(|| "-".into(), |v| t.format(v)),
                t.max.map_or_else(|| "-".into(), |v| t.format(v))
            );
        }
        for o in t.read(game, Some(ws))?.objects {
            out!("  {:>5}  {:<40} {}", o.object, o.label, t.format(o.value));
        }
        return Ok(());
    }
    let mut shown = 0;
    for t in tunables::gf2() {
        if args
            .category
            .as_deref()
            .is_some_and(|c| !t.category.eq_ignore_ascii_case(c))
        {
            continue;
        }
        let values = match t.read(game, Some(ws)) {
            Ok(reading) => {
                let distinct: Vec<String> =
                    reading.distinct().iter().map(|&v| t.format(v)).collect();
                format!(
                    "{} ({} object(s))",
                    distinct.join(" / "),
                    reading.objects.len()
                )
            }
            Err(e) => format!("unavailable: {e}"),
        };
        out!("{:<44} {:<40} {values}", t.id, t.label);
        shown += 1;
    }
    if shown == 0 && args.category.is_some() {
        bail!(
            "no such category; categories: {}",
            tunables::CATEGORIES.join(", ")
        );
    }
    out!("{shown} parameter(s)");
    Ok(())
}

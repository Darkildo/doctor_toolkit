//! `text-add` and `apx-text-refs`: new strings and the references to them.

use std::io::Write as _;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Args;

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::hash::string_id;
use doctor_toolkit::resource::apx::AptFile;
use doctor_toolkit::resource::lh2::StringTable;
use doctor_toolkit::text::{self, NewString, TableRef};

use crate::workspace_cmd::{self, EditMode};

macro_rules! out {
    ($($arg:tt)*) => {
        if writeln!(std::io::stdout(), $($arg)*).is_err() {
            std::process::exit(0);
        }
    };
}

#[derive(Args)]
pub struct TextAdd {
    game_dir: PathBuf,
    /// Entry name (letters, digits, `_`); interface movies show it as `$NAME`
    /// and SimGroup hash attributes store its hash
    name: String,
    /// Text for every language without its own `--lang`
    text: String,
    /// Text in one language, e.g. `--lang es=Hola` (repeatable)
    #[arg(long = "lang", value_name = "CODE=TEXT")]
    translations: Vec<String>,
    /// Add to this table (`STREAM:INDEX`, repeatable) instead of the tables
    /// of the game's languages (`lhr`)
    #[arg(long, value_name = "STREAM:INDEX")]
    table: Vec<String>,
    /// Print what would be done without recording anything
    #[arg(long)]
    dry_run: bool,
    /// Print the summary as JSON
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    mode: EditMode,
}

pub fn text_add(args: TextAdd) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let mut ws = workspace_cmd::open_for_edit(&game, &args.mode)?;
    let (index, _) = game.build_index(|_| {});
    let tables = if args.table.is_empty() {
        text::language_tables(&game, &index)?
    } else {
        args.table
            .iter()
            .map(|t| {
                let (s, r) = t
                    .rsplit_once(':')
                    .with_context(|| format!("`{t}`: STREAM:INDEX expected"))?;
                Ok(TableRef {
                    stream: game
                        .find_stream(s)
                        .with_context(|| format!("stream `{s}` not found"))?,
                    resource: r
                        .parse()
                        .with_context(|| format!("`{r}` is not an index"))?,
                    language: None,
                })
            })
            .collect::<Result<_>>()?
    };
    let mut new = NewString {
        name: args.name.clone(),
        text: args.text.clone(),
        ..Default::default()
    };
    for t in &args.translations {
        let (code, text) = t
            .split_once('=')
            .with_context(|| format!("`{t}`: CODE=TEXT expected"))?;
        new.translations.insert(code.to_owned(), text.to_owned());
    }
    let plan = text::plan_add(&game, &ws, &index, &tables, &new)?;
    if args.json {
        out!("{}", serde_json::to_string_pretty(&plan.added)?);
    } else {
        out!("{}", plan.label);
        for (table, texts) in &plan.added.tables {
            out!("  {table}");
            for (code, text) in texts {
                out!("    {code}: {text}");
            }
        }
        out!(
            "  reference it as ${} in interface movies (apx-text-refs), or as hash {:08x} in \
             SimGroup hash attributes (simgroup-set)",
            plan.added.name,
            plan.added.hash
        );
    }
    if args.dry_run {
        return Ok(());
    }
    if plan.commit(&mut ws, &game)?.is_none() {
        out!("nothing changed");
        return Ok(());
    }
    let streams = tables
        .iter()
        .map(|t| game.streams()[t.stream].stream_name())
        .collect();
    workspace_cmd::finish_edit(&game, &mut ws, &args.mode, streams)
}

#[derive(Args)]
pub struct ApxTextRefs {
    game_dir: PathBuf,
    /// Stream of the interface movie, e.g. `gf2/frontend`
    stream: String,
    /// Resource index of the `apx`
    index: usize,
    /// Point the `$OLD` references at `$NEW` (NEW no longer than OLD)
    #[arg(long, value_name = "OLD=NEW")]
    set: Option<String>,
    #[command(flatten)]
    mode: EditMode,
}

pub fn apx_text_refs(args: ApxTextRefs) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let mut ws = workspace_cmd::open_for_edit(&game, &args.mode)?;
    let stream = game
        .find_stream(&args.stream)
        .with_context(|| format!("stream `{}` not found", args.stream))?;
    if let Some(set) = &args.set {
        let (old, new) = set
            .split_once('=')
            .with_context(|| format!("`{set}`: OLD=NEW expected"))?;
        let (old, new) = (old.trim_start_matches('$'), new.trim_start_matches('$'));
        let changed = text::retarget_apx(&game, &mut ws, stream, args.index, old, new)?;
        if changed == 0 {
            bail!("the movie has no reference ${old}");
        }
        out!("pointed {changed} reference(s) ${old} at ${new}");
        let name = game.streams()[stream].stream_name();
        return workspace_cmd::finish_edit(&game, &mut ws, &args.mode, vec![name]);
    }
    let movie = AptFile::parse(&ws.current_data(&game, stream, args.index)?)?;
    // Texts from the first language table, for orientation.
    let (index, _) = game.build_index(|_| {});
    let english = text::language_tables(&game, &index)
        .ok()
        .and_then(|t| t.into_iter().next())
        .and_then(|t| ws.current_data(&game, t.stream, t.resource).ok())
        .and_then(|d| StringTable::parse(&d).ok());
    let refs = movie.text_references();
    for (_, _, name) in &refs {
        let hash = string_id(name);
        let text = english
            .as_ref()
            .and_then(|t| t.texts(hash))
            .map(|t| t.join(" | "))
            .unwrap_or_else(|| "(not in the language tables)".into());
        out!("${name:<40} {hash:08x}  {text}");
    }
    out!("{} text reference(s)", refs.len());
    Ok(())
}

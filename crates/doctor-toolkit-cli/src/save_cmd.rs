//! `save`: a look into a save file of the game (see `docs/formats/save.md`).
//! The file is only read.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Args;
use serde_json::json;

use doctor_toolkit::live::names::LiveNames;
use doctor_toolkit::save::{self, SaveFile, view};

#[derive(Args)]
pub struct SaveArgs {
    /// A save file (`GF2-EN-autosave`, `GF2-EN-<digits>`), or a folder: its
    /// saves are listed
    pub path: PathBuf,
    /// The game folder: variables and objects are shown by name
    #[arg(long, value_name = "DIR")]
    pub game: Option<PathBuf>,
    /// Show this chunk: its name or id from the summary (may be repeated)
    #[arg(long, value_name = "NAME")]
    pub chunk: Vec<String>,
    /// Show every chunk
    #[arg(long, conflicts_with = "chunk")]
    pub all: bool,
    /// Print JSON: the header and the decoded chunks
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: SaveArgs) -> Result<()> {
    if args.path.is_dir() {
        return list(&args.path, args.json);
    }
    let save = SaveFile::open(&args.path)
        .with_context(|| format!("reading the save {}", args.path.display()))?;
    let chunks: Vec<&save::Chunk> = if args.all {
        save.chunks.iter().collect()
    } else {
        args.chunk
            .iter()
            .map(|name| {
                save.chunk_named(name)
                    .with_context(|| format!("the save has no chunk `{name}`"))
            })
            .collect::<Result<_>>()?
    };
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&view::to_json(&save, &chunks))?
        );
        return Ok(());
    }
    let names = match &args.game {
        Some(dir) if !chunks.is_empty() => Some(
            LiveNames::open(dir, false)
                .with_context(|| format!("reading the names of {}", dir.display()))?,
        ),
        _ => None,
    };
    if chunks.is_empty() || args.all {
        print!("{}", view::summary(&save));
    }
    for chunk in chunks {
        println!("\n[{}]", chunk.name());
        print!("{}", view::chunk_text(chunk, names.as_ref()));
    }
    if !save.checksums.ok() {
        bail!("the checksums of the save do not match: the game will not load it");
    }
    Ok(())
}

/// Lists the saves of a folder and the folders below it.
fn list(dir: &Path, as_json: bool) -> Result<()> {
    let files = view::save_files(dir)?;
    let mut rows = Vec::new();
    for path in &files {
        let Ok(save) = SaveFile::open(path) else {
            continue;
        };
        let shown = path.strip_prefix(dir).unwrap_or(path).display().to_string();
        if as_json {
            rows.push(json!({
                "file": shown,
                "name": save.description,
                "play_seconds": save.play_seconds,
                "checksums_ok": save.checksums.ok(),
            }));
        } else {
            let state = if save.checksums.ok() {
                ""
            } else {
                "  (bad checksums)"
            };
            println!("{shown}\n    {}{state}", save.description);
        }
    }
    if as_json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else if files.is_empty() {
        println!("no saves in {}", dir.display());
    }
    Ok(())
}

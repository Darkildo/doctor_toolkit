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
            serde_json::to_string_pretty(&to_json(&save, &chunks))?
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

fn to_json(save: &SaveFile, chunks: &[&save::Chunk]) -> serde_json::Value {
    let listed: Vec<_> = save
        .chunks
        .iter()
        .map(|c| {
            json!({
                "id": format!("{:08x}", c.id),
                "name": c.name(),
                "offset": c.offset,
                "size": c.data.len(),
            })
        })
        .collect();
    let decoded: serde_json::Map<String, serde_json::Value> = chunks
        .iter()
        .map(|c| {
            let value = match c.content() {
                Ok(save::Content::Raw) => json!({ "hex": hex(&c.data) }),
                Ok(content) => json!(content),
                Err(e) => json!({ "error": e.to_string(), "hex": hex(&c.data) }),
            };
            (c.name(), value)
        })
        .collect();
    json!({
        "name": save.description,
        "title": save.title,
        "profile": save.profile,
        "city": save.city,
        "play_time": save.play_time,
        "play_seconds": save.play_seconds,
        "version": save.version,
        "capacity": save.capacity,
        "used": save.used,
        "mask": save.mask,
        "checksums": save.checksums,
        "chunks": listed,
        "content": decoded,
    })
}

fn hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}

/// Lists the saves of a folder and the folders below it.
fn list(dir: &Path, as_json: bool) -> Result<()> {
    let mut files = Vec::new();
    collect(dir, &mut files)?;
    files.sort();
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

fn collect(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let entries = std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))?;
    for entry in entries {
        let path = entry?.path();
        if path.is_dir() {
            collect(&path, out)?;
        } else if path
            .metadata()
            .is_ok_and(|m| m.len() == save::FILE_SIZE as u64)
        {
            out.push(path);
        }
    }
    Ok(())
}

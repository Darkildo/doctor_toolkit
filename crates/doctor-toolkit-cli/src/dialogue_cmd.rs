//! `dialogues`, `dialogue`, `dialogue-export`, `dialogue-import` and
//! `subtitle`: face-to-face dialogues and the subtitles of speech.

use std::io::Write as _;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Args;

use doctor_toolkit::dialogue::{self, Catalog, DialogueRef, Report};
use doctor_toolkit::game::GameInstance;
use doctor_toolkit::resource::dlg::Dialogue;
use doctor_toolkit::resource::lh2::StringTable;
use doctor_toolkit::resource::speech;
use doctor_toolkit::workspace::Workspace;

use crate::workspace_cmd::{self, EditMode};

macro_rules! out {
    ($($arg:tt)*) => {
        if writeln!(std::io::stdout(), $($arg)*).is_err() {
            std::process::exit(0);
        }
    };
}

fn catalog(game: &GameInstance) -> Result<Catalog> {
    let catalog = Catalog::build(game, &|done, total| {
        eprint!("\rreading streams: {done}/{total}");
    })?;
    eprintln!();
    Ok(catalog)
}

/// The workspace when the game has one; reading commands never create it.
fn workspace(game: &GameInstance, dir: Option<&std::path::Path>) -> Option<Workspace> {
    let dir = dir.map_or_else(|| Workspace::default_dir(game.root()), PathBuf::from);
    if Workspace::exists(&dir) {
        Workspace::open(dir).ok()
    } else {
        None
    }
}

fn strings(
    game: &GameInstance,
    ws: Option<&Workspace>,
    catalog: &Catalog,
    lang: &str,
) -> Option<StringTable> {
    dialogue::language_strings(game, ws, &catalog.languages)
        .into_iter()
        .find(|(code, _)| code.eq_ignore_ascii_case(lang))
        .map(|(_, t)| t)
}

fn load(game: &GameInstance, ws: Option<&Workspace>, d: &DialogueRef) -> Result<Dialogue> {
    let data = match ws {
        Some(ws) => ws.current_data(game, d.stream, d.resource)?,
        None => game.open_stream(d.stream)?.resource_data(d.resource)?,
    };
    Ok(Dialogue::parse(&data)?)
}

#[derive(Args)]
pub struct ListArgs {
    game_dir: PathBuf,
    /// Search option texts, speech events, subtitles and clip names instead
    #[arg(long)]
    search: Option<String>,
    /// Language of option texts
    #[arg(long, default_value = "en")]
    lang: String,
    /// Workspace directory [default: <GAME_DIR>/_doctor_toolkit/workspace]
    #[arg(long, value_name = "DIR")]
    workspace_dir: Option<PathBuf>,
}

pub fn list(args: ListArgs) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let ws = workspace(&game, args.workspace_dir.as_deref());
    let catalog = catalog(&game)?;
    let strings = strings(&game, ws.as_ref(), &catalog, &args.lang);
    if let Some(query) = &args.search {
        let hits = dialogue::search(&catalog, &game, ws.as_ref(), strings.as_ref(), query);
        for hit in &hits {
            let d = &catalog.dialogues[hit.dialogue];
            match hit.node {
                Some(n) => out!("{} #{n}: {}", d.name, hit.what),
                None => out!("{}: {}", d.name, hit.what),
            }
        }
        out!("{} hit(s)", hits.len());
        return Ok(());
    }
    for d in &catalog.dialogues {
        let parsed = load(&game, ws.as_ref(), d);
        let (nodes, lines, choices) = match &parsed {
            Ok(p) => (
                p.nodes.len(),
                p.count(doctor_toolkit::resource::dlg::KIND_LINE),
                p.count(doctor_toolkit::resource::dlg::KIND_CHOICE),
            ),
            Err(_) => (0, 0, 0),
        };
        let used: Vec<String> = catalog
            .usages_of(d)
            .iter()
            .take(3)
            .map(|u| format!("{} ({})", u.object, u.attribute))
            .collect();
        out!(
            "{}  {nodes} node(s), {lines} line(s), {choices} choice(s); used by {} {}",
            d.name,
            catalog.usages_of(d).len(),
            used.join(", ")
        );
    }
    out!("{} dialogue(s)", catalog.dialogues.len());
    Ok(())
}

#[derive(Args)]
pub struct ShowArgs {
    game_dir: PathBuf,
    /// Dialogue: its stream (`gf2/globalfolder/dlg/0823c6d1`) or hash (`0823c6d1`)
    dialogue: String,
    /// Print the report as JSON (the form `dialogue-import` reads)
    #[arg(long)]
    json: bool,
    /// Language of option texts
    #[arg(long, default_value = "en")]
    lang: String,
    #[arg(long, value_name = "DIR")]
    workspace_dir: Option<PathBuf>,
}

pub fn show(args: ShowArgs) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let ws = workspace(&game, args.workspace_dir.as_deref());
    let catalog = catalog(&game)?;
    let d = catalog
        .find(&args.dialogue)
        .with_context(|| format!("no dialogue `{}`", args.dialogue))?
        .clone();
    let strings = strings(&game, ws.as_ref(), &catalog, &args.lang);
    let parsed = load(&game, ws.as_ref(), &d)?;
    let report = Report::of(&catalog, &d, &parsed, strings.as_ref());
    if args.json {
        out!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    out!("{} ({} node(s))", report.stream, report.nodes.len());
    for u in &report.used_by {
        out!(
            "  used by {} {} ({}) in {}",
            u.class,
            u.object,
            u.attribute,
            game.streams()[u.stream].stream_name()
        );
    }
    for n in &report.nodes {
        let kids: Vec<String> = n
            .children
            .iter()
            .map(|c| c.map_or("-".into(), |c| format!("#{c}")))
            .collect();
        out!(
            "#{:<3} {:<14} {}  → {}",
            n.id,
            n.kind,
            n.summary,
            kids.join(" ")
        );
        if let Some(line) = &n.line {
            for (role, clips) in &line.animations {
                out!("       {role}: {}", clips.join(", "));
            }
            for (entry, text) in &line.speech {
                out!(
                    "       {entry}: {}",
                    text.as_deref().unwrap_or("(no subtitle)")
                );
            }
        }
        for o in &n.options {
            out!(
                "       option {}: {} \"{}\" → {}",
                o.option,
                o.label.as_deref().unwrap_or("-"),
                o.text.as_deref().unwrap_or(""),
                o.target.map_or("-".into(), |t| format!("#{t}"))
            );
        }
    }
    Ok(())
}

#[derive(Args)]
pub struct ExportArgs {
    game_dir: PathBuf,
    #[arg(short, long)]
    output: PathBuf,
    #[arg(long, default_value = "en")]
    lang: String,
    #[arg(long, value_name = "DIR")]
    workspace_dir: Option<PathBuf>,
}

pub fn export(args: ExportArgs) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let ws = workspace(&game, args.workspace_dir.as_deref());
    let catalog = catalog(&game)?;
    let strings = strings(&game, ws.as_ref(), &catalog, &args.lang);
    let n = dialogue::export_all(&catalog, &game, ws.as_ref(), strings.as_ref(), &args.output)?;
    out!("exported {n} dialogue(s) to {}", args.output.display());
    Ok(())
}

#[derive(Args)]
pub struct ImportArgs {
    game_dir: PathBuf,
    /// JSON files written by `dialogue-export` or `dialogue --json`; their
    /// `data` part is written back
    #[arg(required = true)]
    files: Vec<PathBuf>,
    /// Write the `dlg` resource of the one file given here instead of
    /// recording it: the data of a new dialogue, to be put into a stream of
    /// its own with `add-stream` and `add-resource --type dlg`. The `stream`
    /// of the JSON is not looked at
    #[arg(long, value_name = "FILE")]
    raw: Option<PathBuf>,
    #[command(flatten)]
    mode: EditMode,
}

/// The tree of a JSON file written by `dialogue --json`.
fn tree_of(path: &std::path::Path) -> Result<(serde_json::Value, Dialogue)> {
    let text = std::fs::read_to_string(path).with_context(|| path.display().to_string())?;
    let json: serde_json::Value = serde_json::from_str(&text)?;
    let data: Dialogue = serde_json::from_value(json["data"].clone())
        .with_context(|| format!("{}: `data`", path.display()))?;
    Ok((json, data))
}

pub fn import(args: ImportArgs) -> Result<()> {
    if let Some(out) = &args.raw {
        let [file] = args.files.as_slice() else {
            bail!("--raw takes one JSON file");
        };
        let (_, data) = tree_of(file)?;
        let bytes = data.to_bytes()?;
        // What the game will read: the tree must parse back as written.
        Dialogue::parse(&bytes).with_context(|| format!("{}: the tree", file.display()))?;
        std::fs::write(out, &bytes).with_context(|| out.display().to_string())?;
        out!(
            "{}: {} node(s), {} byte(s)",
            out.display(),
            data.nodes.len(),
            bytes.len()
        );
        return Ok(());
    }
    let game = GameInstance::open(&args.game_dir)?;
    let mut ws = workspace_cmd::open_for_edit(&game, &args.mode)?;
    let catalog = catalog(&game)?;
    let mut streams = Vec::new();
    for path in &args.files {
        let (json, data) = tree_of(path)?;
        let stream = json["stream"]
            .as_str()
            .with_context(|| format!("{}: no `stream`", path.display()))?;
        let d = catalog
            .find(stream)
            .with_context(|| format!("{}: no dialogue {stream}", path.display()))?
            .clone();
        let label = format!("Import dialogue {}", d.name);
        if dialogue::save(&game, &mut ws, &d, &data, &label)? {
            out!("{}: recorded", d.name);
            streams.push(d.name.clone());
        } else {
            out!("{}: no change", d.name);
        }
    }
    if streams.is_empty() {
        return Ok(());
    }
    workspace_cmd::finish_edit(&game, &mut ws, &args.mode, streams)
}

#[derive(Args)]
pub struct SubtitleArgs {
    game_dir: PathBuf,
    /// Speech file in the archives (`audiostreams\b3_xxx_0\d_mob3_xxx_0001806.exa.snu`)
    entry: String,
    /// Set the subtitle of a language column, e.g. `--set en=Hello` (repeatable)
    #[arg(long, value_name = "LANG=TEXT")]
    set: Vec<String>,
    #[command(flatten)]
    mode: EditMode,
}

pub fn subtitle(args: SubtitleArgs) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let key = doctor_toolkit::workspace::FileKey::archive_entry(&args.entry);
    if args.set.is_empty() {
        let ws = workspace(&game, args.mode.workspace_dir.as_deref());
        let data = match &ws {
            Some(ws) => ws.current_file(&game, &key)?,
            None => doctor_toolkit::workspace::files::original_data(game.root(), &key)?,
        };
        let Some(table) = speech::subtitles(&data)? else {
            bail!("{} has no subtitles", key.name());
        };
        for (lang, column) in table.languages.iter().zip(&table.strings) {
            out!("{lang}: {}", column.first().map_or("", String::as_str));
        }
        return Ok(());
    }
    let mut ws = workspace_cmd::open_for_edit(&game, &args.mode)?;
    let mut data = ws.current_file(&game, &key)?;
    let mut table =
        speech::subtitles(&data)?.with_context(|| format!("{} has no subtitles", key.name()))?;
    for set in &args.set {
        let (lang, text) = set
            .split_once('=')
            .with_context(|| format!("`{set}`: LANG=TEXT expected"))?;
        let col = speech::language_column(&table, lang)
            .with_context(|| format!("no `{lang}` column ({})", table.languages.join(" ")))?;
        table.strings[col][0] = text.to_owned();
    }
    data = speech::with_subtitles(&data, &table)?;
    let label = format!(
        "Subtitle of {}",
        key.name().rsplit('\\').next().unwrap_or("")
    );
    ws.set_file(&game, &key, data, &label)?;
    out!("{label}: recorded");
    workspace_cmd::finish_edit(
        &game,
        &mut ws,
        &args.mode,
        vec![doctor_toolkit::workspace::FILES.to_owned()],
    )
}

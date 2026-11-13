//! Navigation commands: the game index, search, references, the name
//! dictionary, `unpack` and `pack`.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::navigation::dictionary::{self, Dictionary, hash_all};
use doctor_toolkit::navigation::index::default_cache_file;
use doctor_toolkit::navigation::unpack::{self, PackOptions, UnpackOptions};
use doctor_toolkit::navigation::{GameIndex, Query, RecordId, search};
use doctor_toolkit::resource::names::type_tags;
use doctor_toolkit::workspace::ResourceKey;

/// Writes a line to stdout and exits quietly when the pipe is closed.
macro_rules! out {
    ($($arg:tt)*) => {
        if writeln!(std::io::stdout(), $($arg)*).is_err() {
            std::process::exit(0);
        }
    };
}

/// Where the game index is cached.
#[derive(Args)]
pub struct IndexArgs {
    /// Index cache file [default: ~/.cache/doctor-toolkit/index-<hash>.bin]
    #[arg(long, value_name = "FILE", conflicts_with = "no_cache")]
    cache: Option<PathBuf>,
    /// Build the index in memory without reading or writing a cache
    #[arg(long)]
    no_cache: bool,
}

#[derive(Subcommand)]
pub enum NavCommand {
    /// Build or update the game index (names and references of every resource) and print statistics
    ///
    /// Other navigation commands do this on their own; the index is cached and
    /// only streams that changed are read again.
    Index {
        game_dir: PathBuf,
        #[command(flatten)]
        index: IndexArgs,
        /// Read every stream again, ignoring the cache
        #[arg(long)]
        rebuild: bool,
        /// Also list the most frequent unresolved references of each type
        #[arg(long, value_name = "N", default_value_t = 0)]
        unresolved: usize,
    },

    /// Search the whole game by name, hash, key, type or text of strings
    ///
    /// QUERY words: a name part (`wheel`) or glob (`car_*_lod?`), a hash
    /// (`0x9cf57f36`), a key (`gf2/frontend:tg4h:…`), `type:mesh,material`,
    /// `stream:mp_mall`, `kind:object`, `text:Don Corleone` (rest of the line).
    Search {
        game_dir: PathBuf,
        #[arg(required = true, num_args = 1..)]
        query: Vec<String>,
        /// At most this many results; 0 for all
        #[arg(long, default_value_t = 50)]
        limit: usize,
        /// Print only the keys, one per line
        #[arg(long)]
        keys: bool,
        #[command(flatten)]
        index: IndexArgs,
    },

    /// Show what a resource uses and what uses it
    ///
    /// RESOURCE is a key (`stream:type:ids`) or a search query with one result.
    Refs {
        game_dir: PathBuf,
        #[arg(required = true, num_args = 1..)]
        resource: Vec<String>,
        /// At most this many entries per list; 0 for all
        #[arg(long, default_value_t = 40)]
        limit: usize,
        #[command(flatten)]
        index: IndexArgs,
    },

    /// Print the hashes of a text under every hash function the engine uses
    Hash {
        #[arg(required = true)]
        text: Vec<String>,
        /// Treat each argument as a 128-bit GUID (32 hex digits) and print its
        /// table-of-contents hash
        #[arg(long)]
        guid: bool,
    },

    /// Find names for 32-bit hashes in the name dictionary
    ///
    /// Without `--game` only the lists in `data/gf2/` are used; with it, every
    /// name of the game and the places the hash occurs.
    Lookup {
        #[arg(required = true)]
        hash: Vec<String>,
        /// Game directory: add the names found in the game files
        #[arg(long, value_name = "GAME_DIR")]
        game: Option<PathBuf>,
        #[command(flatten)]
        index: IndexArgs,
    },

    /// List hashes found in the game without a known name
    UnknownHashes {
        game_dir: PathBuf,
        /// Only hashes of this kind (as printed, e.g. `bone`, `vfx id`)
        #[arg(long)]
        kind: Option<String>,
        /// Print the hashes, at most this many per kind; 0 prints counts only
        #[arg(long, default_value_t = 0)]
        list: usize,
        #[command(flatten)]
        index: IndexArgs,
    },

    /// Write resources as files in common formats (PNG, glTF, JSON, WAV) with a manifest
    ///
    /// Files go to `<output>/<stream>/<type>/<name>.<ext>`; `pack` records the
    /// files edited afterwards in the workspace.
    Unpack {
        game_dir: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
        /// Only this stream, e.g. `gf2/frontend` (repeatable); default: all
        #[arg(long = "stream", value_name = "STREAM")]
        streams: Vec<String>,
        /// Only this type, e.g. `tg4h` (repeatable); a texture needs `tg4h` or `tg4d`
        #[arg(long = "type", value_name = "TYPE")]
        types: Vec<String>,
        /// Textures as DDS instead of PNG
        #[arg(long)]
        dds: bool,
        /// Sounds of banks as WAV instead of .snu
        #[arg(long)]
        wav: bool,
        /// No raw .bin next to views (models, game logic, rigs, sound banks)
        #[arg(long)]
        no_raw: bool,
        /// No views: raw data, textures and strings only
        #[arg(long, conflicts_with = "no_raw")]
        no_views: bool,
        /// Models without materials, textures and skeletons
        #[arg(long)]
        geometry_only: bool,
        /// Model file format: glb, fbx, obj, usd, usda, usdc or usdz; `pack`
        /// imports edited models of any of them back
        #[arg(long, default_value = "glb")]
        model_format: doctor_toolkit::scene::ModelFormat,
        #[command(flatten)]
        index: IndexArgs,
    },

    /// Record the files changed since `unpack` in the workspace
    ///
    /// Unchanged files are skipped. Changed views, missing files and files
    /// not in the manifest are errors, and then nothing is recorded.
    Pack {
        game_dir: PathBuf,
        /// Directory written by `unpack`
        dir: PathBuf,
        /// Workspace directory [default: <GAME_DIR>/_doctor_toolkit/workspace]
        #[arg(long, value_name = "DIR")]
        workspace_dir: Option<PathBuf>,
        /// Only list the changed files
        #[arg(long)]
        dry_run: bool,
    },
}

pub fn run(command: NavCommand) -> Result<()> {
    match command {
        NavCommand::Index {
            game_dir,
            index,
            rebuild,
            unresolved,
        } => index_command(&game_dir, &index, rebuild, unresolved),
        NavCommand::Search {
            game_dir,
            query,
            limit,
            keys,
            index,
        } => search_command(&game_dir, &query.join(" "), limit, keys, &index),
        NavCommand::Refs {
            game_dir,
            resource,
            limit,
            index,
        } => refs_command(&game_dir, &resource.join(" "), limit, &index),
        NavCommand::Hash { text, guid } => hash_command(&text, guid),
        NavCommand::Lookup { hash, game, index } => lookup_command(&hash, game.as_deref(), &index),
        NavCommand::UnknownHashes {
            game_dir,
            kind,
            list,
            index,
        } => unknown_command(&game_dir, kind.as_deref(), list, &index),
        NavCommand::Unpack {
            game_dir,
            output,
            streams,
            types,
            dds,
            wav,
            no_raw,
            no_views,
            geometry_only,
            model_format,
            index,
        } => {
            let options = UnpackOptions {
                dds,
                wav,
                raw: !no_raw,
                views: !no_views,
                model_assets: !geometry_only,
                model_format,
            };
            unpack_command(&game_dir, &output, &streams, &types, options, &index)
        }
        NavCommand::Pack {
            game_dir,
            dir,
            workspace_dir,
            dry_run,
        } => pack_command(&game_dir, &dir, workspace_dir, dry_run),
    }
}

/// Opens the game and its index (cached unless `--no-cache`), with progress on stderr.
fn open_index(
    game_dir: &Path,
    args: &IndexArgs,
    rebuild: bool,
) -> Result<(GameInstance, GameIndex)> {
    let game = GameInstance::open(game_dir)?;
    let cache = if args.no_cache {
        None
    } else {
        args.cache
            .clone()
            .or_else(|| default_cache_file(game.root()))
    };
    if rebuild && let Some(path) = &cache {
        let _ = std::fs::remove_file(path);
    }
    let started = Instant::now();
    let progress = |done: usize, total: usize| {
        if (done.is_multiple_of(100) || done == total) && started.elapsed().as_millis() > 300 {
            eprint!("\rindexing: {done}/{total} streams");
        }
    };
    let (index, report) = GameIndex::open(&game, cache.as_deref(), &progress);
    if report.rebuilt > 0 {
        eprintln!(
            "\rindexed {} resource(s): {} stream(s) read, {} from cache, in {:.1} s",
            index.len(),
            report.rebuilt,
            report.reused,
            started.elapsed().as_secs_f64()
        );
    } else {
        eprint!("\r");
    }
    if let Some(error) = &report.cache_error {
        eprintln!("warning: {error}");
    }
    Ok((game, index))
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

fn index_command(
    game_dir: &Path,
    args: &IndexArgs,
    rebuild: bool,
    unresolved: usize,
) -> Result<()> {
    let started = Instant::now();
    let (game, index) = open_index(game_dir, args, rebuild)?;
    let elapsed = started.elapsed();
    out!(
        "{} resource(s) in {} stream(s), ready in {:.2} s",
        index.len(),
        index.stream_count(),
        elapsed.as_secs_f64()
    );
    if !args.no_cache
        && let Some(path) = args
            .cache
            .clone()
            .or_else(|| default_cache_file(game.root()))
        && let Ok(meta) = std::fs::metadata(&path)
    {
        out!("cache: {} ({})", path.display(), human_size(meta.len()));
    }
    let (mut names, mut refs, mut resolved, mut anchors, mut texts, mut hashes) =
        (0, 0, 0, 0, 0, 0);
    for id in index.ids() {
        let record = index.record(id);
        names += record.names.len();
        anchors += record.anchors.len();
        texts += record.texts.len();
        hashes += record.hashes.len();
        for (_, targets) in index.uses(id) {
            refs += 1;
            resolved += usize::from(!targets.is_empty());
        }
    }
    let stream_errors = (0..index.stream_count())
        .filter(|&s| index.stream_error(s).is_some())
        .count();
    out!(
        "{names} name(s), {texts} string(s), {hashes} hash(es), {anchors} anchor(s), \
         {refs} reference(s), {resolved} resolved ({:.1}%)",
        percent(resolved, refs)
    );
    out!(
        "{} unreadable stream(s), {} resource(s) not parsed",
        stream_errors,
        index.record_errors()
    );
    out!("");
    out!(
        "{:<26} {:>9} {:>9} {:>9} {:>7}",
        "type",
        "resources",
        "refs",
        "resolved",
        "%"
    );
    for stats in index.resolve_stats() {
        if stats.refs == 0 {
            continue;
        }
        out!(
            "{:<26} {:>9} {:>9} {:>9} {:>6.1}%",
            type_tags().display(stats.type_id),
            stats.records,
            stats.refs,
            stats.resolved,
            percent(stats.resolved, stats.refs)
        );
    }
    if unresolved > 0 {
        print_unresolved(&index, unresolved);
    }
    Ok(())
}

/// The most frequent unresolved references per source type and label.
fn print_unresolved(index: &GameIndex, limit: usize) {
    use std::collections::HashMap;
    let mut counts: HashMap<(u32, String), (usize, String)> = HashMap::new();
    for id in index.ids() {
        let type_id = index.record(id).header.type_id;
        for (reference, targets) in index.uses(id) {
            if targets.is_empty() {
                let kind = match &reference.target {
                    doctor_toolkit::navigation::Target::Resource { type_id, .. } => {
                        type_tags().display(*type_id)
                    }
                    other => other.describe().split(' ').next().unwrap_or("").to_owned(),
                };
                let entry = counts
                    .entry((type_id, format!("{} -> {kind}", reference.label)))
                    .or_insert((0, reference.target.describe()));
                entry.0 += 1;
            }
        }
    }
    let mut list: Vec<_> = counts.into_iter().collect();
    list.sort_by(|a, b| a.0.0.cmp(&b.0.0).then(b.1.0.cmp(&a.1.0)));
    let mut shown: HashMap<u32, usize> = HashMap::new();
    out!("");
    out!("unresolved references (source type, label -> target type, count, example):");
    for ((type_id, label), (count, example)) in list {
        let n = shown.entry(type_id).or_default();
        if *n < limit {
            *n += 1;
            out!(
                "{:<12} {label:<48} {count:>7}  {example}",
                type_tags().display(type_id)
            );
        }
    }
}

fn percent(part: usize, total: usize) -> f64 {
    if total == 0 {
        100.0
    } else {
        part as f64 * 100.0 / total as f64
    }
}

/// `stream:type:ids  name` of a record.
fn describe(index: &GameIndex, id: RecordId) -> String {
    let record = index.record(id);
    match record.name() {
        Some(name) => format!("{}  `{name}`", index.key(id)),
        None => index.key(id).to_string(),
    }
}

fn search_command(
    game_dir: &Path,
    line: &str,
    limit: usize,
    keys: bool,
    args: &IndexArgs,
) -> Result<()> {
    let query = Query::parse(line)?;
    if query.is_empty() {
        bail!("empty search");
    }
    let (_, index) = open_index(game_dir, args, false)?;
    if let Some(hash) = query.hash
        && !keys
    {
        let dict = Dictionary::for_game(&index);
        for (name, origin) in dict.lookup(hash) {
            out!("{hash:08x} = {name:?} ({})", origin.describe());
        }
    }
    let started = Instant::now();
    let (results, total) = search::search(&index, &query, limit);
    let elapsed = started.elapsed();
    for result in &results {
        if keys {
            out!("{}", index.key(result.record));
            continue;
        }
        out!("{}", describe(&index, result.record));
        for hit in &result.hits {
            out!("    {}", hit.describe());
        }
    }
    if !keys {
        out!(
            "{total} resource(s){} in {:.0} ms",
            if total > results.len() {
                format!(", {} shown (--limit)", results.len())
            } else {
                String::new()
            },
            elapsed.as_secs_f64() * 1000.0
        );
    }
    Ok(())
}

/// The record a key or a one-result query names.
fn find_one(index: &GameIndex, text: &str) -> Result<RecordId> {
    if let Ok(key) = text.parse::<ResourceKey>() {
        return index
            .find_key(&key)
            .with_context(|| format!("no resource {key}"));
    }
    let query = Query::parse(text)?;
    let (results, total) = search::search(index, &query, 20);
    match total {
        0 => bail!("nothing matches `{text}`"),
        1 => Ok(results[0].record),
        _ => {
            let shown: Vec<String> = results.iter().map(|r| describe(index, r.record)).collect();
            bail!(
                "{total} resources match `{text}`; give a key:\n  {}",
                shown.join("\n  ")
            )
        }
    }
}

fn refs_command(game_dir: &Path, text: &str, limit: usize, args: &IndexArgs) -> Result<()> {
    let (_, index) = open_index(game_dir, args, false)?;
    let id = find_one(&index, text)?;
    let record = index.record(id);
    let stream = index.stream_of(id);
    out!("{}", describe(&index, id));
    out!(
        "    {} resource {}, {} byte(s)",
        index.stream_path(stream),
        record.resource,
        record.header.data_size
    );
    if let Some(error) = &record.error {
        out!("    not parsed: {error}");
    }
    let cap = |n: usize| if limit == 0 { n } else { n.min(limit) };
    let uses: Vec<_> = index.uses(id).collect();
    out!("uses ({}):", uses.len());
    for (reference, targets) in uses.iter().take(cap(uses.len())) {
        match targets.len() {
            0 => out!(
                "    {} -> {} (not found)",
                reference.label,
                reference.target.describe()
            ),
            n => {
                let more = if n > 1 {
                    format!(" (+{} more)", n - 1)
                } else {
                    String::new()
                };
                out!(
                    "    {} -> {}{more}",
                    reference.label,
                    describe(&index, targets[0])
                );
            }
        }
    }
    let used_by = index.used_by(id);
    out!("used by ({}):", used_by.len());
    for &(source, k) in used_by.iter().take(cap(used_by.len())) {
        let label = &index.record(source).refs[k as usize].label;
        out!("    {}  ({label})", describe(&index, source));
    }
    let copies = index.copies(id);
    if !copies.is_empty() {
        out!("also in ({}):", copies.len());
        for &copy in copies.iter().take(cap(copies.len())) {
            out!("    {}", index.key(copy));
        }
    }
    Ok(())
}

fn hash_command(texts: &[String], guid: bool) -> Result<()> {
    for text in texts {
        if guid {
            let hex: String = text.chars().filter(|c| *c != '-').collect();
            if hex.len() != 32 {
                bail!("`{text}` is not 32 hexadecimal digits");
            }
            let mut bytes = [0u8; 16];
            for (i, byte) in bytes.iter_mut().enumerate() {
                *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16)
                    .with_context(|| format!("`{text}` is not hexadecimal"))?;
            }
            out!("{text}: toc {:08x}", doctor_toolkit::toc::guid_hash(&bytes));
            continue;
        }
        let list: Vec<String> = hash_all(text)
            .into_iter()
            .map(|(f, h)| format!("{} {h:08x}", f.name()))
            .collect();
        out!("{text:?}: {}", list.join(", "));
    }
    Ok(())
}

fn parse_hash(text: &str) -> Result<u32> {
    let digits = text.strip_prefix("0x").unwrap_or(text);
    u32::from_str_radix(digits, 16).with_context(|| format!("`{text}` is not a hexadecimal hash"))
}

fn lookup_command(hashes: &[String], game: Option<&Path>, args: &IndexArgs) -> Result<()> {
    let hashes: Vec<u32> = hashes
        .iter()
        .map(|h| parse_hash(h))
        .collect::<Result<_>>()?;
    let index = match game {
        Some(dir) => Some(open_index(dir, args, false)?.1),
        None => None,
    };
    let dict = match &index {
        Some(index) => Dictionary::for_game(index),
        None => Dictionary::builtin(),
    };
    for hash in hashes {
        let names = dict.lookup(hash);
        if names.is_empty() {
            out!("{hash:08x}: no name known");
        }
        for (name, origin) in names {
            out!("{hash:08x} = {name:?} ({})", origin.describe());
        }
        if let Some(index) = &index {
            let query = Query {
                hash: Some(hash),
                ..Query::default()
            };
            let (results, total) = search::search(index, &query, 10);
            for result in &results {
                let hits: Vec<String> = result.hits.iter().map(|h| h.describe()).collect();
                out!(
                    "    {}: {}",
                    describe(index, result.record),
                    hits.join("; ")
                );
            }
            if total > results.len() {
                out!(
                    "    … {} more; `search {hash:08x} --limit 0` lists all",
                    total - results.len()
                );
            }
        }
    }
    Ok(())
}

fn unknown_command(
    game_dir: &Path,
    kind: Option<&str>,
    list: usize,
    args: &IndexArgs,
) -> Result<()> {
    let (_, index) = open_index(game_dir, args, false)?;
    let dict = Dictionary::for_game(&index);
    let unknown = dictionary::unknown_hashes(&index, &dict);
    out!(
        "dictionary: {} name(s), {} entries",
        dict.name_count(),
        dict.len()
    );
    let mut kinds: Vec<(&str, usize, usize)> = Vec::new();
    for u in &unknown {
        if kind.is_some_and(|k| k != u.kind) {
            continue;
        }
        match kinds.last_mut() {
            Some((k, distinct, total)) if *k == u.kind => {
                *distinct += 1;
                *total += u.count;
            }
            _ => kinds.push((&u.kind, 1, u.count)),
        }
    }
    for (k, distinct, total) in &kinds {
        out!("{k}: {distinct} unknown hash(es) in {total} place(s)");
        if list > 0 {
            for u in unknown.iter().filter(|u| u.kind == *k).take(list) {
                out!("    {:08x}  {}", u.hash, u.count);
            }
        }
    }
    Ok(())
}

fn unpack_command(
    game_dir: &Path,
    output: &Path,
    streams: &[String],
    types: &[String],
    options: UnpackOptions,
    args: &IndexArgs,
) -> Result<()> {
    let types: Vec<u32> = types
        .iter()
        .map(|t| {
            type_tags()
                .resolve(t)
                .with_context(|| format!("unknown type `{t}`"))
        })
        .collect::<Result<_>>()?;
    let (game, index) = open_index(game_dir, args, false)?;
    let selection = unpack::select(&game, streams, &types)?;
    let started = Instant::now();
    let progress = |done: usize, total: usize| {
        if done.is_multiple_of(500) || done == total {
            eprint!("\runpacking: {done}/{total} resources");
        }
    };
    let cancel = AtomicBool::new(false);
    let report = unpack::unpack(
        &game,
        Some(&index),
        &selection,
        output,
        options,
        &progress,
        &cancel,
    )?;
    eprintln!();
    for failure in report.failed.iter().take(20) {
        out!("warning: {failure}");
    }
    if report.failed.len() > 20 {
        out!("warning: … {} more", report.failed.len() - 20);
    }
    out!(
        "unpacked {} resource(s) into {} file(s), {}, in {:.1} s; manifest {}",
        report.resources,
        report.files,
        human_size(report.bytes),
        started.elapsed().as_secs_f64(),
        output.join(unpack::MANIFEST).display()
    );
    Ok(())
}

fn pack_command(
    game_dir: &Path,
    dir: &Path,
    workspace_dir: Option<PathBuf>,
    dry_run: bool,
) -> Result<()> {
    let game = GameInstance::open(game_dir)?;
    let options = PackOptions {
        workspace_dir,
        dry_run,
    };
    let report = unpack::pack(&game, dir, &options)?;
    for warning in &report.warnings {
        out!("warning: {warning}");
    }
    for file in &report.changed {
        let keys: Vec<String> = file.keys.iter().map(ToString::to_string).collect();
        let state = if dry_run {
            "would record"
        } else if file.recorded {
            "recorded"
        } else {
            "no change"
        };
        out!("{state}: {} ({})", file.path, keys.join(", "));
    }
    out!(
        "{} changed file(s), {} unchanged",
        report.changed.len(),
        report.unchanged
    );
    if let Some(ws) = &report.workspace {
        out!(
            "recorded in workspace {}; run `doctor-toolkit workspace apply` to write it into the game",
            ws.display()
        );
    }
    Ok(())
}

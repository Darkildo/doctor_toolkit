//! `game-types`, `types-gen` and `logic-check`: the typed game data of
//! `data/gf2/` (value lists, meanings of attributes, messages) and the check
//! of the messages a workspace adds. See `docs/logic-graph.md` §2.1.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};
use clap::{Args, Subcommand};

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::game_types::{
    self, EnumKind, MessageCatalog, MessageId, SemanticType, export, message,
};
use doctor_toolkit::hash::string_id;
use doctor_toolkit::logic_graph::{LogicSchema, check};
use doctor_toolkit::workspace::Workspace;

macro_rules! out {
    ($($arg:tt)*) => {
        if writeln!(std::io::stdout(), $($arg)*).is_err() {
            std::process::exit(0);
        }
    };
}

#[derive(Subcommand)]
pub enum TypesCommand {
    /// Value lists: families, cities, venues, weapons, crew specialties,
    /// sound categories, emotions
    Enums {
        /// Key of one list (`family`); without it, every list with its size
        key: Option<String>,
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// What is known about a message: where the name comes from, who sends
    /// and hears it in the game files, its data
    Message {
        /// A name, or a hash as `0x` and eight hexadecimal digits
        name: String,
        /// List the messages whose name contains NAME
        #[arg(long)]
        search: bool,
        /// At most this many names with `--search`
        #[arg(long, default_value_t = 40)]
        limit: usize,
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// The meanings of the attributes of a class packet
    Attrs {
        /// Class name (`Building`) or eight hexadecimal digits of its hash
        class: String,
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// Counts: messages by group, typed attributes, lists
    Stats {
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args)]
pub struct TypesGen {
    /// The repository root [default: the nearest folder above the current
    /// one that has data/gf2]
    #[arg(long, value_name = "DIR")]
    pub root: Option<PathBuf>,
    /// Write nothing: fail when a run would change a file
    #[arg(long)]
    pub check: bool,
}

#[derive(Args)]
pub struct LogicCheck {
    pub game_dir: PathBuf,
    /// Print JSON
    #[arg(long)]
    pub json: bool,
}

fn conf_name(conf: doctor_toolkit::logic_graph::Confidence) -> &'static str {
    use doctor_toolkit::logic_graph::Confidence as C;
    match conf {
        C::Exe => "exe",
        C::Data => "data",
        C::Guess => "guess",
    }
}

fn enums(key: Option<&str>, json: bool) -> Result<()> {
    let Some(key) = key else {
        if json {
            out!("{}", serde_json::to_string_pretty(game_types::enums())?);
            return Ok(());
        }
        for e in game_types::enums() {
            out!(
                "{:16} {:4} value(s)  {:5} {:5}  {}",
                e.key,
                e.values.len(),
                e.kind.name(),
                conf_name(e.conf),
                e.title
            );
        }
        return Ok(());
    };
    let e = game_types::enum_def(key).ok_or_else(|| {
        anyhow!(
            "no list `{key}`; lists: {}",
            game_types::enums()
                .iter()
                .map(|e| e.key)
                .collect::<Vec<_>>()
                .join(", ")
        )
    })?;
    if json {
        out!("{}", serde_json::to_string_pretty(e)?);
        return Ok(());
    }
    out!("{} — {} ({})", e.key, e.title, conf_name(e.conf));
    out!("stored as: {}", e.kind.name());
    out!("source: {}", e.source);
    for v in e.values {
        let name = v.name.unwrap_or("—");
        let number = match (v.value, e.kind) {
            (Some(x), EnumKind::Bits) => format!("{x:#04x}"),
            (Some(x), _) => x.to_string(),
            (None, _) => String::new(),
        };
        let hash = v.hash.map(|h| format!("{h:08x}")).unwrap_or_default();
        out!("  {name:28} {number:>7} {hash:8}  {}", v.note);
    }
    Ok(())
}

fn parse_message(text: &str) -> MessageId {
    match text
        .strip_prefix("0x")
        .filter(|h| h.len() == 8)
        .and_then(|h| u32::from_str_radix(h, 16).ok())
    {
        Some(hash) => MessageId::from_hash(hash),
        None => MessageId::from_name(text),
    }
}

fn pin_uses(uses: &[game_types::PinUse]) -> String {
    if uses.is_empty() {
        return "nobody".to_owned();
    }
    let mut sorted: Vec<_> = uses.iter().collect();
    sorted.sort_by(|a, b| b.count.cmp(&a.count).then(a.class.cmp(b.class)));
    sorted
        .iter()
        .map(|u| format!("{}.{} ×{}", u.class, u.index, u.count))
        .collect::<Vec<_>>()
        .join(", ")
}

fn message_cmd(name: &str, search: bool, limit: usize, json: bool) -> Result<()> {
    if search {
        let found = MessageCatalog::gf2().search(name, limit);
        if json {
            let infos: Vec<_> = found
                .iter()
                .map(|m| MessageId::from_hash(m.hash).info())
                .collect();
            out!("{}", serde_json::to_string_pretty(&infos)?);
            return Ok(());
        }
        for m in found {
            out!(
                "{:48} {:6} {:4} sender attr(s) {:4} listener attr(s)  {}",
                m.name,
                MessageId::from_hash(m.hash).kind(),
                m.senders().len(),
                m.listeners().len(),
                m.scope
            );
        }
        return Ok(());
    }
    let id = parse_message(name);
    let info = id.info();
    if json {
        out!("{}", serde_json::to_string_pretty(&info)?);
        return Ok(());
    }
    out!(
        "{} (sdbm {:08x}): {}",
        info.name.as_deref().unwrap_or("?"),
        info.hash,
        match &id {
            MessageId::Engine(_) => "engine — named by godfather2.exe",
            MessageId::Game(_) => "game — named by the SimGroups of the game",
            MessageId::Custom { .. } => "custom — not a message of the game",
            MessageId::Unknown(_) => "unknown — no name for this hash",
        }
    );
    if let MessageId::Custom { name, .. } = &id {
        match message::collision(name) {
            Some(other) => out!(
                "COLLISION: the hash is the one of the game message `{other}`; the game cannot tell them apart"
            ),
            None => out!("its hash is free: no game message has it"),
        }
        return Ok(());
    }
    out!(
        "usage: {}",
        serde_json::to_value(info.usage)?.as_str().unwrap_or("")
    );
    out!("sent by:  {}", pin_uses(&info.senders));
    out!("heard by: {}", pin_uses(&info.listeners));
    if let Some(scope) = info.scope {
        out!("scope: {scope} ({} stream(s))", info.streams);
    }
    if !info.mentions.is_empty() {
        out!(
            "also named by: {}",
            info.mentions
                .iter()
                .map(|(l, n)| format!("{l} ({n})"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    for s in &info.sent_by {
        out!("sent besides SimGroups by: {s}");
    }
    if info.engine_listens > 0 {
        out!(
            "heard by engine code: {} place(s) subscribe to it",
            info.engine_listens
        );
    }
    if let Some(s) = info.summary {
        out!("what: {s}");
    }
    match (info.data_conf, info.data_source) {
        (Some(conf), Some(source)) => out!("data: {} ({conf}; {source})", info.data),
        _ => out!("data: {}", info.data),
    }
    for key in info.data.split(" | ") {
        if let Some(t) = game_types::message::data_type(key) {
            out!("  {key}: {}", t.summary);
        }
    }
    out!(
        "confidence: {} ({})",
        info.conf.unwrap_or("—"),
        info.source.unwrap_or("—")
    );
    Ok(())
}

fn attrs(class: &str, json: bool) -> Result<()> {
    let schema = LogicSchema::gf2();
    let hash = match u32::from_str_radix(class, 16) {
        Ok(h) if class.len() == 8 => h,
        _ => string_id(class),
    };
    let mut indexes: Vec<u16> = schema.packet(hash).iter().map(|a| a.index).collect();
    indexes.extend(
        game_types::attr_types()
            .iter()
            .filter(|r| r.0 == hash)
            .map(|r| r.1),
    );
    indexes.sort_unstable();
    indexes.dedup();
    let rows: Vec<_> = indexes
        .into_iter()
        .filter_map(|i| {
            let (ty, conf) = schema.semantic(hash, i)?;
            let extra = game_types::attr_type(hash, i);
            Some((i, ty, conf, extra))
        })
        .collect();
    if rows.is_empty() {
        bail!("no typed attribute of the packet of `{class}` (hash {hash:08x})");
    }
    if json {
        let list: Vec<_> = rows
            .iter()
            .map(|(i, ty, conf, extra)| {
                serde_json::json!({
                    "index": i,
                    "name": schema.attr(hash, *i).and_then(|a| a.name.clone()),
                    "type": ty,
                    "conf": conf,
                    "seen": extra.map(|e| e.seen),
                    "source": extra.map(|e| e.source).filter(|s| !s.is_empty()),
                })
            })
            .collect();
        out!("{}", serde_json::to_string_pretty(&list)?);
        return Ok(());
    }
    out!(
        "{} (hash {hash:08x})",
        schema.class_name(hash).unwrap_or(class)
    );
    for (i, ty, conf, extra) in rows {
        let name = schema
            .attr(hash, i)
            .and_then(|a| a.name.clone())
            .unwrap_or_default();
        let seen = extra
            .map(|e| format!("{}/{}", e.seen.0, e.seen.1))
            .unwrap_or_default();
        out!(
            "  {i:3} {name:16} {:28} {:5} {seen}",
            ty.to_string(),
            conf_name(conf)
        );
    }
    Ok(())
}

fn stats(json: bool) -> Result<()> {
    let catalog = MessageCatalog::gf2();
    let engine = message::engine_messages();
    let engine_in_files = engine
        .iter()
        .filter(|(h, _)| catalog.get(*h).is_some())
        .count();
    let mut by_type: std::collections::BTreeMap<&str, usize> = Default::default();
    let mut by_conf: std::collections::BTreeMap<&str, usize> = Default::default();
    for (_, _, t) in game_types::attr_types() {
        let name = match t.ty {
            SemanticType::Ref(_) => "ref",
            SemanticType::Resource(_) => "res",
            SemanticType::Enum(_) => "enum",
            SemanticType::Seconds => "seconds",
            SemanticType::Money => "money",
            SemanticType::Bool => "bool",
            SemanticType::MsgIn | SemanticType::MsgOut => "message",
        };
        *by_type.entry(name).or_default() += 1;
        *by_conf.entry(conf_name(t.conf)).or_default() += 1;
    }
    let coverage = LogicSchema::gf2().coverage();
    let msg_in: usize = coverage.inputs.iter().sum();
    let msg_out: usize = coverage.outputs.iter().sum();
    let value = serde_json::json!({
        "messages": {
            "engine": engine.len(),
            "engine_used_by_simgroups": engine_in_files,
            "game": catalog.len() - engine_in_files,
            "catalog": catalog.len(),
            "described": message::notes().len(),
        },
        "attributes": {
            "typed": game_types::attr_types().len(),
            "by_type": by_type,
            "by_conf": by_conf,
            "msg_in": msg_in,
            "msg_out": msg_out,
        },
        "enums": game_types::enums().iter().map(|e| (e.key, e.values.len())).collect::<std::collections::BTreeMap<_, _>>(),
    });
    if json {
        out!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    out!(
        "messages: {} named by the exe (Engine; {engine_in_files} of them used by SimGroups), {} named only by the game files (Game), {} in the catalogue, {} described by hand",
        engine.len(),
        catalog.len() - engine_in_files,
        catalog.len(),
        message::notes().len()
    );
    out!(
        "attributes: {msg_in} MsgIn and {msg_out} MsgOut pins; {} more typed:",
        game_types::attr_types().len()
    );
    for (k, n) in &by_type {
        out!("  {k:8} {n}");
    }
    out!(
        "  by confidence: {}",
        by_conf
            .iter()
            .map(|(k, n)| format!("{k} {n}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    out!("lists:");
    for e in game_types::enums() {
        out!("  {:16} {}", e.key, e.values.len());
    }
    Ok(())
}

pub fn run(command: TypesCommand) -> Result<()> {
    match command {
        TypesCommand::Enums { key, json } => enums(key.as_deref(), json),
        TypesCommand::Message {
            name,
            search,
            limit,
            json,
        } => message_cmd(&name, search, limit, json),
        TypesCommand::Attrs { class, json } => attrs(&class, json),
        TypesCommand::Stats { json } => stats(json),
    }
}

/// Marker of the repository root for `types-gen`.
const DATA: &str = "data/gf2";

fn root(given: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(root) = given {
        return Ok(root);
    }
    let start = std::env::current_dir()?;
    let mut dir: &Path = &start;
    loop {
        if dir.join(DATA).is_dir() {
            return Ok(dir.to_owned());
        }
        dir = dir.parent().ok_or_else(|| {
            anyhow!(
                "no {DATA} here or above {}; give the repository with --root",
                start.display()
            )
        })?;
    }
}

/// `types-gen`: the JSON Schema and the Python module of the typed data.
pub fn generate(args: TypesGen) -> Result<()> {
    let root = root(args.root)?;
    let changed = export::write(&root, args.check)?;
    for path in &changed {
        out!(
            "{} {}",
            if args.check { "stale" } else { "wrote" },
            path.strip_prefix(&root).unwrap_or(path).display()
        );
    }
    out!(
        "{} generated file(s): {} changed",
        export::files().len(),
        changed.len()
    );
    if args.check && !changed.is_empty() {
        bail!("the generated files are out of date; run `doctor-toolkit types-gen`");
    }
    Ok(())
}

/// `logic-check`: the messages of the SimGroups the workspace changes.
pub fn logic_check(args: &LogicCheck) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let dir = Workspace::default_dir(game.root());
    if !Workspace::exists(&dir) {
        if args.json {
            out!(
                "{}",
                serde_json::json!({ "ok": true, "note": "no workspace" })
            );
        } else {
            out!("there is no workspace: nothing to check");
        }
        return Ok(());
    }
    let ws = Workspace::open(&dir)?;
    let report = check::check_workspace(&ws);
    let errors = report.errors().count();
    if args.json {
        let mut value = serde_json::to_value(&report)?;
        value["ok"] = (errors == 0).into();
        out!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        out!(
            "{} changed SimGroup(s), {} new message(s)",
            report.groups,
            report.custom.len()
        );
        for name in &report.custom {
            out!("  new: {name} (sdbm {:08x})", string_id(name));
        }
        for (key, error) in &report.unreadable {
            out!("unreadable {key}: {error}");
        }
        for p in &report.problems {
            out!(
                "{}: {} [{}]",
                match p.severity {
                    check::Severity::Error => "error",
                    check::Severity::Warning => "warning",
                },
                p.what,
                p.groups.join(", ")
            );
        }
        out!(
            "{errors} error(s), {} warning(s)",
            report.warnings().count()
        );
    }
    if errors > 0 {
        bail!("{errors} message name(s) collide with messages of the game");
    }
    Ok(())
}

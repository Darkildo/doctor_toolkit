//! The logic graph of missions: `logic-graph`, `messages`, `logic-missions`,
//! `logic-schema`.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use clap::Args;

use doctor_toolkit::game::{GameInstance, ResourceIndex};
use doctor_toolkit::logic_graph::{
    BuildOptions, LogicSchema, MessageIndex, MessageState, ScopeSpec, index::Site, scope,
};
use doctor_toolkit::workspace::Workspace;

macro_rules! out {
    ($($arg:tt)*) => {
        if writeln!(std::io::stdout(), $($arg)*).is_err() {
            std::process::exit(0);
        }
    };
}

#[derive(Args)]
pub struct GraphArgs {
    pub game_dir: PathBuf,
    /// `mission:<name>`, `checkpoint:<number>`, `stream:<stream>`,
    /// `group:<stream>#<resource>`, `level:<stream>` (with its ancestors) or
    /// `game`; a bare name is a stream or a mission
    pub scope: String,
    /// Print the graph in the DOT language of Graphviz
    #[arg(long, conflicts_with = "json")]
    pub dot: bool,
    /// Print the graph as JSON
    #[arg(long)]
    pub json: bool,
    /// Keep objects without messages or links
    #[arg(long)]
    pub all: bool,
    /// Draw every wire, never through a message hub node
    #[arg(long)]
    pub no_hubs: bool,
    /// Read the game files only, without the edits of the workspace
    #[arg(long)]
    pub original: bool,
}

#[derive(Args)]
pub struct MessagesArgs {
    pub game_dir: PathBuf,
    /// Message name; with `--search`, a part of names
    pub name: String,
    /// List messages whose name contains NAME
    #[arg(long)]
    pub search: bool,
    /// Print JSON
    #[arg(long)]
    pub json: bool,
    /// Read the game files only, without the edits of the workspace
    #[arg(long)]
    pub original: bool,
}

#[derive(Args)]
pub struct MissionsArgs {
    pub game_dir: PathBuf,
    /// Print JSON
    #[arg(long)]
    pub json: bool,
}

fn open(
    game_dir: &Path,
    original: bool,
) -> Result<(GameInstance, ResourceIndex, Option<Workspace>)> {
    let game = GameInstance::open(game_dir)?;
    let (index, errors) = game.build_index(|_| {});
    if let Some((stream, error)) = errors.first() {
        eprintln!(
            "warning: {} unreadable stream(s), first {}: {error}",
            errors.len(),
            game.streams()[*stream].rel_path
        );
    }
    let dir = Workspace::default_dir(game.root());
    let ws = (!original && Workspace::exists(&dir))
        .then(|| Workspace::open(&dir))
        .transpose()?;
    Ok((game, index, ws))
}

/// `logic-graph`: the graph of a scope.
pub fn graph(args: &GraphArgs) -> Result<()> {
    let (game, index, ws) = open(&args.game_dir, args.original)?;
    let spec = ScopeSpec::parse(&game, &args.scope)?;
    let options = BuildOptions {
        hub_threshold: if args.no_hubs { 0 } else { 48 },
        keep_unwired: args.all,
        ..BuildOptions::default()
    };
    let (mut graph, errors) = scope::build(&game, &index, ws.as_ref(), &spec, options)?;
    if !args.dot {
        graph.annotate(&MessageIndex::build(&game, &index, ws.as_ref()));
    }
    for (l, e) in &errors {
        eprintln!(
            "warning: {} #{}: {e}",
            game.streams()[l.stream].stream_name(),
            l.resource
        );
    }
    if args.dot {
        print!("{}", graph.to_dot());
        return Ok(());
    }
    if args.json {
        out!("{}", serde_json::to_string_pretty(&graph)?);
        return Ok(());
    }
    let o = graph.overview();
    out!("{}", spec.label(&game));
    out!(
        "{} SimGroup(s), {} node(s), {} wire(s), {} message(s)",
        o.groups,
        o.nodes,
        o.wires,
        o.messages
    );
    for (state, n) in &o.states {
        out!("  {state:12} {n}");
    }
    out!("nodes by category:");
    for (cat, n) in &o.categories {
        out!("  {cat:12} {n}");
    }
    let dangling: Vec<_> = graph.dangling().collect();
    if !dangling.is_empty() {
        out!(
            "dangling messages ({}, {} of them nowhere else in the game):",
            dangling.len(),
            o.dangling_in_game
        );
        for m in dangling.iter().take(40) {
            let (s, l) = m.elsewhere.unwrap_or_default();
            let what = if m.state == MessageState::NeverSent {
                if s == 0 {
                    "heard, sent nowhere in the game".to_owned()
                } else {
                    format!("heard, sent elsewhere ({s})")
                }
            } else if l == 0 {
                "sent, heard nowhere in the game".to_owned()
            } else {
                format!("sent, heard elsewhere ({l})")
            };
            out!("  {:44} {what}", m.name);
        }
        if dangling.len() > 40 {
            out!("  … and {} more (--json lists all)", dangling.len() - 40);
        }
    }
    Ok(())
}

fn print_site(index: &MessageIndex, s: &Site) {
    let name = if s.name.is_empty() {
        String::new()
    } else {
        format!(" \"{}\"", s.name)
    };
    out!(
        "  {} #{} object {:5} {}{name}.{}",
        index.streams[s.group.stream],
        s.group.resource,
        s.object,
        s.class_name,
        s.pin
    );
}

/// `messages`: who sends and who hears a message, across the game.
pub fn messages(args: &MessagesArgs) -> Result<()> {
    let (game, index, ws) = open(&args.game_dir, args.original)?;
    let messages = MessageIndex::build(&game, &index, ws.as_ref());
    let found: Vec<_> = if args.search {
        messages.search(&args.name)
    } else {
        messages.who(&args.name).into_iter().collect()
    };
    if args.json {
        out!("{}", serde_json::to_string_pretty(&found)?);
        return Ok(());
    }
    if found.is_empty() {
        let engine = LogicSchema::gf2()
            .engine_message(doctor_toolkit::hash::string_id(&args.name))
            .is_some();
        if engine {
            out!(
                "{}: no SimGroup object sends or hears it; the engine names it",
                args.name
            );
            return Ok(());
        }
        bail!("no SimGroup object sends or hears `{}`", args.name);
    }
    if args.search {
        for m in &found {
            out!(
                "{:48} {:4} sender(s) {:4} listener(s)",
                m.name,
                m.senders.len(),
                m.listeners.len()
            );
        }
        return Ok(());
    }
    let m = found[0];
    let engine = LogicSchema::gf2()
        .engine_message(doctor_toolkit::hash::string_id(&m.name))
        .is_some();
    out!(
        "{} (sdbm {:08x}){}",
        m.name,
        doctor_toolkit::hash::string_id(&m.name),
        if engine { ", named by the engine" } else { "" }
    );
    out!("sent by {}:", m.senders.len());
    for s in &m.senders {
        print_site(&messages, s);
    }
    out!("heard by {}:", m.listeners.len());
    for s in &m.listeners {
        print_site(&messages, s);
    }
    Ok(())
}

/// `logic-missions`: missions and their streams.
pub fn missions(args: &MissionsArgs) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let missions = scope::missions(&game);
    if args.json {
        out!("{}", serde_json::to_string_pretty(&missions)?);
        return Ok(());
    }
    for m in &missions {
        out!("{:56} {:3} stream(s)", m.name, m.streams.len());
    }
    out!("{} mission(s)", missions.len());
    Ok(())
}

/// `logic-schema`: what the schema of SimGroup pins covers.
pub fn schema(json: bool) -> Result<()> {
    let s = LogicSchema::gf2();
    let c = s.coverage();
    if json {
        out!("{}", serde_json::to_string_pretty(&c)?);
        return Ok(());
    }
    out!(
        "{} object classes, {} class packets with pins",
        c.classes,
        c.packets
    );
    out!("pins by confidence:     exe   data  other");
    for (name, row) in [
        ("in (heard)", c.inputs),
        ("out (sent)", c.outputs),
        ("link", c.links),
        ("asset", c.assets),
        ("param (named)", c.params),
    ] {
        out!("  {name:16} {:6} {:6} {:6}", row[0], row[1], row[2]);
    }
    out!("{} attributes named", c.named);
    Ok(())
}

#[derive(Args)]
pub struct ConnectArgs {
    pub game_dir: PathBuf,
    /// The sender: `<stream>#<resource>:<object>:<pin>` (pin name as in
    /// `logic-graph --json`, e.g. `send_2`, `on_enter`)
    #[arg(long)]
    pub from: String,
    /// The listener, written the same way
    #[arg(long)]
    pub to: String,
    /// Remove the wire between the two pins instead
    #[arg(long)]
    pub disconnect: bool,
    #[command(flatten)]
    pub mode: crate::workspace_cmd::EditMode,
}

/// `<stream>#<resource>:<object>:<pin>`.
fn parse_pin(
    game: &GameInstance,
    text: &str,
) -> Result<(doctor_toolkit::level::Location, usize, String)> {
    let mut parts = text.rsplitn(3, ':');
    let (Some(pin), Some(object), Some(group)) = (parts.next(), parts.next(), parts.next()) else {
        bail!("`{text}`: expected <stream>#<resource>:<object>:<pin>");
    };
    let ScopeSpec::Group(location) = ScopeSpec::parse(game, &format!("group:{group}"))? else {
        bail!("`{group}` is not <stream>#<resource>");
    };
    let object = object
        .parse()
        .map_err(|_| anyhow::anyhow!("`{object}` is not an object number"))?;
    Ok((location, object, pin.to_owned()))
}

/// `logic-connect`: wire a sender to a listener (sets message attributes).
pub fn connect(args: &ConnectArgs) -> Result<()> {
    use doctor_toolkit::logic_graph::{GraphBuilder, PinRef, edit};
    let game = GameInstance::open(&args.game_dir)?;
    let (index, _) = game.build_index(|_| {});
    let mut ws = crate::workspace_cmd::open_for_edit(&game, &args.mode)?;
    let (from_group, from_object, from_pin) = parse_pin(&game, &args.from)?;
    let (to_group, to_object, to_pin) = parse_pin(&game, &args.to)?;
    let mut groups = vec![from_group];
    if to_group != from_group {
        groups.push(to_group);
    }
    let (loaded, errors) = scope::load(&game, Some(&ws), &groups);
    if let Some((_, e)) = errors.into_iter().next() {
        return Err(e.into());
    }
    let mut builder = GraphBuilder::new(LogicSchema::gf2());
    for g in &loaded {
        builder.add_group(g.location, &g.stream, &g.group);
    }
    let graph = builder.build(BuildOptions {
        hub_threshold: 0,
        keep_unwired: true,
        ..BuildOptions::default()
    });
    let find = |group, object, pin: &str| -> Result<PinRef> {
        let node = graph
            .node_of(group, object)
            .ok_or_else(|| anyhow::anyhow!("no object {object} in that SimGroup"))?;
        let pins = &graph.nodes[node].pins;
        let p = pins.iter().position(|p| p.name == pin).ok_or_else(|| {
            let names: Vec<&str> = pins.iter().map(|p| p.name.as_str()).collect();
            anyhow::anyhow!(
                "{} has no pin `{pin}`; pins: {}",
                graph.nodes[node].class_name,
                names.join(", ")
            )
        })?;
        Ok(PinRef { node, pin: p })
    };
    let a = find(from_group, from_object, &from_pin)?;
    let b = find(to_group, to_object, &to_pin)?;
    let changes = if args.disconnect {
        let w = graph
            .wires
            .iter()
            .position(|w| w.from == a && w.to == b)
            .ok_or_else(|| anyhow::anyhow!("no wire between these pins"))?;
        edit::plan_disconnect(&graph, w)?
    } else {
        edit::plan_connect(&graph, a, b, || {
            let base = format!("dtk_{}_{from_pin}", graph.nodes[a.node].class_name);
            edit::fresh_message(&graph, &base)
        })?
    };
    if changes.is_empty() {
        out!("already connected");
        return Ok(());
    }
    for c in &changes {
        out!(
            "{} #{} object {}: {}.{} = {}",
            game.streams()[c.at.group.stream].stream_name(),
            c.at.group.resource,
            c.at.object,
            LogicSchema::gf2().class_name(c.at.class).unwrap_or("?"),
            c.at.index,
            c.value
                .as_ref()
                .map_or_else(|| "(default)".to_owned(), |v| v.to_string())
        );
    }
    let label = if args.disconnect {
        "Disconnect logic"
    } else {
        "Connect logic"
    };
    edit::apply(&game, &index, &mut ws, &changes, label)?;
    let streams = changes
        .iter()
        .map(|c| game.streams()[c.at.group.stream].stream_name())
        .collect();
    crate::workspace_cmd::finish_edit(&game, &mut ws, &args.mode, streams)
}

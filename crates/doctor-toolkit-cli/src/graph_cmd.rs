//! Navigation graph commands: `graph-info`, `graph-export`, `graph-import`,
//! `graph-edit`.

use std::io::Write as _;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Args;

use doctor_toolkit::game::{GameInstance, StreamFile};
use doctor_toolkit::level::navgraph::{GraphRef, save_graph};
use doctor_toolkit::resource::graph::{
    Graph, Neighbourhood, TYPE_GRAPH, flag_names, graph_flags, link_flags,
};
use doctor_toolkit::workspace::{ResourceKey, Workspace};

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
pub struct InfoArgs {
    pub game_dir: PathBuf,
    /// The graph, as a resource key (`gf2/miami/local:graph:39315a45`)
    pub key: String,
    /// List the nodes with their links
    #[arg(long)]
    pub nodes: bool,
    /// Print the summary as JSON
    #[arg(long)]
    pub json: bool,
    /// Workspace directory [default: <GAME_DIR>/_doctor_toolkit/workspace]
    #[arg(long, value_name = "DIR")]
    pub workspace_dir: Option<PathBuf>,
}

#[derive(Args)]
pub struct ExportArgs {
    pub game_dir: PathBuf,
    /// The graph, as a resource key
    pub key: String,
    /// Output JSON file
    #[arg(short, long)]
    pub output: PathBuf,
    /// Workspace directory [default: <GAME_DIR>/_doctor_toolkit/workspace]
    #[arg(long, value_name = "DIR")]
    pub workspace_dir: Option<PathBuf>,
}

#[derive(Args)]
pub struct ImportArgs {
    pub game_dir: PathBuf,
    /// The graph, as a resource key
    pub key: String,
    /// JSON file as `graph-export` writes it; lengths, R-tree and paths are
    /// brought up to date with the positions and links
    pub input: PathBuf,
    #[command(flatten)]
    pub mode: EditMode,
}

#[derive(Args)]
pub struct EditArgs {
    pub game_dir: PathBuf,
    /// The graph, as a resource key
    pub key: String,
    /// Move a node: `N:X,Y,Z`
    #[arg(long = "move", value_name = "N:X,Y,Z", allow_hyphen_values = true)]
    pub move_node: Vec<String>,
    /// Add a node without links: `X,Y,Z`
    #[arg(long, value_name = "X,Y,Z", allow_hyphen_values = true)]
    pub add_node: Vec<String>,
    /// Add a node and link it with the nearest nodes (both ways): `X,Y,Z`
    #[arg(long, value_name = "X,Y,Z", allow_hyphen_values = true)]
    pub attach: Vec<String>,
    /// For `--attach`: most nodes to link to
    #[arg(long, default_value_t = 2)]
    pub max_links: usize,
    /// For `--attach`: only nodes closer than this
    #[arg(long, default_value_t = 15.0)]
    pub max_distance: f32,
    /// Remove a node and its links (links of other graphs into this one follow)
    #[arg(long, value_name = "N")]
    pub remove_node: Option<usize>,
    /// Add a link `A:B` (with `--two-way` also `B:A`)
    #[arg(long, value_name = "A:B")]
    pub link: Vec<String>,
    #[arg(long)]
    pub two_way: bool,
    /// Remove the link `A:B`
    #[arg(long, value_name = "A:B")]
    pub unlink: Vec<String>,
    /// Set the flags of the link `A:B`: `A:B=FLAGS` (vault 0x2, ladder 0x4,
    /// pathing only 0x8, crosswalk 0x10, fall 0x20)
    #[arg(long, value_name = "A:B=FLAGS")]
    pub link_flags: Vec<String>,
    /// Set the flags of a node: `N=FLAGS` (disabled 0x4000)
    #[arg(long, value_name = "N=FLAGS")]
    pub node_flags: Vec<String>,
    /// Set the graph type flags (ground 0x1, patrol 0x2, vehicle 0x4, ped
    /// 0x8, parking 0x10, audio 0x20)
    #[arg(long, value_parser = parse_u16)]
    pub graph_flags: Option<u16>,
    /// Build the R-tree again
    #[arg(long)]
    pub rebuild_rtree: bool,
    #[command(flatten)]
    pub mode: EditMode,
}

fn parse_u16(text: &str) -> Result<u16, String> {
    match text.strip_prefix("0x") {
        Some(hex) => u16::from_str_radix(hex, 16),
        None => text.parse(),
    }
    .map_err(|e| e.to_string())
}

fn parse_point(text: &str) -> Result<[f32; 3]> {
    let v: Vec<f32> = text
        .split(',')
        .map(|p| p.trim().parse::<f32>())
        .collect::<Result<_, _>>()
        .with_context(|| format!("`{text}`: expected X,Y,Z"))?;
    v.try_into()
        .map_err(|_| anyhow::anyhow!("`{text}`: expected three numbers X,Y,Z"))
}

fn parse_pair(text: &str) -> Result<(usize, usize)> {
    let (a, b) = text
        .split_once(':')
        .with_context(|| format!("`{text}`: expected A:B"))?;
    Ok((a.trim().parse()?, b.trim().parse()?))
}

fn load(game: &GameInstance, ws: &Workspace, key: &str) -> Result<(ResourceKey, GraphRef)> {
    let key: ResourceKey = key.parse()?;
    if key.type_id != TYPE_GRAPH {
        bail!("{key} is not a graph resource");
    }
    let stream = game
        .find_stream(&key.stream)
        .with_context(|| format!("no stream {}", key.stream))?;
    let file = StreamFile::open(&game.streams()[stream].path)?;
    let resource = key
        .find_in(file.layout())
        .with_context(|| format!("{key} is not in its stream"))?;
    let data = ws.current_data(game, stream, resource)?;
    let graph = Graph::parse(&data).with_context(|| format!("reading {key}"))?;
    Ok((
        key,
        GraphRef {
            stream,
            resource,
            graph,
        },
    ))
}

/// `graph-info`.
pub fn info(args: &InfoArgs) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let ws = Workspace::for_game(&game, args.workspace_dir.as_deref())?;
    let (key, g) = load(&game, &ws, &args.key)?;
    let graph = &g.graph;
    let info = graph.info();
    if args.json {
        out!("{}", serde_json::to_string_pretty(&info)?);
        return Ok(());
    }
    out!("{key}: navigation graph, version {}", info.version);
    out!(
        "  guid {}  type {}  identity {}",
        info.guid,
        info.flags,
        info.identity
    );
    out!(
        "  {} node(s), {} link(s) ({} one-way), {} link(s) into {} neighbouring graph(s)",
        info.nodes,
        info.links,
        info.one_way_links,
        info.neighbour_links,
        info.neighbours
    );
    if let (Some(min), Some(max)) = (info.min, info.max) {
        out!(
            "  bounds ({:.1}, {:.1}, {:.1}) – ({:.1}, {:.1}, {:.1})",
            min[0],
            min[1],
            min[2],
            max[0],
            max[1],
            max[2]
        );
    }
    out!(
        "  properties: {} set(s), {} instance(s); visibility: {}; precomputed paths: {}; R-tree: {} node(s), {}",
        info.property_sets,
        info.property_instances,
        match info.hidden_pairs {
            Some(0) => "all pairs visible".to_owned(),
            Some(n) => format!("{n} hidden pair(s)"),
            None => "raw bits".to_owned(),
        },
        if info.precomputed_paths { "yes" } else { "no" },
        info.rtree_nodes,
        info.rtree_form
    );
    for (k, guid) in graph.neighbours.iter().enumerate() {
        out!("  neighbour {k}: {guid}");
    }
    if args.nodes {
        for (k, node) in graph.nodes.iter().enumerate() {
            let p = node.position;
            let links: Vec<String> = node
                .links
                .iter()
                .map(|l| {
                    let flags = if l.flags != 0 {
                        format!(" {}", flag_names(l.flags, &link_flags::NAMES))
                    } else {
                        String::new()
                    };
                    format!("{}({:.1}{flags})", l.to, l.length)
                })
                .collect();
            let neighbours: Vec<String> = node
                .neighbour_links
                .iter()
                .map(|l| format!("g{}:{}", l.neighbour, l.node))
                .collect();
            out!(
                "  #{k} ({:.2}, {:.2}, {:.2}) r {:.2}{}{} → {}{}",
                p[0],
                p[1],
                p[2],
                node.radius,
                if node.flags != 0 {
                    format!(" flags {:#06x}", node.flags)
                } else {
                    String::new()
                },
                node.properties
                    .map(|i| format!(" props #{i}"))
                    .unwrap_or_default(),
                links.join(" "),
                if neighbours.is_empty() {
                    String::new()
                } else {
                    format!(" | {}", neighbours.join(" "))
                }
            );
        }
    }
    for problem in graph.problems() {
        out!("problem: {problem}");
    }
    Ok(())
}

/// `graph-export`.
pub fn export(args: &ExportArgs) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let ws = Workspace::for_game(&game, args.workspace_dir.as_deref())?;
    let (key, g) = load(&game, &ws, &args.key)?;
    let json = serde_json::to_string_pretty(&g.graph)?;
    std::fs::write(&args.output, json + "\n")
        .with_context(|| format!("writing {}", args.output.display()))?;
    out!("{key} → {}", args.output.display());
    Ok(())
}

/// `graph-import`.
pub fn import(args: &ImportArgs) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let mut ws = workspace_cmd::open_for_edit(&game, &args.mode)?;
    let (key, mut g) = load(&game, &ws, &args.key)?;
    let text = std::fs::read_to_string(&args.input)
        .with_context(|| format!("reading {}", args.input.display()))?;
    let mut graph: Graph = serde_json::from_str(&text)
        .with_context(|| format!("{} is not a graph", args.input.display()))?;
    if graph.guid != g.graph.guid {
        bail!(
            "the file is graph {}, not {} ({key})",
            graph.guid,
            g.graph.guid
        );
    }
    for note in graph.repair_derived() {
        out!("{note}");
    }
    if graph == g.graph {
        out!("{key}: no change");
        return Ok(());
    }
    g.graph = graph;
    let label = format!("Import graph {key}");
    save_graph(&game, &mut ws, &g, None, &label)?;
    out!("{label}");
    workspace_cmd::finish_edit(&game, &mut ws, &args.mode, vec![key.stream.clone()])
}

/// `graph-edit`.
pub fn edit(args: &EditArgs) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let mut ws = workspace_cmd::open_for_edit(&game, &args.mode)?;
    let (key, mut g) = load(&game, &ws, &args.key)?;
    let graph = &mut g.graph;
    let mut done: Vec<String> = Vec::new();
    for text in &args.move_node {
        let (n, p) = text
            .split_once(':')
            .with_context(|| format!("`{text}`: expected N:X,Y,Z"))?;
        let n: usize = n.trim().parse()?;
        graph.move_node(n, parse_point(p)?)?;
        done.push(format!("node {n} moved"));
    }
    for text in &args.add_node {
        let k = graph.add_node(parse_point(text)?, 0.5)?;
        done.push(format!("node {k} added"));
    }
    let how = Neighbourhood {
        max_links: args.max_links,
        max_distance: args.max_distance,
        two_way: true,
    };
    for text in &args.attach {
        let (k, near) = graph.attach_point(parse_point(text)?, 0.5, how)?;
        done.push(format!("node {k} added, linked with {near:?}"));
    }
    for text in &args.link {
        let (a, b) = parse_pair(text)?;
        graph.add_link(a, b, 0)?;
        if args.two_way {
            graph.add_link(b, a, 0)?;
        }
        done.push(format!(
            "link {a} {} {b}",
            if args.two_way { "↔" } else { "→" }
        ));
    }
    for text in &args.unlink {
        let (a, b) = parse_pair(text)?;
        graph.remove_link(a, b)?;
        done.push(format!("link {a} → {b} removed"));
    }
    for text in &args.link_flags {
        let (pair, flags) = text
            .split_once('=')
            .with_context(|| format!("`{text}`: expected A:B=FLAGS"))?;
        let (a, b) = parse_pair(pair)?;
        let flags = parse_u16(flags.trim()).map_err(anyhow::Error::msg)?;
        graph.set_link_flags(a, b, flags)?;
        done.push(format!(
            "link {a} → {b}: {}",
            flag_names(flags, &link_flags::NAMES)
        ));
    }
    for text in &args.node_flags {
        let (n, flags) = text
            .split_once('=')
            .with_context(|| format!("`{text}`: expected N=FLAGS"))?;
        let n: usize = n.trim().parse()?;
        let flags = parse_u16(flags.trim()).map_err(anyhow::Error::msg)?;
        graph.set_node_flags(n, flags)?;
        done.push(format!("node {n} flags {flags:#06x}"));
    }
    if let Some(flags) = args.graph_flags {
        graph.flags = flags;
        done.push(format!("type {}", flag_names(flags, &graph_flags::NAMES)));
    }
    let mut renumbered = None;
    if let Some(n) = args.remove_node {
        renumbered = Some(graph.remove_node(n)?);
        done.push(format!("node {n} removed"));
    }
    if args.rebuild_rtree {
        graph.rebuild_rtree();
        done.push("R-tree built again".into());
    }
    if done.is_empty() {
        bail!("nothing to change: give --move, --attach, --link, … (see --help)");
    }
    let label = format!("Edit graph {key}: {}", done.join(", "));
    let others = save_graph(&game, &mut ws, &g, renumbered.as_deref(), &label)?;
    out!("{label}");
    if others > 0 {
        out!("{others} link(s) of other graphs into this one renumbered");
    }
    workspace_cmd::finish_edit(&game, &mut ws, &args.mode, vec![key.stream.clone()])
}

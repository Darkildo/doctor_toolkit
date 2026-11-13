//! Visual scripts: list nodes, compile, simulate, deploy into a level, remove.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Args;

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::resource::lua::vm::Value;
use doctor_toolkit::script::{self, ScriptGraph, deploy, library, sim::Sim};

use crate::workspace_cmd::{self, EditMode};

macro_rules! out {
    ($($arg:tt)*) => {
        if writeln!(std::io::stdout(), $($arg)*).is_err() {
            std::process::exit(0);
        }
    };
}

#[derive(Args)]
pub struct Nodes {
    /// Print the library as JSON
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct Compile {
    /// Script graph (JSON)
    pub graph: PathBuf,
    /// Write the Lua source here [default: print it]
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// Also write the game chunk (Lua 5.0 bytecode) here
    #[arg(long)]
    pub chunk: Option<PathBuf>,
    /// Print the diagnostics as JSON
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct Run {
    /// Script graph (JSON), Lua source (`.lua`) or chunk (`.luac`)
    pub graph: PathBuf,
    /// What happens, in order: `send:MESSAGE`, `wait:SECONDS`,
    /// `var:NAME=NUMBER`, `player:X,Y,Z`, `relay:FROM=TO`, `stop`
    pub steps: Vec<String>,
    /// Print the log as JSON lines
    #[arg(long)]
    pub json: bool,
    /// Significant digits when numbers become text (`tostring`, `..`)
    #[arg(long, default_value_t = 14, hide = true)]
    pub number_digits: usize,
}

#[derive(Args)]
pub struct Deploy {
    pub game_dir: PathBuf,
    /// Level stream that holds a SimGroup, e.g. `gf2/globals_global` (always
    /// loaded in the game) or a zone or mission stream
    pub stream: String,
    /// Script graph (JSON)
    pub graph: PathBuf,
    /// Start when this message is sent instead of when the level loads
    #[arg(long)]
    pub start_message: Option<String>,
    /// Start when the level loads (with --start-message: both)
    #[arg(long)]
    pub on_load: bool,
    /// Stop when this message is sent
    #[arg(long)]
    pub stop_message: Option<String>,
    #[command(flatten)]
    pub mode: EditMode,
}

#[derive(Args)]
pub struct Remove {
    pub game_dir: PathBuf,
    /// Level stream the script was deployed to
    pub stream: String,
    /// Script name (the graph's name, with or without `.lua`)
    pub name: String,
    #[command(flatten)]
    pub mode: EditMode,
}

#[derive(Args)]
pub struct List {
    pub game_dir: PathBuf,
    /// Level stream
    pub stream: String,
    /// Workspace directory [default: <GAME_DIR>/_doctor_toolkit/workspace]
    #[arg(long, value_name = "DIR")]
    pub workspace_dir: Option<PathBuf>,
}

fn read_graph(path: &Path) -> Result<ScriptGraph> {
    let text =
        fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    ScriptGraph::from_json(&text).with_context(|| path.display().to_string())
}

fn compiled(path: &Path, graph: &ScriptGraph) -> Result<script::Compiled> {
    let c = script::compile(graph);
    for d in &c.diagnostics {
        eprintln!("{}: {d}", path.display());
    }
    if c.has_errors() {
        bail!("{}: {} error(s)", path.display(), c.errors().count());
    }
    Ok(c)
}

/// `script-nodes`
pub fn nodes(args: &Nodes) -> Result<()> {
    let g = ScriptGraph::new("x");
    if args.json {
        let list: Vec<_> = library::all()
            .iter()
            .map(|d| {
                let n = script::Node::new(0, d.kind, [0.0, 0.0]);
                let (ins, outs) = library::pins(&g, &n).unwrap_or_default();
                let pins = |v: Vec<library::Pin>| -> Vec<_> {
                    v.into_iter()
                        .map(|p| serde_json::json!({"name": p.name, "type": p.ty.name(), "label": p.label}))
                        .collect()
                };
                serde_json::json!({
                    "kind": d.kind, "title": d.title, "category": d.category.name(),
                    "help": d.help, "verified": d.verified,
                    "inputs": pins(ins), "outputs": pins(outs),
                    "params": d.params.iter().map(|p| serde_json::json!({
                        "name": p.name, "label": p.label, "default": serde_json::from_str::<serde_json::Value>(p.default).ok(),
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        out!("{}", serde_json::to_string_pretty(&list)?);
        return Ok(());
    }
    let mut category = None;
    for d in library::all() {
        if category != Some(d.category) {
            category = Some(d.category);
            out!("\n{}", d.category.name());
        }
        let n = script::Node::new(0, d.kind, [0.0, 0.0]);
        let (ins, outs) = library::pins(&g, &n).unwrap_or_default();
        let list = |v: &[library::Pin]| {
            v.iter()
                .map(|p| format!("{}:{}", p.name, p.ty.name()))
                .collect::<Vec<_>>()
                .join(", ")
        };
        out!(
            "  {:<26} {}{}\n  {:<26}   in ({}) -> out ({})",
            d.kind,
            d.title,
            if d.verified {
                ""
            } else {
                " [not verified in game]"
            },
            "",
            list(&ins),
            list(&outs)
        );
    }
    Ok(())
}

/// `script-compile`
pub fn compile(args: &Compile) -> Result<()> {
    let graph = read_graph(&args.graph)?;
    let c = script::compile(&graph);
    if args.json {
        out!("{}", serde_json::to_string_pretty(&c.diagnostics)?);
    } else {
        for d in &c.diagnostics {
            eprintln!("{}: {d}", args.graph.display());
        }
    }
    match &args.output {
        Some(p) => {
            fs::write(p, &c.source).with_context(|| format!("cannot write {}", p.display()))?
        }
        None if !args.json => print!("{}", c.source),
        None => {}
    }
    if c.has_errors() {
        bail!("{}: {} error(s)", args.graph.display(), c.errors().count());
    }
    if let (Some(p), Some(chunk)) = (&args.chunk, &c.chunk) {
        fs::write(p, chunk).with_context(|| format!("cannot write {}", p.display()))?;
        eprintln!("{}: {} bytes", p.display(), chunk.len());
    }
    Ok(())
}

/// `script-run`
pub fn run(args: &Run) -> Result<()> {
    let path = &args.graph;
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (chunk, name) = match ext.as_str() {
        "lua" => {
            let source =
                fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
            let chunk =
                doctor_toolkit::resource::lua::compile_for_game(&source, name.as_bytes())
                    .map_err(|e| anyhow::anyhow!("{}:{}: {}", path.display(), e.line, e.message))?;
            (chunk, name)
        }
        "luac" => {
            let data = fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
            let source = doctor_toolkit::resource::lua::Chunk::parse(&data)?.source();
            let name = doctor_toolkit::resource::lua::file_name(&source).to_owned();
            (data, name)
        }
        _ => {
            let graph = read_graph(path)?;
            let c = compiled(path, &graph)?;
            (c.chunk.context("no chunk")?, graph.chunk_name())
        }
    };
    let mut sim = Sim::new();
    sim.vm.number_digits = args.number_digits;
    sim.load(&chunk)?;
    if sim.registered(&name, "startup") {
        sim.start(&name)?;
    }
    let print = |sim: &mut Sim| -> Result<()> {
        for line in std::mem::take(&mut sim.vm.printed) {
            out!("{:8.3}  print {line}", sim.state.borrow().time);
        }
        for e in sim.take_log() {
            if args.json {
                out!("{}", serde_json::to_string(&e)?);
            } else {
                out!("{}", describe(&e));
            }
        }
        Ok(())
    };
    print(&mut sim)?;
    for step in &args.steps {
        let (what, arg) = step.split_once(':').unwrap_or((step.as_str(), ""));
        match what {
            "send" => sim.send(arg)?,
            "wait" => sim.advance(arg.parse().with_context(|| format!("bad step {step}"))?)?,
            "var" => {
                let (n, v) = arg
                    .split_once('=')
                    .with_context(|| format!("bad step {step}"))?;
                let value = match v.parse::<f64>() {
                    Ok(x) => Value::Number(x),
                    Err(_) => Value::str(v),
                };
                sim.set_variable(n, value);
            }
            "player" => {
                let p: Vec<f64> = arg
                    .split(',')
                    .map(|s| s.trim().parse())
                    .collect::<std::result::Result<_, _>>()
                    .with_context(|| format!("bad step {step}"))?;
                if p.len() != 3 {
                    bail!("bad step {step}: player:X,Y,Z");
                }
                sim.state.borrow_mut().player = [p[0], p[1], p[2]];
            }
            "relay" => {
                let (a, b) = arg
                    .split_once('=')
                    .with_context(|| format!("bad step {step}"))?;
                sim.state.borrow_mut().relays.push((a.into(), b.into()));
            }
            "stop" => sim.stop(&name)?,
            _ => bail!("unknown step {step:?} (send:, wait:, var:, player:, relay:, stop)"),
        }
        print(&mut sim)?;
    }
    Ok(())
}

fn describe(e: &script::sim::SimEvent) -> String {
    use script::sim::SimEvent as E;
    match e {
        E::Message {
            time,
            message,
            data,
        } => match data {
            Some(d) => format!("{time:8.3}  send {message} ({d})"),
            None => format!("{time:8.3}  send {message}"),
        },
        E::Probe { time, file } => format!("{time:8.3}  probe {file}"),
        E::Variable { time, name, value } => format!("{time:8.3}  set {name} = {value}"),
        E::Call {
            time,
            function,
            args,
        } => format!("{time:8.3}  {function}({})", args.join(", ")),
        E::Timer { time, callback } => format!("{time:8.3}  timer {callback}"),
        E::Error { time, message } => format!("{time:8.3}  error {message}"),
    }
}

fn stream_of(game: &GameInstance, name: &str) -> Result<usize> {
    let n = doctor_toolkit::workspace::key::normalize_stream(name);
    game.find_stream(&n)
        .with_context(|| format!("stream `{name}` not found in {}", game.root().display()))
}

/// `script-deploy`
pub fn deploy(args: &Deploy) -> Result<()> {
    let mut graph = read_graph(&args.graph)?;
    if args.start_message.is_some() || args.on_load || args.stop_message.is_some() {
        graph.start.message = args.start_message.clone().or(graph.start.message);
        graph.start.on_load = args.on_load || args.start_message.is_none() && graph.start.on_load;
        graph.start.stop_message = args.stop_message.clone().or(graph.start.stop_message);
    }
    let c = compiled(&args.graph, &graph)?;
    let game = GameInstance::open(&args.game_dir)?;
    let stream = stream_of(&game, &args.stream)?;
    let mut ws = workspace_cmd::open_for_edit(&game, &args.mode)?;
    let (index, _) = game.build_index(|_| {});
    let d = deploy::deploy(
        &game,
        &index,
        &mut ws,
        stream,
        &graph,
        c.chunk.as_ref().context("no chunk")?,
    )?;
    let saved = deploy::save_graph(&ws, &graph)?;
    out!(
        "{} {} in {}: resource {}, LuaAudioScript object {}{}",
        if d.updated { "updated" } else { "deployed" },
        graph.chunk_name(),
        d.stream,
        d.key,
        d.object,
        if d.changed { "" } else { " (no change)" }
    );
    out!("graph kept as {}", saved.display());
    if !d.changed {
        return Ok(());
    }
    workspace_cmd::finish_edit(&game, &mut ws, &args.mode, vec![d.stream])
}

/// `script-remove`
pub fn remove(args: &Remove) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let stream = stream_of(&game, &args.stream)?;
    let mut ws = workspace_cmd::open_for_edit(&game, &args.mode)?;
    let name = if args.name.to_ascii_lowercase().ends_with(".lua") {
        args.name.clone()
    } else {
        format!("{}.lua", args.name)
    };
    if !deploy::remove(&game, &mut ws, stream, &name)? {
        bail!("no script {name} in {}", args.stream);
    }
    out!("removed {name} from {}", args.stream);
    workspace_cmd::finish_edit(
        &game,
        &mut ws,
        &args.mode,
        vec![game.streams()[stream].stream_name()],
    )
}

/// `script-list`
pub fn list(args: &List) -> Result<()> {
    let game = GameInstance::open(&args.game_dir)?;
    let stream = stream_of(&game, &args.stream)?;
    let ws = doctor_toolkit::workspace::Workspace::for_game(&game, args.workspace_dir.as_deref())?;
    let list = deploy::list(&game, &ws, stream)?;
    if list.is_empty() {
        out!("no scripts made from graphs in {}", args.stream);
    }
    for d in list {
        let start = d.start.as_ref().map_or_else(
            || "no LuaAudioScript object".to_owned(),
            |s| {
                let mut parts = Vec::new();
                if s.on_load {
                    parts.push("on load".to_owned());
                }
                if let Some(m) = &s.message {
                    parts.push(format!("on {m}"));
                }
                if let Some(m) = &s.stop_message {
                    parts.push(format!("stops on {m}"));
                }
                parts.join(", ")
            },
        );
        out!("{}  objects {:?}  {start}", d.key, d.objects);
    }
    let graphs = deploy::saved_graphs(&ws);
    if !graphs.is_empty() {
        out!("graphs kept in the workspace:");
        for (n, p) in graphs {
            out!("  {n}  {}", p.display());
        }
    }
    Ok(())
}

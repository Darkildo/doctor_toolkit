//! A trial run of a script on the stand-in for the game ([`Sim`]): load it,
//! start it, feed it messages, time and variables, and collect what it did.
//!
//! One description for every surface: `script-run` of the command line, the
//! tool `script_run` of the MCP server and the trial run of the script
//! editor. The node kinds as data ([`node_kinds`]) are here for the same
//! reason.

use std::path::Path;

use serde::Serialize;
use serde_json::{Value as Json, json};

use crate::resource::lua;
use crate::resource::lua::vm::Value;
use crate::{Error, Result};

use super::sim::{Sim, SimEvent};
use super::{ScriptGraph, library};

/// What to run.
#[derive(Debug, Clone)]
pub enum TrialSource {
    /// Lua source and the name of its file (`myscript.lua`).
    Lua { source: Vec<u8>, name: String },
    /// A compiled chunk (`.luac`).
    Chunk(Vec<u8>),
    /// A script graph.
    Graph(Box<ScriptGraph>),
}

impl TrialSource {
    /// Reads a file by its extension: `.lua`, `.luac`, else a script graph (JSON).
    pub fn read(path: &Path) -> Result<Self> {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let data = std::fs::read(path).map_err(|e| Error::io(path, e))?;
        Ok(match ext.as_str() {
            "lua" => Self::Lua {
                source: data,
                name: path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            },
            "luac" => Self::Chunk(data),
            _ => {
                let text = String::from_utf8(data).map_err(|_| {
                    Error::InvalidArgument(format!("{} is not text", path.display()))
                })?;
                Self::Graph(Box::new(ScriptGraph::from_json(&text).map_err(|e| {
                    Error::InvalidArgument(format!("{}: {e}", path.display()))
                })?))
            }
        })
    }

    /// The chunk to load, the name the script registers under, and the
    /// notes of the graph compiler.
    fn chunk(&self) -> Result<(Vec<u8>, String, Vec<String>)> {
        match self {
            Self::Lua { source, name } => {
                let chunk = lua::compile_for_game(source, name.as_bytes()).map_err(|e| {
                    Error::InvalidArgument(format!("{name}:{}: {}", e.line, e.message))
                })?;
                Ok((chunk, name.clone(), Vec::new()))
            }
            Self::Chunk(data) => {
                let source = lua::Chunk::parse(data)?.source();
                Ok((data.clone(), lua::file_name(&source).to_owned(), Vec::new()))
            }
            Self::Graph(graph) => {
                let compiled = super::compile(graph);
                let notes: Vec<String> = compiled
                    .diagnostics
                    .iter()
                    .map(ToString::to_string)
                    .collect();
                if compiled.has_errors() {
                    return Err(Error::InvalidArgument(format!(
                        "the graph does not compile:\n{}",
                        notes.join("\n")
                    )));
                }
                let chunk = compiled
                    .chunk
                    .ok_or_else(|| Error::InvalidArgument("the compiler made no chunk".into()))?;
                Ok((chunk, graph.chunk_name(), notes))
            }
        }
    }
}

/// One line of the log of a trial run.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum TrialEntry {
    /// `print` of the script.
    Print { time: f64, print: String },
    /// Something the script did to the game.
    Event(SimEvent),
}

impl TrialEntry {
    /// The entry as a line of text.
    pub fn line(&self) -> String {
        use SimEvent as E;
        match self {
            Self::Print { time, print } => format!("{time:8.3}  print {print}"),
            Self::Event(E::Message {
                time,
                message,
                data,
            }) => match data {
                Some(d) => format!("{time:8.3}  send {message} ({d})"),
                None => format!("{time:8.3}  send {message}"),
            },
            Self::Event(E::Probe { time, file }) => format!("{time:8.3}  probe {file}"),
            Self::Event(E::Variable { time, name, value }) => {
                format!("{time:8.3}  set {name} = {value}")
            }
            Self::Event(E::Call {
                time,
                function,
                args,
            }) => format!("{time:8.3}  {function}({})", args.join(", ")),
            Self::Event(E::Timer { time, callback }) => format!("{time:8.3}  timer {callback}"),
            Self::Event(E::Error { time, message }) => format!("{time:8.3}  error {message}"),
        }
    }
}

/// What a trial run did.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TrialReport {
    /// Name the script registered under.
    pub script: String,
    /// Notes of the graph compiler.
    pub notes: Vec<String>,
    pub log: Vec<TrialEntry>,
    /// Game time at the end, seconds.
    pub time: f64,
    /// The step that failed and why; the log holds what happened before it.
    pub error: Option<String>,
}

fn drain(sim: &mut Sim, log: &mut Vec<TrialEntry>) {
    let time = sim.state.borrow().time;
    for print in std::mem::take(&mut sim.vm.printed) {
        log.push(TrialEntry::Print { time, print });
    }
    log.extend(sim.take_log().into_iter().map(TrialEntry::Event));
}

fn step(sim: &mut Sim, name: &str, step: &str) -> Result<()> {
    let bad = || Error::InvalidArgument(format!("bad step {step}"));
    let (what, arg) = step.split_once(':').unwrap_or((step, ""));
    match what {
        "send" => sim.send(arg),
        "wait" => sim.advance(arg.parse().map_err(|_| bad())?),
        "var" => {
            let (n, v) = arg.split_once('=').ok_or_else(bad)?;
            let value = match v.parse::<f64>() {
                Ok(x) => Value::Number(x),
                Err(_) => Value::str(v),
            };
            sim.set_variable(n, value);
            Ok(())
        }
        "player" => {
            let p: Vec<f64> = arg
                .split(',')
                .map(|s| s.trim().parse())
                .collect::<std::result::Result<_, _>>()
                .map_err(|_| bad())?;
            if p.len() != 3 {
                return Err(Error::InvalidArgument(format!(
                    "bad step {step}: player:X,Y,Z"
                )));
            }
            sim.state.borrow_mut().player = [p[0], p[1], p[2]];
            Ok(())
        }
        "relay" => {
            let (a, b) = arg.split_once('=').ok_or_else(bad)?;
            sim.state.borrow_mut().relays.push((a.into(), b.into()));
            Ok(())
        }
        "stop" => sim.stop(name),
        _ => Err(Error::InvalidArgument(format!(
            "unknown step {step:?} (send:, wait:, var:, player:, relay:, stop)"
        ))),
    }
}

/// Loads the script, starts it (when it registers `startup`) and does
/// `steps` in order: `send:MESSAGE`, `wait:SECONDS`, `var:NAME=NUMBER`,
/// `player:X,Y,Z`, `relay:FROM=TO`, `stop`. A script that does not load is
/// an error; a step that fails ends the run with the log so far.
pub fn run(
    source: &TrialSource,
    steps: &[String],
    number_digits: Option<usize>,
) -> Result<TrialReport> {
    let (chunk, name, notes) = source.chunk()?;
    let mut sim = Sim::new();
    if let Some(digits) = number_digits {
        sim.vm.number_digits = digits;
    }
    let mut log = Vec::new();
    let mut error = None;
    let started = sim.load(&chunk).and_then(|()| {
        if sim.registered(&name, "startup") {
            sim.start(&name)
        } else {
            Ok(())
        }
    });
    drain(&mut sim, &mut log);
    match started {
        Err(e) => error = Some(e.to_string()),
        Ok(()) => {
            for text in steps {
                let done = step(&mut sim, &name, text);
                drain(&mut sim, &mut log);
                if let Err(e) = done {
                    error = Some(format!("{text}: {e}"));
                    break;
                }
            }
        }
    }
    let time = sim.state.borrow().time;
    Ok(TrialReport {
        script: name,
        notes,
        log,
        time,
        error,
    })
}

/// Every kind of node of a script graph: title, category, pins with their
/// types, parameters with defaults, and whether it was checked in the game.
pub fn node_kinds() -> Vec<Json> {
    let g = ScriptGraph::new("x");
    library::all()
        .iter()
        .map(|d| {
            let n = super::Node::new(0, d.kind, [0.0, 0.0]);
            let (ins, outs) = library::pins(&g, &n).unwrap_or_default();
            let pins = |v: Vec<library::Pin>| -> Vec<Json> {
                v.into_iter()
                    .map(|p| json!({"name": p.name, "type": p.ty.name(), "label": p.label}))
                    .collect()
            };
            json!({
                "kind": d.kind, "title": d.title, "category": d.category.name(),
                "help": d.help, "verified": d.verified,
                "inputs": pins(ins), "outputs": pins(outs),
                "params": d.params.iter().map(|p| json!({
                    "name": p.name, "label": p.label,
                    "default": serde_json::from_str::<Json>(p.default).ok(),
                })).collect::<Vec<_>>(),
            })
        })
        .collect()
}

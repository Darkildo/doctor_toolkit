//! The logic graph of missions: `logic_graph`, `logic_messages`,
//! `logic_missions`, `logic_schema` (graphs and lists as dicts) and
//! `logic_run` (the logic run without the game, stage 38).

use doctor_toolkit::logic_graph::{BuildOptions, LogicSchema, MessageIndex, ScopeSpec, scope};
use doctor_toolkit::workspace::Workspace;
use pyo3::prelude::*;

use crate::error::ResultExt;
use crate::game::Game;
use crate::util::to_python;

fn workspace(game: &Game, original: bool) -> PyResult<Option<Workspace>> {
    let dir = Workspace::default_dir(game.game.root());
    if original || !Workspace::exists(&dir) {
        return Ok(None);
    }
    Workspace::open(&dir).py().map(Some)
}

/// The logic graph of a scope as a dict: `nodes` (class, name, stream,
/// pins with messages), `wires` (`from`/`to` = node and pin indices,
/// `kind`, `message` hash), `messages` (by hash: name, senders, listeners,
/// state, `elsewhere` counts) and `overview`.
///
/// `scope`: `mission:<name>`, `checkpoint:<n>`, `stream:<stream>`,
/// `group:<stream>#<resource>`, `level:<stream>` or `game`. `hubs` draws
/// messages with many ends through hub nodes; `all` keeps objects without
/// messages; `original` ignores the workspace.
#[pyfunction]
#[pyo3(signature = (game, scope, hubs = true, all = false, original = false))]
fn logic_graph<'py>(
    py: Python<'py>,
    game: &Game,
    scope: &str,
    hubs: bool,
    all: bool,
    original: bool,
) -> PyResult<Bound<'py, PyAny>> {
    let ws = workspace(game, original)?;
    let g = game.game.clone();
    let value = py
        .detach(|| -> doctor_toolkit::Result<serde_json::Value> {
            let (index, _) = g.build_index(|_| {});
            let spec = ScopeSpec::parse(&g, scope)?;
            let options = BuildOptions {
                hub_threshold: if hubs { 48 } else { 0 },
                keep_unwired: all,
                ..BuildOptions::default()
            };
            let (mut graph, _) = scope::build(&g, &index, ws.as_ref(), &spec, options)?;
            graph.annotate(&MessageIndex::build(&g, &index, ws.as_ref()));
            let mut v = serde_json::to_value(&graph)
                .map_err(|e| doctor_toolkit::Error::InvalidArgument(e.to_string()))?;
            v["overview"] = serde_json::to_value(graph.overview())
                .map_err(|e| doctor_toolkit::Error::InvalidArgument(e.to_string()))?;
            Ok(v)
        })
        .py()?;
    to_python(py, &value)
}

/// Who sends and who hears a message across the game: a dict with `name`,
/// `senders`, `listeners` (each: `group` = (stream, resource), `object`,
/// `class_name`, `name`, `pin`), or `None`. With `search`, a list of the
/// messages whose name contains `name`.
#[pyfunction]
#[pyo3(signature = (game, name, search = false, original = false))]
fn logic_messages<'py>(
    py: Python<'py>,
    game: &Game,
    name: &str,
    search: bool,
    original: bool,
) -> PyResult<Bound<'py, PyAny>> {
    let ws = workspace(game, original)?;
    let g = game.game.clone();
    let index = py.detach(|| {
        let (resources, _) = g.build_index(|_| {});
        MessageIndex::build(&g, &resources, ws.as_ref())
    });
    if search {
        to_python(py, &index.search(name))
    } else {
        to_python(py, &index.who(name))
    }
}

/// The missions: a list of dicts with `name` and `streams` (stream names).
#[pyfunction]
fn logic_missions<'py>(py: Python<'py>, game: &Game) -> PyResult<Bound<'py, PyAny>> {
    let missions: Vec<serde_json::Value> = scope::missions(&game.game)
        .into_iter()
        .map(|m| {
            serde_json::json!({
                "name": m.name,
                "streams": m
                    .streams
                    .iter()
                    .map(|&s| game.game.streams()[s].stream_name())
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    to_python(py, &missions)
}

/// What the schema of SimGroup pins covers (counts by pin kind and confidence).
#[pyfunction]
fn logic_schema<'py>(py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
    to_python(py, &LogicSchema::gf2().coverage())
}

/// Runs the logic of a scope without the game and returns a dict:
/// `journal` (entries: `seq`, `step`, `time`, `kind` — `send`, `schedule`,
/// `deliver`, `variable`, `input`, `lua`, `warning` — and their fields),
/// `stop`, `time`, `steps`, `actors` (class, name, model, messages heard),
/// `variables`, and `comparison` when `compare` (the text of a journal of
/// the game in JSON) is given.
///
/// `events`: inputs as in `logic-run --events` (`load`, `send:MSG`,
/// `enter:OBJ`, `exit:OBJ`, `var:NAME=N`, `checkpoint:N`, `wait:S`,
/// `at:T:…`), default `["load"]`; `until`: game time to stop at;
/// `reactions`: list of dicts `{"object", "on", "send": [{"message",
/// "delay"}]}` for black boxes; `breaks`: message names to stop before.
#[pyfunction]
#[pyo3(signature = (game, scope, events = None, until = None, steps = 20000, seed = 1, lua = true, reactions = None, breaks = None, compare = None, original = false))]
#[allow(clippy::too_many_arguments)]
fn logic_run<'py>(
    py: Python<'py>,
    game: &Game,
    scope: &str,
    events: Option<Vec<String>>,
    until: Option<f64>,
    steps: usize,
    seed: u32,
    lua: bool,
    reactions: Option<Bound<'py, PyAny>>,
    breaks: Option<Vec<String>>,
    compare: Option<String>,
    original: bool,
) -> PyResult<Bound<'py, PyAny>> {
    use doctor_toolkit::logic_graph::run::{
        self, Breakpoint, ManualReaction, Run, RunOptions, Until,
    };
    let reactions: Vec<ManualReaction> = match reactions {
        Some(r) => {
            let text = py
                .import("json")?
                .call_method1("dumps", (r,))?
                .extract::<String>()?;
            serde_json::from_str(&text)
                .map_err(|e| doctor_toolkit::Error::InvalidArgument(format!("reactions: {e}")))
                .py()?
        }
        None => Vec::new(),
    };
    let ws = workspace(game, original)?;
    let g = game.game.clone();
    let value = py
        .detach(|| -> doctor_toolkit::Result<serde_json::Value> {
            let (index, _) = g.build_index(|_| {});
            let spec = ScopeSpec::parse(&g, scope)?;
            let (input, _) = run::load(&g, &index, ws.as_ref(), &spec)?;
            let mut r = Run::new(
                input,
                RunOptions {
                    seed,
                    lua,
                    ..RunOptions::default()
                },
            );
            for m in &reactions {
                r.add_reaction(m);
            }
            for b in breaks.unwrap_or_default() {
                r.toggle_breakpoint(Breakpoint::Message(doctor_toolkit::hash::string_id(&b)));
            }
            let events = events.unwrap_or_else(|| vec!["load".to_owned()]);
            for (t, i) in run::parse_inputs(&events)? {
                r.schedule(t, i);
            }
            let stop = r.run(until.map_or(Until::Idle, Until::Time), steps);
            let comparison = match &compare {
                Some(text) => {
                    let game_journal = run::parse_game_journal(text)?;
                    Some(run::compare(r.journal(), &game_journal, &r.scope_messages()))
                }
                None => None,
            };
            let actors: Vec<serde_json::Value> = r
                .actors()
                .iter()
                .map(|a| {
                    serde_json::json!({
                        "class": a.class,
                        "name": a.name,
                        "label": a.label(),
                        "stream": a.stream,
                        "object": a.object,
                        "guid": a.guid.to_string(),
                        "model": a.model.kind(),
                        "listens": a.listens.iter().map(|(m, pin)| serde_json::json!({"message": m.name, "pin": pin})).collect::<Vec<_>>(),
                    })
                })
                .collect();
            Ok(serde_json::json!({
                "journal": r.journal(),
                "stop": stop,
                "time": r.time(),
                "steps": r.steps(),
                "actors": actors,
                "variables": r.variables(),
                "comparison": comparison,
            }))
        })
        .py()?;
    to_python(py, &value)
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(logic_run, m)?)?;
    m.add_function(wrap_pyfunction!(logic_graph, m)?)?;
    m.add_function(wrap_pyfunction!(logic_messages, m)?)?;
    m.add_function(wrap_pyfunction!(logic_missions, m)?)?;
    m.add_function(wrap_pyfunction!(logic_schema, m)?)?;
    Ok(())
}

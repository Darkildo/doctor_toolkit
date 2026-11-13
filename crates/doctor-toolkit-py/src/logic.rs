//! The logic graph of missions: `logic_graph`, `logic_messages`,
//! `logic_missions`, `logic_schema` (graphs and lists as dicts).

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

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(logic_graph, m)?)?;
    m.add_function(wrap_pyfunction!(logic_messages, m)?)?;
    m.add_function(wrap_pyfunction!(logic_missions, m)?)?;
    m.add_function(wrap_pyfunction!(logic_schema, m)?)?;
    Ok(())
}

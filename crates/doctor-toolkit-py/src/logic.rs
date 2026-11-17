//! The logic graph of missions: `logic_graph`, `logic_messages`,
//! `logic_missions`, `logic_schema` (graphs and lists as dicts) and
//! `logic_run` (the logic run without the game, stage 38); the typed game
//! data: `message_info`, `message_search`, `enum_values`, `attr_types`,
//! `logic_check`.

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

fn message_id(name: &str) -> doctor_toolkit::game_types::MessageId {
    use doctor_toolkit::game_types::MessageId;
    match name
        .strip_prefix("0x")
        .filter(|h| h.len() == 8)
        .and_then(|h| u32::from_str_radix(h, 16).ok())
    {
        Some(hash) => MessageId::from_hash(hash),
        None => MessageId::from_name(name),
    }
}

/// What is known about a message, as a dict: `kind` (`engine` — named by
/// the executable, `game` — used by the game files, `custom` — new,
/// `unknown` — a hash without a name), `name`, `hash`, `usage`, `senders`
/// and `listeners` (class, attribute index, number of objects over the whole
/// game), `scope`, `streams`, `mentions`, `data` (kinds of the data word joined
/// with ` | `: `none`, `object`, `forward`, a structure of the engine; or
/// `"unknown"`), `data_conf`, `data_source`, `summary`, `sent_by`,
/// `engine_sites`, `engine_listens`, `conf`, `source`, and `collides_with` — the game
/// message a new name shares its hash with (`None` when the hash is free).
///
/// `name`: a message name, or a hash as `0x` and eight hexadecimal digits.
/// Needs no game.
#[pyfunction]
fn message_info<'py>(py: Python<'py>, name: &str) -> PyResult<Bound<'py, PyAny>> {
    use doctor_toolkit::game_types::{MessageId, message};
    let id = message_id(name);
    let mut value = serde_json::to_value(id.info())
        .map_err(|e| doctor_toolkit::Error::InvalidArgument(e.to_string()))
        .py()?;
    value["collides_with"] = match &id {
        MessageId::Custom { name, .. } => serde_json::json!(message::collision(name)),
        _ => serde_json::Value::Null,
    };
    to_python(py, &value)
}

/// Messages of the game files whose name contains `text`: a list of dicts
/// as `message_info` gives, at most `limit`.
#[pyfunction]
#[pyo3(signature = (text, limit = 40))]
fn message_search<'py>(py: Python<'py>, text: &str, limit: usize) -> PyResult<Bound<'py, PyAny>> {
    use doctor_toolkit::game_types::{MessageCatalog, MessageId};
    let found: Vec<_> = MessageCatalog::gf2()
        .search(text, limit)
        .into_iter()
        .map(|m| MessageId::from_hash(m.hash).info())
        .collect();
    to_python(py, &found)
}

/// A value list of the game by key (`family`, `city`, `venue`,
/// `building_type`, `weapon`, `crew_specialty`, `sound_category`,
/// `emotion`) as a dict: `title`, `kind`, `source`, `conf` and `values`
/// (`name`, `value`, `hash`, `note`). Without a key, every list.
#[pyfunction]
#[pyo3(signature = (key = None))]
fn enum_values<'py>(py: Python<'py>, key: Option<&str>) -> PyResult<Bound<'py, PyAny>> {
    use doctor_toolkit::game_types;
    match key {
        None => to_python(py, &game_types::enums()),
        Some(key) => {
            let list = game_types::enum_def(key)
                .ok_or_else(|| {
                    doctor_toolkit::Error::InvalidArgument(format!(
                        "no list `{key}`; lists: {}",
                        game_types::enums()
                            .iter()
                            .map(|e| e.key)
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                })
                .py()?;
            to_python(py, list)
        }
    }
}

/// What the attributes of the packet of a SimGroup class mean: a list of
/// dicts `index`, `name`, `type` (`MsgIn`, `MsgOut`, `Ref<Class>`,
/// `Res<type>`, `Enum<list>`, `Seconds`, `Money`, `Bool`), `conf` and, for
/// types read from the game files, `seen` (values that fit, values seen).
/// `class_name`: a class name or eight hexadecimal digits of its hash.
#[pyfunction]
fn attr_types<'py>(py: Python<'py>, class_name: &str) -> PyResult<Bound<'py, PyAny>> {
    use doctor_toolkit::game_types;
    let schema = LogicSchema::gf2();
    let hash = match u32::from_str_radix(class_name, 16) {
        Ok(h) if class_name.len() == 8 => h,
        _ => doctor_toolkit::hash::string_id(class_name),
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
    let rows: Vec<serde_json::Value> = indexes
        .into_iter()
        .filter_map(|i| {
            let (ty, conf) = schema.semantic(hash, i)?;
            let extra = game_types::attr_type(hash, i);
            Some(serde_json::json!({
                "index": i,
                "name": schema.attr(hash, i).and_then(|a| a.name.clone()),
                "type": ty,
                "conf": conf,
                "seen": extra.map(|e| e.seen),
            }))
        })
        .collect();
    to_python(py, &rows)
}

/// Checks the messages of the SimGroups the workspace of `game` changes: a
/// dict with `groups`, `custom` (new message names), `problems` (`severity`
/// — `error` for a new name with the hash of a game message, `warning` for
/// a message heard but never sent or sent but never heard — `kind`,
/// `message`, `what`, `groups`) and `ok`. Without a workspace: `ok` only.
#[pyfunction]
fn logic_check<'py>(py: Python<'py>, game: &Game) -> PyResult<Bound<'py, PyAny>> {
    let Some(ws) = workspace(game, false)? else {
        return to_python(py, &serde_json::json!({ "ok": true, "groups": 0 }));
    };
    let report = py.detach(|| doctor_toolkit::logic_graph::check::check_workspace(&ws));
    let mut value = serde_json::to_value(&report)
        .map_err(|e| doctor_toolkit::Error::InvalidArgument(e.to_string()))
        .py()?;
    value["ok"] = (report.errors().count() == 0).into();
    to_python(py, &value)
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
    m.add_function(wrap_pyfunction!(message_info, m)?)?;
    m.add_function(wrap_pyfunction!(message_search, m)?)?;
    m.add_function(wrap_pyfunction!(enum_values, m)?)?;
    m.add_function(wrap_pyfunction!(attr_types, m)?)?;
    m.add_function(wrap_pyfunction!(logic_check, m)?)?;
    Ok(())
}

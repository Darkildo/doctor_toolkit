//! Runs without the game and looks into files (stage 104): the logic of a
//! scope run on the model of the game's classes, a script (graph or Lua)
//! on the stand-in for the game, and a save file. Nothing is written
//! anywhere; long journals come in pages.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use doctor_toolkit::logic_graph::run::{RunSpec, What};
use doctor_toolkit::save::{self, SaveFile, view};
use doctor_toolkit::script::ScriptGraph;
use doctor_toolkit::script::trial::{self, TrialSource};

use super::{Output, clamp};
use crate::state::{ToolError, ToolResult, Toolkit, fail};

// ------------------------------------------------------------ parameters

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LogicRunParams {
    #[serde(flatten)]
    pub run: RunSpec,
    /// List the objects of the run with their models (which classes are
    /// black boxes) instead of running.
    #[serde(default)]
    pub actors: Option<bool>,
    /// Read the game files only, without the edits of the workspace.
    #[serde(default)]
    pub original: Option<bool>,
    /// The journal as JSON entries instead of lines of text.
    #[serde(default)]
    pub json: Option<bool>,
    /// Skip this many journal entries (or objects): the run is repeatable,
    /// so the next page is the same call with a larger offset.
    #[serde(default)]
    pub offset: Option<usize>,
    /// At most this many entries (default 200, at most 2000).
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ScriptRunParams {
    /// A script graph (format `doctor-toolkit-script`, as `edit_script`
    /// takes it): it is compiled and run.
    #[serde(default)]
    pub graph: Option<Value>,
    /// Or: Lua source.
    #[serde(default)]
    pub lua: Option<String>,
    /// With `lua`: the file name the script registers under [default: `script.lua`].
    #[serde(default)]
    pub name: Option<String>,
    /// Or: a file (an absolute path): a graph (JSON), `.lua` or `.luac`.
    #[serde(default)]
    pub file: Option<String>,
    /// What happens, in order: `send:MESSAGE`, `wait:SECONDS`,
    /// `var:NAME=NUMBER`, `player:X,Y,Z`, `relay:FROM=TO`, `stop`.
    #[serde(default)]
    pub steps: Vec<String>,
    /// The log as JSON entries instead of lines of text.
    #[serde(default)]
    pub json: Option<bool>,
    /// Skip this many log entries (paging).
    #[serde(default)]
    pub offset: Option<usize>,
    /// At most this many entries (default 200, at most 2000).
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct SaveInfoParams {
    /// A save file (`GF2-EN-autosave`, `GF2-EN-<digits>`; an absolute
    /// path), or a folder: its saves are listed.
    pub path: String,
    /// Chunks to decode: names or ids from the summary (`variables`,
    /// `families`, `buildings`, ...).
    #[serde(default)]
    pub chunks: Vec<String>,
    /// Decode every chunk.
    #[serde(default)]
    pub all: Option<bool>,
}

// ------------------------------------------------------------ logic

pub fn logic_run(kit: &Toolkit, p: LogicRunParams) -> ToolResult<Output> {
    let ws = kit.workspace_unless(p.original.unwrap_or(false))?;
    let (mut run, warnings) = p.run.prepare(&kit.game, kit.resources(), ws.as_ref())?;
    let offset = p.offset.unwrap_or(0);
    let limit = clamp(p.limit, 200, 2000);
    if p.actors.unwrap_or(false) {
        let actors = run.actors();
        let rows: Vec<Value> = actors
            .iter()
            .enumerate()
            .skip(offset)
            .take(limit)
            .map(|(i, a)| {
                json!({
                    "actor": i,
                    "model": a.model.kind(),
                    "black_box": a.model.is_black_box(),
                    "class": a.class,
                    "name": a.label(),
                    "listens": a
                        .listens
                        .iter()
                        .map(|(m, pin)| format!("{pin}={}", m.name))
                        .collect::<Vec<_>>(),
                })
            })
            .collect();
        return Ok(Output::Json(json!({
            "scope": p.run.scope,
            "total": actors.len(),
            "offset": offset,
            "actors": rows,
            "warnings": warnings,
        })));
    }
    let stop = p.run.run(&mut run);
    let journal = run.journal();
    let sends = journal
        .iter()
        .filter(|e| matches!(e.what, What::Send { .. }))
        .count();
    let page: Vec<Value> = journal
        .iter()
        .skip(offset)
        .take(limit)
        .map(|e| {
            if p.json.unwrap_or(false) {
                serde_json::to_value(e).unwrap_or(Value::Null)
            } else {
                json!(e.line())
            }
        })
        .collect();
    let next = offset + page.len();
    let pending: Vec<String> = run
        .pending()
        .iter()
        .take(10)
        .map(|(t, what)| format!("{t:.3}: {what}"))
        .collect();
    Ok(Output::Json(json!({
        "scope": p.run.scope,
        "objects": run.actors().len(),
        "black_boxes": run.actors().iter().filter(|a| a.model.is_black_box()).count(),
        "steps": run.steps(),
        "messages_sent": sends,
        "time": run.time(),
        "stopped": stop.text(),
        "queued": pending,
        "warnings": warnings,
        "journal_total": journal.len(),
        "offset": offset,
        "journal": page,
        "next_offset": (next < journal.len()).then_some(next),
    })))
}

// ------------------------------------------------------------ scripts

pub fn script_run(kit: &Toolkit, p: ScriptRunParams) -> ToolResult<Output> {
    let _ = kit;
    let source = match (p.graph, p.lua, p.file) {
        (Some(graph), None, None) => TrialSource::Graph(Box::new(
            ScriptGraph::from_json(&graph.to_string())
                .map_err(|e| ToolError(format!("the graph is not a script graph: {e}")))?,
        )),
        (None, Some(lua), None) => TrialSource::Lua {
            source: lua.into_bytes(),
            name: p.name.unwrap_or_else(|| "script.lua".into()),
        },
        (None, None, Some(file)) => TrialSource::read(&super::create::input_file(&file)?)?,
        _ => return fail("give one of `graph`, `lua` or `file`"),
    };
    let report = trial::run(&source, &p.steps, None)?;
    let offset = p.offset.unwrap_or(0);
    let limit = clamp(p.limit, 200, 2000);
    let page: Vec<Value> = report
        .log
        .iter()
        .skip(offset)
        .take(limit)
        .map(|e| {
            if p.json.unwrap_or(false) {
                serde_json::to_value(e).unwrap_or(Value::Null)
            } else {
                json!(e.line())
            }
        })
        .collect();
    let next = offset + page.len();
    Ok(Output::Json(json!({
        "script": report.script,
        "compiler_notes": report.notes,
        "time": report.time,
        "error": report.error,
        "log_total": report.log.len(),
        "offset": offset,
        "log": page,
        "next_offset": (next < report.log.len()).then_some(next),
        "note": "a stand-in for the game: messages, timers and variables are modelled, \
                 the game's own functions are only logged",
    })))
}

// ------------------------------------------------------------ saves

pub fn save_info(kit: &Toolkit, p: SaveInfoParams) -> ToolResult<Output> {
    let _ = kit;
    let path = std::path::PathBuf::from(p.path.trim());
    if path.is_dir() {
        let rows: Vec<Value> = view::save_files(&path)?
            .iter()
            .filter_map(|file| {
                let save = SaveFile::open(file).ok()?;
                Some(json!({
                    "file": file.display().to_string(),
                    "name": save.description,
                    "play_seconds": save.play_seconds,
                    "checksums_ok": save.checksums.ok(),
                }))
            })
            .collect();
        return Ok(Output::Json(json!({ "folder": path, "saves": rows })));
    }
    if !path.is_file() {
        return fail(format!(
            "no file or folder {}: give the absolute path of a save or of the folder of saves",
            path.display()
        ));
    }
    let save = SaveFile::open(&path)
        .map_err(|e| ToolError(format!("{} is not a save: {e}", path.display())))?;
    let chunks: Vec<&save::Chunk> = if p.all.unwrap_or(false) {
        save.chunks.iter().collect()
    } else {
        p.chunks
            .iter()
            .map(|name| {
                save.chunk_named(name).ok_or_else(|| {
                    ToolError(format!(
                        "the save has no chunk `{name}`; chunks: {}",
                        save.chunks
                            .iter()
                            .map(save::Chunk::name)
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                })
            })
            .collect::<ToolResult<_>>()?
    };
    let mut out = view::to_json(&save, &chunks);
    out["summary"] = json!(view::summary(&save));
    out["loads_in_game"] = json!(save.checksums.ok());
    out["note"] = json!("read only: the save file is never written");
    Ok(Output::Json(out))
}

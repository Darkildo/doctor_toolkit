//! A file into an existing resource, read by the resource's type: the
//! JSON of the exports (string table, SimGroup, metamodel, variable
//! dictionary, parts database, navigation graph, dialogue, cut-scene shot),
//! the source or
//! the bytecode of a Lua script, or the data as stored (`raw`).
//!
//! One function for every surface (the MCP tool `import_file`, the editor's
//! "Replace with a file…"): the surfaces only say which resource and which
//! file. Textures, models, sounds and movies have importers of their own.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::game::GameInstance;
use crate::level::navgraph::{GraphRef, save_graph};
use crate::resource::dlg::{self, Dialogue};
use crate::resource::graph::{Graph, TYPE_GRAPH};
use crate::resource::lh2::StringTable;
use crate::resource::names::type_tags;
use crate::resource::trinity::{SeqMaster, TYPE_TRINITY_SEQ_MASTER};
use crate::resource::{logic_import, lua};
use crate::workspace::{ResourceKey, Workspace};
use crate::{Error, Result};

/// The resource a file goes into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileTarget {
    /// Resource `resource` of stream `stream` of the game.
    Game {
        stream: usize,
        resource: usize,
        key: ResourceKey,
    },
    /// A resource the workspace added.
    Added(ResourceKey),
}

impl FileTarget {
    /// The target for resource `resource` of game stream `stream`.
    pub fn game(game: &GameInstance, stream: usize, resource: usize) -> Result<Self> {
        let file = game.open_stream(stream)?;
        let header = file
            .layout()
            .resources
            .get(resource)
            .ok_or_else(|| {
                Error::InvalidArgument(format!("the stream has no resource {resource}"))
            })?
            .header;
        Ok(Self::Game {
            stream,
            resource,
            key: ResourceKey::new(&game.streams()[stream].stream_name(), &header),
        })
    }

    pub fn key(&self) -> &ResourceKey {
        match self {
            Self::Game { key, .. } | Self::Added(key) => key,
        }
    }

    /// The data of the target as the workspace has it.
    pub fn current(&self, game: &GameInstance, ws: &Workspace) -> Result<Vec<u8>> {
        match self {
            Self::Game {
                stream, resource, ..
            } => ws.current_data(game, *stream, *resource),
            Self::Added(key) => ws.data(key)?.ok_or_else(|| {
                Error::InvalidArgument(format!("{key} has no data in the workspace"))
            }),
        }
    }

    /// Records `data` for the target as one operation `label`.
    pub fn record(
        &self,
        game: &GameInstance,
        ws: &mut Workspace,
        data: Vec<u8>,
        label: &str,
    ) -> Result<()> {
        match self {
            Self::Game {
                stream, resource, ..
            } => {
                ws.set_resource(game, *stream, *resource, data, label)?;
            }
            Self::Added(key) => {
                ws.change_resources_and_added(game, label, &[], &[], &[(key.clone(), Some(data))])?;
            }
        }
        Ok(())
    }
}

/// What to import.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileOptions {
    /// Take the file as the stored data of the resource, whatever its type.
    #[serde(default)]
    pub raw: bool,
}

/// What an import did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileImport {
    /// The resource (its workspace key).
    pub resource: String,
    /// Tag of the resource type (`lh2`, `simgroup`, …).
    pub type_tag: String,
    /// False when the file gives what the resource already holds: nothing
    /// was recorded.
    pub changed: bool,
    /// Label of the recorded operation.
    pub label: String,
    /// What was changed, repaired or counted while reading the file.
    pub notes: Vec<String>,
}

fn read(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path)
        .map_err(|e| Error::InvalidArgument(format!("cannot read {}: {e}", path.display())))
}

fn read_json(path: &Path) -> Result<serde_json::Value> {
    let bytes = read(path)?;
    serde_json::from_slice(&bytes)
        .map_err(|e| Error::InvalidArgument(format!("{} is not JSON: {e}", path.display())))
}

/// Whether resources of type `type_id` are read from a file by type (any
/// resource takes a `raw` file).
pub fn is_supported(type_id: u32) -> bool {
    type_id == crate::text::TYPE_LH2
        || type_id == TYPE_GRAPH
        || type_id == lua::TYPE_LUA
        || type_id == dlg::TYPE_DLG
        || type_id == TYPE_TRINITY_SEQ_MASTER
        || logic_import::is_supported(type_id)
}

/// Replaces `target` with what `path` holds, as one undoable operation of
/// the workspace. Nothing is recorded when the file gives the data the
/// resource already has.
pub fn import_file(
    game: &GameInstance,
    ws: &mut Workspace,
    target: &FileTarget,
    path: &Path,
    options: &FileOptions,
) -> Result<FileImport> {
    let key = target.key().clone();
    let tag = type_tags().display(key.type_id).to_string();
    let label = format!(
        "Import {} into {key}",
        path.file_name().unwrap_or_default().to_string_lossy()
    );
    let data = target.current(game, ws)?;
    let mut notes: Vec<String> = Vec::new();
    let done = |changed: bool, notes: Vec<String>| FileImport {
        resource: key.to_string(),
        type_tag: tag.clone(),
        changed,
        label: label.clone(),
        notes,
    };
    let new = if options.raw {
        read(path)?
    } else {
        match key.type_id {
            crate::text::TYPE_LH2 => {
                let mut table = StringTable::parse(&data)?;
                let changed = crate::text::import_json(&mut table, &read_json(path)?)?;
                notes.push(format!("{changed} string(s) changed"));
                table.to_bytes()?
            }
            TYPE_GRAPH => {
                let mut graph: Graph = serde_json::from_value(read_json(path)?).map_err(|e| {
                    Error::InvalidArgument(format!("{} is not a graph: {e}", path.display()))
                })?;
                let old = Graph::parse(&data)?;
                if graph.guid != old.guid {
                    return Err(Error::InvalidArgument(format!(
                        "the file is graph {}, not {} ({key})",
                        graph.guid, old.guid
                    )));
                }
                notes.extend(graph.repair_derived());
                if let FileTarget::Game {
                    stream, resource, ..
                } = target
                {
                    if graph.to_bytes()? == data {
                        return Ok(done(false, notes));
                    }
                    // Links of other graphs into this one follow it.
                    let g = GraphRef {
                        stream: *stream,
                        resource: *resource,
                        graph,
                    };
                    save_graph(game, ws, &g, None, &label)?;
                    return Ok(done(true, notes));
                }
                graph.to_bytes()?
            }
            lua::TYPE_LUA => {
                let bytes = read(path)?;
                if path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("luac"))
                {
                    lua::Chunk::parse(&bytes)?;
                    bytes
                } else {
                    let old = lua::Chunk::parse(&data)?;
                    let name = old.main.source.clone().unwrap_or_default();
                    let chunk = lua::compile(&bytes, &name, old.number_size).map_err(|e| {
                        Error::InvalidArgument(format!(
                            "{}:{}: {}",
                            path.display(),
                            e.line,
                            e.message
                        ))
                    })?;
                    notes.push(format!(
                        "{} function(s), {} instruction(s)",
                        chunk.main.function_count(),
                        chunk.main.instruction_count()
                    ));
                    chunk.to_bytes()
                }
            }
            dlg::TYPE_DLG => {
                let doc = read_json(path)?;
                let part = doc.get("data").cloned().unwrap_or(doc);
                let d: Dialogue = serde_json::from_value(part).map_err(|e| {
                    Error::InvalidArgument(format!("{}: not a dialogue: {e}", path.display()))
                })?;
                d.to_bytes()?
            }
            // A cut-scene shot: the JSON `export` and `resource_json` give.
            TYPE_TRINITY_SEQ_MASTER => {
                let seq: SeqMaster = serde_json::from_value(read_json(path)?).map_err(|e| {
                    Error::InvalidArgument(format!("{} is not a shot: {e}", path.display()))
                })?;
                notes.push(format!(
                    "{} command(s)",
                    seq.blocks.iter().map(|b| b.commands.len()).sum::<usize>()
                ));
                seq.to_bytes()?
            }
            t if logic_import::is_supported(t) => {
                let imported = logic_import::apply_json(t, &data, &read_json(path)?)?;
                notes.extend(imported.changes.iter().map(ToString::to_string));
                imported.data
            }
            _ => {
                return Err(Error::InvalidArgument(format!(
                    "{tag} resources have no import from a file by type: textures, models, \
                     sounds and movies have importers of their own; `raw` takes the data as stored"
                )));
            }
        }
    };
    if new == data {
        return Ok(done(false, notes));
    }
    target.record(game, ws, new, &label)?;
    Ok(done(true, notes))
}

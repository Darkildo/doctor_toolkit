//! Putting a compiled script into a level: a `lua` resource and a
//! `LuaAudioScript` object in the level's SimGroup, as one workspace operation.
//!
//! The resource goes into the stream before its SimGroup, with a new id
//! (`id × 4`, like the game's scripts). The object is a copy of a
//! `LuaAudioScript` of `gf2/globals_global` with attribute 0 (flags; bit 0:
//! run when the level loads), 1 (start message), 2 (stop message) and 3
//! (GUID of the resource). Deploying again replaces the chunk and the
//! settings of the same object; removing takes both out. A script is found
//! by its chunk name (`<name>.lua`), so the graph and its deployment stay
//! linked across undo and `workspace apply`.
//!
//! The graph itself is kept next to the workspace as
//! `<workspace>/scripts/<name>.json` ([`save_graph`]).

use std::fs;
use std::path::PathBuf;

use crate::container::ustream::StreamLayout;
use crate::create;
use crate::game::{GameInstance, ResourceIndex};
use crate::level::{self, Location, TYPE_SIMGROUP};
use crate::resource::lua::{Chunk, TYPE_LUA};
use crate::resource::simgroup::{Guid, SimGroup, Value};
use crate::workspace::{NewResourceEdit, ResourceEdit, ResourceKey, StorageChoice, Workspace};
use crate::{Error, Result};

use super::graph::{ScriptGraph, StartSettings};

/// Class hash of `LuaAudioScript`.
pub const CLASS_LUA_AUDIO_SCRIPT: u32 = 0x07B6_C269;
/// The stream whose SimGroup holds the game's `LuaAudioScript` objects.
pub const TEMPLATE_STREAM: &str = "gf2/globals_global";

/// A script found in a stream.
#[derive(Debug, Clone, PartialEq)]
pub struct Deployed {
    /// Stream name.
    pub stream: String,
    /// Key of the `lua` resource.
    pub key: ResourceKey,
    /// Whether the workspace added the resource (otherwise it is the game's).
    pub added: bool,
    /// `LuaAudioScript` objects of the stream's SimGroup that run it.
    pub objects: Vec<usize>,
    /// Start settings read from the first object.
    pub start: Option<StartSettings>,
}

impl Deployed {
    pub fn guid(&self) -> Guid {
        Guid(create::guid_bytes(&self.key.ids))
    }
}

/// What [`deploy`] did.
#[derive(Debug, Clone, PartialEq)]
pub struct Deployment {
    pub stream: String,
    pub key: ResourceKey,
    /// Index of the `LuaAudioScript` object in the SimGroup.
    pub object: usize,
    /// Whether an earlier deployment was updated.
    pub updated: bool,
    /// Whether anything changed in the workspace.
    pub changed: bool,
}

/// Whether a stream belongs to the main menu (`gf2/frontend…`), where the
/// game runs no simulation and so no Lua: there a `LuaAudioScript` that
/// finds its chunk reads a null Lua state and the game crashes (bench,
/// `0x00480E03`).
pub fn is_front_end(stream: &str) -> bool {
    stream.to_ascii_lowercase().starts_with("gf2/frontend")
}

/// Index of the SimGroup resource of a stream (current file).
pub fn simgroup_of(game: &GameInstance, stream: usize) -> Result<usize> {
    let s = game.open_stream(stream)?;
    s.layout()
        .resources
        .iter()
        .position(|r| r.header.type_id == TYPE_SIMGROUP)
        .ok_or_else(|| {
            Error::InvalidArgument(format!(
                "{} has no SimGroup: pick a level stream with objects",
                game.streams()[stream].stream_name()
            ))
        })
}

fn stream_index(game: &GameInstance, name: &str) -> Result<usize> {
    game.find_stream(name)
        .ok_or_else(|| Error::InvalidArgument(format!("no stream {name}")))
}

/// `lua` resources of a stream as the workspace has them: key, data, added.
fn scripts_of(
    game: &GameInstance,
    ws: &Workspace,
    stream: usize,
) -> Result<Vec<(ResourceKey, Vec<u8>, bool)>> {
    let name = game.streams()[stream].stream_name();
    let file = game.open_stream(stream)?;
    let original = original_layout(game, stream)?;
    let mut out: Vec<(ResourceKey, Vec<u8>, bool)> = Vec::new();
    for (i, r) in file.layout().resources.iter().enumerate() {
        if r.header.type_id != TYPE_LUA {
            continue;
        }
        let key = ResourceKey::new(&name, &r.header);
        let change = ws.change(&key);
        if change.is_some_and(|c| c.is_added()) {
            continue; // listed below with its current data
        }
        if change.is_none() && key.find_in(&original).is_none() {
            continue; // applied earlier, since removed from the workspace
        }
        let data = match ws.data(&key)? {
            Some(d) => d,
            None => file.resource_data(i)?,
        };
        out.push((key, data, false));
    }
    for c in ws.changes() {
        if c.is_added() && c.key.stream == name && c.key.type_id == TYPE_LUA {
            out.push((c.key.clone(), ws.store().get(&c.payload)?, true));
        }
    }
    Ok(out)
}

fn chunk_source(data: &[u8]) -> Option<String> {
    Chunk::parse(data).ok().map(|c| c.source())
}

fn start_of(group: &SimGroup, object: usize) -> StartSettings {
    let o = &group.objects[object];
    let int = |i| match group.attr(o, CLASS_LUA_AUDIO_SCRIPT, i) {
        Some(Value::Int(n)) => *n,
        _ => 0,
    };
    let text = |i| match group.attr(o, CLASS_LUA_AUDIO_SCRIPT, i) {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        _ => None,
    };
    StartSettings {
        on_load: int(0) & 1 != 0,
        message: text(1),
        stop_message: text(2),
    }
}

/// `LuaAudioScript` objects of `group` that run the resource with `guid`.
pub fn objects_running(group: &SimGroup, guid: &Guid) -> Vec<usize> {
    group
        .objects
        .iter()
        .enumerate()
        .filter(|(_, o)| o.class == CLASS_LUA_AUDIO_SCRIPT)
        .filter(|(_, o)| {
            matches!(group.attr(o, CLASS_LUA_AUDIO_SCRIPT, 3), Some(Value::Guid(g)) if g == guid)
        })
        .map(|(i, _)| i)
        .collect()
}

/// The script with chunk name `chunk_name` in stream `stream`, if any.
pub fn find(
    game: &GameInstance,
    ws: &Workspace,
    stream: usize,
    chunk_name: &str,
) -> Result<Option<Deployed>> {
    let name = game.streams()[stream].stream_name();
    let Some((key, _, added)) = scripts_of(game, ws, stream)?
        .into_iter()
        .find(|(_, data, _)| {
            chunk_source(data).is_some_and(|s| s.eq_ignore_ascii_case(chunk_name))
        })
    else {
        return Ok(None);
    };
    let objects = match simgroup_of(game, stream) {
        Ok(g) => {
            let group = level::edit::current_group(
                game,
                ws,
                Location {
                    stream,
                    resource: g,
                },
            )?;
            let guid = Guid(create::guid_bytes(&key.ids));
            let objects = objects_running(&group, &guid);
            let start = objects.first().map(|&o| start_of(&group, o));
            return Ok(Some(Deployed {
                stream: name,
                key,
                added,
                objects,
                start,
            }));
        }
        Err(_) => Vec::new(),
    };
    Ok(Some(Deployed {
        stream: name,
        key,
        added,
        objects,
        start: None,
    }))
}

/// Scripts the workspace added to a stream (made from graphs).
pub fn list(game: &GameInstance, ws: &Workspace, stream: usize) -> Result<Vec<Deployed>> {
    let mut out = Vec::new();
    for (key, data, added) in scripts_of(game, ws, stream)? {
        if !added {
            continue;
        }
        if let Some(src) = chunk_source(&data)
            && let Some(d) = find(game, ws, stream, &src)?
            && d.key == key
        {
            out.push(d);
        }
    }
    Ok(out)
}

/// The `LuaAudioScript` template: a SimGroup and the index of the object.
fn template(game: &GameInstance, ws: &Workspace) -> Result<(Location, SimGroup, usize)> {
    let stream = stream_index(game, TEMPLATE_STREAM)?;
    let resource = simgroup_of(game, stream)?;
    let at = Location { stream, resource };
    let group = level::edit::current_group(game, ws, at)?;
    let object = group
        .objects
        .iter()
        .position(|o| o.class == CLASS_LUA_AUDIO_SCRIPT)
        .ok_or_else(|| {
            Error::InvalidArgument(format!("{TEMPLATE_STREAM} has no LuaAudioScript object"))
        })?;
    Ok((at, group, object))
}

fn set_settings(
    group: &mut SimGroup,
    object: usize,
    start: &StartSettings,
    guid: Guid,
) -> Result<()> {
    let c = CLASS_LUA_AUDIO_SCRIPT;
    group.set_class_attr(object, c, 0, Some(Value::Int(i32::from(start.on_load))))?;
    let text = |s: &Option<String>| {
        s.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| Value::String(s.to_owned()))
    };
    group.set_class_attr(object, c, 1, text(&start.message))?;
    group.set_class_attr(object, c, 2, text(&start.stop_message))?;
    group.set_class_attr(object, c, 3, Some(Value::Guid(guid)))?;
    Ok(())
}

/// Puts a compiled script into the level stream `stream` (it must hold a
/// SimGroup), or updates it there. One undoable workspace operation.
pub fn deploy(
    game: &GameInstance,
    index: &ResourceIndex,
    ws: &mut Workspace,
    stream: usize,
    graph: &ScriptGraph,
    chunk: &[u8],
) -> Result<Deployment> {
    let start = &graph.start;
    if !start.on_load && start.message.as_deref().is_none_or(|m| m.trim().is_empty()) {
        return Err(Error::InvalidArgument(
            "the script would never start: run it on load or give a start message".into(),
        ));
    }
    let stream_name = game.streams()[stream].stream_name();
    if is_front_end(&stream_name) {
        return Err(Error::InvalidArgument(format!(
            "{stream_name} is the main menu: it has no game simulation, and a \
             LuaAudioScript there crashes the game (docs/scripts.md); pick a level stream"
        )));
    }
    let group_at = Location {
        stream,
        resource: simgroup_of(game, stream)?,
    };
    let chunk_name = graph.chunk_name();
    let existing = find(game, ws, stream, &chunk_name)?;
    if let Some(e) = &existing
        && !e.added
    {
        return Err(Error::InvalidArgument(format!(
            "{stream_name} already has a script of the game named {chunk_name}: rename the graph"
        )));
    }
    let mut group = level::edit::current_group(game, ws, group_at)?;
    let file = game.open_stream(stream)?;
    let group_key = ResourceKey::new(
        &stream_name,
        &file.layout().resources[group_at.resource].header,
    );

    let (key, additions, added) = match &existing {
        Some(e) => (
            e.key.clone(),
            Vec::new(),
            vec![(e.key.clone(), Some(chunk.to_vec()))],
        ),
        None => {
            let mut used = ws.used_ids(index);
            let id = used.allocate(&format!("script:{stream_name}:{chunk_name}"));
            let defaults = create::type_defaults(TYPE_LUA);
            let header = crate::container::ustream::ResourceHeader {
                mem_policy: defaults.mem_policy,
                alignment: defaults.alignment,
                type_id: TYPE_LUA,
                ids: [id; 4],
                data_size: 0,
            };
            let before = original_key(game, stream, &group_key)?;
            let key = ResourceKey::new(&stream_name, &header);
            let addition = NewResourceEdit {
                stream: stream_name.clone(),
                header,
                data: chunk.to_vec(),
                storage: StorageChoice::Keep,
                before,
            };
            (key, vec![addition], Vec::new())
        }
    };
    let guid = Guid(create::guid_bytes(&key.ids));
    let running = objects_running(&group, &guid);
    let object = match running.first() {
        Some(&o) => o,
        None => {
            let seed = format!("{stream_name}/script/{chunk_name}");
            let new = level::edit::new_guid(&group, &seed, &|_| false);
            let (at, template, t) = template(game, ws)?;
            if at == group_at {
                group.duplicate_object(t, new)?
            } else {
                group.copy_object_from(&template, t, new)?
            }
        }
    };
    set_settings(&mut group, object, start, guid)?;
    let edits = vec![(
        stream,
        vec![ResourceEdit {
            index: group_at.resource,
            data: group.encode(),
            storage: StorageChoice::Keep,
        }],
    )];
    let label = format!(
        "{} script {chunk_name} in {stream_name}",
        if existing.is_some() {
            "Update"
        } else {
            "Deploy"
        }
    );
    let changed = ws
        .change_resources_and_added(game, &label, &edits, &additions, &added)?
        .is_some();
    Ok(Deployment {
        stream: stream_name,
        key,
        object,
        updated: existing.is_some(),
        changed,
    })
}

/// The key of `key` if the resource is in the original stream (a place to
/// insert before), else `None`.
fn original_key(
    game: &GameInstance,
    stream: usize,
    key: &ResourceKey,
) -> Result<Option<ResourceKey>> {
    let layout = original_layout(game, stream)?;
    Ok(key.find_in(&layout).map(|_| key.clone()))
}

/// Layout of the stream as the unmodified game has it.
fn original_layout(game: &GameInstance, stream: usize) -> Result<StreamLayout> {
    let path = game.original_path(&game.streams()[stream]);
    Ok(crate::game::StreamFile::open(&path)?.layout().clone())
}

/// Takes a deployed script out of a stream: its objects and its resource.
/// Returns whether anything was there.
pub fn remove(
    game: &GameInstance,
    ws: &mut Workspace,
    stream: usize,
    chunk_name: &str,
) -> Result<bool> {
    let Some(d) = find(game, ws, stream, chunk_name)? else {
        return Ok(false);
    };
    if !d.added {
        return Err(Error::InvalidArgument(format!(
            "{chunk_name} is a script of the game, not one made from a graph"
        )));
    }
    let group_at = Location {
        stream,
        resource: simgroup_of(game, stream)?,
    };
    let mut group = level::edit::current_group(game, ws, group_at)?;
    for &o in d.objects.iter().rev() {
        group.remove_object(o)?;
    }
    let edits = if d.objects.is_empty() {
        Vec::new()
    } else {
        vec![(
            stream,
            vec![ResourceEdit {
                index: group_at.resource,
                data: group.encode(),
                storage: StorageChoice::Keep,
            }],
        )]
    };
    let label = format!("Remove script {chunk_name} from {}", d.stream);
    ws.change_resources_and_added(game, &label, &edits, &[], &[(d.key.clone(), None)])?;
    Ok(true)
}

/// Directory of the graphs kept with a workspace.
pub fn graphs_dir(ws: &Workspace) -> PathBuf {
    ws.dir().join("scripts")
}

/// Saves a graph as `<workspace>/scripts/<name>.json`.
pub fn save_graph(ws: &Workspace, graph: &ScriptGraph) -> Result<PathBuf> {
    let dir = graphs_dir(ws);
    fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;
    let path = dir.join(format!("{}.json", graph.name));
    crate::atomic::write(&path, graph.to_json().as_bytes())?;
    Ok(path)
}

/// Graphs kept with a workspace, by name.
pub fn saved_graphs(ws: &Workspace) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = fs::read_dir(graphs_dir(ws))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .filter_map(|p| Some((p.file_stem()?.to_string_lossy().into_owned(), p)))
        .collect();
    out.sort();
    out
}

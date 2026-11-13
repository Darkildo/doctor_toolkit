//! The writing tools (`--allow-edits`): edits recorded in the workspace,
//! never in the game files. Each call is one undoable operation of the
//! workspace (several recording steps are joined with
//! [`Workspace::squash`]) and answers with the diff of what it changed.
//! The workspace is written under its lock ([`Toolkit::write_workspace`]).

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use doctor_toolkit::game::StreamFile;
use doctor_toolkit::level::edit::{self as level_edit, ModelChoice};
use doctor_toolkit::level::navgraph::{GraphRef, save_graph};
use doctor_toolkit::level::{Location, TYPE_SIMGROUP};
use doctor_toolkit::resource::graph::{Graph, Neighbourhood, TYPE_GRAPH};
use doctor_toolkit::resource::lh2::StringTable;
use doctor_toolkit::resource::names::sim_classes;
use doctor_toolkit::resource::simgroup::{Guid, Kind, SimGroup, Value as SimValue};
use doctor_toolkit::resource::simgroup_edit::{self, Transform, describe};
use doctor_toolkit::script::{self, deploy, graph::ScriptGraph};
use doctor_toolkit::text::{self, NewString, TableRef};
use doctor_toolkit::workspace::{ResourceEdit, ResourceKey, StorageChoice, Workspace};

use super::workspace::detail_lines;
use super::{Output, hex, parse_hash, parse_object_id};
use crate::state::{ToolError, ToolResult, Toolkit, fail};

// ------------------------------------------------------------ parameters

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct SetAttributeParams {
    /// The object: `<stream>#<resource>:<object>` as `level_objects` and
    /// `logic_graph` print it.
    pub object: String,
    /// Attributes to set: `"Class.attribute": "value"`. Class and attribute
    /// by name (`TriggerBase.4`, `CSystemCommands.CMD_SetName`) or as
    /// hexadecimal class hash and attribute index (`b390b11a.2`). Values as
    /// text: numbers, `0x…` hashes or names hashed by the attribute's kind,
    /// strings, `x,y,z` vectors. An empty value resets the attribute to its
    /// default.
    pub values: BTreeMap<String, String>,
    /// Show another model: a metamodel id in hexadecimal.
    #[serde(default)]
    pub model: Option<String>,
    /// Record the change even if the object's class cannot show its model
    /// (the game may crash).
    #[serde(default)]
    pub force: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct MoveObjectParams {
    /// The object: `<stream>#<resource>:<object>`.
    pub object: String,
    /// New position `[x, y, z]` (Y is up).
    #[serde(default)]
    pub position: Option<[f32; 3]>,
    /// Or: move by `[dx, dy, dz]`.
    #[serde(default)]
    pub offset: Option<[f32; 3]>,
    /// Rotation in degrees about X, then Y, then Z (replaces the current one).
    #[serde(default)]
    pub rotation: Option<[f32; 3]>,
    /// Scale along the object's axes (replaces the current one).
    #[serde(default)]
    pub scale: Option<[f32; 3]>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct DuplicateObjectParams {
    /// The object to copy: `<stream>#<resource>:<object>`. The copy shares
    /// its attribute packets and gets a new GUID.
    pub object: String,
    /// Move the copy by `[dx, dy, dz]`.
    #[serde(default)]
    pub offset: Option<[f32; 3]>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct AddObjectParams {
    /// The SimGroup to add to: `<stream>#<resource>`.
    pub group: String,
    /// The template object (any SimGroup): `<stream>#<resource>:<object>`.
    pub template: String,
    /// Show this metamodel (hexadecimal id); it must be in the group's
    /// stream or one loaded before it.
    #[serde(default)]
    pub model: Option<String>,
    /// Or: show a clone of the metamodel `<stream>#<resource>`, made in the
    /// group's stream with the meshes of its own stream.
    #[serde(default)]
    pub clone_model: Option<String>,
    /// With `clone_model`: give the clone collision of its own built from
    /// its meshes: `mesh`, `hull` or `box`.
    #[serde(default)]
    pub collision: Option<String>,
    /// Position `[x, y, z]` (Y is up).
    #[serde(default)]
    pub position: Option<[f32; 3]>,
    /// Add the object even if its class cannot show the model (the game may crash).
    #[serde(default)]
    pub force: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct RemoveObjectParams {
    /// The object: `<stream>#<resource>:<object>`. Objects after it move up by one.
    pub object: String,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct EditStringParams {
    /// The entry: its hash (`0x…`) or its name (hashed as the game does).
    pub entry: String,
    /// The new text.
    pub text: String,
    /// Only this language column (its code, or `text`, `text1`, … as
    /// `strings` shows); every column when not given.
    #[serde(default)]
    pub language: Option<String>,
    /// Only this string table (a resource key from `strings` or `search`);
    /// every table with the entry when not given.
    #[serde(default)]
    pub table: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct AddStringParams {
    /// Entry name: letters, digits and `_`, starting with a letter.
    /// Interface movies show it as `$NAME`, SimGroup hash attributes store
    /// its hash.
    pub name: String,
    /// The text for every language without its own translation.
    pub text: String,
    /// Texts by language code, e.g. `{"es": "Hola"}`.
    #[serde(default)]
    pub translations: BTreeMap<String, String>,
    /// Add to these tables (resource keys) instead of the tables of the
    /// game's languages.
    #[serde(default)]
    pub tables: Vec<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct EditScriptParams {
    /// A level stream with a SimGroup, e.g. `gf2/globals_global` (always
    /// loaded) or a city or mission stream.
    pub stream: String,
    /// The script graph (format `doctor-toolkit-script`, see the resource
    /// `doctor-toolkit://docs/scripts.md`): `name`, `start`, `variables`,
    /// `nodes`, `links`. A graph of the same name already in the stream is
    /// replaced.
    #[serde(default)]
    pub graph: Option<Value>,
    /// Instead: take the script of this name out of the stream.
    #[serde(default)]
    pub remove: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ScriptsParams {
    /// A level stream.
    pub stream: String,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct NodeMove {
    pub node: usize,
    pub position: [f32; 3],
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct Flags {
    /// A node (`node_flags`) or the start of a link (`link_flags`).
    pub node: usize,
    /// The end of the link (`link_flags` only).
    #[serde(default)]
    pub to: Option<usize>,
    pub flags: u16,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct GraphEditParams {
    /// The navigation graph: a resource key of type `graph`.
    pub graph: String,
    /// Move nodes.
    #[serde(default)]
    pub move_nodes: Vec<NodeMove>,
    /// Add nodes without links at these points.
    #[serde(default)]
    pub add_nodes: Vec<[f32; 3]>,
    /// Add nodes linked both ways with the nearest nodes.
    #[serde(default)]
    pub attach: Vec<[f32; 3]>,
    /// For `attach`: most nodes to link to (default 2).
    #[serde(default)]
    pub max_links: Option<usize>,
    /// For `attach`: only nodes closer than this, metres (default 15).
    #[serde(default)]
    pub max_distance: Option<f32>,
    /// Add links `[from, to]`.
    #[serde(default)]
    pub link: Vec<[usize; 2]>,
    /// Add each link of `link` both ways.
    #[serde(default)]
    pub two_way: Option<bool>,
    /// Remove links `[from, to]`.
    #[serde(default)]
    pub unlink: Vec<[usize; 2]>,
    /// Set flags of links (vault 0x2, ladder 0x4, pathing only 0x8,
    /// crosswalk 0x10, fall 0x20).
    #[serde(default)]
    pub link_flags: Vec<Flags>,
    /// Set flags of nodes (disabled 0x4000).
    #[serde(default)]
    pub node_flags: Vec<Flags>,
    /// Set the graph type flags (ground 0x1, patrol 0x2, vehicle 0x4, ped
    /// 0x8, parking 0x10, audio 0x20).
    #[serde(default)]
    pub graph_flags: Option<u16>,
    /// Remove a node and its links (links of other graphs into this one follow).
    #[serde(default)]
    pub remove_node: Option<usize>,
    /// Build the R-tree again.
    #[serde(default)]
    pub rebuild_rtree: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct UndoParams {}

// ------------------------------------------------------------ helpers

/// The answer of a writing tool: what was done, the operation and the
/// diff of the resources it changed.
fn finish(ws: &mut Workspace, since: usize, label: &str, done: Value) -> ToolResult<Output> {
    if ws.head() > since + 1 {
        ws.squash(since, label)?;
    }
    if ws.head() <= since {
        return Ok(Output::Json(json!({
            "done": done,
            "changed": false,
            "note": "nothing changed: the workspace already has this",
        })));
    }
    let op = &ws.operations()[ws.head() - 1];
    let keys: BTreeSet<ResourceKey> = op.edits.iter().map(|e| e.key.clone()).collect();
    Ok(Output::Json(json!({
        "done": done,
        "changed": true,
        "operation": op.label,
        "undo": ws.undo_label(),
        "diff": diff_of(ws, &keys),
        "game_files": "unchanged: the edit is in the workspace only (`apply` writes it into the game)",
    })))
}

/// `workspace_diff` of `keys`; a key the edit took out of the workspace
/// (an undone addition) is listed as back to the original.
fn diff_of(ws: &Workspace, keys: &BTreeSet<ResourceKey>) -> Vec<Value> {
    let entries = ws.diff();
    keys.iter()
        .map(|key| match entries.iter().find(|e| &e.key == key) {
            Some(e) => json!({
                "key": e.key.to_string(),
                "label": e.label,
                "added": e.added,
                "original_bytes": e.original_size,
                "new_bytes": e.new_size,
                "detail": detail_lines(ws, &e.key)
                    .unwrap_or_else(|err| vec![format!("cannot compare: {}", err.0)]),
            }),
            None => json!({
                "key": key.to_string(),
                "detail": ["as in the game files (no change in the workspace)"],
            }),
        })
        .collect()
}

/// `<stream>#<resource>`.
fn parse_group(kit: &Toolkit, text: &str) -> ToolResult<Location> {
    let (stream, resource) = text.trim().rsplit_once('#').ok_or_else(|| {
        ToolError(format!(
            "`{text}`: give a SimGroup as `<stream>#<resource>` (as `level_objects` prints it)"
        ))
    })?;
    Ok(Location {
        stream: kit.stream(stream)?,
        resource: resource
            .parse()
            .map_err(|_| ToolError(format!("`{resource}` is not a resource number")))?,
    })
}

/// An object id, which must name an object.
fn object(kit: &Toolkit, text: &str) -> ToolResult<(Location, usize)> {
    parse_object_id(kit, text)?.ok_or_else(|| {
        ToolError(format!(
            "`{text}`: give an object as `<stream>#<resource>:<object>` (as `level_objects` prints it)"
        ))
    })
}

/// The SimGroup at `at` as the workspace has it, checked to be one.
fn group(kit: &Toolkit, ws: &Workspace, at: Location) -> ToolResult<SimGroup> {
    let header = kit
        .game
        .open_stream(at.stream)?
        .layout()
        .resources
        .get(at.resource)
        .map(|r| r.header)
        .ok_or_else(|| ToolError(format!("no resource {} in that stream", at.resource)))?;
    if header.type_id != TYPE_SIMGROUP {
        return fail(format!("resource {} is not a SimGroup", at.resource));
    }
    Ok(level_edit::current_group(&kit.game, ws, at)?)
}

fn class_label(g: &SimGroup, o: usize) -> String {
    g.objects.get(o).map_or_else(String::new, |o| {
        sim_classes()
            .get(o.class)
            .map_or_else(|| format!("{:08x}", o.class), str::to_owned)
    })
}

fn parse_hex(text: &str) -> ToolResult<u32> {
    let t = text.trim();
    u32::from_str_radix(t.trim_start_matches("0x").trim_start_matches("0X"), 16)
        .map_err(|_| ToolError(format!("`{t}` is not a hexadecimal id")))
}

/// Parses `text` for an attribute of `kind`; an unknown kind follows the
/// stored scalar value.
fn parse_value(
    kind: Option<Kind>,
    current: Option<&SimValue>,
    text: &str,
) -> ToolResult<Option<SimValue>> {
    let kind = kind.or(match current {
        Some(SimValue::Int(_)) => Some(Kind::Int),
        Some(SimValue::Float(_)) => Some(Kind::Float),
        Some(SimValue::Hash(_)) => Some(Kind::Hash),
        _ => None,
    });
    match kind {
        Some(kind) => Ok(simgroup_edit::parse_value(kind, text)?),
        None if text.is_empty() => Ok(None),
        None => {
            fail("the kind of this attribute is not known, so its value cannot be read from text")
        }
    }
}

/// Problems of an edited SimGroup the game may crash on, unless forced.
fn check_models(
    kit: &Toolkit,
    ws: &Workspace,
    before: &SimGroup,
    after: &SimGroup,
    force: bool,
) -> ToolResult<()> {
    if force {
        return Ok(());
    }
    let problems = doctor_toolkit::level::compat::check_edit(
        &kit.game,
        kit.resources(),
        Some(ws),
        Some(before),
        after,
    )?;
    match problems.first() {
        Some(p) => fail(format!("{p} (`force: true` records it anyway)")),
        None => Ok(()),
    }
}

fn stream_name(kit: &Toolkit, stream: usize) -> String {
    kit.game.streams()[stream].stream_name()
}

// ------------------------------------------------------------ SimGroup objects

pub fn set_attribute(kit: &Toolkit, p: SetAttributeParams) -> ToolResult<Output> {
    if p.values.is_empty() && p.model.is_none() {
        return fail("give `values` (\"Class.attribute\": \"value\") or `model`");
    }
    let (at, o) = object(kit, &p.object)?;
    let mut access = kit.write_workspace(&format!(
        "set_attribute: {} of {}",
        p.values.keys().cloned().collect::<Vec<_>>().join(", "),
        p.object
    ))?;
    let ws = &mut access.ws;
    let mut g = group(kit, ws, at)?;
    let original = g.clone();
    let object = g
        .objects
        .get(o)
        .ok_or_else(|| ToolError(format!("the SimGroup has {} object(s)", g.objects.len())))?
        .clone();
    let schema = doctor_toolkit::resource::simgroup::Schema::gf2();
    let mut changes = Vec::new();
    for (target, text) in &p.values {
        let (class, attr) = target
            .split_once('.')
            .ok_or_else(|| ToolError(format!("`{target}`: Class.attribute expected")))?;
        let slot = object
            .packets
            .iter()
            .position(|&i| {
                let packet = &g.packets[i];
                packet
                    .class_name
                    .is_some_and(|n| n.eq_ignore_ascii_case(class))
                    || parse_hex(class).is_ok_and(|h| h == packet.class.0)
            })
            .ok_or_else(|| {
                let classes: Vec<String> = object
                    .packets
                    .iter()
                    .map(|&i| {
                        let packet = &g.packets[i];
                        packet
                            .class_name
                            .map_or_else(|| hex(packet.class.0), str::to_owned)
                    })
                    .collect();
                ToolError(format!(
                    "the object has no packet of class `{class}`; its packets: {}",
                    classes.join(", ")
                ))
            })?;
        let packet = &g.packets[object.packets[slot]];
        let class_hash = packet.class.0;
        let index = match attr.parse::<u16>() {
            Ok(index) => index,
            Err(_) => (0..packet.attr_count)
                .find(|&i| {
                    schema
                        .get(class_hash, i)
                        .and_then(|info| info.name)
                        .is_some_and(|n| n.eq_ignore_ascii_case(attr))
                })
                .ok_or_else(|| {
                    ToolError(format!("class `{class}` has no attribute named `{attr}`"))
                })?,
        };
        let current = packet
            .attrs
            .iter()
            .find(|a| a.index == index)
            .map(|a| a.value.clone());
        let value = parse_value(g.attr_kind(class_hash, index), current.as_ref(), text)
            .map_err(|e| ToolError(format!("`{target}`: {}", e.0)))?;
        if g.set_attr(o, slot, index, value.clone())? {
            changes.push(json!({
                "attribute": target,
                "old": describe(current.as_ref()),
                "new": describe(value.as_ref()),
            }));
        }
    }
    if let Some(model) = &p.model {
        let id = parse_hex(model)?;
        let before = g.model(&object);
        if g.set_model(o, Some(id))? {
            changes.push(json!({
                "attribute": "model",
                "old": before.map(hex),
                "new": hex(id),
            }));
        }
    }
    if changes.is_empty() {
        return Ok(Output::Json(
            json!({ "changed": false, "note": "the object already has these values" }),
        ));
    }
    check_models(kit, ws, &original, &g, p.force.unwrap_or(false))?;
    let label = format!(
        "Edit object {o} ({}) in {}#{}",
        class_label(&g, o),
        stream_name(kit, at.stream),
        at.resource
    );
    let since = ws.head();
    ws.set_resource(&kit.game, at.stream, at.resource, g.encode(), &label)?;
    finish(
        ws,
        since,
        &label,
        json!({ "object": p.object, "changes": changes }),
    )
}

pub fn move_object(kit: &Toolkit, p: MoveObjectParams) -> ToolResult<Output> {
    if p.position.is_none() && p.offset.is_none() && p.rotation.is_none() && p.scale.is_none() {
        return fail("give `position`, `offset`, `rotation` or `scale`");
    }
    if p.position.is_some() && p.offset.is_some() {
        return fail("give `position` or `offset`, not both");
    }
    let (at, o) = object(kit, &p.object)?;
    let mut access = kit.write_workspace(&format!("move_object: {}", p.object))?;
    let ws = &mut access.ws;
    let g = group(kit, ws, at)?;
    let object = g
        .objects
        .get(o)
        .ok_or_else(|| ToolError(format!("the SimGroup has {} object(s)", g.objects.len())))?;
    let current = g.transform(object).ok_or_else(|| {
        ToolError(format!(
            "object {o} has no transform (CMD_LoadMatrix): it is not placed in the world"
        ))
    })?;
    let mut m = current;
    if p.rotation.is_some() || p.scale.is_some() {
        let mut t = Transform::from_matrix(&current);
        if let Some(r) = p.rotation {
            t.rotation = r;
        }
        if let Some(s) = p.scale {
            t.scale = s;
        }
        m = t.to_matrix();
        m[3] = current[3];
    }
    if let Some(pos) = p.position {
        m[3][..3].copy_from_slice(&pos);
    }
    if let Some(d) = p.offset {
        for (i, v) in d.iter().enumerate() {
            m[3][i] += v;
        }
    }
    let since = ws.head();
    level_edit::set_transform(&kit.game, ws, at, o, m)?;
    let label = ws
        .operations()
        .get(ws.head().wrapping_sub(1))
        .map(|op| op.label.clone())
        .unwrap_or_default();
    finish(
        ws,
        since,
        &label,
        json!({
            "object": p.object,
            "old_position": &current[3][..3],
            "new_position": &m[3][..3],
        }),
    )
}

pub fn duplicate_object(kit: &Toolkit, p: DuplicateObjectParams) -> ToolResult<Output> {
    let (at, o) = object(kit, &p.object)?;
    let taken = kit.object_guids();
    let mut access = kit.write_workspace(&format!("duplicate_object: {}", p.object))?;
    let ws = &mut access.ws;
    group(kit, ws, at)?;
    let since = ws.head();
    let (copy, guid) = level_edit::duplicate(
        &kit.game,
        ws,
        at,
        o,
        p.offset.unwrap_or_default(),
        &|g: &Guid| taken.contains(g),
    )?;
    let label = ws.operations()[ws.head() - 1].label.clone();
    finish(
        ws,
        since,
        &label,
        json!({
            "object": p.object,
            "copy": super::object_id(kit, at, copy),
            "guid": guid.to_string(),
        }),
    )
}

pub fn add_object(kit: &Toolkit, p: AddObjectParams) -> ToolResult<Output> {
    let at = parse_group(kit, &p.group)?;
    let (template_group, template) = object(kit, &p.template)?;
    let model = match (&p.model, &p.clone_model) {
        (Some(_), Some(_)) => return fail("give `model` or `clone_model`, not both"),
        (Some(id), None) => ModelChoice::Existing(parse_hex(id)?),
        (None, Some(from)) => {
            let from = parse_group(kit, from)?;
            match &p.collision {
                Some(kind) => ModelChoice::CloneWithCollision(
                    from,
                    kind.parse()
                        .map_err(|e| ToolError(format!("`collision`: {e}")))?,
                    None,
                ),
                None => ModelChoice::Clone(from),
            }
        }
        (None, None) => {
            if p.collision.is_some() {
                return fail("`collision` goes with `clone_model`");
            }
            ModelChoice::Template
        }
    };
    let taken = kit.object_guids();
    let mut access = kit.write_workspace(&format!(
        "add_object: a copy of {} in {}",
        p.template, p.group
    ))?;
    let ws = &mut access.ws;
    group(kit, ws, at)?;
    let since = ws.head();
    let added = level_edit::add(
        &kit.game,
        kit.resources(),
        ws,
        at,
        (template_group, template),
        model,
        p.position,
        &|g: &Guid| taken.contains(g),
        !p.force.unwrap_or(false),
    )?;
    // Cloning a model and adding the object are two recordings: one operation.
    let label = ws.operations()[ws.head() - 1].label.clone();
    finish(
        ws,
        since,
        &label,
        json!({
            "object": super::object_id(kit, at, added.object),
            "guid": added.guid.to_string(),
            "model": added.model.map(hex),
            "cloned": added.cloned.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "collision": if p.collision.is_some() {
                "its own, built from the clone's meshes"
            } else {
                "that of its model"
            },
        }),
    )
}

pub fn remove_object(kit: &Toolkit, p: RemoveObjectParams) -> ToolResult<Output> {
    let (at, o) = object(kit, &p.object)?;
    let mut access = kit.write_workspace(&format!("remove_object: {}", p.object))?;
    let ws = &mut access.ws;
    let g = group(kit, ws, at)?;
    let class = class_label(&g, o);
    let since = ws.head();
    let dangling = level_edit::remove(&kit.game, ws, at, o)?;
    let label = ws.operations()[ws.head() - 1].label.clone();
    finish(
        ws,
        since,
        &label,
        json!({
            "removed": p.object,
            "class": class,
            "dangling_references": dangling,
            "note": if dangling > 0 {
                "attributes of other objects still name its GUID; they are left as they are"
            } else {
                "objects after it in the SimGroup moved up by one"
            },
        }),
    )
}

// ------------------------------------------------------------ strings

/// Column names of a string table as the game index (and `strings`) name them.
fn columns(table: &StringTable) -> Vec<String> {
    if table.languages.is_empty() {
        (0..table.strings.len())
            .map(|i| {
                if i == 0 {
                    "text".to_owned()
                } else {
                    format!("text{i}")
                }
            })
            .collect()
    } else {
        table.languages.clone()
    }
}

/// An entry hash: `0x…`/eight hex digits, or a name.
fn entry_hash(text: &str) -> u32 {
    parse_hash(text).unwrap_or_else(|_| doctor_toolkit::hash::string_id(text.trim()))
}

fn table_ref(kit: &Toolkit, text: &str) -> ToolResult<TableRef> {
    let id = super::resolve(kit, text)?;
    let index = kit.index();
    let record = index.record(id);
    if record.header.type_id != text::TYPE_LH2 {
        return fail(format!("{} is not a string table", index.key(id)));
    }
    Ok(TableRef {
        stream: index.stream_of(id),
        resource: record.resource as usize,
        language: None,
    })
}

pub fn edit_string(kit: &Toolkit, p: EditStringParams) -> ToolResult<Output> {
    if p.text.contains('\0') {
        return fail("the text contains a NUL character");
    }
    let hash = entry_hash(&p.entry);
    let tables = match &p.table {
        Some(t) => vec![table_ref(kit, t)?],
        None => text::all_tables(kit.resources()),
    };
    let language = p.language.as_deref().map(str::to_ascii_lowercase);
    let mut access =
        kit.write_workspace(&format!("edit_string: entry {} -> {:?}", p.entry, p.text))?;
    let ws = &mut access.ws;
    let mut by_stream: BTreeMap<usize, Vec<ResourceEdit>> = BTreeMap::new();
    let mut changes = Vec::new();
    let mut found = 0;
    for t in &tables {
        let mut table = StringTable::parse(&ws.current_data(&kit.game, t.stream, t.resource)?)?;
        let Some(at) = table.find(hash) else {
            continue;
        };
        found += 1;
        let names = columns(&table);
        let mut changed = false;
        for (c, name) in names.iter().enumerate() {
            if language
                .as_deref()
                .is_some_and(|l| !name.eq_ignore_ascii_case(l))
            {
                continue;
            }
            if table.strings[c][at] != p.text {
                changes.push(json!({
                    "table": format!("{}#{}", stream_name(kit, t.stream), t.resource),
                    "language": name,
                    "old": table.strings[c][at],
                    "new": p.text,
                }));
                table.strings[c][at] = p.text.clone();
                changed = true;
            }
        }
        if changed {
            by_stream.entry(t.stream).or_default().push(ResourceEdit {
                index: t.resource,
                data: table.to_bytes()?,
                storage: StorageChoice::Keep,
            });
        }
    }
    if found == 0 {
        return fail(format!(
            "no string table has the entry {} (`{}`); `strings` finds entries, `add_string` adds one",
            hex(hash),
            p.entry
        ));
    }
    if changes.is_empty() {
        let note = if language.is_some() && by_stream.is_empty() {
            "no column of that language changed: it has this text already, or the tables name their columns otherwise (see `strings`)"
        } else {
            "the entry already has this text"
        };
        return Ok(Output::Json(json!({ "changed": false, "note": note })));
    }
    let label = format!(
        "Edit string {} in {} table(s)",
        hex(hash),
        by_stream.values().map(Vec::len).sum::<usize>()
    );
    let since = ws.head();
    let edits: Vec<(usize, Vec<ResourceEdit>)> = by_stream.into_iter().collect();
    ws.change_resources(&kit.game, &label, &edits, &[])?;
    finish(
        ws,
        since,
        &label,
        json!({ "entry": hex(hash), "changes": changes }),
    )
}

pub fn add_string(kit: &Toolkit, p: AddStringParams) -> ToolResult<Output> {
    let tables = if p.tables.is_empty() {
        text::language_tables(&kit.game, kit.resources())?
    } else {
        p.tables
            .iter()
            .map(|t| table_ref(kit, t))
            .collect::<ToolResult<_>>()?
    };
    let new = NewString {
        name: p.name.trim().to_owned(),
        text: p.text.clone(),
        translations: p.translations.clone(),
    };
    let mut access = kit.write_workspace(&format!("add_string: {} = {:?}", p.name, p.text))?;
    let ws = &mut access.ws;
    let plan = text::plan_add(&kit.game, ws, kit.resources(), &tables, &new)?;
    let since = ws.head();
    plan.commit(ws, &kit.game)?;
    finish(
        ws,
        since,
        &plan.label,
        json!({
            "name": plan.added.name,
            "hash": hex(plan.added.hash),
            "tables": plan.added.tables,
            "use": format!(
                "${} in interface movies; hash {} in SimGroup hash attributes",
                plan.added.name,
                hex(plan.added.hash)
            ),
        }),
    )
}

// ------------------------------------------------------------ scripts

fn level_stream(kit: &Toolkit, name: &str) -> ToolResult<usize> {
    let n = doctor_toolkit::workspace::key::normalize_stream(name);
    kit.stream(&n)
}

pub fn edit_script(kit: &Toolkit, p: EditScriptParams) -> ToolResult<Output> {
    let stream = level_stream(kit, &p.stream)?;
    match (p.graph, p.remove) {
        (Some(_), Some(_)) => fail("give `graph` or `remove`, not both"),
        (None, None) => fail("give `graph` (to add or replace a script) or `remove` (its name)"),
        (None, Some(name)) => {
            let name = if name.to_ascii_lowercase().ends_with(".lua") {
                name
            } else {
                format!("{name}.lua")
            };
            let mut access =
                kit.write_workspace(&format!("edit_script: remove {name} from {}", p.stream))?;
            let ws = &mut access.ws;
            let since = ws.head();
            if !deploy::remove(&kit.game, ws, stream, &name)? {
                return fail(format!("no script {name} in {}", p.stream));
            }
            let label = ws.operations()[ws.head() - 1].label.clone();
            finish(ws, since, &label, json!({ "removed": name }))
        }
        (Some(graph), None) => {
            let graph = ScriptGraph::from_json(&graph.to_string())
                .map_err(|e| ToolError(format!("the graph is not a script graph: {e}")))?;
            let compiled = script::compile(&graph);
            let diagnostics: Vec<String> = compiled
                .diagnostics
                .iter()
                .map(ToString::to_string)
                .collect();
            if compiled.has_errors() {
                return fail(format!(
                    "the graph does not compile; nothing was recorded:\n{}",
                    diagnostics.join("\n")
                ));
            }
            let chunk = compiled
                .chunk
                .as_ref()
                .ok_or_else(|| ToolError("the compiler made no chunk".into()))?;
            let mut access =
                kit.write_workspace(&format!("edit_script: a script in {}", p.stream))?;
            let ws = &mut access.ws;
            let since = ws.head();
            let d = deploy::deploy(&kit.game, kit.resources(), ws, stream, &graph, chunk)?;
            if ws.is_saved() {
                deploy::save_graph(ws, &graph)?;
            }
            let label = ws
                .operations()
                .get(ws.head().wrapping_sub(1))
                .map(|op| op.label.clone())
                .unwrap_or_default();
            finish(
                ws,
                since,
                &label,
                json!({
                    "script": graph.chunk_name(),
                    "stream": d.stream,
                    "resource": d.key.to_string(),
                    "object": d.object,
                    "updated": d.updated,
                    "warnings": diagnostics,
                    "lua": compiled.source,
                }),
            )
        }
    }
}

/// The scripts made from graphs in a stream and their graphs (read-only).
pub fn scripts(kit: &Toolkit, p: ScriptsParams) -> ToolResult<Output> {
    let stream = level_stream(kit, &p.stream)?;
    let ws = match kit.workspace()? {
        Some(ws) => ws,
        None => Workspace::for_game(&kit.game, Some(&kit.workspace_dir))?,
    };
    let list = deploy::list(&kit.game, &ws, stream)?;
    let graphs: BTreeMap<String, Value> = deploy::saved_graphs(&ws)
        .into_iter()
        .filter_map(|(name, path)| {
            let text = std::fs::read_to_string(path).ok()?;
            Some((name, serde_json::from_str(&text).ok()?))
        })
        .collect();
    let scripts: Vec<Value> = list
        .iter()
        .map(|d| {
            json!({
                "resource": d.key.to_string(),
                "made_from_graph": d.added,
                "objects": d.objects,
                "start": d.start.as_ref().map(|s| json!({
                    "on_load": s.on_load,
                    "message": s.message,
                    "stop_message": s.stop_message,
                })),
            })
        })
        .collect();
    Ok(Output::Json(json!({
        "stream": stream_name(kit, stream),
        "scripts": scripts,
        "graphs": graphs,
    })))
}

// ------------------------------------------------------------ navigation graphs

pub fn graph_edit(kit: &Toolkit, p: GraphEditParams) -> ToolResult<Output> {
    let key: ResourceKey = p
        .graph
        .trim()
        .parse()
        .map_err(|e| ToolError(format!("`{}` is not a resource key: {e}", p.graph)))?;
    if key.type_id != TYPE_GRAPH {
        return fail(format!("{key} is not a navigation graph (type graph)"));
    }
    let stream = kit.stream(&key.stream)?;
    let file = StreamFile::open(&kit.game.streams()[stream].path)?;
    let resource = key
        .find_in(file.layout())
        .ok_or_else(|| ToolError(format!("{key} is not in its stream")))?;
    let mut access = kit.write_workspace(&format!("graph_edit: {key}"))?;
    let ws = &mut access.ws;
    let data = ws.current_data(&kit.game, stream, resource)?;
    let mut g = GraphRef {
        stream,
        resource,
        graph: Graph::parse(&data)?,
    };
    let graph = &mut g.graph;
    let mut done: Vec<String> = Vec::new();
    for m in &p.move_nodes {
        graph.move_node(m.node, m.position)?;
        done.push(format!("node {} moved", m.node));
    }
    for point in &p.add_nodes {
        let k = graph.add_node(*point, 0.5)?;
        done.push(format!("node {k} added"));
    }
    let how = Neighbourhood {
        max_links: p.max_links.unwrap_or(2),
        max_distance: p.max_distance.unwrap_or(15.0),
        two_way: true,
    };
    for point in &p.attach {
        let (k, near) = graph.attach_point(*point, 0.5, how)?;
        done.push(format!("node {k} added, linked with {near:?}"));
    }
    let two_way = p.two_way.unwrap_or(false);
    for &[a, b] in &p.link {
        graph.add_link(a, b, 0)?;
        if two_way {
            graph.add_link(b, a, 0)?;
        }
        done.push(format!(
            "link {a} {} {b}",
            if two_way { "<->" } else { "->" }
        ));
    }
    for &[a, b] in &p.unlink {
        graph.remove_link(a, b)?;
        done.push(format!("link {a} -> {b} removed"));
    }
    for f in &p.link_flags {
        let to =
            f.to.ok_or_else(|| ToolError("`link_flags` entries need `to`".into()))?;
        graph.set_link_flags(f.node, to, f.flags)?;
        done.push(format!("link {} -> {to} flags {:#06x}", f.node, f.flags));
    }
    for f in &p.node_flags {
        graph.set_node_flags(f.node, f.flags)?;
        done.push(format!("node {} flags {:#06x}", f.node, f.flags));
    }
    if let Some(flags) = p.graph_flags {
        graph.flags = flags;
        done.push(format!("graph type {flags:#06x}"));
    }
    let mut renumbered = None;
    if let Some(n) = p.remove_node {
        renumbered = Some(graph.remove_node(n)?);
        done.push(format!("node {n} removed"));
    }
    if p.rebuild_rtree.unwrap_or(false) {
        graph.rebuild_rtree();
        done.push("R-tree built again".into());
    }
    if done.is_empty() {
        return fail(
            "nothing to change: give move_nodes, add_nodes, attach, link, unlink, link_flags, \
             node_flags, graph_flags, remove_node or rebuild_rtree",
        );
    }
    let problems = graph.problems();
    let label = format!("Edit graph {key}: {}", done.join(", "));
    let since = ws.head();
    let others = save_graph(&kit.game, ws, &g, renumbered.as_deref(), &label)?;
    finish(
        ws,
        since,
        &label,
        json!({
            "graph": key.to_string(),
            "edits": done,
            "nodes": g.graph.nodes.len(),
            "links_of_other_graphs_renumbered": others,
            "problems": problems,
        }),
    )
}

// ------------------------------------------------------------ history

pub fn undo(kit: &Toolkit, _: UndoParams) -> ToolResult<Output> {
    history(kit, true)
}

pub fn redo(kit: &Toolkit, _: UndoParams) -> ToolResult<Output> {
    history(kit, false)
}

fn history(kit: &Toolkit, back: bool) -> ToolResult<Output> {
    if !Workspace::exists(&kit.workspace_dir) {
        return fail("there is no workspace: nothing to undo or redo");
    }
    let mut access = kit.write_workspace(if back { "undo" } else { "redo" })?;
    let ws = &mut access.ws;
    // The operation that is undone or redone: its resources make the diff.
    let op = if back {
        ws.head().checked_sub(1).map(|i| ws.operations()[i].clone())
    } else {
        ws.operations().get(ws.head()).cloned()
    };
    let label = if back { ws.undo()? } else { ws.redo()? };
    let Some(label) = label else {
        return Ok(Output::Json(json!({
            "changed": false,
            "note": if back { "nothing to undo" } else { "nothing to redo" },
        })));
    };
    let keys: BTreeSet<ResourceKey> = op
        .map(|op| op.edits.iter().map(|e| e.key.clone()).collect())
        .unwrap_or_default();
    let done = if back { "undone" } else { "redone" };
    Ok(Output::Json(json!({
        "changed": true,
        "done": format!("{done}: {label}"),
        "undo": ws.undo_label(),
        "redo": ws.redo_label(),
        "diff": diff_of(ws, &keys),
        "game_files": "unchanged: undo and redo change the workspace only",
    })))
}

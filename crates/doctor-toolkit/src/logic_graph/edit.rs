//! Edits of the logic graph as SimGroup edits.
//!
//! A wire is not stored anywhere: it exists because a sender's `out`
//! attribute and a listener's `in` attribute hold the same message name. So:
//!
//! - **connect** `out → in`: when the sender has no message, it gets the
//!   listener's (one more sender, nothing else changes); when the listener
//!   has none, it gets the sender's; when both have different ones, the
//!   listener switches to the sender's message (it stops hearing its old
//!   one). When neither has one, both get a new name;
//! - **disconnect** a wire: the side that has no other wire of that message
//!   is cleared — the sender when the listener is its only one, else the
//!   listener;
//! - add, remove nodes and set parameters are SimGroup edits too.
//!
//! [`plan_connect`] and [`plan_disconnect`] say what to change; [`apply`]
//! changes the SimGroups as the workspace has them and records them as one
//! undoable operation.

use std::collections::BTreeMap;

use crate::game::{GameInstance, ResourceIndex};
use crate::level::{Location, edit as level_edit};
use crate::resource::simgroup::{Guid, SimGroup, Value};
use crate::workspace::{ResourceEdit, Workspace};
use crate::{Error, Result};

use super::schema::{LogicSchema, PinKind};
use super::{LogicGraph, NodeSource, PinRef, WireKind};

/// An attribute of an object of a SimGroup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AttrAt {
    pub group: Location,
    pub object: usize,
    /// Class of the packet.
    pub class: u32,
    pub index: u16,
}

/// One change of an attribute.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub at: AttrAt,
    /// New value; `None` makes it the default (no message).
    pub value: Option<Value>,
}

impl Change {
    pub fn message(at: AttrAt, message: Option<&str>) -> Self {
        Self {
            at,
            value: message
                .filter(|m| !m.is_empty())
                .map(|m| Value::String(m.to_owned())),
        }
    }
}

/// The attribute behind a pin of an object node.
pub fn attr_of(graph: &LogicGraph, pin: PinRef) -> Result<AttrAt> {
    let node = graph
        .nodes
        .get(pin.node)
        .ok_or_else(|| Error::InvalidArgument(format!("no node {}", pin.node)))?;
    let NodeSource::Object { group, object } = node.source else {
        return Err(Error::InvalidArgument(format!(
            "{} is not a SimGroup object",
            node.class_name
        )));
    };
    let p = node
        .pins
        .get(pin.pin)
        .ok_or_else(|| Error::InvalidArgument(format!("no pin {}", pin.pin)))?;
    Ok(AttrAt {
        group,
        object,
        class: p.class,
        index: p.index,
    })
}

/// What to change to wire `from` (an `out` pin) to `to` (an `in` pin).
/// `fresh` makes a new message name when neither pin has one.
pub fn plan_connect(
    graph: &LogicGraph,
    from: PinRef,
    to: PinRef,
    fresh: impl FnOnce() -> String,
) -> Result<Vec<Change>> {
    let out = &graph.nodes[from.node].pins[from.pin];
    let inp = &graph.nodes[to.node].pins[to.pin];
    if out.kind != PinKind::Out || inp.kind != PinKind::In {
        return Err(Error::InvalidArgument(
            "a wire goes from a message sent (out) to a message heard (in)".into(),
        ));
    }
    if out.hash.is_some() && out.hash == inp.hash {
        return Ok(Vec::new());
    }
    let object_attr = |p: PinRef| match graph.nodes[p.node].source {
        NodeSource::Object { .. } => attr_of(graph, p).map(Some),
        _ => Ok(None),
    };
    let sender = object_attr(from)?;
    let listener = object_attr(to)?;
    Ok(match (&out.message, &inp.message, sender, listener) {
        (_, _, None, None) => {
            return Err(Error::InvalidArgument(
                "neither end is a SimGroup object".into(),
            ));
        }
        // From the engine or a hub: the listener takes its message.
        (Some(m), _, None, Some(l)) => vec![Change::message(l, Some(m))],
        // To the engine or a hub: the sender takes its message.
        (_, Some(m), Some(s), None) => vec![Change::message(s, Some(m))],
        (None, Some(m), Some(s), _) => vec![Change::message(s, Some(m))],
        (Some(m), _, _, Some(l)) => vec![Change::message(l, Some(m))],
        (None, None, Some(s), Some(l)) => {
            let name = fresh();
            vec![
                Change::message(s, Some(&name)),
                Change::message(l, Some(&name)),
            ]
        }
        _ => {
            return Err(Error::InvalidArgument(
                "the engine and message hubs are not edited".into(),
            ));
        }
    })
}

/// What to change to remove wire `wire` of the graph.
pub fn plan_disconnect(graph: &LogicGraph, wire: usize) -> Result<Vec<Change>> {
    let w = graph
        .wires
        .get(wire)
        .ok_or_else(|| Error::InvalidArgument(format!("no wire {wire}")))?;
    match w.kind {
        WireKind::Link => {
            return Ok(vec![Change {
                at: attr_of(graph, w.from)?,
                value: None,
            }]);
        }
        WireKind::Message | WireKind::Engine => {}
    }
    let h = w
        .message
        .ok_or_else(|| Error::InvalidArgument("the wire has no message".into()))?;
    let m = &graph.messages[&h];
    let is_object = |p: PinRef| matches!(graph.nodes[p.node].source, NodeSource::Object { .. });
    // A wire through a hub or from the engine: clear the object end.
    if !is_object(w.from) {
        return Ok(vec![Change::message(attr_of(graph, w.to)?, None)]);
    }
    if !is_object(w.to) {
        return Ok(vec![Change::message(attr_of(graph, w.from)?, None)]);
    }
    // One listener: clearing the sender removes exactly this wire; else
    // clearing the listener does (with one sender) or cuts it off the message.
    let end = if m.listeners.len() == 1 { w.from } else { w.to };
    Ok(vec![Change::message(attr_of(graph, end)?, None)])
}

/// Applies changes to SimGroups in memory.
pub fn apply_to(group: &mut SimGroup, changes: &[&Change]) -> Result<bool> {
    let mut changed = false;
    for c in changes {
        changed |= group.set_class_attr(c.at.object, c.at.class, c.at.index, c.value.clone())?;
    }
    Ok(changed)
}

/// Applies changes to the SimGroups as the workspace has them and records
/// them as one undoable operation. Each changed group is checked by
/// [`crate::level::compat::check_resource_data`] first. Returns whether
/// anything changed.
pub fn apply(
    game: &GameInstance,
    index: &ResourceIndex,
    ws: &mut Workspace,
    changes: &[Change],
    label: &str,
) -> Result<bool> {
    let mut by_group: BTreeMap<(usize, usize), Vec<&Change>> = BTreeMap::new();
    for c in changes {
        by_group
            .entry((c.at.group.stream, c.at.group.resource))
            .or_default()
            .push(c);
    }
    let mut edits: BTreeMap<usize, Vec<ResourceEdit>> = BTreeMap::new();
    for ((stream, resource), list) in by_group {
        let location = Location { stream, resource };
        let mut g = level_edit::current_group(game, ws, location)?;
        if !apply_to(&mut g, &list)? {
            continue;
        }
        let data = g.encode();
        let problems = crate::level::compat::check_resource_data(
            game,
            index,
            Some(ws),
            (stream, resource),
            &data,
        )?;
        if let Some(p) = problems.into_iter().next() {
            return Err(Error::InvalidArgument(p));
        }
        edits.entry(stream).or_default().push(ResourceEdit {
            index: resource,
            data,
            storage: crate::workspace::StorageChoice::Keep,
        });
    }
    if edits.is_empty() {
        return Ok(false);
    }
    let edits: Vec<(usize, Vec<ResourceEdit>)> = edits.into_iter().collect();
    Ok(ws.change_resources(game, label, &edits, &[])?.is_some())
}

/// The first free `out` pin of a list (MultiManager, TriggerRandom, …) of
/// object `object`: the first schema `out` attribute of `class` that is not
/// set. `None` when all are used.
pub fn free_output(group: &SimGroup, object: usize, class: u32) -> Option<u16> {
    let o = group.objects.get(object)?;
    let packet = group.object_packets(o).find(|p| p.class.0 == class)?;
    LogicSchema::gf2()
        .packet(class)
        .iter()
        .filter(|a| a.pin == PinKind::Out && a.index < packet.attr_count)
        .map(|a| a.index)
        .find(|i| packet.attrs.iter().all(|v| v.index != *i))
}

/// Clears the message pins (`in`, `out`) and links of object `object`, so
/// that a copy does not react to or send the messages of its template.
pub fn clear_wiring(group: &mut SimGroup, object: usize) -> Result<bool> {
    let schema = LogicSchema::gf2();
    let o = group
        .objects
        .get(object)
        .ok_or_else(|| Error::InvalidArgument(format!("no object {object}")))?;
    let mut targets = Vec::new();
    for (slot, p) in group.object_packets(o).enumerate() {
        for a in schema.packet(p.class.0) {
            if matches!(a.pin, PinKind::In | PinKind::Out | PinKind::Link)
                && p.attrs.iter().any(|v| v.index == a.index)
            {
                targets.push((slot, a.index));
            }
        }
    }
    let mut changed = false;
    for (slot, index) in targets {
        changed |= group.set_attr(object, slot, index, None)?;
    }
    Ok(changed)
}

/// Where a new node comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeTemplate {
    /// A copy of an object of the game (stage 11 insertion): its packets,
    /// components and parameters; its messages and links are cleared.
    Copy(Location, usize),
    /// A new object of a class with empty packets of the schema layout.
    Empty(u32),
}

/// Adds a node to SimGroup `group`. Returns the index and GUID of the new
/// object.
pub fn add_node(
    game: &GameInstance,
    index: &ResourceIndex,
    ws: &mut Workspace,
    group: Location,
    template: NodeTemplate,
    taken: &dyn Fn(&Guid) -> bool,
) -> Result<(usize, Guid)> {
    let mut g = level_edit::current_group(game, ws, group)?;
    let name = game.streams()[group.stream].stream_name();
    let (object, guid, class) = match template {
        NodeTemplate::Copy(at, object) => {
            let source = if at == group {
                None
            } else {
                Some(level_edit::current_group(game, ws, at)?)
            };
            let seed = format!("{name}/logic{}", g.objects.len());
            let guid = level_edit::new_guid(&g, &seed, taken);
            let new = match &source {
                None => g.duplicate_object(object, guid)?,
                Some(from) => g.copy_object_from(from, object, guid)?,
            };
            clear_wiring(&mut g, new)?;
            (new, guid, g.objects[new].class)
        }
        NodeTemplate::Empty(class) => {
            let layout = LogicSchema::gf2().layout(class).ok_or_else(|| {
                Error::InvalidArgument(format!("the schema has no layout of class {class:08x}"))
            })?;
            let seed = format!("{name}/logic{}", g.objects.len());
            let guid = level_edit::new_guid(&g, &seed, taken);
            let new = g.push_object(class, guid, layout)?;
            (new, guid, class)
        }
    };
    let class_name = LogicSchema::gf2()
        .class_name(class)
        .map_or_else(|| format!("{class:08x}"), str::to_owned);
    let label = format!("Add logic node {object} ({class_name}) to {name}");
    let data = g.encode();
    if let Some(p) = crate::level::compat::check_resource_data(
        game,
        index,
        Some(ws),
        (group.stream, group.resource),
        &data,
    )?
    .into_iter()
    .next()
    {
        return Err(Error::InvalidArgument(p));
    }
    ws.set_resource(game, group.stream, group.resource, data, &label)?;
    Ok((object, guid))
}

/// Removes a node (the object). Returns how many attributes of the group
/// still hold its GUID.
pub fn remove_node(
    game: &GameInstance,
    ws: &mut Workspace,
    group: Location,
    object: usize,
) -> Result<usize> {
    level_edit::remove(game, ws, group, object)
}

/// A message name not used in the graph: `<base>`, `<base>_2`, ….
pub fn fresh_message(graph: &LogicGraph, base: &str) -> String {
    let base: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let base = if base.is_empty() {
        "msg".to_owned()
    } else {
        base
    };
    if graph.message(&base).is_none() {
        return base;
    }
    (2..)
        .map(|n| format!("{base}_{n}"))
        .find(|name| graph.message(name).is_none())
        .expect("some name is free")
}

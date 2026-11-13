//! `logic_graph`: objects, the messages they send and hear, and who is at
//! the other end — from one object, for one message, or over a scope.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use serde_json::{Value, json};

use doctor_toolkit::hash::string_id;
use doctor_toolkit::logic_graph::{
    BuildOptions, LogicSchema, MessageState, PinKind, ScopeSpec, Site, scope,
};

use super::{LogicGraphParams, Output, clamp, object_id, parse_object_id};
use crate::state::{Messages, ObjectKey, ToolResult, Toolkit, fail};

/// Listeners or senders listed per message and object at most.
const PER_MESSAGE: usize = 25;

pub fn logic_graph(kit: &Toolkit, p: LogicGraphParams) -> ToolResult<Output> {
    let given = [&p.object, &p.message, &p.scope]
        .iter()
        .filter(|v| v.is_some())
        .count();
    if given != 1 {
        return fail("give exactly one of `object`, `message` or `scope`");
    }
    if let Some(scope) = &p.scope {
        return scope_overview(kit, scope, &p);
    }
    let messages = kit.messages()?;
    if let Some(name) = &p.message {
        return message(kit, &messages, name, &p);
    }
    object(kit, &messages, p.object.as_deref().unwrap_or_default(), &p)
}

fn site_json(kit: &Toolkit, s: &Site) -> Value {
    json!({
        "object": object_id(kit, s.group, s.object),
        "class": s.class_name,
        "name": s.name,
        "pin": s.pin,
    })
}

fn engine(hash: u32) -> bool {
    LogicSchema::gf2().engine_message(hash).is_some()
}

fn message(kit: &Toolkit, m: &Messages, name: &str, p: &LogicGraphParams) -> ToolResult<Output> {
    let hash = string_id(name);
    let Some(sites) = m.index.who(name) else {
        let similar: Vec<&str> = m
            .index
            .search(name)
            .into_iter()
            .take(30)
            .map(|s| s.name.as_str())
            .collect();
        return Ok(Output::Json(json!({
            "message": name,
            "hash": super::hex(hash),
            "found": false,
            "named_by_engine": engine(hash),
            "similar_names": similar,
        })));
    };
    let offset = p.offset.unwrap_or(0);
    let limit = clamp(p.limit, 100, 400);
    let list = |v: &[Site]| -> Value {
        json!({
            "total": v.len(),
            "items": v.iter().skip(offset).take(limit).map(|s| site_json(kit, s)).collect::<Vec<_>>(),
        })
    };
    Ok(Output::Json(json!({
        "message": sites.name,
        "hash": super::hex(hash),
        "found": true,
        "named_by_engine": engine(hash),
        "offset": offset,
        "senders": list(&sites.senders),
        "listeners": list(&sites.listeners),
    })))
}

/// The object a text names: an id, or the name of an object with messages.
fn find_object(kit: &Toolkit, m: &Messages, text: &str) -> ToolResult<ObjectKey> {
    if let Some(found) = parse_object_id(kit, text)? {
        return Ok(found);
    }
    let want = text.trim().to_lowercase();
    let mut hits: Vec<(ObjectKey, &Site)> = Vec::new();
    let mut seen = HashSet::new();
    for sites in m.index.by_hash.values() {
        for s in sites.senders.iter().chain(&sites.listeners) {
            if s.name.to_lowercase() == want && seen.insert((s.group, s.object)) {
                hits.push(((s.group, s.object), s));
            }
        }
    }
    match hits.len() {
        0 => fail(format!(
            "no object with messages is named `{text}`; give an id <stream>#<resource>:<object> \
             (level_objects lists them)"
        )),
        1 => Ok(hits[0].0),
        n => {
            hits.sort_by_key(|(k, _)| (k.0.stream, k.0.resource, k.1));
            let ids: Vec<String> = hits
                .iter()
                .take(20)
                .map(|(k, s)| format!("{} ({})", object_id(kit, k.0, k.1), s.class_name))
                .collect();
            fail(format!(
                "{n} objects are named `{text}`; pick one id: {}",
                ids.join(", ")
            ))
        }
    }
}

fn object(kit: &Toolkit, m: &Messages, text: &str, p: &LogicGraphParams) -> ToolResult<Output> {
    let start = find_object(kit, m, text)?;
    let depth = p.depth.unwrap_or(1).clamp(1, 4);
    let (down, up) = match p.direction.as_deref().unwrap_or("both") {
        "down" => (true, false),
        "up" => (false, true),
        "both" => (true, true),
        other => return fail(format!("direction `{other}`: use down, up or both")),
    };
    let limit = clamp(p.limit, 100, 400);
    // Distance of every object reached, edges between them, messages used.
    let mut level: HashMap<ObjectKey, usize> = HashMap::from([(start, 0)]);
    let mut order: Vec<ObjectKey> = vec![start];
    let mut edges: Vec<(ObjectKey, ObjectKey, u32)> = Vec::new();
    let mut edge_set = HashSet::new();
    let mut used: BTreeMap<u32, usize> = BTreeMap::new();
    let mut cut: Vec<String> = Vec::new();
    let mut truncated = false;
    let mut queue = VecDeque::from([start]);
    while let Some(at) = queue.pop_front() {
        let d = level[&at];
        if d >= depth {
            continue;
        }
        let Some(pins) = m.by_object.get(&at) else {
            continue;
        };
        for (hash, kind, _) in pins {
            let Some(sites) = m.index.by_hash.get(hash) else {
                continue;
            };
            *used.entry(*hash).or_default() += 1;
            let (others, outgoing) = match kind {
                PinKind::Out if down => (&sites.listeners, true),
                PinKind::In if up => (&sites.senders, false),
                _ => continue,
            };
            if others.len() > PER_MESSAGE {
                cut.push(format!(
                    "{}: {} {} (only {PER_MESSAGE} followed; logic_graph message=… lists all)",
                    sites.name,
                    others.len(),
                    if outgoing { "listeners" } else { "senders" }
                ));
            }
            for s in others.iter().take(PER_MESSAGE) {
                let other = (s.group, s.object);
                let edge = if outgoing {
                    (at, other, *hash)
                } else {
                    (other, at, *hash)
                };
                if edge_set.insert(edge) {
                    edges.push(edge);
                }
                if let std::collections::hash_map::Entry::Vacant(slot) = level.entry(other) {
                    if order.len() >= limit {
                        truncated = true;
                        continue;
                    }
                    slot.insert(d + 1);
                    order.push(other);
                    queue.push_back(other);
                }
            }
        }
    }
    edges.retain(|(a, b, _)| level.contains_key(a) && level.contains_key(b));
    // Names and classes of the objects reached.
    let mut describe: HashMap<ObjectKey, (&str, &str)> = HashMap::new();
    for sites in m.index.by_hash.values() {
        for s in sites.senders.iter().chain(&sites.listeners) {
            describe
                .entry((s.group, s.object))
                .or_insert((s.class_name.as_str(), s.name.as_str()));
        }
    }
    let id = |k: &ObjectKey| object_id(kit, k.0, k.1);
    let name_of = |h: &u32| {
        m.index
            .by_hash
            .get(h)
            .map_or_else(|| super::hex(*h), |s| s.name.clone())
    };
    let pins: Vec<Value> = m
        .by_object
        .get(&start)
        .map(|pins| {
            pins.iter()
                .map(|(h, kind, pin)| {
                    let sites = &m.index.by_hash[h];
                    json!({
                        "pin": pin,
                        "kind": kind,
                        "message": sites.name,
                        "senders": sites.senders.len(),
                        "listeners": sites.listeners.len(),
                        "named_by_engine": engine(*h),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let objects: Vec<Value> = order
        .iter()
        .map(|k| {
            let (class, name) = describe.get(k).copied().unwrap_or(("?", ""));
            json!({ "object": id(k), "class": class, "name": name, "hops": level[k] })
        })
        .collect();
    let edges: Vec<Value> = edges
        .iter()
        .map(|(a, b, h)| json!({ "from": id(a), "to": id(b), "message": name_of(h) }))
        .collect();
    let messages: Vec<Value> = used
        .keys()
        .map(|h| {
            let s = &m.index.by_hash[h];
            json!({
                "message": s.name,
                "senders": s.senders.len(),
                "listeners": s.listeners.len(),
                "named_by_engine": engine(*h),
            })
        })
        .collect();
    let (class, name) = describe.get(&start).copied().unwrap_or(("?", ""));
    Ok(Output::Json(json!({
        "start": { "object": id(&start), "class": class, "name": name, "pins": pins },
        "depth": depth,
        "direction": p.direction.as_deref().unwrap_or("both"),
        "objects": objects,
        "edges": edges,
        "messages": messages,
        "not_followed": cut,
        "truncated": truncated,
        "note": if pins.is_empty() {
            "the object sends and hears no messages (or is not a SimGroup object with pins)"
        } else {
            "edges go from the sender to the listener of a message"
        },
    })))
}

fn scope_overview(kit: &Toolkit, text: &str, p: &LogicGraphParams) -> ToolResult<Output> {
    let spec = ScopeSpec::parse(&kit.game, text)?;
    if spec == ScopeSpec::Game {
        return fail(
            "the whole game is too big for one answer; use a mission, a checkpoint, a stream or a level",
        );
    }
    let ws = kit.workspace_unless(p.original.unwrap_or(false))?;
    let options = BuildOptions {
        hub_threshold: 48,
        ..BuildOptions::default()
    };
    let (mut graph, errors) =
        scope::build(&kit.game, kit.resources(), ws.as_ref(), &spec, options)?;
    let messages = kit.messages()?;
    graph.annotate(&messages.index);
    let offset = p.offset.unwrap_or(0);
    let limit = clamp(p.limit, 100, 400);
    let overview = serde_json::to_value(graph.overview())?;
    let mut list: Vec<&doctor_toolkit::logic_graph::Message> = graph.messages.values().collect();
    // Dangling messages first: they are what usually needs looking at.
    list.sort_by_key(|m| (m.state == MessageState::Connected, m.name.clone()));
    let total_messages = list.len();
    let message_items: Vec<Value> = list
        .iter()
        .skip(offset)
        .take(limit)
        .map(|m| {
            json!({
                "message": m.name,
                "state": m.state,
                "senders": m.senders.len(),
                "listeners": m.listeners.len(),
                "elsewhere_in_game": m.elsewhere.map(|(s, l)| json!({ "senders": s, "listeners": l })),
            })
        })
        .collect();
    let nodes: Vec<Value> = graph
        .nodes
        .iter()
        .filter_map(|n| {
            let (group, object) = n.object()?;
            Some(json!({
                "object": object_id(kit, group, object),
                "class": n.class_name,
                "name": n.name,
                "category": n.category,
            }))
        })
        .collect();
    let total_nodes = nodes.len();
    let nodes: Vec<Value> = nodes.into_iter().skip(offset).take(limit).collect();
    Ok(Output::Json(json!({
        "scope": spec.label(&kit.game),
        "overview": overview,
        "offset": offset,
        "messages": { "total": total_messages, "items": message_items },
        "objects": { "total": total_nodes, "items": nodes },
        "unreadable_simgroups": errors.len(),
        "note": "logic_graph object=<id> follows one object; message=<name> lists every sender and listener",
    })))
}

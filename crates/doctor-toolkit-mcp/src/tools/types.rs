//! `game_types`: the typed game data built into the binary — value lists,
//! meanings of SimGroup attributes, messages with their senders and
//! listeners (`data/gf2/`, see `docs/logic-graph.md` §2.1).

use serde_json::{Value, json};

use doctor_toolkit::game_types::{self, MessageCatalog, MessageId, message};
use doctor_toolkit::hash::string_id;
use doctor_toolkit::logic_graph::LogicSchema;

use super::{GameTypesParams, Output, clamp};
use crate::state::{ToolResult, Toolkit, fail};

fn message_id(text: &str) -> MessageId {
    match text
        .strip_prefix("0x")
        .filter(|h| h.len() == 8)
        .and_then(|h| u32::from_str_radix(h, 16).ok())
    {
        Some(hash) => MessageId::from_hash(hash),
        None => MessageId::from_name(text),
    }
}

fn message_json(id: &MessageId) -> Value {
    let mut value = serde_json::to_value(id.info()).unwrap_or(Value::Null);
    value["hash_hex"] = json!(format!("{:08x}", id.hash()));
    if let MessageId::Custom { name, .. } = id {
        value["collides_with"] = json!(message::collision(name));
    }
    value
}

/// Value lists, messages and attribute meanings.
pub fn game_types(_kit: &Toolkit, p: GameTypesParams) -> ToolResult<Output> {
    let mut out = serde_json::Map::new();
    if let Some(key) = &p.list {
        let Some(e) = game_types::enum_def(key) else {
            let keys: Vec<&str> = game_types::enums().iter().map(|e| e.key).collect();
            return fail(format!("unknown list `{key}`; one of {}", keys.join(", ")));
        };
        out.insert(
            "enum".into(),
            serde_json::to_value(e).unwrap_or(Value::Null),
        );
    }
    if let Some(name) = &p.message {
        out.insert("message".into(), message_json(&message_id(name)));
    }
    if let Some(text) = &p.search {
        let limit = clamp(p.limit, 40, 200);
        let found: Vec<Value> = MessageCatalog::gf2()
            .search(text, limit)
            .into_iter()
            .map(|m| {
                json!({
                    "name": m.name,
                    "kind": MessageId::from_hash(m.hash).kind(),
                    "senders": m.senders().iter().map(|u| u64::from(u.count)).sum::<u64>(),
                    "listeners": m.listeners().iter().map(|u| u64::from(u.count)).sum::<u64>(),
                    "scope": m.scope,
                })
            })
            .collect();
        out.insert("messages".into(), found.into());
    }
    if let Some(class) = &p.class {
        let schema = LogicSchema::gf2();
        let hash = match u32::from_str_radix(class, 16) {
            Ok(h) if class.len() == 8 => h,
            _ => string_id(class),
        };
        let attrs = schema.typed_attrs(hash);
        out.insert(
            "class".into(),
            json!({
                "name": schema.class_name(hash),
                "hash": format!("{hash:08x}"),
                "attrs": attrs,
                "note": "the packet of this class only; an object also has the packets of its base classes",
            }),
        );
    }
    if out.is_empty() {
        let catalog = MessageCatalog::gf2();
        let engine = message::engine_messages();
        let engine_used = engine
            .iter()
            .filter(|(h, _)| catalog.get(*h).is_some())
            .count();
        out.insert(
            "enums".into(),
            game_types::enums()
                .iter()
                .map(|e| {
                    json!({
                        "key": e.key,
                        "title": e.title,
                        "values": e.values.len(),
                        "kind": e.kind,
                        "conf": e.conf,
                    })
                })
                .collect::<Vec<_>>()
                .into(),
        );
        out.insert(
            "messages".into(),
            json!({
                "engine": engine.len(),
                "game": catalog.len() - engine_used,
                "catalog": catalog.len(),
                "data": "unknown for every message until it is read in the code",
            }),
        );
        out.insert(
            "typed_attributes".into(),
            json!(game_types::attr_types().len()),
        );
        out.insert(
            "hint".into(),
            json!("give `enum`, `message`, `search` or `class`; the lists are the resource doctor-toolkit://schema/enums.toml"),
        );
    }
    Ok(Output::Json(Value::Object(out)))
}

//! MCP resources: the format documents and guides and the knowledge base
//! `docs/kb/` (embedded in the binary, so they always match the code that
//! serves them), the schema of logic pins, the engine's message names, and
//! lists read from the game.

use std::sync::OnceLock;

use serde_json::json;

use doctor_toolkit::kb::{Kb, NoteType};
use doctor_toolkit::logic_graph::scope;

use crate::output::fence;
use crate::state::{ToolResult, Toolkit, fail};

include!(concat!(env!("OUT_DIR"), "/docs.rs"));

/// The schema of SimGroup pins: which attributes are messages heard and
/// sent, links and assets.
const LOGIC_SCHEMA: &str = include_str!("../../../data/gf2/logic_schema.toml");
/// Message names the game's code sends or hears.
const ENGINE_MESSAGES: &str = include_str!("../../../data/gf2/engine_messages.txt");

/// Value lists: families, cities, venues, weapons, crew specialties, sound
/// categories, emotions.
const ENUMS: &str = include_str!("../../../data/gf2/enums.toml");
/// What the values of SimGroup attributes mean.
const ATTR_TYPES: &str = include_str!("../../../data/gf2/attr_types.toml");
/// Facts about single messages.
const MESSAGES: &str = include_str!("../../../data/gf2/messages.toml");

pub const SCHEME: &str = "doctor-toolkit://";

/// The knowledge base built into the binary.
pub fn knowledge_base() -> &'static Kb {
    static BASE: OnceLock<Kb> = OnceLock::new();
    BASE.get_or_init(|| Kb::from_texts(KB.iter().copied()))
}

/// One resource the server lists.
pub struct Entry {
    pub uri: String,
    pub name: String,
    pub description: String,
    pub mime: &'static str,
    pub size: Option<u64>,
}

fn doc_title(text: &str, rel: &str) -> String {
    text.lines()
        .find_map(|l| l.strip_prefix("# "))
        .map_or_else(|| rel.to_owned(), |t| t.trim().to_owned())
}

/// Every resource, in a stable order.
pub fn list() -> Vec<Entry> {
    let mut out: Vec<Entry> = DOCS
        .iter()
        .map(|(rel, text)| Entry {
            uri: format!("{SCHEME}docs/{rel}"),
            name: format!("docs/{rel}"),
            description: format!("{} (documentation of doctor-toolkit)", doc_title(text, rel)),
            mime: "text/markdown",
            size: Some(text.len() as u64),
        })
        .collect();
    // Of the knowledge base only the maps and the lists of the folders are
    // listed; every note is read by its URI (`kb_search` finds them).
    for note in &knowledge_base().notes {
        let listed = note.meta.as_ref().is_ok_and(|m| {
            matches!(
                m.note_type,
                NoteType::Map | NoteType::Index | NoteType::Meta
            )
        });
        if listed {
            out.push(Entry {
                uri: format!("{SCHEME}kb/{}", note.rel),
                name: format!("kb/{}", note.rel),
                description: format!("{} (knowledge base of doctor-toolkit)", note.title),
                mime: "text/markdown",
                size: Some(note.text.len() as u64),
            });
        }
    }
    out.push(Entry {
        uri: format!("{SCHEME}schema/logic_schema.toml"),
        name: "schema/logic_schema.toml".into(),
        description: "Schema of SimGroup pins: per class, which attributes are messages \
                      heard (in) and sent (out), links to objects and assets"
            .into(),
        mime: "application/toml",
        size: Some(LOGIC_SCHEMA.len() as u64),
    });
    out.push(Entry {
        uri: format!("{SCHEME}schema/engine_messages.txt"),
        name: "schema/engine_messages.txt".into(),
        description: "Names of the messages the game's code sends or hears".into(),
        mime: "text/plain",
        size: Some(ENGINE_MESSAGES.len() as u64),
    });
    out.push(Entry {
        uri: format!("{SCHEME}schema/enums.toml"),
        name: "schema/enums.toml".into(),
        description: "Value lists of the game with their sources: families, cities, venues, \
                      building types, weapons, crew specialties, sound categories, emotions"
            .into(),
        mime: "application/toml",
        size: Some(ENUMS.len() as u64),
    });
    out.push(Entry {
        uri: format!("{SCHEME}schema/attr_types.toml"),
        name: "schema/attr_types.toml".into(),
        description: "What the values of SimGroup attributes mean: a reference to an object \
                      of a class, a resource of a type, a value of a list, seconds, money, a flag"
            .into(),
        mime: "application/toml",
        size: Some(ATTR_TYPES.len() as u64),
    });
    out.push(Entry {
        uri: format!("{SCHEME}schema/messages.toml"),
        name: "schema/messages.toml".into(),
        description: "Facts about single messages: what they do, their data, who sends them \
                      besides SimGroup objects"
            .into(),
        mime: "application/toml",
        size: Some(MESSAGES.len() as u64),
    });
    out.push(Entry {
        uri: format!("{SCHEME}schema/gf2-types.schema.json"),
        name: "schema/gf2-types.schema.json".into(),
        description: "JSON Schema of the typed game data: a definition of every value list, \
                      of message names and of the `messages` of mod.json"
            .into(),
        mime: "application/schema+json",
        size: None,
    });
    out.push(Entry {
        uri: format!("{SCHEME}game/messages"),
        name: "game/messages".into(),
        description: "Every message SimGroup objects of this game send or hear, with the \
                      number of senders and listeners (read from the game; takes seconds the first time)"
            .into(),
        mime: "text/plain",
        size: None,
    });
    out.push(Entry {
        uri: format!("{SCHEME}game/missions"),
        name: "game/missions".into(),
        description: "Missions of this game (stream directories under objectives/ and \
                      checkpoints/) and their streams"
            .into(),
        mime: "application/json",
        size: None,
    });
    out
}

/// The text and MIME type of resource `uri`. Only the URIs [`list`] gives
/// are known; nothing is read from a path the URI names.
pub fn read(kit: &Toolkit, uri: &str) -> ToolResult<(String, &'static str)> {
    let Some(rest) = uri.strip_prefix(SCHEME) else {
        return fail(format!("unknown resource {uri}"));
    };
    if let Some(rel) = rest.strip_prefix("docs/")
        && let Some((_, text)) = DOCS.iter().find(|(r, _)| *r == rel)
    {
        return Ok(((*text).to_owned(), "text/markdown"));
    }
    if let Some(rel) = rest.strip_prefix("kb/")
        && let Some(note) = knowledge_base().note(rel)
    {
        return Ok((note.text.clone(), "text/markdown"));
    }
    match rest {
        "schema/logic_schema.toml" => Ok((LOGIC_SCHEMA.to_owned(), "application/toml")),
        "schema/engine_messages.txt" => Ok((ENGINE_MESSAGES.to_owned(), "text/plain")),
        "schema/enums.toml" => Ok((ENUMS.to_owned(), "application/toml")),
        "schema/attr_types.toml" => Ok((ATTR_TYPES.to_owned(), "application/toml")),
        "schema/messages.toml" => Ok((MESSAGES.to_owned(), "application/toml")),
        "schema/gf2-types.schema.json" => Ok((
            doctor_toolkit::game_types::export::json_schema_text(),
            "application/schema+json",
        )),
        "game/messages" => {
            let messages = kit.messages()?;
            let mut list: Vec<_> = messages.index.by_hash.values().collect();
            list.sort_by(|a, b| a.name.cmp(&b.name));
            let mut text = String::from("message\tsenders\tlisteners\n");
            for m in list {
                text.push_str(&format!(
                    "{}\t{}\t{}\n",
                    m.name,
                    m.senders.len(),
                    m.listeners.len()
                ));
            }
            Ok((fence("game/messages", &text), "text/plain"))
        }
        "game/missions" => {
            let missions: Vec<_> = scope::missions(&kit.game)
                .into_iter()
                .map(|m| {
                    json!({
                        "name": m.name,
                        "streams": m.streams.iter().map(|&s| kit.game.streams()[s].stream_name()).collect::<Vec<_>>(),
                    })
                })
                .collect();
            let text = serde_json::to_string_pretty(&missions)?;
            Ok((fence("game/missions", &text), "application/json"))
        }
        _ => fail(format!("unknown resource {uri}; resources/list lists them")),
    }
}

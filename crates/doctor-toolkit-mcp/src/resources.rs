//! MCP resources: the format documents and guides (embedded in the binary,
//! so they always match the code that serves them), the schema of logic
//! pins, the engine's message names, and lists read from the game.

use serde_json::json;

use doctor_toolkit::logic_graph::scope;

use crate::output::fence;
use crate::state::{ToolResult, Toolkit, fail};

include!(concat!(env!("OUT_DIR"), "/docs.rs"));

/// The schema of SimGroup pins: which attributes are messages heard and
/// sent, links and assets.
const LOGIC_SCHEMA: &str = include_str!("../../../data/gf2/logic_schema.toml");
/// Message names the game's code sends or hears.
const ENGINE_MESSAGES: &str = include_str!("../../../data/gf2/engine_messages.txt");

pub const SCHEME: &str = "doctor-toolkit://";

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
    match rest {
        "schema/logic_schema.toml" => Ok((LOGIC_SCHEMA.to_owned(), "application/toml")),
        "schema/engine_messages.txt" => Ok((ENGINE_MESSAGES.to_owned(), "text/plain")),
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

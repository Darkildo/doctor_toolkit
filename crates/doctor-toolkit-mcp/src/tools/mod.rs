//! The read-only tools. Each takes the toolkit and its parameters and
//! returns JSON (or text) that the server fences off as game data.
//!
//! The writing tools of stage 51 are in modules of their own ([`edit`],
//! [`build`], [`play`], the writes of [`live`]) behind routers of their own
//! that the flags of the server turn on.

pub mod build;
pub mod create;
pub mod edit;
pub mod export;
pub mod game;
pub mod import;
pub mod json;
pub mod level;
pub mod live;
pub mod logic;
pub mod mods;
pub mod play;
pub mod search;
pub mod workspace;

use schemars::JsonSchema;
use serde::Deserialize;

use doctor_toolkit::navigation::{Query, RecordId, search as nav_search};
use doctor_toolkit::workspace::ResourceKey;

use crate::state::{ToolError, ToolResult, Toolkit, fail};

/// What a tool returns: JSON, or plain text (Lua source) with a JSON head.
pub enum Output {
    Json(serde_json::Value),
    Text(String),
    /// A picture (PNG) and what it shows.
    Image {
        png: Vec<u8>,
        info: serde_json::Value,
    },
}

// ------------------------------------------------------------ parameters

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct GameInfoParams {}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ListStreamsParams {
    /// Only streams whose name starts with this, e.g. `gf2/miami/local`.
    #[serde(default)]
    pub prefix: Option<String>,
    /// Skip this many streams (paging).
    #[serde(default)]
    pub offset: Option<usize>,
    /// At most this many streams (default 200, at most 1000).
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct SearchParams {
    /// Search words: a part of a name (`wheel`, `"rear wheel"`), a glob
    /// (`car_*_lod?`), a hash (`0x9cf57f36`), a resource key, and filters
    /// `type:mesh,material`, `stream:mp_mall`, `kind:object`,
    /// `text:Don Corleone` (text of strings, rest of the line). A query of
    /// filters only (`type:dlg`) lists every such resource.
    pub query: String,
    /// Skip this many results (paging).
    #[serde(default)]
    pub offset: Option<usize>,
    /// At most this many results (default 30, at most 200).
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ResourceParams {
    /// A resource key `stream:type:ids` (from `search`), or a search query
    /// with exactly one result.
    pub resource: String,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ResourceJsonParams {
    /// A resource key `stream:type:ids` (from `search`), or a search query
    /// with exactly one result.
    pub resource: String,
    /// Only this part of the JSON, as a JSON Pointer: `/objects/3`,
    /// `/strings/0`. Not for Lua scripts.
    #[serde(default)]
    pub pointer: Option<String>,
    /// Start at this character of the result (from `next_offset` of the previous call).
    #[serde(default)]
    pub offset: Option<usize>,
    /// At most this many characters (default 20000; capped by the server limit).
    #[serde(default)]
    pub max_chars: Option<usize>,
    /// Read the game files only, without the edits of the workspace.
    #[serde(default)]
    pub original: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct LookupHashParams {
    /// A 32-bit hash: `0x9cf57f36`, `9cf57f36` (eight hex digits) or decimal.
    #[serde(default)]
    pub hash: Option<String>,
    /// A name to hash under every hash function of the engine (and look up
    /// where those hashes occur).
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct StringsParams {
    /// Text to find in the localised strings (case-insensitive).
    #[serde(default)]
    pub text: Option<String>,
    /// Or: the hash of a string entry (`0x…`), in every table and language.
    #[serde(default)]
    pub hash: Option<String>,
    /// Only this language column: its code when the table names it (`en`),
    /// else the column name — `text` (English), `text1`, `text2`, ... as
    /// results show them.
    #[serde(default)]
    pub language: Option<String>,
    /// Skip this many strings (paging).
    #[serde(default)]
    pub offset: Option<usize>,
    /// At most this many strings (default 50, at most 300).
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct LogicGraphParams {
    /// Start object: `<stream>#<resource>:<object>` (as `level_objects`
    /// and `logic_graph` print ids), or an object name (`CMD_SetName`).
    #[serde(default)]
    pub object: Option<String>,
    /// Or: a message name — who sends it and who hears it across the game.
    #[serde(default)]
    pub message: Option<String>,
    /// Or: a scope for an overview — `mission:<name>`, `checkpoint:<n>`,
    /// `stream:<stream>`, `group:<stream>#<resource>`, `level:<stream>`, `game`.
    #[serde(default)]
    pub scope: Option<String>,
    /// With `object`: how many message hops to follow (default 1, at most 4).
    #[serde(default)]
    pub depth: Option<usize>,
    /// With `object`: `down` (messages it sends → listeners), `up` (senders
    /// of what it hears) or `both` (default).
    #[serde(default)]
    pub direction: Option<String>,
    /// Skip this many listed items (paging of `message` and `scope`).
    #[serde(default)]
    pub offset: Option<usize>,
    /// At most this many objects or messages (default 100, at most 400).
    #[serde(default)]
    pub limit: Option<usize>,
    /// Read the game files only, without the edits of the workspace.
    #[serde(default)]
    pub original: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct LevelObjectsParams {
    /// A stream with SimGroups (`gf2/miami/local/bonita/high/terrain06`) or
    /// a prefix of such streams (`gf2/miami/local/bonita`).
    pub stream: String,
    /// Only objects whose class or name contains this text.
    #[serde(default)]
    pub filter: Option<String>,
    /// Only objects within `radius` of this point `[x, y, z]` (Y is up), nearest first.
    #[serde(default)]
    pub near: Option<[f32; 3]>,
    /// Radius for `near` in metres (default 30).
    #[serde(default)]
    pub radius: Option<f32>,
    /// Skip this many objects (paging).
    #[serde(default)]
    pub offset: Option<usize>,
    /// At most this many objects (default 100, at most 500).
    #[serde(default)]
    pub limit: Option<usize>,
    /// Read the game files only, without the edits of the workspace.
    #[serde(default)]
    pub original: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct WorkspaceStatusParams {
    /// Skip this many changes (paging).
    #[serde(default)]
    pub offset: Option<usize>,
    /// At most this many changes (default 100, at most 500).
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct WorkspaceDiffParams {
    /// Only changes whose key contains this text (a stream, a type, an id).
    #[serde(default)]
    pub filter: Option<String>,
    /// Skip this many changes (paging).
    #[serde(default)]
    pub offset: Option<usize>,
    /// At most this many changes (default 20, at most 100).
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ProjectStatusParams {
    /// List unchanged entries too.
    #[serde(default)]
    pub all: Option<bool>,
    /// Skip this many entries (paging).
    #[serde(default)]
    pub offset: Option<usize>,
    /// At most this many entries (default 100, at most 500).
    #[serde(default)]
    pub limit: Option<usize>,
}

// ------------------------------------------------------------ helpers

/// `value` or `default`, at most `max`.
pub fn clamp(value: Option<usize>, default: usize, max: usize) -> usize {
    value.unwrap_or(default).clamp(1, max)
}

/// A 32-bit hash: `0x…`, eight hex digits, or a decimal number.
pub fn parse_hash(text: &str) -> ToolResult<u32> {
    let t = text.trim();
    let hex = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X"));
    let parsed = match hex {
        Some(h) => u32::from_str_radix(h, 16).ok(),
        None if t.len() == 8 && t.chars().all(|c| c.is_ascii_hexdigit()) => {
            u32::from_str_radix(t, 16).ok()
        }
        None => t.parse::<u32>().ok(),
    };
    parsed.ok_or_else(|| {
        ToolError(format!(
            "`{t}` is not a hash: write 0x9cf57f36, 9cf57f36 or a decimal number"
        ))
    })
}

pub fn hex(value: u32) -> String {
    format!("{value:08x}")
}

/// The record a key or a one-result query names.
pub fn resolve(kit: &Toolkit, text: &str) -> ToolResult<RecordId> {
    let index = kit.index();
    let text = text.trim();
    if let Ok(key) = text.parse::<ResourceKey>() {
        return index
            .find_key(&key)
            .ok_or_else(|| ToolError(format!("no resource {key} in the game index")));
    }
    let query = Query::parse(text)?;
    if query.is_empty() {
        return fail("give a resource key (`stream:type:ids`) or a search query");
    }
    let (results, total) = nav_search::search(index, &query, 10);
    match total {
        0 => fail(format!("nothing matches `{text}`; try `search`")),
        1 => Ok(results[0].record),
        n => {
            let keys: Vec<String> = results
                .iter()
                .map(|r| format!("{} ({})", index.key(r.record), index.label(r.record)))
                .collect();
            fail(format!(
                "`{text}` matches {n} resources; give one key. First ones: {}",
                keys.join("; ")
            ))
        }
    }
}

/// `key`, `label`, `type`, `stream`, `index` of a record.
pub fn brief(kit: &Toolkit, id: RecordId) -> serde_json::Value {
    let index = kit.index();
    let record = index.record(id);
    serde_json::json!({
        "key": index.key(id).to_string(),
        "label": index.label(id),
        "type": doctor_toolkit::resource::names::type_tags().display(record.header.type_id),
        "stream": index.stream_name(index.stream_of(id)),
        "index": record.resource,
    })
}

/// An object id as the tools print it: `<stream>#<resource>:<object>`.
pub fn object_id(kit: &Toolkit, group: doctor_toolkit::level::Location, object: usize) -> String {
    let stream = kit
        .game
        .streams()
        .get(group.stream)
        .map_or_else(|| format!("#{}", group.stream), |s| s.stream_name());
    format!("{stream}#{}:{object}", group.resource)
}

/// Parses `<stream>#<resource>:<object>`.
pub fn parse_object_id(
    kit: &Toolkit,
    text: &str,
) -> ToolResult<Option<(doctor_toolkit::level::Location, usize)>> {
    let Some((group, object)) = text.trim().rsplit_once(':') else {
        return Ok(None);
    };
    let Some((stream, resource)) = group.rsplit_once('#') else {
        return Ok(None);
    };
    let object = object
        .parse()
        .map_err(|_| ToolError(format!("`{object}` is not an object number")))?;
    let resource = resource
        .parse()
        .map_err(|_| ToolError(format!("`{resource}` is not a resource number")))?;
    let stream = kit.stream(stream)?;
    Ok(Some((
        doctor_toolkit::level::Location { stream, resource },
        object,
    )))
}

//! Who sends and who hears every message of the game.

use std::collections::HashMap;

use serde::Serialize;

use crate::Error;
use crate::game::{GameInstance, ResourceIndex};
use crate::hash::string_id;
use crate::level::Location;
use crate::resource::simgroup::Value;
use crate::workspace::Workspace;

use super::schema::{LogicSchema, PinKind};
use super::scope::{self, ScopeSpec};

/// One attribute of one object that sends or hears a message.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Site {
    #[serde(serialize_with = "ser_location")]
    pub group: Location,
    pub object: usize,
    pub class: u32,
    pub class_name: String,
    /// The object's name (`CMD_SetName`).
    pub name: String,
    /// The pin: `<attribute name>` from the schema or `<class>.<index>`.
    pub pin: String,
    pub kind: PinKind,
}

fn ser_location<S: serde::Serializer>(l: &Location, s: S) -> Result<S::Ok, S::Error> {
    (l.stream, l.resource).serialize(s)
}

/// Senders and listeners of one message across the game.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct MessageSites {
    /// The name as first seen.
    pub name: String,
    pub senders: Vec<Site>,
    pub listeners: Vec<Site>,
}

/// Every message of the game (SimGroups as the workspace has them).
#[derive(Debug, Default)]
pub struct MessageIndex {
    pub by_hash: HashMap<u32, MessageSites>,
    /// Stream names by index, for printing sites.
    pub streams: Vec<String>,
    /// SimGroups that could not be read.
    pub errors: Vec<(Location, Error)>,
}

impl MessageIndex {
    /// Reads every SimGroup of the game.
    pub fn build(game: &GameInstance, index: &ResourceIndex, ws: Option<&Workspace>) -> Self {
        let locations = scope::groups(game, index, &ScopeSpec::Game).unwrap_or_default();
        let (loaded, errors) = scope::load(game, ws, &locations);
        let schema = LogicSchema::gf2();
        let mut by_hash: HashMap<u32, MessageSites> = HashMap::new();
        for g in &loaded {
            for (i, o) in g.group.objects.iter().enumerate() {
                for op in schema.object_pins(&g.group, o) {
                    let (PinKind::In | PinKind::Out, Some(Value::String(s))) =
                        (op.attr.pin, &op.value)
                    else {
                        continue;
                    };
                    let class_name = o
                        .class_name()
                        .or_else(|| schema.class_name(o.class))
                        .map_or_else(|| format!("{:08x}", o.class), str::to_owned);
                    let site = Site {
                        group: g.location,
                        object: i,
                        class: o.class,
                        class_name,
                        name: g.group.object_name(o).unwrap_or_default().to_owned(),
                        pin: op.attr.name.clone().unwrap_or_else(|| {
                            format!(
                                "{}.{}",
                                schema
                                    .class_name(op.class)
                                    .map_or_else(|| format!("{:08x}", op.class), str::to_owned),
                                op.attr.index
                            )
                        }),
                        kind: op.attr.pin,
                    };
                    let entry = by_hash.entry(string_id(s)).or_insert_with(|| MessageSites {
                        name: s.clone(),
                        ..MessageSites::default()
                    });
                    if op.attr.pin == PinKind::Out {
                        entry.senders.push(site);
                    } else {
                        entry.listeners.push(site);
                    }
                }
            }
        }
        Self {
            by_hash,
            streams: game.streams().iter().map(|f| f.stream_name()).collect(),
            errors,
        }
    }

    /// Senders and listeners of a message by name.
    pub fn who(&self, name: &str) -> Option<&MessageSites> {
        self.by_hash.get(&string_id(name))
    }

    /// Message names that contain `text` (case-insensitive), sorted.
    pub fn search(&self, text: &str) -> Vec<&MessageSites> {
        let want = text.to_ascii_lowercase();
        let mut out: Vec<&MessageSites> = self
            .by_hash
            .values()
            .filter(|m| m.name.to_ascii_lowercase().contains(&want))
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Counts over the whole game.
    pub fn stats(&self) -> IndexStats {
        let schema = LogicSchema::gf2();
        let mut s = IndexStats {
            messages: self.by_hash.len(),
            ..IndexStats::default()
        };
        for (&h, m) in &self.by_hash {
            s.senders += m.senders.len();
            s.listeners += m.listeners.len();
            match (m.senders.is_empty(), m.listeners.is_empty()) {
                (false, false) => s.connected += 1,
                (true, false) if schema.engine_message(h).is_some() => s.from_engine += 1,
                (false, true) if schema.engine_message(h).is_some() => s.to_engine += 1,
                (true, false) => s.never_sent += 1,
                _ => s.never_heard += 1,
            }
        }
        s
    }
}

/// Message counts of the whole game.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct IndexStats {
    pub messages: usize,
    pub senders: usize,
    pub listeners: usize,
    pub connected: usize,
    pub from_engine: usize,
    pub to_engine: usize,
    pub never_sent: usize,
    pub never_heard: usize,
}

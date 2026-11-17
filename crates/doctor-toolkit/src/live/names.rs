//! Names for what the live link reports (stage 49): the module knows GUIDs
//! of objects and hashes of messages, the toolkit knows the SimGroups — of
//! the game and, where the workspace changed them, of the workspace. This
//! index maps a GUID to the object's name (`CMD_SetName`), class, stream
//! and place, and a message hash to its name, so the journal and the list
//! of objects show names and a search by name finds GUIDs.

use std::collections::HashMap;
use std::path::Path;

use crate::Result;
use crate::game::{GameInstance, ResourceIndex};
use crate::hash::string_id;
use crate::logic_graph::scope::{self, LoadedGroup};
use crate::logic_graph::{LogicSchema, PinKind, ScopeSpec};
use crate::resource::simgroup::Value;
use crate::workspace::Workspace;

use super::proto::Guid;

/// What the toolkit knows of one object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectName {
    /// `CMD_SetName`, empty when the object has none.
    pub name: String,
    /// Class name (`TriggerBox`), or the hash in hex.
    pub class: String,
    /// The stream of its SimGroup (`gf2/cuba/local`).
    pub stream: String,
    /// Resource index of the SimGroup in the stream and the object's index
    /// in it.
    pub resource: usize,
    pub object: usize,
    /// How many SimGroups have an object with this GUID (the game has a few
    /// hundred GUIDs twice).
    pub copies: u32,
}

/// A variable of the story as the `variabledictionary` resources name it
/// (stage 48).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariableName {
    pub name: String,
    /// The stream of the first dictionary that has it.
    pub stream: String,
    /// Its value at the start of the level.
    pub initial: i32,
    /// Hash of its record in the dictionary (the game's `+0`).
    pub record: u32,
}

/// Names of objects by GUID, of messages and variables by hash.
#[derive(Debug, Default)]
pub struct LiveNames {
    objects: HashMap<Guid, ObjectName>,
    messages: HashMap<u32, String>,
    /// Variables by `sdbm(lower(name))` — what the game keeps at `+4`.
    variables: HashMap<u32, VariableName>,
    /// SimGroups that could not be read.
    pub errors: usize,
}

impl LiveNames {
    /// Reads every SimGroup of the game (the workspace's version where it
    /// has one).
    pub fn build(game: &GameInstance, index: &ResourceIndex, ws: Option<&Workspace>) -> Self {
        let locations = scope::groups(game, index, &ScopeSpec::Game).unwrap_or_default();
        let (loaded, errors) = scope::load(game, ws, &locations);
        let mut out = Self::from_groups(&loaded);
        out.errors = errors.len();
        out.read_variables(game, index, ws);
        out
    }

    /// Names of the variables of every `variabledictionary` of the game
    /// (the workspace's version where it has one).
    fn read_variables(
        &mut self,
        game: &GameInstance,
        index: &ResourceIndex,
        ws: Option<&Workspace>,
    ) {
        let dict_type = crate::resource::logic::TYPE_VARIABLE_DICTIONARY;
        for e in index
            .entries
            .iter()
            .filter(|e| e.header.type_id == dict_type)
        {
            let file = &game.streams()[e.stream as usize];
            let key = crate::workspace::ResourceKey::new(&file.stream_name(), &e.header);
            let data = ws.and_then(|w| w.data(&key).ok().flatten()).or_else(|| {
                game.open_stream(e.stream as usize)
                    .ok()?
                    .resource_data(e.resource as usize)
                    .ok()
            });
            let Some(dict) = data.and_then(|d| {
                crate::resource::variabledictionary::VariableDictionary::parse(&d).ok()
            }) else {
                continue;
            };
            for v in dict.variables {
                self.insert_variable(VariableName {
                    record: v.hash.0,
                    initial: v.value as i32,
                    stream: file.stream_name(),
                    name: v.name,
                });
            }
        }
    }

    /// Adds a variable (the first one of a name wins).
    pub fn insert_variable(&mut self, v: VariableName) {
        self.variables
            .entry(super::proto::name_hash(&v.name))
            .or_insert(v);
    }

    /// A variable by the hash of its name or of its record.
    pub fn variable(&self, hash: u32) -> Option<&VariableName> {
        self.variables
            .get(&hash)
            .or_else(|| self.variables.values().find(|v| v.record == hash))
    }

    /// Variables whose name contains `text` (case-insensitive), by name;
    /// with the hash of the name.
    pub fn search_variables(&self, text: &str) -> Vec<(u32, &VariableName)> {
        let want = text.trim().to_ascii_lowercase();
        let mut out: Vec<(u32, &VariableName)> = self
            .variables
            .iter()
            .filter(|(_, v)| v.name.to_ascii_lowercase().contains(&want))
            .map(|(h, v)| (*h, v))
            .collect();
        out.sort_by(|a, b| a.1.name.cmp(&b.1.name));
        out
    }

    pub fn variables(&self) -> usize {
        self.variables.len()
    }

    /// Opens the game at `root` and its workspace (unless `original`) and
    /// builds the index.
    pub fn open(root: &Path, original: bool) -> Result<Self> {
        let game = GameInstance::open(root)?;
        let (index, _) = game.build_index(|_| {});
        let dir = Workspace::default_dir(game.root());
        let ws = (!original && Workspace::exists(&dir))
            .then(|| Workspace::open(&dir))
            .transpose()?;
        Ok(Self::build(&game, &index, ws.as_ref()))
    }

    /// The index of loaded SimGroups.
    pub fn from_groups(groups: &[LoadedGroup]) -> Self {
        let schema = LogicSchema::gf2();
        let mut out = Self::default();
        for g in groups {
            for (i, o) in g.group.objects.iter().enumerate() {
                let class = o
                    .class_name()
                    .or_else(|| schema.class_name(o.class))
                    .map_or_else(|| format!("{:08x}", o.class), str::to_owned);
                let name = g.group.object_name(o).unwrap_or_default().to_owned();
                out.objects
                    .entry(o.guid.0)
                    .and_modify(|e| e.copies += 1)
                    .or_insert(ObjectName {
                        name,
                        class,
                        stream: g.stream.clone(),
                        resource: g.location.resource,
                        object: i,
                        copies: 1,
                    });
                for pin in schema.object_pins(&g.group, o) {
                    if let (PinKind::In | PinKind::Out, Some(Value::String(s))) =
                        (pin.attr.pin, &pin.value)
                        && !s.is_empty()
                    {
                        out.messages
                            .entry(string_id(s))
                            .or_insert_with(|| s.clone());
                    }
                }
            }
        }
        out
    }

    /// Adds a name by hand (tests, names the client learnt elsewhere).
    pub fn insert_object(&mut self, guid: Guid, name: ObjectName) {
        self.objects.insert(guid, name);
    }

    pub fn insert_message(&mut self, name: &str) {
        self.messages.insert(string_id(name), name.to_owned());
    }

    pub fn object(&self, guid: &Guid) -> Option<&ObjectName> {
        self.objects.get(guid)
    }

    /// A message name: from the SimGroups, else one the engine sends or
    /// hears (the logic schema).
    pub fn message(&self, hash: u32) -> Option<&str> {
        self.messages
            .get(&hash)
            .map(String::as_str)
            .or_else(|| LogicSchema::gf2().engine_message(hash))
    }

    /// Objects whose name contains `text` (case-insensitive) or whose GUID
    /// (32 hex digits in memory order) starts with it, by name.
    pub fn search(&self, text: &str) -> Vec<(Guid, &ObjectName)> {
        let want = text.trim().to_ascii_lowercase();
        if want.is_empty() {
            return Vec::new();
        }
        let mut out: Vec<(Guid, &ObjectName)> = self
            .objects
            .iter()
            .filter(|(g, o)| {
                o.name.to_ascii_lowercase().contains(&want)
                    || super::proto::guid_hex(g).starts_with(&want)
            })
            .map(|(g, o)| (*g, o))
            .collect();
        out.sort_by(|a, b| a.1.name.cmp(&b.1.name).then(a.0.cmp(&b.0)));
        out
    }

    pub fn objects(&self) -> usize {
        self.objects.len()
    }

    pub fn messages(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.objects.is_empty() && self.messages.is_empty() && self.variables.is_empty()
    }
}

/// One line about an event of the journal, with names where the index
/// knows them: `print scripts/x.lua:3: text`, `Lua error: …`, `message
/// name -> 3 listener(s): a, b`. The call stack of a Lua error is not in it.
pub fn describe(e: &super::proto::Event, names: Option<&LiveNames>) -> String {
    use super::proto::journal::{EventBody, MessageOrigin};
    match &e.body {
        EventBody::Output {
            source,
            text,
            script,
            line,
        } => {
            if script.is_empty() {
                format!("{}: {text}", source.name())
            } else {
                format!("{} {script}:{line}: {text}", source.name())
            }
        }
        EventBody::LuaError { message, .. } => format!("Lua error: {message}"),
        EventBody::Message {
            hash,
            listeners,
            receivers,
            origin,
            from,
            ..
        } => {
            let name = names
                .and_then(|n| n.message(*hash))
                .map_or_else(|| format!("{hash:#010x}"), str::to_owned);
            let mut line = format!("message {name} -> {listeners} listener(s)");
            if !receivers.is_empty() {
                let list: Vec<String> = receivers
                    .iter()
                    .map(|g| match names.and_then(|n| n.object(g)) {
                        Some(o) if !o.name.is_empty() => o.name.clone(),
                        Some(o) => format!("{} {}", o.class, &super::proto::guid_hex(g)[..8]),
                        None => super::proto::guid_hex(g)[..8].to_owned(),
                    })
                    .collect();
                line += &format!(": {}", list.join(", "));
            }
            if *origin == MessageOrigin::Link {
                line += " (live link)";
            }
            if let Some(g) = from.and_then(|f| f.sender) {
                let who = match names.and_then(|n| n.object(&g)) {
                    Some(o) if !o.name.is_empty() => o.name.clone(),
                    Some(o) => format!("{} {}", o.class, &super::proto::guid_hex(&g)[..8]),
                    None => super::proto::guid_hex(&g)[..8].to_owned(),
                };
                line += &format!(" from {who}");
            }
            line
        }
        EventBody::MessageCounts { window_ms, counts } => {
            let total: u64 = counts.iter().map(|(_, n)| u64::from(*n)).sum();
            let mut top = counts.clone();
            top.sort_by_key(|x| std::cmp::Reverse(x.1));
            let list: Vec<String> = top
                .iter()
                .take(5)
                .map(|(h, n)| {
                    let name = names
                        .and_then(|x| x.message(*h))
                        .map_or_else(|| format!("{h:#010x}"), str::to_owned);
                    format!("{name} ×{n}")
                })
                .collect();
            format!(
                "messages without own events in {:.1} s: {total} of {} kind(s): {}",
                f64::from(*window_ms) / 1000.0,
                counts.len(),
                list.join(", ")
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(name: &str, class: &str) -> ObjectName {
        ObjectName {
            name: name.into(),
            class: class.into(),
            stream: "gf2/cuba".into(),
            resource: 3,
            object: 1,
            copies: 1,
        }
    }

    #[test]
    fn names_and_search() {
        let mut n = LiveNames::default();
        n.insert_object([0xAB; 16], object("trig_gate_enter", "TriggerBox"));
        n.insert_object([0x12; 16], object("relay_gate", "MessageRelay"));
        n.insert_message("doctor_trig_enter");
        assert_eq!(n.object(&[0xAB; 16]).unwrap().class, "TriggerBox");
        assert_eq!(
            n.message(crate::hash::string_id("Doctor_Trig_Enter")),
            Some("doctor_trig_enter")
        );
        let found = n.search("GATE");
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].1.name, "relay_gate");
        assert_eq!(n.search("abab").len(), 1);
        assert!(n.search("  ").is_empty());
        assert!(n.message(0x0BAD_F00D).is_none());
        n.insert_variable(VariableName {
            name: "global_tutorial_payday".into(),
            stream: "gf2/globals_global".into(),
            initial: 1,
            record: 0x001A_01D2,
        });
        // The pair read on the stand: sdbm of the name, the record's hash.
        assert_eq!(n.variable(0x9381_3AB9).unwrap().initial, 1);
        assert_eq!(
            n.variable(0x001A_01D2).unwrap().name,
            "global_tutorial_payday"
        );
        assert_eq!(n.search_variables("PAYDAY").len(), 1);
        assert_eq!(n.variables(), 1);
    }

    #[test]
    fn events_in_one_line() {
        use crate::live::proto::Event;
        use crate::live::proto::journal::{EventBody, MessageOrigin, OutputSource};
        let mut n = LiveNames::default();
        n.insert_object([0xAB; 16], object("trig_gate_enter", "TriggerBox"));
        n.insert_message("doctor_trig_enter");
        let ev = |body| Event {
            seq: 1,
            frame: 2,
            sim_time_ms: 3,
            body,
        };
        let m = ev(EventBody::Message {
            hash: crate::hash::string_id("doctor_trig_enter"),
            data: 0,
            forced: false,
            listeners: 2,
            receivers: vec![[0xAB; 16], [0x01; 16]],
            origin: MessageOrigin::Link,
            from: None,
        });
        assert_eq!(
            describe(&m, Some(&n)),
            "message doctor_trig_enter -> 2 listener(s): trig_gate_enter, 01010101 (live link)"
        );
        let p = ev(EventBody::Output {
            source: OutputSource::Print,
            text: "hi".into(),
            script: "x.lua".into(),
            line: 3,
        });
        assert_eq!(describe(&p, None), "print x.lua:3: hi");
    }
}

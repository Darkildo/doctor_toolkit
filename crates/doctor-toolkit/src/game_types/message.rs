//! Messages as an open set: named by the engine, named by the game files,
//! made by a mod, or known only by hash.
//!
//! A message is `sdbm(lower(name))` on the bus (`docs/logic-graph.md` §1), so
//! two names with one hash are one message for the game. [`MessageId`] says
//! where a name is known from; [`MessageCatalog`] holds who sends and who
//! hears every message of the SimGroups of the game
//! (`data/gf2/game_messages.tsv`, built by `research/message_catalog.py`).

use std::collections::HashMap;
use std::io::Read as _;
use std::sync::OnceLock;

use serde::Serialize;

use super::{CLASS_DATA, DATA_TYPES, MESSAGE_FACTS, MESSAGE_NOTES};
use super::{ENGINE_MESSAGES, GAME_MESSAGES_COUNT, GAME_MESSAGES_DEFLATED, GAME_MESSAGES_SIZE};
use crate::hash::string_id;

/// How a fact of `data/gf2/messages.toml` is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NoteConfidence {
    /// Seen in the code of `godfather2.exe`.
    Exe,
    /// Inferred from the game files.
    Data,
    /// Checked in the running game.
    Bench,
    Guess,
}

impl NoteConfidence {
    pub fn name(self) -> &'static str {
        match self {
            Self::Exe => "exe",
            Self::Data => "data",
            Self::Bench => "bench",
            Self::Guess => "guess",
        }
    }
}

/// Facts about one message, kept by hand in `data/gf2/messages.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct MessageNote {
    pub name: &'static str,
    pub hash: u32,
    pub summary: &'static str,
    /// The summary in Russian, for the notes.
    pub ru: &'static str,
    /// The data word of the message; `"unknown"` until it is read in the code.
    pub data: &'static str,
    /// Senders that are not SimGroup objects.
    pub sent_by: &'static [&'static str],
    /// The same in Russian, for the notes.
    #[serde(skip)]
    pub sent_by_ru: &'static [&'static str],
    pub conf: NoteConfidence,
    pub source: &'static str,
}

/// What the data word of a message can be (`data_type` of `messages.toml`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DataType {
    /// `none`, `object`, `forward`, or a structure of the engine.
    pub key: &'static str,
    /// Method table of the data object in `godfather2.exe`; 0 when the word
    /// is not an object.
    pub table: u32,
    pub summary: &'static str,
    #[serde(skip)]
    pub ru: &'static str,
    pub conf: NoteConfidence,
    pub source: &'static str,
}

/// The data word the outputs of a SimGroup class send (`class_data` of
/// `messages.toml`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ClassData {
    /// The packet class as `logic_schema.toml` spells it.
    pub class: &'static str,
    /// A key of [`data_types`].
    pub data: &'static str,
    /// Whose object, when the data is an object pointer; may be empty.
    pub who: &'static str,
    #[serde(skip)]
    pub who_ru: &'static str,
    pub conf: NoteConfidence,
    pub source: &'static str,
}

/// What the code of the executable and the other resources of the game do
/// with a message: `data/gf2/bus_sites.tsv` (built by
/// `research/bus_sites.py`) and `data/gf2/message_senders.tsv`
/// (`research/message_senders.py`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct MessageFact {
    pub hash: u32,
    /// Places of `godfather2.exe` that send it: the address of the call and
    /// how — 0 to every listener (`0x00408A00`), 1 to one listener
    /// (`0x00408BF0`), 2 through the delayed queue (`0x00440590`).
    pub sites: &'static [(u32, u8)],
    /// Places of the executable that subscribe a listener to it.
    pub listens: u32,
    /// Senders in other resources: kind (`lua`, `cutscene`, `score`), number
    /// of places, number of resources.
    pub other: &'static [(&'static str, u32, u32)],
    /// Keys of [`data_types`] those senders put into the data word;
    /// `unknown` where the code was not followed.
    pub data: &'static [&'static str],
}

/// Every kind of data word.
pub fn data_types() -> &'static [DataType] {
    DATA_TYPES
}

/// The kind of data word with this key.
pub fn data_type(key: &str) -> Option<&'static DataType> {
    DATA_TYPES
        .binary_search_by_key(&key, |t| t.key)
        .ok()
        .map(|i| &DATA_TYPES[i])
}

/// What the outputs of every class with a known answer send.
pub fn class_data() -> &'static [ClassData] {
    CLASS_DATA
}

/// The data word the outputs of `class` send, when it was read in the code.
pub fn data_of_class(class: &str) -> Option<&'static ClassData> {
    CLASS_DATA
        .binary_search_by_key(&class, |c| c.class)
        .ok()
        .map(|i| &CLASS_DATA[i])
}

/// What the code and the other resources do with the message, by hash.
pub fn fact(hash: u32) -> Option<&'static MessageFact> {
    MESSAGE_FACTS
        .binary_search_by_key(&hash, |f| f.hash)
        .ok()
        .map(|i| &MESSAGE_FACTS[i])
}

/// Every message with a sender or a listener outside the SimGroups.
pub fn facts() -> &'static [MessageFact] {
    MESSAGE_FACTS
}

impl MessageFact {
    /// Senders that are not SimGroup objects, for people.
    pub fn sent_by(&self) -> Vec<String> {
        self.sent_by_in(false)
    }

    /// The same in Russian, for the notes.
    pub fn sent_by_ru(&self) -> Vec<String> {
        self.sent_by_in(true)
    }

    fn sent_by_in(&self, ru: bool) -> Vec<String> {
        let mut out = Vec::new();
        if !self.sites.is_empty() {
            let list: Vec<String> = self
                .sites
                .iter()
                .take(4)
                .map(|(site, how)| {
                    let how = match (how, ru) {
                        (1, false) => " (to one listener)",
                        (1, true) => " (одному слушателю)",
                        (2, false) => " (delayed queue)",
                        (2, true) => " (очередь задержки)",
                        _ => "",
                    };
                    format!("{site:#010X}{how}").replace("0X", "0x")
                })
                .collect();
            let more = self.sites.len().saturating_sub(4);
            let tail = match (more, ru) {
                (0, _) => String::new(),
                (n, false) => format!(" and {n} more"),
                (n, true) => format!(" и ещё {n}"),
            };
            out.push(if ru {
                format!("код движка: {}{tail}", list.join(", "))
            } else {
                format!("engine code: {}{tail}", list.join(", "))
            });
        }
        for &(kind, places, resources) in self.other {
            out.push(match (kind, ru) {
                ("lua", false) => {
                    format!("Lua: {places} call(s) of SendMessage in {resources} script(s)")
                }
                ("lua", true) => {
                    format!("Lua: вызовов SendMessage — {places}, скриптов — {resources}")
                }
                ("cutscene", false) => {
                    format!("cut-scene: {places} command(s) 0x402 in {resources} sequence(s)")
                }
                ("cutscene", true) => {
                    format!("ролик: команд 0x402 — {places}, кадров — {resources}")
                }
                (_, false) => format!("score document: {places} action(s)"),
                (_, true) => format!("документ счёта (score): действий — {places}"),
            });
        }
        out
    }
}

/// The data word of a message, from what is known: the hand-written note,
/// else the senders in the code and in other resources, else — for a message
/// of the game files — the classes that send it. Returns the kinds joined
/// with ` | ` (keys of [`data_types`], `unknown` for a sender that was not
/// read), how it is known and the source.
fn data_word(
    note: Option<&MessageNote>,
    fact: Option<&MessageFact>,
    game: Option<&GameMessage>,
) -> (String, Option<&'static str>, Option<&'static str>) {
    if let Some(n) = note.filter(|n| n.data != "unknown") {
        return (n.data.to_owned(), Some(n.conf.name()), Some(n.source));
    }
    let mut kinds: Vec<&str> = Vec::new();
    let mut sources: Vec<&'static str> = Vec::new();
    if let Some(f) = fact.filter(|f| !f.data.is_empty()) {
        kinds.extend(f.data);
        if !f.sites.is_empty() {
            sources.push("data/gf2/bus_sites.tsv");
        }
        if !f.other.is_empty() {
            sources.push("data/gf2/message_senders.tsv");
        }
    }
    if let Some(g) = game {
        let senders = g.senders();
        if !senders.is_empty() {
            sources.push("data/gf2/messages.toml (class_data)");
        }
        for s in senders {
            kinds.push(data_of_class(s.class).map_or("unknown", |c| c.data));
        }
    }
    kinds.sort_unstable();
    kinds.dedup();
    if kinds.is_empty() || kinds == ["unknown"] {
        return ("unknown".to_owned(), None, None);
    }
    (kinds.join(" | "), Some("exe"), sources.first().copied())
}

/// The note of a message, by hash.
pub fn note(hash: u32) -> Option<&'static MessageNote> {
    MESSAGE_NOTES
        .binary_search_by_key(&hash, |n| n.hash)
        .ok()
        .map(|i| &MESSAGE_NOTES[i])
}

/// Every note of `messages.toml`.
pub fn notes() -> &'static [MessageNote] {
    MESSAGE_NOTES
}

/// The name the executable gives to the message with this hash.
pub fn engine_name(hash: u32) -> Option<&'static str> {
    ENGINE_MESSAGES
        .binary_search_by_key(&hash, |e| e.0)
        .ok()
        .map(|i| ENGINE_MESSAGES[i].1)
}

/// Messages named by the executable: (hash, name), sorted by hash.
pub fn engine_messages() -> &'static [(u32, &'static str)] {
    ENGINE_MESSAGES
}

/// Attribute `index` of the packet of `class`, used `count` times.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PinUse {
    /// The packet class as `logic_schema.toml` spells it.
    pub class: &'static str,
    pub index: u16,
    pub count: u32,
}

/// A message of the SimGroups of the game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GameMessage {
    /// The most frequent spelling.
    pub name: &'static str,
    pub hash: u32,
    senders: &'static str,
    listeners: &'static str,
    /// Common directory of the streams that use the message; `*` when they
    /// share none.
    pub scope: &'static str,
    /// Number of streams (SimGroups) that use it.
    pub streams: u32,
    mentions: &'static str,
}

fn pin_uses(cell: &'static str) -> Vec<PinUse> {
    cell.split(',')
        .filter_map(|item| {
            let (pin, count) = item.rsplit_once(':')?;
            let (class, index) = pin.rsplit_once('.')?;
            Some(PinUse {
                class,
                index: index.parse().ok()?,
                count: count.parse().ok()?,
            })
        })
        .collect()
}

impl GameMessage {
    /// Attributes that send the message, with the number of objects.
    pub fn senders(&self) -> Vec<PinUse> {
        pin_uses(self.senders)
    }

    /// Attributes that listen to the message.
    pub fn listeners(&self) -> Vec<PinUse> {
        pin_uses(self.listeners)
    }

    pub fn is_sent(&self) -> bool {
        self.senders != "-"
    }

    pub fn is_heard(&self) -> bool {
        self.listeners != "-"
    }

    /// Other resources that name the message: (`lua`, `score` or `seq` for
    /// cut-scenes; number of resources). A mention is not a proof of sending.
    pub fn mentions(&self) -> Vec<(&'static str, u32)> {
        self.mentions
            .split(',')
            .filter_map(|item| {
                let (label, count) = item.split_once(':')?;
                Some((label, count.parse().ok()?))
            })
            .collect()
    }
}

/// Every message of the SimGroups of the game, by hash.
#[derive(Debug)]
pub struct MessageCatalog {
    by_hash: HashMap<u32, GameMessage>,
}

impl MessageCatalog {
    /// The catalogue of The Godfather II (PC 1.0.789.0); inflated on first use.
    pub fn gf2() -> &'static Self {
        static CATALOG: OnceLock<MessageCatalog> = OnceLock::new();
        CATALOG.get_or_init(|| {
            let mut text = String::with_capacity(GAME_MESSAGES_SIZE);
            flate2::read::DeflateDecoder::new(GAME_MESSAGES_DEFLATED)
                .read_to_string(&mut text)
                .expect("the catalogue was deflated by build.rs");
            // Lives as long as the program: entries borrow from it.
            let text: &'static str = Box::leak(text.into_boxed_str());
            let mut by_hash = HashMap::with_capacity(GAME_MESSAGES_COUNT);
            for line in text.lines() {
                let mut fields = line.split('\t');
                let mut next = || fields.next().expect("build.rs checked the columns");
                let name = next();
                let hash = string_id(name);
                by_hash.insert(
                    hash,
                    GameMessage {
                        name,
                        hash,
                        senders: next(),
                        listeners: next(),
                        scope: next(),
                        streams: next().parse().unwrap_or(0),
                        mentions: next(),
                    },
                );
            }
            MessageCatalog { by_hash }
        })
    }

    pub fn len(&self) -> usize {
        self.by_hash.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_hash.is_empty()
    }

    pub fn get(&self, hash: u32) -> Option<&GameMessage> {
        self.by_hash.get(&hash)
    }

    /// A message by name; letter case does not matter.
    pub fn by_name(&self, name: &str) -> Option<&GameMessage> {
        self.get(string_id(name))
    }

    pub fn iter(&self) -> impl Iterator<Item = &GameMessage> {
        self.by_hash.values()
    }

    /// Names that contain `text` (letter case does not matter): names
    /// starting with it first, then by name; at most `limit`.
    pub fn search(&self, text: &str, limit: usize) -> Vec<&GameMessage> {
        let needle = text.to_ascii_lowercase();
        let mut found: Vec<(bool, &GameMessage)> = self
            .by_hash
            .values()
            .filter_map(|m| {
                let at = m.name.to_ascii_lowercase().find(&needle)?;
                Some((at != 0, m))
            })
            .collect();
        found.sort_by(|a, b| (a.0, a.1.name).cmp(&(b.0, b.1.name)));
        found.into_iter().take(limit).map(|(_, m)| m).collect()
    }
}

/// Why a name cannot be a message of a mod.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CustomMessageError {
    #[error("a message name must not be empty")]
    Empty,
    #[error("message name `{0}` has characters the game files do not hold (printable ASCII only)")]
    NotAscii(String),
    #[error("`{name}` is a message of the game ({kind}), not a new one")]
    Taken { name: String, kind: &'static str },
    #[error(
        "`{name}` has the hash {hash:#010x} of the game message `{other}`: the game cannot tell them apart"
    )]
    Collision {
        name: String,
        hash: u32,
        other: &'static str,
    },
}

/// The game or engine message with the hash of `name` and a different name:
/// a mod must not use such a name.
pub fn collision(name: &str) -> Option<&'static str> {
    let hash = string_id(name);
    let known = engine_name(hash).or_else(|| MessageCatalog::gf2().get(hash).map(|m| m.name))?;
    (!known.eq_ignore_ascii_case(name)).then_some(known)
}

/// Where a message is known from.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MessageId {
    /// Named by a literal of `godfather2.exe`.
    Engine(&'static str),
    /// Named by the SimGroups of the game.
    Game(&'static str),
    /// A message of a mod: the mod it belongs to (empty when not known), its
    /// name and the hash of the name. [`MessageId::custom`] checks that the
    /// hash is not the one of a game message.
    Custom {
        owner: String,
        name: String,
        hash: u32,
    },
    /// Only the hash is known (a message seen on the bus of the running game).
    Unknown(u32),
}

impl MessageId {
    /// Classifies a name. A name the game does not have is a custom one
    /// without an owner; its hash is not checked here ([`collision`]).
    pub fn from_name(name: &str) -> Self {
        match Self::from_hash(string_id(name)) {
            Self::Unknown(hash) => Self::Custom {
                owner: String::new(),
                name: name.to_owned(),
                hash,
            },
            // One hash with another name: for the game it is that message,
            // but the name written is not the game's.
            known if !known.name().is_some_and(|n| n.eq_ignore_ascii_case(name)) => Self::Custom {
                owner: String::new(),
                name: name.to_owned(),
                hash: string_id(name),
            },
            known => known,
        }
    }

    /// Classifies a hash.
    pub fn from_hash(hash: u32) -> Self {
        if let Some(name) = engine_name(hash) {
            Self::Engine(name)
        } else if let Some(m) = MessageCatalog::gf2().get(hash) {
            Self::Game(m.name)
        } else {
            Self::Unknown(hash)
        }
    }

    /// A message of the mod `owner`: the name must be new to the game and
    /// its hash must not be the hash of a game message.
    pub fn custom(owner: &str, name: &str) -> Result<Self, CustomMessageError> {
        if name.is_empty() {
            return Err(CustomMessageError::Empty);
        }
        if !name.bytes().all(|b| (0x20..0x7F).contains(&b)) {
            return Err(CustomMessageError::NotAscii(name.to_owned()));
        }
        let hash = string_id(name);
        match Self::from_hash(hash) {
            Self::Unknown(_) => Ok(Self::Custom {
                owner: owner.to_owned(),
                name: name.to_owned(),
                hash,
            }),
            known => {
                let other = known
                    .name_static()
                    .expect("engine and game messages have names");
                if other.eq_ignore_ascii_case(name) {
                    Err(CustomMessageError::Taken {
                        name: name.to_owned(),
                        kind: known.kind(),
                    })
                } else {
                    Err(CustomMessageError::Collision {
                        name: name.to_owned(),
                        hash,
                        other,
                    })
                }
            }
        }
    }

    pub fn hash(&self) -> u32 {
        match self {
            Self::Engine(name) | Self::Game(name) => string_id(name),
            Self::Custom { hash, .. } | Self::Unknown(hash) => *hash,
        }
    }

    pub fn name(&self) -> Option<&str> {
        match self {
            Self::Engine(name) | Self::Game(name) => Some(name),
            Self::Custom { name, .. } => Some(name),
            Self::Unknown(_) => None,
        }
    }

    fn name_static(&self) -> Option<&'static str> {
        match self {
            Self::Engine(name) | Self::Game(name) => Some(name),
            _ => None,
        }
    }

    /// `engine`, `game`, `custom` or `unknown`.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Engine(_) => "engine",
            Self::Game(_) => "game",
            Self::Custom { .. } => "custom",
            Self::Unknown(_) => "unknown",
        }
    }

    /// Everything known about the message.
    pub fn info(&self) -> MessageInfo {
        let hash = self.hash();
        let game = MessageCatalog::gf2().get(hash).filter(|_| {
            // A custom name with the hash of a game message is not that message.
            !matches!(self, Self::Custom { .. })
        });
        let note = note(hash).filter(|_| !matches!(self, Self::Custom { .. }));
        let fact = fact(hash).filter(|_| !matches!(self, Self::Custom { .. }));
        let (data, data_conf, data_source) = data_word(note, fact, game);
        let sent_by: Vec<String> = match note {
            Some(n) if !n.sent_by.is_empty() => n.sent_by.iter().map(|s| (*s).to_owned()).collect(),
            _ => fact.map(MessageFact::sent_by).unwrap_or_default(),
        };
        let engine = matches!(self, Self::Engine(_));
        let usage = match (game.map(|g| (g.is_sent(), g.is_heard())), engine) {
            (Some((true, true)), _) => Usage::SentAndHeard,
            (Some((false, true)), true) => Usage::HeardFromEngine,
            (Some((true, false)), true) => Usage::SentToEngine,
            (Some((false, _)), false) => Usage::HeardNeverSent,
            (Some((true, false)), false) => Usage::SentNeverHeard,
            (Some((false, false)), true) | (None, true) => Usage::EngineOnly,
            (None, false) => Usage::NotInGame,
        };
        MessageInfo {
            kind: self.kind(),
            name: self.name().map(str::to_owned),
            hash,
            owner: match self {
                Self::Custom { owner, .. } if !owner.is_empty() => Some(owner.clone()),
                _ => None,
            },
            usage,
            senders: game.map(GameMessage::senders).unwrap_or_default(),
            listeners: game.map(GameMessage::listeners).unwrap_or_default(),
            scope: game.map(|g| g.scope),
            streams: game.map_or(0, |g| g.streams),
            mentions: game.map(GameMessage::mentions).unwrap_or_default(),
            data,
            data_conf,
            data_source,
            summary: note.map(|n| n.summary),
            sent_by,
            engine_sites: fact.map_or(&[], |f| f.sites),
            engine_listens: fact.map_or(0, |f| f.listens),
            conf: match (note, self) {
                (Some(n), _) => Some(n.conf.name()),
                (None, Self::Engine(_)) => Some("exe"),
                (None, Self::Game(_)) => Some("data"),
                _ => None,
            },
            source: match (note, self) {
                (Some(n), _) => Some(n.source),
                (None, Self::Engine(_)) => Some("data/gf2/engine_messages.txt"),
                (None, Self::Game(_)) => Some("data/gf2/game_messages.tsv"),
                _ => None,
            },
        }
    }
}

impl std::fmt::Display for MessageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Engine(name) | Self::Game(name) => f.write_str(name),
            Self::Custom { name, .. } => f.write_str(name),
            Self::Unknown(hash) => write!(f, "{hash:#010x}"),
        }
    }
}

/// How the SimGroups of the whole game use a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Usage {
    /// SimGroup objects send it and hear it.
    SentAndHeard,
    /// SimGroup objects hear it; the executable names it.
    HeardFromEngine,
    /// SimGroup objects send it; the executable names it.
    SentToEngine,
    /// SimGroup objects hear it, none sends it and the executable does not
    /// name it (a cut-scene or a script may send it: see `mentions`).
    HeardNeverSent,
    /// SimGroup objects send it, none hears it.
    SentNeverHeard,
    /// Named by the executable, not used by any SimGroup.
    EngineOnly,
    /// Not a message of the game: a custom name or an unknown hash.
    NotInGame,
}

/// Everything known about a message, for tools and people.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MessageInfo {
    /// `engine`, `game`, `custom` or `unknown`.
    pub kind: &'static str,
    pub name: Option<String>,
    pub hash: u32,
    /// The mod a custom message belongs to.
    pub owner: Option<String>,
    pub usage: Usage,
    /// Attributes of SimGroup classes that send it, over the whole game.
    pub senders: Vec<PinUse>,
    pub listeners: Vec<PinUse>,
    /// Common directory of the streams using it.
    pub scope: Option<&'static str>,
    pub streams: u32,
    /// Other resources naming it: label and number of resources.
    pub mentions: Vec<(&'static str, u32)>,
    /// The data word: keys of [`data_types`] joined with ` | ` (`none`,
    /// `object`, `forward`, a structure of the engine; `unknown` for a
    /// sender whose code was not read), or just `"unknown"`.
    pub data: String,
    /// How the data word is known (`exe`, `data`, `bench`, `guess`).
    pub data_conf: Option<&'static str>,
    /// Where the data word is known from.
    pub data_source: Option<&'static str>,
    pub summary: Option<&'static str>,
    /// Senders that are not SimGroup objects, when known: places of the
    /// executable, Lua scripts, cut-scenes, the score document.
    pub sent_by: Vec<String>,
    /// Places of `godfather2.exe` that send it: address of the call and how
    /// (0 to every listener, 1 to one listener, 2 delayed queue).
    pub engine_sites: &'static [(u32, u8)],
    /// Places of the executable that subscribe a listener to it.
    pub engine_listens: u32,
    /// `exe`, `data`, `bench` or `guess`.
    pub conf: Option<&'static str>,
    pub source: Option<&'static str>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_classified() {
        assert_eq!(
            MessageId::from_name("imsgpostspawninitialize"),
            MessageId::Engine("iMsgPostSpawnInitialize")
        );
        assert_eq!(
            MessageId::from_name("CP_325_START_NOW"),
            MessageId::Game("cp_325_start_now")
        );
        let custom = MessageId::from_name("my_mod_door_opened");
        assert_eq!(custom.kind(), "custom");
        assert_eq!(custom.hash(), string_id("my_mod_door_opened"));
        assert_eq!(
            MessageId::from_hash(0xDEAD_BEEF),
            MessageId::Unknown(0xDEAD_BEEF)
        );
        assert_eq!(MessageId::Unknown(0x12).to_string(), "0x00000012");
    }

    #[test]
    fn custom_names_are_checked() {
        let ok = MessageId::custom("my-mod", "my_mod_door_opened").unwrap();
        assert_eq!(
            ok,
            MessageId::Custom {
                owner: "my-mod".into(),
                name: "my_mod_door_opened".into(),
                hash: string_id("my_mod_door_opened"),
            }
        );
        assert_eq!(MessageId::custom("m", ""), Err(CustomMessageError::Empty));
        assert!(matches!(
            MessageId::custom("m", "дверь"),
            Err(CustomMessageError::NotAscii(_))
        ));
        assert!(matches!(
            MessageId::custom("m", "boundary_fade_out"),
            Err(CustomMessageError::Taken { kind: "game", .. })
        ));
        assert!(matches!(
            MessageId::custom("m", "iMsgSystemTick"),
            Err(CustomMessageError::Taken { kind: "engine", .. })
        ));
        assert_eq!(collision("boundary_fade_out"), None);
        assert_eq!(collision("my_mod_door_opened"), None);
    }

    #[test]
    fn the_catalogue_tells_senders_and_listeners() {
        let catalog = MessageCatalog::gf2();
        assert_eq!(catalog.len(), GAME_MESSAGES_COUNT);
        assert!(catalog.len() > 40_000);
        let fade = catalog.by_name("Boundary_Fade_Out").unwrap();
        assert!(
            fade.senders()
                .iter()
                .any(|s| s.class == "TriggerBase" && s.index == 4)
        );
        assert!(
            fade.listeners()
                .iter()
                .any(|s| s.class == "FadeScreenFx" && s.index == 0)
        );
        let info = MessageId::from_name("boundary_fade_out").info();
        assert_eq!(info.usage, Usage::SentAndHeard);
        assert_eq!(info.conf, Some("bench"));
        // The volume passes on the data of its own iMsgTrigger: who entered.
        assert_eq!(info.data, "forward");
        assert_eq!(info.data_conf, Some("exe"));
        // Heard by SimGroups, sent by a cut-scene: the catalogue only knows
        // that a sequence names it.
        let music = MessageId::from_name("mus_play_igc_01").info();
        assert_eq!(music.usage, Usage::HeardNeverSent);
        assert!(music.mentions.iter().any(|m| m.0 == "seq"));
        assert!(!music.sent_by.is_empty());
        let spawn = MessageId::from_name("iMsgNewCheckpointLoaded").info();
        assert_eq!(spawn.usage, Usage::HeardFromEngine);
        assert_eq!(spawn.conf, Some("exe"));
        let tick = MessageId::from_name("iMsgDoRender").info();
        assert_eq!(tick.usage, Usage::EngineOnly);
        assert!(!catalog.search("cp_325_", 10).is_empty());
        assert!(catalog.search("cp_325_", 3).len() <= 3);
    }

    #[test]
    fn engine_names_are_sorted_by_hash() {
        assert!(ENGINE_MESSAGES.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(ENGINE_MESSAGES.iter().all(|(h, n)| *h == string_id(n)));
        assert!(MESSAGE_NOTES.windows(2).all(|w| w[0].hash < w[1].hash));
    }
}

#[cfg(test)]
mod bus_tests {
    use super::*;

    /// What stage 78 read in the executable and the game files: senders in
    /// the code and the data word (the numbers of
    /// `docs/analysis/message-bus.md`).
    #[test]
    fn senders_and_data_words_are_known() {
        let engine = engine_messages();
        let info: Vec<MessageInfo> = engine
            .iter()
            .map(|(_, n)| MessageId::from_name(n).info())
            .collect();
        let whole = |data: &str| data != "unknown" && !data.split(" | ").any(|k| k == "unknown");
        let with_sender = info.iter().filter(|i| !i.engine_sites.is_empty()).count();
        let with_listener = info.iter().filter(|i| i.engine_listens > 0).count();
        let with_data = info.iter().filter(|i| i.data != "unknown").count();
        let with_whole_data = info.iter().filter(|i| whole(&i.data)).count();
        let catalog = MessageCatalog::gf2();
        let (mut game_data, mut game_whole, mut other) = (0, 0, 0);
        for m in catalog.iter() {
            let i = MessageId::from_hash(m.hash).info();
            if i.data != "unknown" {
                game_data += 1;
                if whole(&i.data) {
                    game_whole += 1;
                }
            }
            if fact(m.hash).is_some_and(|f| !f.other.is_empty()) {
                other += 1;
            }
        }
        eprintln!(
            "engine {}: sender {with_sender}, listener {with_listener}, data {with_data} (whole {with_whole_data}); catalogue {}: data {game_data} (whole {game_whole}), lua/cut-scene/score sender {other}",
            engine.len(),
            catalog.len()
        );
        assert_eq!(engine.len(), 645);
        assert_eq!(
            (with_sender, with_listener, with_data, with_whole_data),
            (320, 262, 310, 277)
        );
        assert_eq!((game_data, game_whole, other), (33_023, 32_890, 124));
        // A relay passes the data on, a gate sends itself.
        assert_eq!(
            data_of_class("MultiManager").map(|c| c.data),
            Some("forward")
        );
        assert_eq!(data_of_class("TriggerAuto").map(|c| c.data), Some("object"));
        assert_eq!(data_type("object").map(|t| t.table), Some(0x00E3_1E2C));
        // The frame message is sent by the render loop without data.
        let before = MessageId::from_name("iMsgBeforeRender").info();
        assert_eq!(before.data, "none");
        assert!(before.engine_sites.contains(&(0x0040_E230, 0)));
        // A cut-scene sends the music message; its data word is 0.
        let music = MessageId::from_name("mus_play_igc_01").info();
        assert!(music.data.split(" | ").any(|k| k == "none"));
        // Every class and key of the tables is real.
        for c in class_data() {
            assert!(data_type(c.data).is_some(), "{}", c.class);
        }
    }
}

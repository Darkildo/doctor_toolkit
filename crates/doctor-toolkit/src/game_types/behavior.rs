//! What SimGroup classes do, as read in the code of `godfather2.exe`:
//! `data/gf2/class_behavior.toml`.
//!
//! For a class (or a packet of base attributes shared by several classes)
//! the file gives the addresses of the constructor, the attribute reader
//! and the message handler, what the handler does on each message, what
//! the class does on a tick, and every attribute as the reader uses it:
//! name, kind of the value, the field of the object it goes to.
//!
//! The knowledge base puts it into the class notes; the attribute names
//! and message pins go into `data/gf2/logic_schema.toml`
//! (`research/class_behavior.py schema`), a test here holds the two files
//! together; the offline run (`logic_graph::run`) is checked against it in
//! its tests. Nothing is derived here: the file is hand-kept.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use super::class_hash;
use crate::logic_graph::schema::Confidence;
use crate::{Error, Result};

static GF2_BEHAVIOR: &str = include_str!("../../../../data/gf2/class_behavior.toml");

/// How much of the code of a class is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReadStatus {
    /// Constructor, attribute reader, message handler and update of the
    /// class itself are read to the end.
    Full,
    /// A part of the class code is not read.
    Partial,
}

/// How the code uses the value of an attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UseKind {
    Int,
    Float,
    /// Only zero / non-zero matters.
    Bool,
    /// Single bits are tested.
    Flags,
    /// One of a few numbers.
    Enum,
    /// `sdbm(lower(name))` of a name.
    Hash,
    String,
    Guid,
    Vec3,
    Matrix,
    /// A message the object listens to.
    MsgIn,
    /// A message the object sends.
    MsgOut,
}

impl UseKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Int => "int",
            Self::Float => "float",
            Self::Bool => "bool",
            Self::Flags => "flags",
            Self::Enum => "enum",
            Self::Hash => "hash",
            Self::String => "string",
            Self::Guid => "guid",
            Self::Vec3 => "vec3",
            Self::Matrix => "matrix",
            Self::MsgIn => "msg_in",
            Self::MsgOut => "msg_out",
        }
    }
}

/// The unit of a number, where the code shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    Seconds,
}

/// A message of a reaction: an engine message by name, or the message an
/// attribute names.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MsgRef {
    Engine(String),
    Attr {
        /// Name of the packet as the file writes it.
        packet_name: String,
        packet: u32,
        index: u16,
    },
}

impl MsgRef {
    fn parse(text: &str) -> Result<Self> {
        let Some(rest) = text.strip_prefix("attr:") else {
            return Ok(Self::Engine(text.to_owned()));
        };
        let bad = || Error::InvalidArgument(format!("class behaviour: bad message `{text}`"));
        let (packet, index) = rest.rsplit_once(':').ok_or_else(bad)?;
        Ok(Self::Attr {
            packet_name: packet.to_owned(),
            packet: class_hash(packet),
            index: index.parse().map_err(|_| bad())?,
        })
    }
}

/// What the message handler does on one message.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Reaction {
    pub msg: MsgRef,
    /// What happens (Russian).
    pub does: String,
    pub sends: Vec<MsgRef>,
    /// Address of the code, `0x…`.
    pub addr: String,
    pub conf: Confidence,
}

/// One attribute as the reader of the class uses it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AttrUse {
    pub packet_name: String,
    pub packet: u32,
    pub index: u16,
    pub name: String,
    pub kind: UseKind,
    pub unit: Option<Unit>,
    /// Where the reader stores the value in the object (`+0x…`).
    pub field: Option<String>,
    /// What the code does with it (Russian).
    pub does: String,
    pub addr: String,
    /// `Exe`: the meaning is seen in the code; `Guess`: the name is a guess.
    pub conf: Confidence,
}

/// The behaviour of one class.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ClassBehavior {
    pub name: String,
    pub hash: u32,
    pub status: ReadStatus,
    pub ctor: Option<String>,
    pub reader: Option<String>,
    pub handler: Option<String>,
    pub summary: String,
    pub update: String,
    pub fires: String,
    /// What is not read or not understood.
    pub open: Vec<String>,
    pub on: Vec<Reaction>,
    pub attrs: Vec<AttrUse>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileReaction {
    msg: String,
    does: String,
    #[serde(default)]
    sends: Vec<String>,
    addr: String,
    conf: Confidence,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileAttr {
    packet: String,
    i: u16,
    name: String,
    kind: UseKind,
    unit: Option<Unit>,
    field: Option<String>,
    does: String,
    addr: String,
    conf: Confidence,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileClass {
    status: ReadStatus,
    ctor: Option<String>,
    reader: Option<String>,
    handler: Option<String>,
    summary: String,
    update: String,
    fires: String,
    #[serde(default)]
    open: Vec<String>,
    #[serde(default)]
    on: Vec<FileReaction>,
    #[serde(default)]
    attr: Vec<FileAttr>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    class: toml::map::Map<String, toml::Value>,
}

/// All behaviours of a file, with look-ups.
#[derive(Debug, Default)]
pub struct Behaviors {
    classes: Vec<ClassBehavior>,
    by_hash: HashMap<u32, usize>,
    /// (packet, attribute) → (class, attribute of the class).
    attrs: HashMap<(u32, u16), (usize, usize)>,
}

fn address(what: &str, class: &str, text: &str) -> Result<()> {
    let ok = text.len() == 10
        && text.starts_with("0x")
        && text[2..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b));
    if ok {
        Ok(())
    } else {
        Err(Error::InvalidArgument(format!(
            "class behaviour: {class}: {what} `{text}` is not an address (0x + 8 upper-case hex digits)"
        )))
    }
}

impl Behaviors {
    /// Parses `class_behavior.toml`.
    pub fn parse(text: &str) -> Result<Self> {
        let bad = |e: toml::de::Error| Error::InvalidArgument(format!("class behaviour: {e}"));
        let file: File = toml::from_str(text).map_err(bad)?;
        let mut out = Self::default();
        for (name, value) in file.class {
            let c: FileClass = value
                .try_into()
                .map_err(|e| Error::InvalidArgument(format!("class behaviour: {name}: {e}")))?;
            for (what, a) in [
                ("ctor", &c.ctor),
                ("reader", &c.reader),
                ("handler", &c.handler),
            ] {
                if let Some(a) = a {
                    address(what, &name, a)?;
                }
            }
            let mut on = Vec::new();
            for r in c.on {
                address("reaction", &name, &r.addr)?;
                on.push(Reaction {
                    msg: MsgRef::parse(&r.msg)?,
                    does: r.does,
                    sends: r
                        .sends
                        .iter()
                        .map(|s| MsgRef::parse(s))
                        .collect::<Result<_>>()?,
                    addr: r.addr,
                    conf: r.conf,
                });
            }
            let class = out.classes.len();
            let mut attrs = Vec::new();
            for a in c.attr {
                address("attribute", &name, &a.addr)?;
                let packet = class_hash(&a.packet);
                if out
                    .attrs
                    .insert((packet, a.i), (class, attrs.len()))
                    .is_some()
                {
                    return Err(Error::InvalidArgument(format!(
                        "class behaviour: attribute {} of {} is described twice",
                        a.i, a.packet
                    )));
                }
                attrs.push(AttrUse {
                    packet_name: a.packet,
                    packet,
                    index: a.i,
                    name: a.name,
                    kind: a.kind,
                    unit: a.unit,
                    field: a.field,
                    does: a.does,
                    addr: a.addr,
                    conf: a.conf,
                });
            }
            let hash = class_hash(&name);
            if out.by_hash.insert(hash, class).is_some() {
                return Err(Error::InvalidArgument(format!(
                    "class behaviour: {name} is described twice"
                )));
            }
            out.classes.push(ClassBehavior {
                name,
                hash,
                status: c.status,
                ctor: c.ctor,
                reader: c.reader,
                handler: c.handler,
                summary: c.summary,
                update: c.update,
                fires: c.fires,
                open: c.open,
                on,
                attrs,
            });
        }
        Ok(out)
    }

    /// The behaviours of The Godfather II (PC 1.0.789.0).
    pub fn gf2() -> &'static Self {
        static ALL: OnceLock<Behaviors> = OnceLock::new();
        ALL.get_or_init(|| Self::parse(GF2_BEHAVIOR).expect("valid built-in class behaviour"))
    }

    /// Every described class.
    pub fn classes(&self) -> &[ClassBehavior] {
        &self.classes
    }

    /// The behaviour of the class (or base packet) with this hash.
    pub fn class(&self, hash: u32) -> Option<&ClassBehavior> {
        self.by_hash.get(&hash).map(|&i| &self.classes[i])
    }

    /// How the code uses attribute `index` of the packet of `packet`.
    pub fn attr(&self, packet: u32, index: u16) -> Option<&AttrUse> {
        self.attrs
            .get(&(packet, index))
            .map(|&(c, a)| &self.classes[c].attrs[a])
    }

    /// Every described attribute of a packet, by index.
    pub fn packet_attrs(&self, packet: u32) -> Vec<&AttrUse> {
        let mut out: Vec<&AttrUse> = self
            .classes
            .iter()
            .flat_map(|c| c.attrs.iter())
            .filter(|a| a.packet == packet)
            .collect();
        out.sort_by_key(|a| a.index);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::string_id;

    #[test]
    fn the_built_in_file_parses_and_names_what_it_points_at() {
        let all = Behaviors::gf2();
        assert!(all.classes().len() >= 40);
        for c in all.classes() {
            assert!(!c.summary.is_empty(), "{}", c.name);
            // A message of a reaction that an attribute names is a
            // described attribute holding a message.
            for r in &c.on {
                for m in std::iter::once(&r.msg).chain(&r.sends) {
                    if let MsgRef::Attr {
                        packet,
                        index,
                        packet_name,
                    } = m
                    {
                        let a = all.attr(*packet, *index).unwrap_or_else(|| {
                            panic!("{}: attr:{packet_name}:{index} is not described", c.name)
                        });
                        assert!(
                            matches!(a.kind, UseKind::MsgIn | UseKind::MsgOut),
                            "{}: attr:{packet_name}:{index} is not a message",
                            c.name
                        );
                    }
                }
            }
        }
        let mm = all.class(string_id("MultiManager")).unwrap();
        assert_eq!(mm.status, ReadStatus::Full);
        assert_eq!(mm.handler.as_deref(), Some("0x0047A1C0"));
        let flags = all.attr(string_id("TriggerBase"), 8).unwrap();
        assert_eq!(flags.kind, UseKind::Flags);
    }

    /// `research/class_behavior.py schema` was run: the logic schema has
    /// the names and the message pins of the file.
    #[test]
    fn the_logic_schema_has_the_names_and_pins_read_in_the_code() {
        use crate::logic_graph::schema::{LogicSchema, PinKind};
        let schema = LogicSchema::gf2();
        let common = [string_id("Base"), string_id("CSystemCommands")];
        for c in Behaviors::gf2().classes() {
            for a in c.attrs.iter().filter(|a| !common.contains(&a.packet)) {
                let pin = schema.attr(a.packet, a.index).unwrap_or_else(|| {
                    panic!("{} {} is not in the schema", a.packet_name, a.index)
                });
                assert_eq!(
                    pin.name.as_deref(),
                    Some(a.name.as_str()),
                    "{} {}",
                    a.packet_name,
                    a.index
                );
                assert_eq!(pin.name_conf, Some(a.conf), "{} {}", a.packet_name, a.index);
                match a.kind {
                    UseKind::MsgIn => {
                        assert_eq!(pin.pin, PinKind::In, "{} {}", a.packet_name, a.index)
                    }
                    UseKind::MsgOut => {
                        assert_eq!(pin.pin, PinKind::Out, "{} {}", a.packet_name, a.index)
                    }
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn a_bad_address_and_a_repeated_attribute_are_refused() {
        let head = "[class.A]\nstatus = \"full\"\nsummary = \"s\"\nupdate = \"u\"\nfires = \"f\"\n";
        assert!(Behaviors::parse(&format!("{head}handler = \"47a1c0\"\n")).is_err());
        let attr = "[[class.A.attr]]\npacket = \"A\"\ni = 0\nname = \"n\"\nkind = \"int\"\n\
                    does = \"d\"\naddr = \"0x0047A1C0\"\nconf = \"exe\"\n";
        assert!(Behaviors::parse(&format!("{head}{attr}")).is_ok());
        assert!(Behaviors::parse(&format!("{head}{attr}{attr}")).is_err());
    }
}

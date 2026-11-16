//! Typed game data of The Godfather II (PC 1.0.789.0): value lists,
//! meanings of SimGroup attributes and messages.
//!
//! The source is `data/gf2/`: `enums.toml` (families, cities, venues,
//! weapons, crew specialties, sound categories, emotions),
//! `attr_types.toml` (what the value of an attribute means),
//! `engine_messages.txt` (names in `godfather2.exe`), `game_messages.tsv`
//! (names in the SimGroups, with senders and listeners) and `messages.toml`
//! (facts about single messages). `build.rs` turns them into the statics and
//! the enums of this module; [`export`] writes the same data as Python stubs
//! and JSON Schema, the knowledge base (`kb::generate`) as notes.
//!
//! Nothing here is guessed: every list names its source and how sure it is
//! ([`Confidence`]); what is not known stays `None` or `"unknown"`.

pub mod export;
pub mod message;

use serde::Serialize;

use crate::logic_graph::schema::Confidence;
use crate::resource::simgroup::{Hash32, Value};

pub use message::{
    CustomMessageError, MessageCatalog, MessageId, MessageInfo, MessageNote, NoteConfidence,
    PinUse, Usage,
};

include!(concat!(env!("OUT_DIR"), "/gf2_types.rs"));

/// How an attribute stores a value of a list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EnumKind {
    /// `sdbm(lower(name))` as a 32-bit number.
    Hash,
    /// The name as text.
    Name,
    /// The number of the value.
    Int,
    /// A bit mask; a field may hold several values.
    Bits,
}

impl EnumKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Hash => "hash",
            Self::Name => "name",
            Self::Int => "int",
            Self::Bits => "bits",
        }
    }
}

/// One value of a list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct EnumValue {
    /// The name; `None` when only the number is known.
    pub name: Option<&'static str>,
    pub value: Option<i64>,
    /// `sdbm(lower(name))`, for lists stored as hashes.
    pub hash: Option<u32>,
    /// Where the value occurs, what goes with it.
    pub note: &'static str,
}

impl EnumValue {
    /// The name, or the number when there is no name.
    pub fn label(&self) -> String {
        match (self.name, self.value) {
            (Some(n), _) => n.to_owned(),
            (None, Some(v)) => v.to_string(),
            (None, None) => String::new(),
        }
    }
}

/// A value list of `data/gf2/enums.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct EnumDef {
    pub key: &'static str,
    pub title: &'static str,
    /// Title for the Russian documents.
    pub ru: &'static str,
    pub kind: EnumKind,
    /// Where the list comes from.
    pub source: &'static str,
    /// The source in Russian, for the notes.
    #[serde(skip)]
    pub source_ru: &'static str,
    pub conf: Confidence,
    pub values: &'static [EnumValue],
}

impl EnumDef {
    /// The value an attribute holds, when it is one of the list.
    pub fn find(&self, stored: &Value) -> Option<&'static EnumValue> {
        self.values.iter().find(|v| match (self.kind, stored) {
            (EnumKind::Hash, Value::Hash(h)) => v.hash == Some(h.0),
            // Hash attributes without a known kind are read as integers.
            (EnumKind::Hash, Value::Int(i)) => v.hash == Some(*i as u32),
            (EnumKind::Name, Value::String(s)) => v.name == Some(s.as_str()),
            (EnumKind::Int | EnumKind::Bits, Value::Int(i)) => v.value == Some(i64::from(*i)),
            _ => false,
        })
    }

    /// The value with this label: a name (letter case does not matter) or,
    /// for values without a name, the number.
    pub fn by_label(&self, label: &str) -> Option<&'static EnumValue> {
        let label = label.trim();
        self.values
            .iter()
            .find(|v| v.name.is_some_and(|n| n.eq_ignore_ascii_case(label)))
            .or_else(|| {
                let number: i64 = label.parse().ok()?;
                self.values.iter().find(|v| v.value == Some(number))
            })
    }

    /// What an attribute stores for a value of the list.
    pub fn stored(&self, value: &EnumValue) -> Option<Value> {
        match self.kind {
            EnumKind::Hash => value.hash.map(|h| Value::Hash(Hash32(h))),
            EnumKind::Name => value.name.map(|n| Value::String(n.to_owned())),
            EnumKind::Int | EnumKind::Bits => i32::try_from(value.value?).ok().map(Value::Int),
        }
    }

    /// The text a value editor takes for a value (`parse_value` of
    /// `simgroup_edit` reads it back).
    pub fn stored_text(&self, value: &EnumValue) -> Option<String> {
        Some(match self.stored(value)? {
            Value::String(s) => s,
            other => other.to_string(),
        })
    }

    /// The names set in a bit mask (lists of kind `bits`), in list order.
    pub fn bits(&self, mask: u32) -> Vec<&'static EnumValue> {
        self.values
            .iter()
            .filter(|v| v.value.is_some_and(|b| b != 0 && i64::from(mask) & b == b))
            .collect()
    }
}

/// Every value list, in the order of `enums.toml`.
pub fn enums() -> &'static [EnumDef] {
    ENUMS
}

/// A value list by key (`family`, `city`, …).
pub fn enum_def(key: &str) -> Option<&'static EnumDef> {
    ENUMS.iter().find(|e| e.key == key)
}

/// Hash of a class as the data files name it: `sdbm(lower(name))`, or the
/// hash itself for a class without a name (eight hexadecimal digits).
pub fn class_hash(name: &str) -> u32 {
    match u32::from_str_radix(name, 16) {
        Ok(hash) if name.len() == 8 => hash,
        _ => crate::hash::string_id(name),
    }
}

/// What the value of an attribute means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SemanticType {
    /// The name of a message the object listens to.
    MsgIn,
    /// The name of a message the object sends.
    MsgOut,
    /// The GUID of an object that has the packet of this class (a name, or
    /// eight hexadecimal digits of the hash: [`class_hash`]).
    Ref(&'static str),
    /// The id of a resource of this type.
    Resource(&'static str),
    /// A value of the list with this key ([`enum_def`]).
    Enum(&'static str),
    Seconds,
    Money,
    Bool,
}

impl SemanticType {
    /// The value list of an `Enum` type.
    pub fn enum_def(self) -> Option<&'static EnumDef> {
        match self {
            Self::Enum(key) => enum_def(key),
            _ => None,
        }
    }
}

impl std::fmt::Display for SemanticType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MsgIn => f.write_str("MsgIn"),
            Self::MsgOut => f.write_str("MsgOut"),
            Self::Ref(class) => write!(f, "Ref<{class}>"),
            Self::Resource(ty) => write!(f, "Res<{ty}>"),
            Self::Enum(key) => write!(f, "Enum<{key}>"),
            Self::Seconds => f.write_str("Seconds"),
            Self::Money => f.write_str("Money"),
            Self::Bool => f.write_str("Bool"),
        }
    }
}

impl Serialize for SemanticType {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

/// The meaning of one attribute with its evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AttrType {
    #[serde(rename = "type")]
    pub ty: SemanticType,
    pub conf: Confidence,
    /// Values of the game files that fit the type, and all values seen.
    pub seen: (u32, u32),
    /// Where a hand-read type comes from; empty for derived ones.
    pub source: &'static str,
}

/// The meaning of attribute `index` of the packet of `class`
/// (`data/gf2/attr_types.toml`). Message pins are not listed there: see
/// `LogicSchema::semantic`.
pub fn attr_type(class: u32, index: u16) -> Option<&'static AttrType> {
    ATTR_TYPES
        .binary_search_by_key(&(class, index), |r| (r.0, r.1))
        .ok()
        .map(|i| &ATTR_TYPES[i].2)
}

/// Every typed attribute: (packet class, index, meaning), sorted.
pub fn attr_types() -> &'static [(u32, u16, AttrType)] {
    ATTR_TYPES
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::string_id;

    #[test]
    fn lists_have_the_values_of_the_game() {
        assert_eq!(enum_def("family").unwrap().values.len(), 28);
        assert_eq!(Family::Corleone.hash(), 0x0637_B907);
        assert_eq!(Family::from_hash(0x529e_27ee), Some(Family::RosatoCarmine));
        assert_eq!(Family::from_name("ROSATO_TONY"), Some(Family::RosatoTony));
        assert_eq!(City::Florida.hash(), string_id("florida"));
        assert_eq!(CrewSpecialty::Medic.value(), Some(0x40));
        assert_eq!(Emotion::ALL.len(), 7);
        assert_eq!(Weapon::WeaponPistolLevel01.value(), Some(20));
        assert_eq!(
            SoundCategory::from_name("music"),
            Some(SoundCategory::Music)
        );
        for e in enums() {
            assert!(!e.values.is_empty(), "{}", e.key);
            assert!(!e.source.is_empty(), "{}", e.key);
        }
    }

    #[test]
    fn stored_values_round_trip() {
        let family = enum_def("family").unwrap();
        let tony = family.by_label("Rosato_Tony").unwrap();
        let stored = family.stored(tony).unwrap();
        assert_eq!(stored, Value::Hash(Hash32(0x5c85_59bf)));
        assert_eq!(family.find(&stored), Some(tony));
        assert_eq!(family.stored_text(tony).as_deref(), Some("5c8559bf"));
        let venue = enum_def("venue").unwrap();
        let v = venue.by_label("125").unwrap();
        assert_eq!(venue.stored(v), Some(Value::Int(125)));
        assert!(v.note.contains("florida"));
        let crew = enum_def("crew_specialty").unwrap();
        let names: Vec<_> = crew.bits(0x44).iter().map(|v| v.label()).collect();
        assert_eq!(names, ["demolition", "medic"]);
        let weapon = enum_def("weapon").unwrap();
        let cash = weapon.by_label("cash").unwrap();
        assert_eq!(weapon.stored(cash), Some(Value::String("Cash".into())));
    }

    #[test]
    fn attribute_types_are_sorted_and_found() {
        assert!(
            ATTR_TYPES
                .windows(2)
                .all(|w| (w[0].0, w[0].1) < (w[1].0, w[1].1))
        );
        let building = string_id("Building");
        assert_eq!(
            attr_type(building, 3).unwrap().ty,
            SemanticType::Enum("city")
        );
        assert_eq!(
            attr_type(building, 4).unwrap().ty,
            SemanticType::Enum("family")
        );
        let entity = string_id("Entity");
        let model = attr_type(entity, 1).unwrap();
        assert_eq!(model.ty, SemanticType::Resource("metamodel"));
        assert_eq!(model.ty.to_string(), "Res<metamodel>");
        assert_eq!(model.conf, Confidence::Data);
        let npc = string_id("CommonNPC");
        assert_eq!(attr_type(npc, 1).unwrap().ty.to_string(), "Ref<Gun>");
        assert_eq!(
            attr_type(string_id("MultiManager"), 4).unwrap().ty,
            SemanticType::Seconds
        );
        assert!(attr_type(building, 999).is_none());
        // Every list an attribute names exists.
        for (_, _, t) in ATTR_TYPES {
            if let SemanticType::Enum(key) = t.ty {
                assert!(enum_def(key).is_some(), "{key}");
            }
        }
    }
}

//! Delta resources: the difference between two versions of a resource as a
//! list of operations on addressed elements (`docs/formats/delta.md`).
//!
//! A mod that replaces a whole SimGroup carries a copy of everything in it,
//! and the copy goes stale as soon as the game or a mod below changes the
//! group. A delta carries only what the author did — "add these objects, set
//! these attributes" — and is applied to whatever the player has. Four
//! resource types have elements with stable addresses:
//!
//! | Type | Address | Operations |
//! |---|---|---|
//! | `simgroup` | object GUID | add and remove an object, set an attribute, the transform, the model |
//! | `lh2` | string hash and language column | set a text, add a string |
//! | `partsdb` | names of parts, variants and sets | add a part, add a variant, allow a variant |
//! | `wgi` | stream GUID | add and remove a cell of the world grid |
//!
//! # The exactness rule
//!
//! [`diff`](diff_resource) returns a delta only when applying it to the base
//! it was made from gives the other version **byte for byte**; otherwise it
//! returns `None` and the caller keeps the whole resource. The check is made
//! on the document as it is read back from its bytes, so what a mod stores
//! is what was checked.
//!
//! # A changed base
//!
//! [`apply`](apply_resource) on another base applies what still has a
//! target. An operation whose target is gone is skipped and listed in
//! [`Applied::skipped`]; the rest applies. Adding an element whose address
//! is taken (an object GUID, a string hash) is an error: addresses are
//! unique, and two deltas that add the same one cannot both be right.
//!
//! # The document
//!
//! A delta is a JSON document with `format` ([`FORMAT`]) and `kind`
//! ([`Kind`]). It is written without whitespace, the keys in the order of
//! the structures and the lists in the order of the operations, so the same
//! delta always has the same bytes.

pub mod lh2;
pub mod partsdb;
pub mod simgroup;
pub mod wgi;

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::hash::sdbm;
use crate::{Error, Result};

/// Version of the delta documents this library writes and reads.
pub const FORMAT: u32 = 1;

const TYPE_SIMGROUP: u32 = sdbm(b"simgroup");
const TYPE_LH2: u32 = sdbm(b"lh2");
const TYPE_PARTSDB: u32 = sdbm(b"partsdb");
const TYPE_WGI: u32 = crate::resource::wgi::TYPE_WGI;

/// The resource types that have deltas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    SimGroup,
    Lh2,
    PartsDb,
    Wgi,
}

impl Kind {
    /// The tag of the kind in a document, also the tag of the resource type.
    pub fn name(self) -> &'static str {
        match self {
            Self::SimGroup => "simgroup",
            Self::Lh2 => "lh2",
            Self::PartsDb => "partsdb",
            Self::Wgi => "wgi",
        }
    }

    /// Type id of the resources of this kind.
    pub fn type_id(self) -> u32 {
        match self {
            Self::SimGroup => TYPE_SIMGROUP,
            Self::Lh2 => TYPE_LH2,
            Self::PartsDb => TYPE_PARTSDB,
            Self::Wgi => TYPE_WGI,
        }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The kind of delta resources of type `type_id` have, if any.
pub fn kind_of(type_id: u32) -> Option<Kind> {
    [Kind::SimGroup, Kind::Lh2, Kind::PartsDb, Kind::Wgi]
        .into_iter()
        .find(|k| k.type_id() == type_id)
}

/// Something [`apply`](apply_resource) has to tell about one operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    /// Index of the operation in the document, from 0.
    pub op: usize,
    /// What the operation addresses, for people: `object <GUID>`, `string
    /// <hash>`.
    pub target: String,
    pub reason: String,
}

impl fmt::Display for Note {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "operation {} ({}): {}",
            self.op + 1,
            self.target,
            self.reason
        )
    }
}

/// What applying a delta gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    /// The new data of the resource; the base itself when nothing applied.
    pub data: Vec<u8>,
    /// Number of operations that were applied.
    pub applied: usize,
    /// Operations that were not applied because their target is not in the
    /// base, each with the reason.
    pub skipped: Vec<Note>,
    /// Operations that were applied over a value other than the one their
    /// author saw: something else changed the same element. The operation
    /// wins; the note names the element.
    pub conflicts: Vec<Note>,
}

/// The notes of one [`apply`](apply_resource) while it runs.
#[derive(Debug, Default)]
pub(crate) struct Report {
    applied: usize,
    skipped: Vec<Note>,
    conflicts: Vec<Note>,
}

impl Report {
    pub(crate) fn applied(&mut self) {
        self.applied += 1;
    }

    pub(crate) fn skip(&mut self, op: usize, target: impl Into<String>, reason: impl Into<String>) {
        self.skipped.push(Note {
            op,
            target: target.into(),
            reason: reason.into(),
        });
    }

    pub(crate) fn conflict(
        &mut self,
        op: usize,
        target: impl Into<String>,
        reason: impl Into<String>,
    ) {
        self.conflicts.push(Note {
            op,
            target: target.into(),
            reason: reason.into(),
        });
    }

    pub(crate) fn any_applied(&self) -> bool {
        self.applied > 0
    }

    pub(crate) fn finish(self, data: Vec<u8>) -> Applied {
        Applied {
            data,
            applied: self.applied,
            skipped: self.skipped,
            conflicts: self.conflicts,
        }
    }
}

/// A delta of any kind.
#[derive(Debug, Clone, PartialEq)]
pub enum Delta {
    SimGroup(simgroup::SimGroupDelta),
    Lh2(lh2::StringsDelta),
    PartsDb(partsdb::PartsDelta),
    Wgi(wgi::GridDelta),
}

/// The fields every delta document starts with.
#[derive(Deserialize)]
struct Head {
    format: u32,
    kind: Kind,
}

impl Delta {
    /// Reads a delta document. A document of a later [`FORMAT`] is refused:
    /// it may hold operations this version would leave out.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let bad = |e: serde_json::Error| Error::InvalidArgument(format!("delta document: {e}"));
        let head: Head = serde_json::from_slice(bytes).map_err(bad)?;
        if head.format == 0 || head.format > FORMAT {
            return Err(Error::UnsupportedVersion {
                what: "delta document",
                version: head.format,
            });
        }
        Ok(match head.kind {
            Kind::SimGroup => {
                let delta: simgroup::SimGroupDelta = serde_json::from_slice(bytes).map_err(bad)?;
                delta.validate()?;
                Self::SimGroup(delta)
            }
            Kind::Lh2 => Self::Lh2(serde_json::from_slice(bytes).map_err(bad)?),
            Kind::PartsDb => Self::PartsDb(serde_json::from_slice(bytes).map_err(bad)?),
            Kind::Wgi => Self::Wgi(serde_json::from_slice(bytes).map_err(bad)?),
        })
    }

    /// The document: JSON without whitespace, keys and lists in a fixed
    /// order. The same delta always gives the same bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let written = match self {
            Self::SimGroup(d) => serde_json::to_vec(d),
            Self::Lh2(d) => serde_json::to_vec(d),
            Self::PartsDb(d) => serde_json::to_vec(d),
            Self::Wgi(d) => serde_json::to_vec(d),
        };
        written.expect("a delta has no maps with other keys than strings and no failing values")
    }

    pub fn kind(&self) -> Kind {
        match self {
            Self::SimGroup(_) => Kind::SimGroup,
            Self::Lh2(_) => Kind::Lh2,
            Self::PartsDb(_) => Kind::PartsDb,
            Self::Wgi(_) => Kind::Wgi,
        }
    }

    /// Number of operations.
    pub fn len(&self) -> usize {
        match self {
            Self::SimGroup(d) => d.ops.len(),
            Self::Lh2(d) => d.ops.len(),
            Self::PartsDb(d) => d.ops.len(),
            Self::Wgi(d) => d.ops.len(),
        }
    }

    /// Whether the delta has no operations.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// What each operation addresses, in the words of the notes of
    /// [`Self::apply`] ([`Note::target`]): `object <GUID>, Entity.CMD_SetModel`,
    /// `string <hash>, language en`. Two deltas that name one target change
    /// the same element; this is how a conflict is traced to the other mod.
    pub fn targets(&self) -> Vec<String> {
        match self {
            Self::SimGroup(d) => d.ops.iter().map(simgroup::Op::target).collect(),
            Self::Lh2(d) => d.ops.iter().map(lh2::Op::target).collect(),
            Self::PartsDb(d) => d.ops.iter().map(partsdb::Op::target).collect(),
            Self::Wgi(d) => d.ops.iter().map(wgi::Op::target).collect(),
        }
    }

    /// Applies the delta to `base`: see [`apply_resource`].
    pub fn apply(&self, base: &[u8]) -> Result<Applied> {
        match self {
            Self::SimGroup(d) => simgroup::apply(d, base),
            Self::Lh2(d) => lh2::apply(d, base),
            Self::PartsDb(d) => partsdb::apply(d, base),
            Self::Wgi(d) => wgi::apply(d, base),
        }
    }
}

/// The delta that turns `base` into `mine`, both resources of type
/// `type_id`. `None` when the type has no deltas, or the change cannot be
/// written as operations that give `mine` back byte for byte: the caller
/// then keeps the whole resource.
pub fn diff_resource(type_id: u32, base: &[u8], mine: &[u8]) -> Result<Option<Delta>> {
    Ok(match kind_of(type_id) {
        None => None,
        Some(Kind::SimGroup) => simgroup::diff(base, mine)?.map(Delta::SimGroup),
        Some(Kind::Lh2) => lh2::diff(base, mine)?.map(Delta::Lh2),
        Some(Kind::PartsDb) => partsdb::diff(base, mine)?.map(Delta::PartsDb),
        Some(Kind::Wgi) => wgi::diff(base, mine)?.map(Delta::Wgi),
    })
}

/// What changed between `base` and `mine` as operations, for applying to
/// another version of the resource — a rebase (stage 111,
/// [`crate::workspace::Workspace::rebase`]). Unlike [`diff_resource`] a
/// SimGroup is compared by its contents alone: the operations are not
/// asked to give the bytes of `mine` back and carry no layout, which holds
/// on the base it was made for only and is of no use on another one
/// ([`simgroup::operations`]). The other kinds have one way to be written,
/// so theirs are the operations of [`diff_resource`]. `None` when the type
/// has no deltas or the change is not one of the operations.
pub fn operations(type_id: u32, base: &[u8], mine: &[u8]) -> Result<Option<Delta>> {
    Ok(match kind_of(type_id) {
        Some(Kind::SimGroup) => simgroup::operations(base, mine)?.map(|ops| {
            Delta::SimGroup(simgroup::SimGroupDelta {
                format: FORMAT,
                kind: Kind::SimGroup,
                ops,
                layout: None,
            })
        }),
        _ => diff_resource(type_id, base, mine)?,
    })
}

/// Applies the delta document `delta` to `base`, a resource of type
/// `type_id`. Operations whose target is not in `base` are skipped and
/// listed; adding an element whose address is taken is an error.
pub fn apply_resource(type_id: u32, delta: &[u8], base: &[u8]) -> Result<Applied> {
    let delta = Delta::parse(delta)?;
    if kind_of(type_id) != Some(delta.kind()) {
        return Err(Error::InvalidArgument(format!(
            "a {} delta does not apply to a resource of type {type_id:08x}",
            delta.kind()
        )));
    }
    delta.apply(base)
}

/// The bytes of the delta that turns `base` into `mine`, only when applying
/// them to `base` gives `mine` byte for byte. This is the check a mod build
/// makes before it stores a delta instead of the resource.
pub fn exact(type_id: u32, base: &[u8], mine: &[u8]) -> Result<Option<Vec<u8>>> {
    let Some(delta) = diff_resource(type_id, base, mine)? else {
        return Ok(None);
    };
    let bytes = delta.to_bytes();
    // `diff_resource` made this check already; it is repeated on the very
    // bytes the caller gets, so nothing between the two can break the rule.
    Ok(match apply_resource(type_id, &bytes, base) {
        Ok(applied) if applied.data == mine => Some(bytes),
        _ => None,
    })
}

/// Keeps `delta` when the document, read back from its bytes and applied to
/// `base`, gives `mine`. An operation `base` refuses means the change cannot
/// be expressed, which is `None` as well.
pub(crate) fn checked(delta: Delta, base: &[u8], mine: &[u8]) -> Option<Delta> {
    let read = Delta::parse(&delta.to_bytes()).ok()?;
    let applied = read.apply(base).ok()?;
    (applied.data == mine && applied.skipped.is_empty()).then_some(delta)
}

/// Checks the head of a document against the kind a module reads.
pub(crate) fn check_head(format: u32, kind: Kind, expected: Kind) -> Result<()> {
    if format == 0 || format > FORMAT {
        return Err(Error::UnsupportedVersion {
            what: "delta document",
            version: format,
        });
    }
    if kind != expected {
        return Err(Error::InvalidArgument(format!(
            "a {kind} delta, {expected} expected"
        )));
    }
    Ok(())
}

/// An `f32` that survives JSON bit for bit: a number when it is finite, its
/// bits as eight hexadecimal digits in a string otherwise (JSON has no NaN
/// and no infinity). Two values are equal when their bits are.
#[derive(Debug, Clone, Copy)]
pub struct Float(pub f32);

impl PartialEq for Float {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}

impl Eq for Float {}

impl Serialize for Float {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        if self.0.is_finite() {
            s.serialize_f32(self.0)
        } else {
            s.collect_str(&format_args!("{:08x}", self.0.to_bits()))
        }
    }
}

impl<'de> Deserialize<'de> for Float {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Visitor;
        impl serde::de::Visitor<'_> for Visitor {
            type Value = Float;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a number, or the bits of one as eight hexadecimal digits")
            }

            fn visit_f64<E: serde::de::Error>(self, v: f64) -> std::result::Result<Float, E> {
                Ok(Float(v as f32))
            }

            fn visit_i64<E: serde::de::Error>(self, v: i64) -> std::result::Result<Float, E> {
                Ok(Float(v as f32))
            }

            fn visit_u64<E: serde::de::Error>(self, v: u64) -> std::result::Result<Float, E> {
                Ok(Float(v as f32))
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> std::result::Result<Float, E> {
                if v.len() != 8 {
                    return Err(E::custom(format!(
                        "`{v}`: eight hexadecimal digits expected"
                    )));
                }
                u32::from_str_radix(v, 16)
                    .map(|bits| Float(f32::from_bits(bits)))
                    .map_err(|_| E::custom(format!("`{v}`: eight hexadecimal digits expected")))
            }
        }
        d.deserialize_any(Visitor)
    }
}

/// Bytes written as hexadecimal digits.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Hex(pub Vec<u8>);

impl Hex {
    pub(crate) fn is_zero(&self) -> bool {
        self.0.iter().all(|&b| b == 0)
    }

    pub(crate) fn text(bytes: &[u8]) -> String {
        use fmt::Write as _;
        let mut out = String::with_capacity(2 * bytes.len());
        for b in bytes {
            let _ = write!(out, "{b:02x}");
        }
        out
    }

    pub(crate) fn bytes(text: &str) -> Option<Vec<u8>> {
        if !text.len().is_multiple_of(2) || !text.is_ascii() {
            return None;
        }
        (0..text.len() / 2)
            .map(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16).ok())
            .collect()
    }
}

impl Serialize for Hex {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&Self::text(&self.0))
    }
}

impl<'de> Deserialize<'de> for Hex {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        Self::bytes(&text)
            .map(Hex)
            .ok_or_else(|| serde::de::Error::custom(format!("`{text}` is not hexadecimal bytes")))
    }
}

#[cfg(test)]
mod game_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_follow_the_type_ids() {
        for kind in [Kind::SimGroup, Kind::Lh2, Kind::PartsDb, Kind::Wgi] {
            assert_eq!(kind_of(kind.type_id()), Some(kind));
            assert_eq!(sdbm(kind.name().as_bytes()), kind.type_id());
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(json, format!("\"{}\"", kind.name()));
        }
        assert_eq!(kind_of(sdbm(b"mesh")), None);
        assert!(diff_resource(sdbm(b"mesh"), b"a", b"b").unwrap().is_none());
        assert!(exact(sdbm(b"mesh"), b"a", b"b").unwrap().is_none());
    }

    #[test]
    fn floats_keep_their_bits() {
        for bits in [
            0u32,
            0x8000_0000,
            1,
            0x3F80_0000,
            0x3DCC_CCCD,
            0x7F7F_FFFF,
            0x7F80_0000,
            0xFF80_0000,
            0x7FC0_1234,
            0x0080_0000,
            0xC2F7_3333,
        ] {
            let json = serde_json::to_string(&Float(f32::from_bits(bits))).unwrap();
            let back: Float = serde_json::from_str(&json).unwrap();
            assert_eq!(back.0.to_bits(), bits, "{json}");
        }
        assert_eq!(serde_json::from_str::<Float>("3").unwrap(), Float(3.0));
        assert_eq!(serde_json::from_str::<Float>("-3").unwrap(), Float(-3.0));
        assert!(serde_json::from_str::<Float>("\"7fc0\"").is_err());
        assert!(serde_json::from_str::<Float>("\"zzzzzzzz\"").is_err());
        assert_eq!(
            serde_json::from_str::<Hex>("\"00ff10\"").unwrap().0,
            [0, 255, 16]
        );
        assert!(serde_json::from_str::<Hex>("\"0\"").is_err());
        assert!(serde_json::from_str::<Hex>("\"zz\"").is_err());
    }

    #[test]
    fn documents_of_other_formats_and_kinds_are_refused() {
        assert!(matches!(
            Delta::parse(br#"{"format":2,"kind":"lh2","ops":[]}"#),
            Err(Error::UnsupportedVersion { version: 2, .. })
        ));
        assert!(Delta::parse(br#"{"format":0,"kind":"lh2","ops":[]}"#).is_err());
        assert!(Delta::parse(br#"{"format":1,"kind":"mesh","ops":[]}"#).is_err());
        assert!(Delta::parse(br#"{"format":1,"kind":"lh2","ops":[],"more":1}"#).is_err());
        assert!(Delta::parse(b"").is_err());
        let delta = Delta::parse(br#"{ "format": 1, "kind": "lh2", "ops": [] }"#).unwrap();
        assert!(delta.is_empty());
        assert_eq!(delta.to_bytes(), br#"{"format":1,"kind":"lh2","ops":[]}"#);
        // A delta of one kind does not apply to a resource of another.
        let error = apply_resource(TYPE_SIMGROUP, &delta.to_bytes(), b"").unwrap_err();
        assert!(error.to_string().contains("lh2 delta"), "{error}");
    }
}

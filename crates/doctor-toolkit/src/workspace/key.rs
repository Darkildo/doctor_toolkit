//! Stable identifiers of resources.
//!
//! A resource is identified by its stream and its `SHDR` header: the type id
//! and the four ids. Rewriting a stream keeps the headers (only `data_size`
//! changes) and the order of resources, so the key survives any number of
//! edits. On the whole Godfather II corpus (2300 streams, 212 544 resources)
//! the triple *(stream, type id, ids)* is unique; `tests/corpus.rs` checks it.
//!
//! The text form is `stream:type:ids`:
//!
//! ```text
//! gf2/frontend:tg4h:00000000.4802e0a7.9e3779b9.0badf00d
//! gf2/text/dd7d7050:lh2:9e583c54
//! gf2/globals_global:treemodel:12345678
//! mod/stream:0x0badf00d:00000001
//! ```
//!
//! - `stream` is the stream name as tables of contents spell it: lower case,
//!   forward slashes, no extension;
//! - `type` is the type tag (`tg4h`) or, for types without a known tag, the id
//!   in hexadecimal with a `0x` prefix. A tag always parses back to the same
//!   id, because type ids are SDBM hashes of their tags;
//! - `ids` is one hexadecimal number when all four ids are equal (the common
//!   case), otherwise four numbers joined by dots.

use std::fmt;
use std::str::FromStr;

use crate::container::ustream::{ResourceHeader, StreamLayout};
use crate::hash::sdbm;
use crate::resource::names::type_tags;
use crate::{Error, Result};

/// Identifies one resource of a game across rewrites of its stream.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResourceKey {
    /// Stream name: lower case, forward slashes, no extension (`gf2/frontend`).
    pub stream: String,
    pub type_id: u32,
    pub ids: [u32; 4],
}

impl ResourceKey {
    /// The key of the resource with `header` in stream `stream`. The stream
    /// name is normalised.
    pub fn new(stream: &str, header: &ResourceHeader) -> Self {
        Self {
            stream: normalize_stream(stream),
            type_id: header.type_id,
            ids: header.ids,
        }
    }

    /// Whether `header` belongs to this resource (stream aside).
    pub fn matches(&self, header: &ResourceHeader) -> bool {
        header.type_id == self.type_id && header.ids == self.ids
    }

    /// Index of the resource in a layout of its stream.
    pub fn find_in(&self, layout: &StreamLayout) -> Option<usize> {
        layout
            .resources
            .iter()
            .position(|r| self.matches(&r.header))
    }

    /// The type tag, or the id in hexadecimal when the tag is unknown.
    pub fn type_name(&self) -> String {
        type_tags().display(self.type_id)
    }

    /// The last id: the name hash for most types, shown in resource lists.
    pub fn name_id(&self) -> u32 {
        self.ids[3]
    }
}

/// Normalises a stream name or path: lower case, forward slashes, no `.str`.
pub fn normalize_stream(name: &str) -> String {
    let mut name = name.replace('\\', "/").to_ascii_lowercase();
    if let Some(stem) = name.strip_suffix(".str") {
        name.truncate(stem.len());
    }
    name.trim_start_matches('/').to_owned()
}

impl fmt::Display for ResourceKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:", self.stream)?;
        match type_tags().get(self.type_id) {
            Some(tag) if sdbm(tag.as_bytes()) == self.type_id => write!(f, "{tag}:")?,
            _ => write!(f, "0x{:08x}:", self.type_id)?,
        }
        let [a, b, c, d] = self.ids;
        if a == b && b == c && c == d {
            write!(f, "{a:08x}")
        } else {
            write!(f, "{a:08x}.{b:08x}.{c:08x}.{d:08x}")
        }
    }
}

impl FromStr for ResourceKey {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self> {
        let bad = |why: &str| Error::InvalidArgument(format!("bad resource key `{text}`: {why}"));
        let mut parts = text.rsplitn(3, ':');
        let (Some(ids), Some(kind), Some(stream)) = (parts.next(), parts.next(), parts.next())
        else {
            return Err(bad("expected `stream:type:ids`"));
        };
        if stream.is_empty() {
            return Err(bad("empty stream name"));
        }
        let hex = |s: &str| {
            if s.len() == 8 {
                u32::from_str_radix(s, 16).ok()
            } else {
                None
            }
        };
        let type_id = match kind.strip_prefix("0x") {
            Some(digits) => hex(digits).ok_or_else(|| bad("bad type id"))?,
            None if !kind.is_empty() => sdbm(kind.to_ascii_lowercase().as_bytes()),
            None => return Err(bad("empty type")),
        };
        let numbers: Vec<u32> = ids
            .split('.')
            .map(|s| hex(s).ok_or_else(|| bad("ids must be 8-digit hexadecimal numbers")))
            .collect::<Result<_>>()?;
        let ids = match numbers[..] {
            [id] => [id; 4],
            [a, b, c, d] => [a, b, c, d],
            _ => return Err(bad("expected one or four ids")),
        };
        Ok(Self {
            stream: normalize_stream(stream),
            type_id,
            ids,
        })
    }
}

impl serde::Serialize for ResourceKey {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for ResourceKey {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(tag: &[u8], ids: [u32; 4]) -> ResourceHeader {
        ResourceHeader {
            mem_policy: 0,
            alignment: 0x10,
            type_id: sdbm(tag),
            ids,
            data_size: 0,
        }
    }

    #[test]
    fn formats_and_parses() {
        let texture = ResourceKey::new(
            "gf2\\Frontend.STR",
            &header(b"tg4h", [0, sdbm(b"tg4h"), 7, 0x55]),
        );
        let text = texture.to_string();
        assert_eq!(
            text,
            "gf2/frontend:tg4h:00000000.4802e0a7.00000007.00000055"
        );
        assert_eq!(text.parse::<ResourceKey>().unwrap(), texture);

        let table = ResourceKey::new("gf2/text/dd7d7050", &header(b"lh2", [0x9e58_3c54; 4]));
        assert_eq!(table.to_string(), "gf2/text/dd7d7050:lh2:9e583c54");
        assert_eq!(table.to_string().parse::<ResourceKey>().unwrap(), table);

        let unknown = ResourceKey {
            stream: "a/b".into(),
            type_id: 0x0bad_f00d,
            ids: [1; 4],
        };
        assert_eq!(unknown.to_string(), "a/b:0x0badf00d:00000001");
        assert_eq!(unknown.to_string().parse::<ResourceKey>().unwrap(), unknown);
        // A tag that is not in the list still parses to its hash.
        let parsed: ResourceKey = "a:newtype:00000001".parse().unwrap();
        assert_eq!(parsed.type_id, sdbm(b"newtype"));
    }

    #[test]
    fn rejects_bad_keys() {
        for bad in [
            "",
            "stream",
            "s:tg4h",
            ":tg4h:00000001",
            "s::00000001",
            "s:tg4h:1",
            "s:tg4h:00000001.00000002",
            "s:0x12:00000001",
            "s:tg4h:0000000g",
        ] {
            assert!(bad.parse::<ResourceKey>().is_err(), "{bad}");
        }
    }

    #[test]
    fn serde_uses_the_text_form() {
        let key: ResourceKey = "gf2/frontend:lh2:00000009".parse().unwrap();
        let json = serde_json::to_string(&key).unwrap();
        assert_eq!(json, "\"gf2/frontend:lh2:00000009\"");
        assert_eq!(serde_json::from_str::<ResourceKey>(&json).unwrap(), key);
    }
}

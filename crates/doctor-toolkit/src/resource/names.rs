//! Readable names for hashed identifiers.
//!
//! Streams store resource types and memory policies as SDBM hashes of short
//! tags. The tags are kept as plain text lists under `data/` and hashed here.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::hash::sdbm;

static GF2_TYPE_TAGS: &str = include_str!("../../../../data/gf2/type_tags.txt");
static GF2_MEM_POLICIES: &str = include_str!("../../../../data/gf2/mem_policies.txt");
static GF2_BONE_NAMES: &str = include_str!("../../../../data/gf2/bone_names.txt");
static GF2_SIM_CLASSES: &str = include_str!("../../../../data/gf2/sim_classes.txt");
static GF2_EMITTER_NAMES: &str = include_str!("../../../../data/gf2/emitter_names.txt");

/// A lookup table from SDBM hash to the name it was computed from.
#[derive(Debug, Default)]
pub struct NameTable {
    by_hash: HashMap<u32, &'static str>,
}

impl NameTable {
    /// Builds a table from a text list: one name per line, `#` starts a comment.
    pub fn from_list(text: &'static str) -> Self {
        Self::from_list_with(text, |name| sdbm(name.as_bytes()))
    }

    /// Like [`NameTable::from_list`], hashing every name with `hash`.
    pub fn from_list_with(text: &'static str, hash: impl Fn(&str) -> u32) -> Self {
        let by_hash = text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(|name| (hash(name), name))
            .collect();
        Self { by_hash }
    }

    /// The name whose hash is `hash`.
    pub fn get(&self, hash: u32) -> Option<&'static str> {
        self.by_hash.get(&hash).copied()
    }

    /// The name, or the hash in hexadecimal when the name is not known.
    pub fn display(&self, hash: u32) -> String {
        match self.get(hash) {
            Some(name) => name.to_owned(),
            None => format!("{hash:08x}"),
        }
    }

    /// Resolves user input that is either a known name or a hexadecimal hash.
    pub fn resolve(&self, text: &str) -> Option<u32> {
        let hash = sdbm(text.to_ascii_lowercase().as_bytes());
        if self.by_hash.contains_key(&hash) {
            return Some(hash);
        }
        u32::from_str_radix(text.trim_start_matches("0x"), 16).ok()
    }

    /// Number of names in the table.
    pub fn len(&self) -> usize {
        self.by_hash.len()
    }

    /// Whether the table has no names.
    pub fn is_empty(&self) -> bool {
        self.by_hash.is_empty()
    }
}

/// Resource type tags (`tg4h`, `mesh`, `simgroup`, ...).
pub fn type_tags() -> &'static NameTable {
    static TABLE: OnceLock<NameTable> = OnceLock::new();
    TABLE.get_or_init(|| NameTable::from_list(GF2_TYPE_TAGS))
}

/// Memory policy names (`default`, `volatile`, `geometry`).
pub fn mem_policies() -> &'static NameTable {
    static TABLE: OnceLock<NameTable> = OnceLock::new();
    TABLE.get_or_init(|| NameTable::from_list(GF2_MEM_POLICIES))
}

/// Bone names of rigs (`m_pelvis`, `l_wrist`, ...), keyed by the SDBM hash of
/// the lower-case name.
pub fn bone_names() -> &'static NameTable {
    static TABLE: OnceLock<NameTable> = OnceLock::new();
    TABLE.get_or_init(|| {
        NameTable::from_list_with(GF2_BONE_NAMES, |name| {
            sdbm(name.to_ascii_lowercase().as_bytes())
        })
    })
}

/// SimGroup class names (`Entity`, `CSystemCommands`, ...), keyed by the SDBM
/// hash of the lower-case name.
pub fn sim_classes() -> &'static NameTable {
    static TABLE: OnceLock<NameTable> = OnceLock::new();
    TABLE.get_or_init(|| {
        NameTable::from_list_with(GF2_SIM_CLASSES, |name| {
            sdbm(name.to_ascii_lowercase().as_bytes())
        })
    })
}

/// Names of the sound emitters of `sbk` banks (`fanfare.emx`, ...), keyed by
/// the SDBM hash of the lower-case name (the emitter's template id).
pub fn emitter_names() -> &'static NameTable {
    static TABLE: OnceLock<NameTable> = OnceLock::new();
    TABLE.get_or_init(|| {
        NameTable::from_list_with(GF2_EMITTER_NAMES, |name| {
            sdbm(name.to_ascii_lowercase().as_bytes())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_tags_resolve_known_ids() {
        let tags = type_tags();
        assert_eq!(tags.get(0x4802_E0A7), Some("tg4h"));
        assert_eq!(tags.get(0x0196_1BED), Some("mesh"));
        assert_eq!(tags.get(0x5949_0267), Some("material"));
        assert_eq!(tags.get(0xDEAD_BEEF), None);
        assert_eq!(tags.display(0xDEAD_BEEF), "deadbeef");
        assert_eq!(tags.get(0xCDB9_D42B), Some("treemodel"));
        assert_eq!(tags.get(0xDF14_DA75), Some("covernodetable"));
        assert_eq!(tags.get(0x33A1_671E), Some("hka"));
        assert_eq!(tags.get(0x3229_3930), Some("emx"));
        assert_eq!(tags.len(), 45);
    }

    #[test]
    fn emitter_names_resolve_template_ids() {
        let names = emitter_names();
        assert_eq!(names.len(), 386);
        assert_eq!(names.get(sdbm(b"fanfare.emx")), Some("fanfare.emx"));
    }

    #[test]
    fn resolve_accepts_names_and_hex() {
        let tags = type_tags();
        assert_eq!(tags.resolve("mesh"), Some(0x0196_1BED));
        assert_eq!(tags.resolve("MESH"), Some(0x0196_1BED));
        assert_eq!(tags.resolve("12345678"), Some(0x1234_5678));
        assert_eq!(tags.resolve("0x12345678"), Some(0x1234_5678));
        assert_eq!(tags.resolve("not a tag"), None);
    }

    #[test]
    fn bone_names_resolve() {
        assert_eq!(bone_names().get(0x94BD_72F4), Some("m_root"));
        assert_eq!(bone_names().get(0x00E8_2F7D), Some("orboris_bl"));
        // Hashed in lower case: `l_elbowTwist` and `l_elbowtwist` are one bone.
        assert_eq!(
            bone_names().get(sdbm(b"l_elbowtwist")),
            Some("l_elbowtwist")
        );
    }

    #[test]
    fn sim_classes_hash_lower_case_names() {
        let classes = sim_classes();
        assert_eq!(classes.get(0x3852_3FC3), Some("Entity"));
        assert_eq!(classes.get(0xB390_B11A), Some("CSystemCommands"));
        assert_eq!(classes.get(0xCCB5_1828), None);
        assert!(classes.len() > 250);
    }

    #[test]
    fn mem_policies_resolve() {
        assert_eq!(mem_policies().get(0x5393_AC01), Some("default"));
        assert_eq!(mem_policies().get(0x6745_6B5C), Some("volatile"));
        assert_eq!(mem_policies().get(0x38B2_B8D2), Some("geometry"));
    }
}

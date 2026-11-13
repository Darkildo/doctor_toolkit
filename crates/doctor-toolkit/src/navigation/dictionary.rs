//! The name dictionary: known strings for 32-bit hashes.
//!
//! The engine hashes names with several functions: SDBM for type tags and
//! memory policies, SDBM of the lower-case name for SimGroup classes,
//! metamodel properties and string table entries (named by `$name` in
//! interface movies and in `data/gf2/string_names.txt`), CRC-32 of the
//! lower-case name for materials and shaders, a fold of the asset GUID for
//! streams in tables of contents. For texture names and script variables the
//! function is not known; their hashes are stored next to the name in the
//! data, and those pairs are taken as they are.
//!
//! A [`Dictionary`] starts with the name lists of `data/gf2/` and, for a game,
//! adds every name of the [`GameIndex`] and the stored pairs. Every name is
//! hashed with every function in [`HashFn::ALL`]. Two kinds of names are
//! derived from the game and kept only when they give a hash the game has:
//! string table entries named by the text of another entry (one table of
//! GF2 PC, `gf2/text/d70e6b6c`, holds the entry names as its text), and the
//! build paths whose hashes are the ids of `smb` and `sbk` resources.

use std::collections::{BTreeMap, HashSet};

use crate::hash::{crc32, fnv1a, sdbm};
use crate::resource::mesh::TYPE_MESH_VOLATILE;
use crate::resource::names::type_tags;
use crate::resource::texture::TYPE_TG4D;

use super::index::{GameIndex, HashKind, Target};

static GF2_TYPE_TAGS: &str = include_str!("../../../../data/gf2/type_tags.txt");
static GF2_MEM_POLICIES: &str = include_str!("../../../../data/gf2/mem_policies.txt");
static GF2_BONE_NAMES: &str = include_str!("../../../../data/gf2/bone_names.txt");
static GF2_SIM_CLASSES: &str = include_str!("../../../../data/gf2/sim_classes.txt");
static GF2_SCHEMA: &str = include_str!("../../../../data/gf2/simgroup_schema.txt");
static GF2_NAMES: &str = include_str!("../../../../data/gf2/names.txt");
static GF2_STRING_NAMES: &str = include_str!("../../../../data/gf2/string_names.txt");

/// Names longer than this are not hashed: paths and sentences, not identifiers.
const MAX_NAME_LEN: usize = 128;

/// A string hash function used by the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HashFn {
    Sdbm,
    SdbmLower,
    Crc32,
    Crc32Lower,
    Fnv1a,
    Fnv1aLower,
}

impl HashFn {
    pub const ALL: [Self; 6] = [
        Self::Sdbm,
        Self::SdbmLower,
        Self::Crc32,
        Self::Crc32Lower,
        Self::Fnv1a,
        Self::Fnv1aLower,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Sdbm => "sdbm",
            Self::SdbmLower => "sdbm(lower)",
            Self::Crc32 => "crc32",
            Self::Crc32Lower => "crc32(lower)",
            Self::Fnv1a => "fnv1a",
            Self::Fnv1aLower => "fnv1a(lower)",
        }
    }

    /// Whether the function lower-cases the text first.
    pub fn is_lower(self) -> bool {
        matches!(self, Self::SdbmLower | Self::Crc32Lower | Self::Fnv1aLower)
    }

    pub fn apply(self, text: &str) -> u32 {
        let lower = || text.to_lowercase();
        match self {
            Self::Sdbm => sdbm(text.as_bytes()),
            Self::SdbmLower => sdbm(lower().as_bytes()),
            Self::Crc32 => crc32(text.as_bytes()),
            Self::Crc32Lower => crc32(lower().as_bytes()),
            Self::Fnv1a => fnv1a(text.as_bytes()),
            Self::Fnv1aLower => fnv1a(lower().as_bytes()),
        }
    }
}

/// Every hash of `text`, one per function.
pub fn hash_all(text: &str) -> Vec<(HashFn, u32)> {
    HashFn::ALL.iter().map(|&f| (f, f.apply(text))).collect()
}

/// How a dictionary knows a name for a hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Origin {
    /// The hash of a known name under this function.
    Computed(HashFn),
    /// Stored next to the name in resources of this type; the function is
    /// not known.
    Stored(u32),
    /// A stream in a table of contents (hash of the stream's asset GUID).
    TocStream,
}

impl Origin {
    pub fn describe(self) -> String {
        match self {
            Self::Computed(f) => f.name().to_owned(),
            Self::Stored(type_id) => format!("stored in {}", type_tags().display(type_id)),
            Self::TocStream => "table of contents".to_owned(),
        }
    }
}

/// Known names of 32-bit hashes.
#[derive(Debug, Default)]
pub struct Dictionary {
    names: Vec<String>,
    numbers: std::collections::HashMap<String, u32>,
    /// `(hash, name number, origin)`, sorted once [`Dictionary::finish`] ran.
    entries: Vec<(u32, u32, Origin)>,
    sorted: bool,
}

impl Dictionary {
    /// The names of the lists in `data/gf2/`.
    pub fn builtin() -> Self {
        let mut dict = Self::default();
        let lists = [
            GF2_TYPE_TAGS,
            GF2_MEM_POLICIES,
            GF2_BONE_NAMES,
            GF2_SIM_CLASSES,
            GF2_NAMES,
            GF2_STRING_NAMES,
        ];
        for list in lists {
            for line in list.lines().map(str::trim) {
                if !line.is_empty() && !line.starts_with('#') {
                    dict.add_name(line);
                }
            }
        }
        for name in crate::resource::shading::shader_names() {
            dict.add_name(name);
        }
        // Attribute names are the fifth column of the schema.
        for line in GF2_SCHEMA.lines().filter(|l| !l.starts_with('#')) {
            if let Some(name) = line.split_whitespace().nth(4) {
                dict.add_name(name);
            }
        }
        dict.finish();
        dict
    }

    /// The builtin names plus every name, stored pair and table-of-contents
    /// name of a game index, string entries named by texts and the build
    /// paths of `smb` and `sbk` resources.
    pub fn for_game(index: &GameIndex) -> Self {
        let mut dict = Self::builtin();
        let mut entries = HashSet::new();
        for id in index.ids() {
            let record = index.record(id);
            for (_, name) in &record.names {
                dict.add_name(name);
            }
            for (hash, name) in &record.pairs {
                dict.add_pair(*hash, name, record.header.type_id);
            }
            entries.extend(
                record
                    .hashes
                    .iter()
                    .filter(|(kind, _)| *kind == HashKind::StringEntry)
                    .map(|&(_, hash)| hash),
            );
        }
        for (hash, name) in index.toc_names() {
            let n = dict.intern(name);
            dict.entries.push((*hash, n, Origin::TocStream));
        }
        dict.add_entry_names(index, &entries);
        dict.add_build_paths(index);
        dict.finish();
        dict
    }

    /// Adds `name` under `function` alone (its hash is known to occur).
    fn add_computed(&mut self, name: &str, function: HashFn) {
        let n = self.intern(name);
        self.entries
            .push((function.apply(name), n, Origin::Computed(function)));
        self.sorted = false;
    }

    /// Texts of string tables that are the name of an entry: SDBM of the
    /// lower-case text equals an entry hash of some table.
    fn add_entry_names(&mut self, index: &GameIndex, entries: &HashSet<u32>) {
        for id in index.ids() {
            for entry in &index.record(id).texts {
                let text = entry.text.as_str();
                if text.len() <= MAX_NAME_LEN && entries.contains(&HashFn::SdbmLower.apply(text)) {
                    self.add_computed(text, HashFn::SdbmLower);
                }
            }
        }
    }

    /// The id of an `smb` or `sbk` resource is the SDBM hash of its build
    /// path, `levels\<game>\<level>\streams\<stream>.<tag>`, where
    /// `<stream>` is the stream name without its first component
    /// (`gf2/cuba/local` gives `levels\gf2\cuba\streams\cuba\local.smb`).
    /// The level folder is the second component of a stream name, or one
    /// without `_global` (`gf2/globals_global` gives
    /// `levels\gf2\globals\streams\globals_global.smb`): every level folder
    /// is tried and the path that gives the id is kept.
    fn add_build_paths(&mut self, index: &GameIndex) {
        const TYPES: [(u32, &str); 2] = [(sdbm(b"smb"), "smb"), (sdbm(b"sbk"), "sbk")];
        let mut levels: Vec<&str> = (0..index.stream_count())
            .filter_map(|s| index.stream_name(s).split('/').nth(1))
            .flat_map(|level| [level, level.strip_suffix("_global").unwrap_or(level)])
            .collect();
        levels.sort_unstable();
        levels.dedup();
        for id in index.ids() {
            let header = &index.record(id).header;
            let Some(&(_, tag)) = TYPES.iter().find(|t| t.0 == header.type_id) else {
                continue;
            };
            let stream = index.stream_name(index.stream_of(id));
            let Some((game, rest)) = stream.split_once('/') else {
                continue;
            };
            let rest = rest.replace('/', "\\");
            for level in &levels {
                let path = format!("levels\\{game}\\{level}\\streams\\{rest}.{tag}");
                if sdbm(path.as_bytes()) == header.ids[3] {
                    self.add_computed(&path, HashFn::Sdbm);
                    break;
                }
            }
        }
    }

    /// Adds a name under every hash function.
    pub fn add_name(&mut self, name: &str) {
        if name.is_empty() || name.len() > MAX_NAME_LEN {
            return;
        }
        let n = self.intern(name);
        // For a lower-case name the lower-case variants give the same hashes.
        let lower = !name.chars().any(char::is_uppercase);
        for f in HashFn::ALL {
            if !(lower && f.is_lower()) {
                self.entries.push((f.apply(name), n, Origin::Computed(f)));
            }
        }
        self.sorted = false;
    }

    /// Adds a hash found next to `name` in a resource of `type_id`. When a
    /// known function gives the hash, that is recorded instead.
    pub fn add_pair(&mut self, hash: u32, name: &str, type_id: u32) {
        if name.is_empty() {
            return;
        }
        let origin = HashFn::ALL
            .into_iter()
            .find(|f| f.apply(name) == hash)
            .map_or(Origin::Stored(type_id), Origin::Computed);
        let n = self.intern(name);
        self.entries.push((hash, n, origin));
        self.sorted = false;
    }

    fn intern(&mut self, name: &str) -> u32 {
        if let Some(&n) = self.numbers.get(name) {
            return n;
        }
        let n = self.names.len() as u32;
        self.names.push(name.to_owned());
        self.numbers.insert(name.to_owned(), n);
        n
    }

    /// Sorts the entries for lookups; called by the constructors.
    pub fn finish(&mut self) {
        self.entries.sort_unstable();
        self.entries.dedup();
        self.sorted = true;
    }

    /// Names known for `hash`, with how each is known.
    pub fn lookup(&self, hash: u32) -> Vec<(&str, Origin)> {
        debug_assert!(self.sorted, "Dictionary::finish not called");
        let start = self.entries.partition_point(|e| e.0 < hash);
        self.entries[start..]
            .iter()
            .take_while(|e| e.0 == hash)
            .map(|&(_, n, origin)| (self.names[n as usize].as_str(), origin))
            .collect()
    }

    /// Whether any name is known for `hash`.
    pub fn knows(&self, hash: u32) -> bool {
        let start = self.entries.partition_point(|e| e.0 < hash);
        self.entries.get(start).is_some_and(|e| e.0 == hash)
    }

    /// Number of distinct names.
    pub fn name_count(&self) -> usize {
        self.names.len()
    }

    /// Number of `(hash, name, origin)` entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Stored pairs: `(hash, name, type)` whose function is not known.
    pub fn stored(&self) -> impl Iterator<Item = (u32, &str, u32)> {
        self.entries
            .iter()
            .filter_map(|&(hash, n, origin)| match origin {
                Origin::Stored(type_id) => Some((hash, self.names[n as usize].as_str(), type_id)),
                _ => None,
            })
    }
}

/// A hash seen in the game that no name is known for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownHash {
    /// What the hash identifies: a [`HashKind`] label or `<type> id`.
    pub kind: String,
    pub hash: u32,
    /// Number of resources it occurs in.
    pub count: usize,
}

/// Hashes found in the game without a known name, grouped by kind, most
/// frequent first: the [`HashKind`] hashes of the records (`bone`, `string
/// entry`, ...), type ids without a tag (`type tag`: of resources and of the
/// targets of references), the ids of resources without a
/// [`super::NameKind::Name`] (`<tag> id`; texture pixels and vertex data
/// share the id of their header) and the ids that references point at when
/// no resource has them (`<tag> reference`).
pub fn unknown_hashes(index: &GameIndex, dict: &Dictionary) -> Vec<UnknownHash> {
    let mut counts: BTreeMap<(String, u32), usize> = BTreeMap::new();
    let kind_labels: Vec<&str> = HashKind::ALL.iter().map(|k| k.label()).collect();
    let tags = type_tags();
    for id in index.ids() {
        let record = index.record(id);
        let mut seen = HashSet::new();
        for &(kind, hash) in &record.hashes {
            if seen.insert((kind_labels[kind as usize], hash)) && !dict.knows(hash) {
                *counts
                    .entry((kind_labels[kind as usize].to_owned(), hash))
                    .or_default() += 1;
            }
        }
        let type_id = record.header.type_id;
        if tags.get(type_id).is_none() && seen.insert(("type tag", type_id)) {
            *counts.entry(("type tag".to_owned(), type_id)).or_default() += 1;
        }
        let resource_id = record.header.ids[3];
        let companion = matches!(type_id, TYPE_TG4D | TYPE_MESH_VOLATILE);
        if record.name().is_none() && !companion && resource_id != 0 && !dict.knows(resource_id) {
            let kind = format!("{} id", tags.display(type_id));
            *counts.entry((kind, resource_id)).or_default() += 1;
        }
        let mut seen_targets = HashSet::new();
        for (reference, targets) in index.uses(id) {
            let Target::Resource { type_id, id } = reference.target else {
                continue;
            };
            if type_id != 0 && tags.get(type_id).is_none() && seen.insert(("type tag", type_id)) {
                *counts.entry(("type tag".to_owned(), type_id)).or_default() += 1;
            }
            if targets.is_empty()
                && id != 0
                && !dict.knows(id)
                && seen_targets.insert((type_id, id))
            {
                let tag = if type_id == 0 {
                    "any".to_owned()
                } else {
                    tags.display(type_id)
                };
                *counts.entry((format!("{tag} reference"), id)).or_default() += 1;
            }
        }
    }
    let mut list: Vec<UnknownHash> = counts
        .into_iter()
        .map(|((kind, hash), count)| UnknownHash { kind, hash, count })
        .collect();
    list.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then(b.count.cmp(&a.count))
            .then(a.hash.cmp(&b.hash))
    });
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_names_resolve_with_their_functions() {
        let dict = Dictionary::builtin();
        let tg4h = dict.lookup(0x4802_E0A7);
        assert!(
            tg4h.contains(&("tg4h", Origin::Computed(HashFn::Sdbm))),
            "{tg4h:?}"
        );
        assert_eq!(tg4h.len(), 1, "lower-case names are not repeated: {tg4h:?}");
        let entity = dict.lookup(0x3852_3FC3);
        assert!(entity.contains(&("Entity", Origin::Computed(HashFn::SdbmLower))));
        assert!(dict.knows(0x5393_AC01)); // default
        assert!(!dict.knows(0x1234_5678));
    }

    #[test]
    fn pairs_prefer_a_known_function() {
        let mut dict = Dictionary::default();
        dict.add_pair(crc32(b"metal_rust"), "Metal_Rust", 7);
        dict.add_pair(0xDEAD_BEEF, "buttons", 0x4802_E0A7);
        dict.finish();
        assert_eq!(
            dict.lookup(crc32(b"metal_rust")),
            [("Metal_Rust", Origin::Computed(HashFn::Crc32Lower))]
        );
        assert_eq!(
            dict.lookup(0xDEAD_BEEF),
            [("buttons", Origin::Stored(0x4802_E0A7))]
        );
        assert_eq!(dict.stored().count(), 1);
    }

    #[test]
    fn hash_all_covers_every_function() {
        let hashes = hash_all("tg4h");
        assert_eq!(hashes.len(), HashFn::ALL.len());
        assert!(hashes.contains(&(HashFn::Sdbm, 0x4802_E0A7)));
        assert!(hashes.contains(&(HashFn::Crc32, crc32(b"tg4h"))));
    }
}

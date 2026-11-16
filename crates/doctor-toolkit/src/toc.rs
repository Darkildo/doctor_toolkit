//! StreamTOC, the table of contents that lists the streams of a level (`.toc`).
//!
//! # Layout of version 10
//!
//! ```text
//! 0x00  u32  magic 0x9CBA7B28 (docs/formats/toc.md says 0x9CBAB728, which is wrong)
//! 0x04  u32  version, 10
//! 0x08  u16  number of file entries
//! 0x0A  u16  number of name records
//! 0x0C  u16  number of dependency words
//! 0x0E  u16  zero
//! 0x10  file entries, 20 bytes each
//!       name records, 8 bytes each: u32 hash, u32 string offset
//!       dependency words, u32 each: offset of a name record
//!       string pool: one NUL, then NUL-terminated names
//!       zero padding to a multiple of 0x800
//! ```
//!
//! A file entry:
//!
//! ```text
//! +0x00  u32      offset of the name record
//! +0x04  u32      sum of the decompressed sizes of all resources in the stream
//! +0x08  u8       number of shared streams (`globals_global`) among the dependencies
//! +0x09  u8       number of streams loaded together with this one ("force load")
//! +0x0A  u8       flags: FLAG_HAS_PARENT 1, FLAG_LOAD_DISABLED_DEFAULT 4, ...
//! +0x0B  u8       padding, zero
//!                 (the four bytes are kept as `flags`; the entry has
//!                 flags[0] + flags[1] + (flags[2] & 1) dependencies)
//! +0x0C  u32      DLC index, zero
//! +0x10  u32      offset of the first dependency word
//! ```
//!
//! File entries are sorted by the hash of their name record: the game finds
//! streams by binary search (`0x004A8180` in `godfather2.exe`).
//!
//! The hash is not computed from the name. It folds the stream's 128-bit asset
//! GUID, see [`guid_hash`]; streams without a GUID (`*_global`) have hash 0 and
//! are looked up by name instead.
//!
//! Version 8 (The Godfather 1) is not implemented: no sample files are available.

use std::ops::Range;

use binrw::{BinRead, BinWrite};

use crate::io::{self, Endian};
use crate::{Error, Result};

/// File magic.
pub const MAGIC: u32 = 0x9CBA_7B28;
/// The files are padded to a multiple of this size.
pub const PADDING: usize = 0x800;

const HEADER_SIZE: usize = 0x10;
const FILE_ENTRY_SIZE: usize = 20;
const NAME_RECORD_SIZE: usize = 8;

#[derive(Debug, Clone, Copy, BinRead, BinWrite)]
struct RawHeader {
    magic: u32,
    version: u32,
    num_files: u16,
    num_names: u16,
    num_dep_words: u16,
    unknown_0e: u16,
}

#[derive(Debug, Clone, Copy, BinRead, BinWrite)]
struct RawFileEntry {
    name_record: u32,
    data_size: u32,
    flags: [u8; 4],
    dlc_index: u32,
    deps: u32,
}

/// A name known to the table: a stream or a reference to an external stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TocName {
    /// [`guid_hash`] of the stream's asset GUID; 0 for `*_global` streams.
    pub hash: u32,
    /// Stream path relative to the game directory, with backslashes and
    /// without the `.str` extension, e.g. `gf2\frontend`. Not always valid UTF-8.
    pub name: Vec<u8>,
}

impl TocName {
    /// The name as text, with invalid UTF-8 replaced.
    pub fn name_lossy(&self) -> String {
        String::from_utf8_lossy(&self.name).into_owned()
    }
}

/// One stream listed in the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TocFile {
    /// Index into [`Toc::names`].
    pub name: usize,
    /// Sum of the decompressed sizes of all resources in the stream.
    pub data_size: u32,
    /// Four flag bytes; they determine the number of dependencies.
    pub flags: [u8; 4],
    /// Index of a downloadable-content path prefix according to the game code
    /// (`DlcIndex`); always zero in The Godfather II, where no entry is
    /// marked as downloadable content.
    pub dlc_index: u32,
    /// The dependencies as a range of [`Toc::dep_words`]. Entries share ranges.
    pub deps: Range<usize>,
    /// The raw dependency offset of an entry without dependencies.
    /// Kept so that writing reproduces the file byte for byte.
    raw_deps_offset: u32,
}

impl TocFile {
    /// Number of dependencies encoded in the flags.
    pub fn dep_count(flags: [u8; 4]) -> usize {
        flags[0] as usize + flags[1] as usize + (flags[2] & 1) as usize
    }
}

/// A parsed table of contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toc {
    pub version: u32,
    pub endian: Endian,
    /// Header field at 0x0E, zero in The Godfather II.
    pub unknown_0e: u16,
    pub files: Vec<TocFile>,
    pub names: Vec<TocName>,
    /// Dependency words; each is an index into [`Toc::names`].
    pub dep_words: Vec<usize>,
}

impl Toc {
    /// Parses a table of contents.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let magic_bytes: [u8; 4] = io::slice(data, 0, 4)?
            .try_into()
            .expect("slice of length 4");
        let endian = if u32::from_le_bytes(magic_bytes) == MAGIC {
            Endian::Little
        } else if u32::from_be_bytes(magic_bytes) == MAGIC {
            Endian::Big
        } else {
            return Err(Error::BadMagic {
                what: "StreamTOC",
                found: u32::from_le_bytes(magic_bytes),
            });
        };
        let header: RawHeader = io::read_record(data, 0, endian)?;
        if header.version != 10 {
            return Err(Error::UnsupportedVersion {
                what: "StreamTOC",
                version: header.version,
            });
        }
        let num_files = header.num_files as usize;
        let num_names = header.num_names as usize;
        let num_dep_words = header.num_dep_words as usize;

        let names_start = HEADER_SIZE + num_files * FILE_ENTRY_SIZE;
        let deps_start = names_start + num_names * NAME_RECORD_SIZE;
        let pool_start = deps_start + num_dep_words * 4;

        // Maps the file offset of a name record to its index.
        let name_index = |offset: u32, at: usize| -> Result<usize> {
            let offset = offset as usize;
            if offset < names_start
                || offset >= deps_start
                || !(offset - names_start).is_multiple_of(NAME_RECORD_SIZE)
            {
                return Err(Error::malformed(
                    "StreamTOC",
                    at,
                    format!("{offset:#x} is not the offset of a name record"),
                ));
            }
            Ok((offset - names_start) / NAME_RECORD_SIZE)
        };

        let mut names = Vec::with_capacity(num_names);
        for i in 0..num_names {
            let at = names_start + i * NAME_RECORD_SIZE;
            let hash = io::read_u32(data, at, endian)?;
            let string_offset = io::read_u32(data, at + 4, endian)? as usize;
            if string_offset < pool_start {
                return Err(Error::malformed(
                    "StreamTOC",
                    at,
                    "name string lies outside the string pool",
                ));
            }
            names.push(TocName {
                hash,
                name: io::cstr(data, string_offset)?.to_vec(),
            });
        }

        let mut dep_words = Vec::with_capacity(num_dep_words);
        for i in 0..num_dep_words {
            let at = deps_start + i * 4;
            dep_words.push(name_index(io::read_u32(data, at, endian)?, at)?);
        }

        let mut files = Vec::with_capacity(num_files);
        for i in 0..num_files {
            let at = HEADER_SIZE + i * FILE_ENTRY_SIZE;
            let raw: RawFileEntry = io::read_record(data, at, endian)?;
            let count = TocFile::dep_count(raw.flags);
            let deps = if count == 0 {
                0..0
            } else {
                let offset = raw.deps as usize;
                if offset < deps_start || !(offset - deps_start).is_multiple_of(4) {
                    return Err(Error::malformed(
                        "StreamTOC",
                        at,
                        format!("{offset:#x} is not the offset of a dependency word"),
                    ));
                }
                let start = (offset - deps_start) / 4;
                if start + count > num_dep_words {
                    return Err(Error::malformed(
                        "StreamTOC",
                        at,
                        "dependency list runs past the dependency table",
                    ));
                }
                start..start + count
            };
            files.push(TocFile {
                name: name_index(raw.name_record, at)?,
                data_size: raw.data_size,
                flags: raw.flags,
                dlc_index: raw.dlc_index,
                deps,
                raw_deps_offset: if count == 0 { raw.deps } else { 0 },
            });
        }

        Ok(Self {
            version: header.version,
            endian,
            unknown_0e: header.unknown_0e,
            files,
            names,
            dep_words,
        })
    }

    /// Serialises the table. Parsing a game file and writing it back yields
    /// the same bytes.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let too_many =
            |what: &str| Error::InvalidArgument(format!("too many {what} for a StreamTOC"));
        let num_files = u16::try_from(self.files.len()).map_err(|_| too_many("files"))?;
        let num_names = u16::try_from(self.names.len()).map_err(|_| too_many("names"))?;
        let num_dep_words =
            u16::try_from(self.dep_words.len()).map_err(|_| too_many("dependencies"))?;

        let names_start = HEADER_SIZE + self.files.len() * FILE_ENTRY_SIZE;
        let deps_start = names_start + self.names.len() * NAME_RECORD_SIZE;
        let pool_start = deps_start + self.dep_words.len() * 4;
        let name_offset = |index: usize| -> Result<u32> {
            if index >= self.names.len() {
                return Err(Error::InvalidArgument(format!(
                    "name index {index} out of range"
                )));
            }
            Ok((names_start + index * NAME_RECORD_SIZE) as u32)
        };

        let mut out = Vec::new();
        io::put_record(
            &mut out,
            &RawHeader {
                magic: MAGIC,
                version: self.version,
                num_files,
                num_names,
                num_dep_words,
                unknown_0e: self.unknown_0e,
            },
            self.endian,
        );
        for file in &self.files {
            let deps = if file.deps.is_empty() {
                file.raw_deps_offset
            } else {
                if file.deps.end > self.dep_words.len() {
                    return Err(Error::InvalidArgument(
                        "dependency range out of bounds".into(),
                    ));
                }
                (deps_start + file.deps.start * 4) as u32
            };
            io::put_record(
                &mut out,
                &RawFileEntry {
                    name_record: name_offset(file.name)?,
                    data_size: file.data_size,
                    flags: file.flags,
                    dlc_index: file.dlc_index,
                    deps,
                },
                self.endian,
            );
        }
        // The pool starts with an empty string; names follow in table order.
        let mut string_offset = pool_start + 1;
        for name in &self.names {
            io::put_u32(&mut out, name.hash, self.endian);
            io::put_u32(&mut out, string_offset as u32, self.endian);
            string_offset += name.name.len() + 1;
        }
        for &word in &self.dep_words {
            io::put_u32(&mut out, name_offset(word)?, self.endian);
        }
        debug_assert_eq!(out.len(), pool_start);
        out.push(0);
        for name in &self.names {
            if name.name.contains(&0) {
                return Err(Error::InvalidArgument("name contains a NUL byte".into()));
            }
            out.extend_from_slice(&name.name);
            out.push(0);
        }
        out.resize(io::align_up(out.len(), PADDING), 0);
        Ok(out)
    }

    /// Finds the file entry whose name equals `name`, ignoring ASCII case and
    /// the direction of path separators.
    pub fn find_file(&self, name: &str) -> Option<usize> {
        let wanted = normalize_name(name.as_bytes());
        self.files
            .iter()
            .position(|f| normalize_name(&self.names[f.name].name) == wanted)
    }

    /// The name of file entry `index`.
    pub fn file_name(&self, index: usize) -> &TocName {
        &self.names[self.files[index].name]
    }

    /// The names file entry `index` depends on.
    pub fn file_deps(&self, index: usize) -> impl Iterator<Item = &TocName> {
        self.dep_words[self.files[index].deps.clone()]
            .iter()
            .map(|&i| &self.names[i])
    }
}

/// Folds a 128-bit asset GUID into the 32-bit hash stored in name records.
///
/// The GUID is read as four little-endian words `a, b, c, d`; the hash is
/// `((a * 33 + b) * 33 + c) * 33 + d` modulo 2³². In `godfather2.exe` it is
/// the function `0x00401F40` (called by the stream manager) and the start of
/// the record lookup `0x004A82B0`.
pub fn guid_hash(guid: &[u8; 16]) -> u32 {
    guid.chunks_exact(4)
        .map(|w| u32::from_le_bytes(w.try_into().expect("chunk of four bytes")))
        .fold(0u32, |hash, word| hash.wrapping_mul(33).wrapping_add(word))
}

/// Bit of `flags[2]`: the first dependency is the parent of the stream. Set
/// in every entry except those of `*_global` streams.
pub const FLAG_HAS_PARENT: u8 = 0x01;
/// Bit of `flags[2]`: not loaded on its own, only by the stream whose
/// force-load list (`flags[1]`) names it ("parented stream").
pub const FLAG_LOAD_DISABLED_DEFAULT: u8 = 0x04;

/// Flags of a stream loaded on request, by its hash: one dependency, its parent.
pub const FLAGS_REQUESTED: [u8; 4] = [0, 0, 1, 0];
/// Flags of a stream loaded together with another one ("parented stream"):
/// one dependency, the stream that loads it. The game never loads such a
/// stream on its own.
pub const FLAGS_PARENTED: [u8; 4] = [0, 0, 5, 0];
/// Flags of a child stream that is not disabled by default: listed in the
/// force-load list of its parent, it is opened with every load of the parent
/// (`0x00403C40` asks `0x00404590`, which refuses only entries with
/// [`FLAG_LOAD_DISABLED_DEFAULT`]). The game files have no such entry.
pub const FLAGS_CHILD: [u8; 4] = [0, 0, 1, 0];

impl TocFile {
    /// A new file entry. `deps` is a range of [`Toc::dep_words`].
    pub fn new(name: usize, data_size: u32, flags: [u8; 4], deps: Range<usize>) -> Self {
        Self {
            name,
            data_size,
            flags,
            dlc_index: 0,
            deps,
            raw_deps_offset: 0,
        }
    }

    /// Number of leading dependencies the game loads before this stream
    /// (`flags[0] + (flags[2] & 1)`); the remaining `flags[1]` are loaded
    /// together with it. Found in `godfather2.exe` at `0x00403C40`.
    pub fn prerequisite_count(&self) -> usize {
        self.flags[0] as usize + (self.flags[2] & 1) as usize
    }
}

impl Toc {
    /// Finds the name record of `name` (case and separators ignored).
    pub fn find_name(&self, name: &str) -> Option<usize> {
        let wanted = normalize_name(name.as_bytes());
        self.names
            .iter()
            .position(|n| normalize_name(&n.name) == wanted)
    }

    /// Returns the name record of `name`, adding one when there is none.
    /// `name` is stored with backslashes, as the game spells it.
    pub fn add_name(&mut self, hash: u32, name: &str) -> usize {
        if let Some(index) = self.find_name(name) {
            return index;
        }
        self.names.push(TocName {
            hash,
            name: name.replace('/', "\\").into_bytes(),
        });
        self.names.len() - 1
    }

    /// Adds a file entry for a stream and keeps the entries sorted by hash.
    /// `deps` are name record indices; the dependency list is appended to
    /// the dependency table. Returns the index of the new entry.
    pub fn add_file(
        &mut self,
        name: usize,
        data_size: u32,
        flags: [u8; 4],
        deps: &[usize],
    ) -> Result<usize> {
        if name >= self.names.len() || deps.iter().any(|&d| d >= self.names.len()) {
            return Err(Error::InvalidArgument("name index out of range".into()));
        }
        if self.files.iter().any(|f| f.name == name) {
            return Err(Error::InvalidArgument(format!(
                "{} already has a file entry",
                self.names[name].name_lossy()
            )));
        }
        let count = TocFile::dep_count(flags);
        if count != deps.len() {
            return Err(Error::InvalidArgument(format!(
                "flags {flags:?} declare {count} dependencies, {} given",
                deps.len()
            )));
        }
        let start = self.dep_words.len();
        self.dep_words.extend_from_slice(deps);
        self.files.push(TocFile::new(
            name,
            data_size,
            flags,
            start..start + deps.len(),
        ));
        self.sort_files_by_hash();
        Ok(self
            .files
            .iter()
            .position(|f| f.name == name)
            .expect("just added"))
    }

    /// Makes file entry `file` load the stream named by `child` together with
    /// itself (appends it to the entry's `flags[1]` dependencies).
    pub fn attach_child(&mut self, file: usize, child: usize) -> Result<()> {
        let entry = self
            .files
            .get(file)
            .ok_or_else(|| Error::InvalidArgument(format!("file entry {file} out of range")))?;
        if child >= self.names.len() {
            return Err(Error::InvalidArgument("name index out of range".into()));
        }
        let mut deps: Vec<usize> = self.dep_words[entry.deps.clone()].to_vec();
        if deps[entry.prerequisite_count()..].contains(&child) {
            return Ok(());
        }
        let mut flags = entry.flags;
        flags[1] = flags[1]
            .checked_add(1)
            .ok_or_else(|| Error::InvalidArgument("too many child streams".into()))?;
        deps.push(child);
        let start = self.dep_words.len();
        self.dep_words.extend_from_slice(&deps);
        let entry = &mut self.files[file];
        entry.flags = flags;
        entry.deps = start..start + deps.len();
        Ok(())
    }

    /// Whether the file entries are sorted by name hash, as the game requires.
    pub fn is_sorted_by_hash(&self) -> bool {
        self.files
            .windows(2)
            .all(|w| self.names[w[0].name].hash <= self.names[w[1].name].hash)
    }

    /// Sorts the file entries by name hash. Call after adding entries.
    pub fn sort_files_by_hash(&mut self) {
        let names = &self.names;
        self.files.sort_by_key(|f| names[f.name].hash);
    }
}

/// Lower-cases a stream name and converts backslashes to forward slashes.
pub fn normalize_name(name: &[u8]) -> Vec<u8> {
    name.iter()
        .map(|&b| {
            if b == b'\\' {
                b'/'
            } else {
                b.to_ascii_lowercase()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(hash: u32, text: &str) -> TocName {
        TocName {
            hash,
            name: text.as_bytes().to_vec(),
        }
    }

    fn file(name: usize, data_size: u32, flags: [u8; 4], deps: Range<usize>) -> TocFile {
        TocFile {
            name,
            data_size,
            flags,
            dlc_index: 0,
            deps,
            raw_deps_offset: 0,
        }
    }

    fn sample() -> Toc {
        Toc {
            version: 10,
            endian: Endian::Little,
            unknown_0e: 0,
            files: vec![
                file(0, 0, [0, 0, 0, 0], 0..0),
                file(1, 0xB6_5A85, [0, 0, 1, 0], 0..1),
                file(2, 0x5_9901, [0, 0, 1, 0], 1..2),
                file(3, 0x4_D574, [0, 0, 1, 0], 1..2),
            ],
            names: vec![
                name(0, "gf2\\frontend_global"),
                name(0x5C85_8FDE, "gf2\\frontend"),
                name(0x856C_146C, "gf2\\text\\856c146c"),
                name(0xAD8B_F024, "gf2\\text\\ad8bf024"),
            ],
            dep_words: vec![0, 1],
        }
    }

    #[test]
    fn roundtrip() {
        let toc = sample();
        let bytes = toc.to_bytes().unwrap();
        assert_eq!(bytes.len(), PADDING);
        assert_eq!(&bytes[..4], &MAGIC.to_le_bytes());
        let parsed = Toc::parse(&bytes).unwrap();
        assert_eq!(parsed, toc);
        assert_eq!(parsed.to_bytes().unwrap(), bytes);
    }

    #[test]
    fn layout_matches_the_game_files() {
        // Offsets below are the ones found in gf2/frontend.toc with four entries.
        let bytes = sample().to_bytes().unwrap();
        let names_start = 0x10 + 4 * 20;
        let deps_start = names_start + 4 * 8;
        let pool_start = deps_start + 2 * 4;
        // Second entry points at the second name record and the first dependency word.
        assert_eq!(
            io::read_u32(&bytes, 0x10 + 20, Endian::Little).unwrap() as usize,
            names_start + 8
        );
        assert_eq!(
            io::read_u32(&bytes, 0x10 + 20 + 16, Endian::Little).unwrap() as usize,
            deps_start
        );
        // First dependency word points at the first name record.
        assert_eq!(
            io::read_u32(&bytes, deps_start, Endian::Little).unwrap() as usize,
            names_start
        );
        // The pool starts with a NUL, then the first name.
        assert_eq!(bytes[pool_start], 0);
        assert_eq!(
            io::cstr(&bytes, pool_start + 1).unwrap(),
            b"gf2\\frontend_global"
        );
    }

    #[test]
    fn lookup_and_dependencies() {
        let toc = sample();
        assert_eq!(toc.find_file("GF2/Frontend"), Some(1));
        assert_eq!(toc.find_file("gf2\\text\\ad8bf024"), Some(3));
        assert_eq!(toc.find_file("missing"), None);
        assert_eq!(toc.file_name(2).name_lossy(), "gf2\\text\\856c146c");
        let deps: Vec<String> = toc.file_deps(2).map(TocName::name_lossy).collect();
        assert_eq!(deps, ["gf2\\frontend"]);
        assert_eq!(toc.file_deps(0).count(), 0);
    }

    #[test]
    fn guid_hash_matches_the_game() {
        // GUID of the shared "globals" stream; bytes 8..16 spell its creator's login.
        let guid: [u8; 16] = [
            0xa3, 0x01, 0xc9, 0x53, 0x74, 0x50, 0x55, 0x8d, b'G', b'R', b'A', b'N', b'D', b'Q',
            b'U', b'I',
        ];
        assert_eq!(guid_hash(&guid), 0x43BA_ECA2);
        assert_eq!(guid_hash(&[0; 16]), 0);
    }

    #[test]
    fn sorting_by_hash() {
        let mut toc = sample();
        assert!(toc.is_sorted_by_hash());
        toc.files.swap(1, 3);
        assert!(!toc.is_sorted_by_hash());
        toc.sort_files_by_hash();
        assert!(toc.is_sorted_by_hash());
        assert_eq!(toc, sample());
    }

    #[test]
    fn big_endian_roundtrip() {
        let toc = Toc {
            endian: Endian::Big,
            ..sample()
        };
        let bytes = toc.to_bytes().unwrap();
        assert_eq!(&bytes[..4], &MAGIC.to_be_bytes());
        assert_eq!(Toc::parse(&bytes).unwrap(), toc);
    }

    #[test]
    fn rejects_malformed_input() {
        assert!(Toc::parse(b"").is_err());
        assert!(matches!(
            Toc::parse(&[0u8; 64]),
            Err(Error::BadMagic { .. })
        ));

        let good = sample().to_bytes().unwrap();

        let mut bad = good.clone();
        bad[4] = 8;
        assert!(matches!(
            Toc::parse(&bad),
            Err(Error::UnsupportedVersion { version: 8, .. })
        ));

        // Name record offset pointing into the header.
        let mut bad = good.clone();
        bad[0x10..0x14].copy_from_slice(&4u32.to_le_bytes());
        assert!(Toc::parse(&bad).is_err());

        // Dependency offset before the dependency table.
        let mut bad = good.clone();
        bad[0x10 + 20 + 16..0x10 + 20 + 20].copy_from_slice(&0x10u32.to_le_bytes());
        assert!(Toc::parse(&bad).is_err());

        // Counts larger than the file.
        let mut bad = good.clone();
        bad[8..10].copy_from_slice(&u16::MAX.to_le_bytes());
        assert!(Toc::parse(&bad).is_err());

        // Truncation must never panic.
        for len in 0..good.len().min(400) {
            let _ = Toc::parse(&good[..len]);
        }
    }

    #[test]
    fn adding_streams_keeps_the_table_valid() {
        let mut toc = sample();
        // A stream loaded on request, between two existing hashes.
        let name = toc.add_name(0x6000_0000, "gf2/text/60000000");
        assert_eq!(toc.add_name(0x6000_0000, "GF2\\TEXT\\60000000"), name);
        let parent = toc.find_name("gf2\\frontend").unwrap();
        let index = toc
            .add_file(name, 1234, FLAGS_REQUESTED, &[parent])
            .unwrap();
        assert_eq!(index, 2);
        assert!(toc.is_sorted_by_hash());
        assert!(toc.add_file(name, 1, FLAGS_REQUESTED, &[parent]).is_err());
        let other = toc.add_name(0x7000_0000, "gf2\\x");
        assert!(toc.add_file(other, 1, FLAGS_REQUESTED, &[]).is_err());

        // A stream loaded together with gf2\frontend.
        let child = toc.add_name(0x1000_0000, "gf2\\child");
        toc.add_file(child, 99, FLAGS_PARENTED, &[parent]).unwrap();
        let frontend = toc.find_file("gf2\\frontend").unwrap();
        toc.attach_child(frontend, child).unwrap();
        toc.attach_child(frontend, child).unwrap();
        let entry = &toc.files[frontend];
        assert_eq!(entry.flags, [0, 1, 1, 0]);
        assert_eq!(entry.prerequisite_count(), 1);
        let deps: Vec<String> = toc.file_deps(frontend).map(TocName::name_lossy).collect();
        assert_eq!(deps, ["gf2\\frontend_global", "gf2\\child"]);

        let bytes = toc.to_bytes().unwrap();
        let parsed = Toc::parse(&bytes).unwrap();
        assert!(parsed.is_sorted_by_hash());
        assert_eq!(parsed.to_bytes().unwrap(), bytes);
        let found = parsed.find_file("gf2/text/60000000").unwrap();
        assert_eq!(parsed.files[found].data_size, 1234);
        let deps: Vec<String> = parsed.file_deps(found).map(TocName::name_lossy).collect();
        assert_eq!(deps, ["gf2\\frontend"]);
        // Other entries sharing the old dependency list are unchanged.
        let text = parsed.find_file("gf2\\text\\856c146c").unwrap();
        let deps: Vec<String> = parsed.file_deps(text).map(TocName::name_lossy).collect();
        assert_eq!(deps, ["gf2\\frontend"]);
    }

    #[test]
    fn to_bytes_validates_indices() {
        let mut toc = sample();
        toc.files[1].name = 99;
        assert!(toc.to_bytes().is_err());
        let mut toc = sample();
        toc.dep_words[0] = 99;
        assert!(toc.to_bytes().is_err());
        let mut toc = sample();
        toc.names[0].name = b"a\0b".to_vec();
        assert!(toc.to_bytes().is_err());
    }
}

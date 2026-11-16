//! Patches of the old closed toolkit (`.vpat`): reading, and conversion into a mod.
//!
//! The format was worked out from the five patches found in a game the old
//! toolkit had patched, by comparing them with the patched files and the
//! originals it kept in `_toolkit/backups/`; the meaning of the flags, the
//! operations and the block of resources addressed by type comes from the
//! comparison with another EARS editor and agrees with the five patches. See `docs/mods.md` §5. All numbers are
//! little-endian; strings are NUL-terminated.
//!
//! ```text
//! "VPAT"  u32 format version (1)
//! cstr    name
//! cstr    author
//! u32     version of the patch
//! u32     game (2: The Godfather II, as in `_toolkit/game_instance.xml`)
//! u32     patch id (random, made when the patch was created)
//! u32     flags of the patch (1: raise the version on every change)
//! u32     number of files
//! file × number of files:
//!     cstr    path relative to the game directory, with backslashes (`gf2\frontend.str`)
//!     u32     version of the stream (0)
//!     u32     flags of the file: 1 none, 2 resources addressed by type follow
//!             (0 in older patches)
//!     u32     number of resources
//!     resource × number of resources:
//!         u32 × 4   ids of the resource (as in its SHDR header)
//!         u32       type id
//!         u32       operation: 1 modify, 2 remove, 3 add
//!         u32       size
//!         u8 × size decompressed data of the resource
//!     with flag 2:
//!     u8      number of resources addressed by type
//!     resource × that number:
//!         u32       type id
//!         u32       operation
//!         u32       size
//!         u8 × size data
//! ```
//!
//! A resource is identified exactly like a [`ResourceKey`]: stream, type id
//! and the four ids, so a patch converts into a mod without guessing. A
//! resource addressed by type is the one resource of that type with all ids
//! zero in the stream (SimGroup, `covernodetable`, `lighttoc`,
//! `variabledictionary`, `bricknmortarres`: at most one per stream in the game).

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use super::{MAX_PAYLOAD_SIZE, MAX_RESOURCES, ModGame, ModInfo, ModManifest, ModResource};
use crate::container::ustream::StreamLayout;
use crate::game::{GameInstance, StreamFile};
use crate::workspace::key::normalize_stream;
use crate::workspace::{Added, Blob, PayloadStore, ResourceKey, StorageChoice};
use crate::{Error, Result};

/// Magic at the start of a `.vpat` file.
pub const MAGIC: &[u8; 4] = b"VPAT";
/// Game number of The Godfather II in `.vpat` files.
pub const GAME_GODFATHER2: u32 = 2;
/// Flag of a patch: the old toolkit raises the version on every change.
pub const PATCH_FLAG_INCREMENT_VERSION: u32 = 1;
/// Flag of a file: nothing special (older patches write 0 instead).
pub const FILE_FLAG_NONE: u32 = 1;
/// Flag of a file: a block of resources addressed by type follows its list.
pub const FILE_FLAG_BY_TYPE: u32 = 2;
/// Ids of a resource addressed by type: all zero, as in the game.
pub const BY_TYPE_IDS: [u32; 4] = [0; 4];

const MAX_STRING: usize = 1024;
const MAX_FILES: u32 = 10_000;

/// A parsed `.vpat` file. Resource data stays in the file until read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VpatPatch {
    pub path: PathBuf,
    pub format_version: u32,
    pub name: String,
    pub author: String,
    pub version: u32,
    pub game: u32,
    /// Random id the old toolkit gave the patch when it was created.
    pub patch_id: u32,
    /// Flags of the patch ([`PATCH_FLAG_INCREMENT_VERSION`]); installing ignores them.
    pub flags: u32,
    pub files: Vec<VpatFile>,
}

/// The resources a patch changes in one stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VpatFile {
    /// Path as written in the patch, e.g. `gf2\frontend.str`.
    pub path: String,
    /// Stream name as in resource keys, e.g. `gf2/frontend`.
    pub stream: String,
    /// Version of the stream the patch was made for (0 in every known patch).
    pub stream_version: u32,
    /// [`FILE_FLAG_NONE`] and [`FILE_FLAG_BY_TYPE`]; 0 in older patches.
    pub flags: u32,
    /// Resources addressed by ids, then those addressed by type.
    pub resources: Vec<VpatResource>,
}

/// What a patch does with a resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VpatOperation {
    /// Nothing; the old toolkit skips such a resource.
    None = 0,
    /// Replace the data: every resource of the known patches.
    Modify = 1,
    /// Remove the resource from the stream.
    Remove = 2,
    /// Add a new resource to the stream.
    Add = 3,
}

impl VpatOperation {
    /// The operation stored as `value`, if it is known.
    pub fn from_u32(value: u32) -> Option<Self> {
        Some(match value {
            0 => Self::None,
            1 => Self::Modify,
            2 => Self::Remove,
            3 => Self::Add,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Modify => "modify",
            Self::Remove => "remove",
            Self::Add => "add",
        }
    }
}

/// One resource of a patch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VpatResource {
    pub type_id: u32,
    /// Ids of the resource; [`BY_TYPE_IDS`] when it is addressed by type.
    pub ids: [u32; 4],
    pub operation: VpatOperation,
    /// Addressed by type: the one resource of `type_id` with zero ids in the stream.
    pub by_type: bool,
    /// Position of the data in the patch file.
    pub offset: u64,
    pub size: u32,
}

impl VpatResource {
    /// The key of the resource in `stream`.
    pub fn key(&self, stream: &str) -> ResourceKey {
        ResourceKey {
            stream: stream.to_owned(),
            type_id: self.type_id,
            ids: self.ids,
        }
    }
}

/// Whether `head` (the first bytes of a file) starts a `.vpat` file.
pub fn is_vpat(head: &[u8]) -> bool {
    head.starts_with(MAGIC)
}

/// A bounded reader over the patch file.
struct Cursor {
    reader: std::io::BufReader<File>,
    pos: u64,
    len: u64,
}

impl Cursor {
    fn fail(&self, why: &str) -> Error {
        Error::mod_error(format!("malformed .vpat at offset {:#x}: {why}", self.pos))
    }

    fn bytes<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut bytes = [0u8; N];
        if self.len - self.pos < N as u64 {
            return Err(self.fail("unexpected end of file"));
        }
        self.reader
            .read_exact(&mut bytes)
            .map_err(|_| self.fail("unexpected end of file"))?;
        self.pos += N as u64;
        Ok(bytes)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes::<1>()?[0])
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.bytes()?))
    }

    fn cstr(&mut self) -> Result<String> {
        let mut bytes = Vec::new();
        loop {
            if self.pos >= self.len {
                return Err(self.fail("unterminated string"));
            }
            let byte = self.u8()?;
            if byte == 0 {
                break;
            }
            if bytes.len() == MAX_STRING {
                return Err(self.fail("string too long"));
            }
            bytes.push(byte);
        }
        // Latin-1: every byte is a character.
        let text: String = bytes.iter().map(|&b| b as char).collect();
        if text.chars().any(char::is_control) {
            return Err(self.fail("control characters in a string"));
        }
        Ok(text)
    }

    fn skip(&mut self, n: u64) -> Result<()> {
        if self.len - self.pos < n {
            return Err(self.fail("resource data past the end of the file"));
        }
        self.reader
            .seek_relative(n as i64)
            .map_err(|_| self.fail("cannot seek"))?;
        self.pos += n;
        Ok(())
    }

    /// Type, operation, size and data of a resource; the ids are read by the caller.
    fn resource(&mut self, ids: [u32; 4], by_type: bool) -> Result<VpatResource> {
        let type_id = self.u32()?;
        let operation = self.u32()?;
        let operation = VpatOperation::from_u32(operation)
            .ok_or_else(|| self.fail(&format!("unknown operation {operation} of a resource")))?;
        let size = self.u32()?;
        if u64::from(size) > MAX_PAYLOAD_SIZE {
            return Err(self.fail("resource larger than allowed"));
        }
        let offset = self.pos;
        self.skip(u64::from(size))?;
        Ok(VpatResource {
            type_id,
            ids,
            operation,
            by_type,
            offset,
            size,
        })
    }
}

impl VpatPatch {
    /// Reads the structure of the patch at `path`; the resource data is not read.
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path).map_err(|e| Error::io(path, e))?;
        let len = file.metadata().map_err(|e| Error::io(path, e))?.len();
        let mut c = Cursor {
            reader: std::io::BufReader::new(file),
            pos: 0,
            len,
        };
        if len < 4 || &c.bytes::<4>()? != MAGIC {
            return Err(Error::mod_error(format!(
                "{} is not a .vpat patch",
                path.display()
            )));
        }
        let format_version = c.u32()?;
        if format_version != 1 {
            return Err(Error::mod_error(format!(
                "{}: .vpat format version {format_version} is unknown (only 1 is)",
                path.display()
            )));
        }
        let name = c.cstr()?;
        let author = c.cstr()?;
        let version = c.u32()?;
        let game = c.u32()?;
        let patch_id = c.u32()?;
        let flags = c.u32()?;
        let file_count = c.u32()?;
        if file_count == 0 || file_count > MAX_FILES {
            return Err(c.fail("implausible number of files"));
        }
        let mut files = Vec::new();
        let mut total = 0usize;
        let mut count_one = |c: &Cursor| {
            total += 1;
            if total > MAX_RESOURCES {
                Err(c.fail("too many resources"))
            } else {
                Ok(())
            }
        };
        for _ in 0..file_count {
            let file_path = c.cstr()?;
            if file_path.is_empty() {
                return Err(c.fail("empty file name"));
            }
            let stream_version = c.u32()?;
            let file_flags = c.u32()?;
            if file_flags & !(FILE_FLAG_NONE | FILE_FLAG_BY_TYPE) != 0 {
                return Err(c.fail(&format!("unknown flags {file_flags:#x} of a file")));
            }
            let count = c.u32()?;
            let mut resources = Vec::new();
            for _ in 0..count {
                count_one(&c)?;
                let ids = [c.u32()?, c.u32()?, c.u32()?, c.u32()?];
                resources.push(c.resource(ids, false)?);
            }
            if file_flags & FILE_FLAG_BY_TYPE != 0 {
                for _ in 0..c.u8()? {
                    count_one(&c)?;
                    resources.push(c.resource(BY_TYPE_IDS, true)?);
                }
            }
            files.push(VpatFile {
                stream: normalize_stream(&file_path),
                path: file_path,
                stream_version,
                flags: file_flags,
                resources,
            });
        }
        if c.pos != len {
            return Err(c.fail("unexpected data after the last resource"));
        }
        Ok(Self {
            path: path.to_owned(),
            format_version,
            name,
            author,
            version,
            game,
            patch_id,
            flags,
            files,
        })
    }

    /// Number of resources in all files.
    pub fn resource_count(&self) -> usize {
        self.files.iter().map(|f| f.resources.len()).sum()
    }

    /// Reads the data of `resource`.
    pub fn read_data(&self, resource: &VpatResource) -> Result<Vec<u8>> {
        let mut file = File::open(&self.path).map_err(|e| Error::io(&self.path, e))?;
        file.seek(SeekFrom::Start(resource.offset))
            .map_err(|e| Error::io(&self.path, e))?;
        let mut data = vec![0u8; resource.size as usize];
        file.read_exact(&mut data)
            .map_err(|e| Error::io(&self.path, e))?;
        Ok(data)
    }

    /// Converts the patch into a mod for `game`: payloads go into `store`,
    /// the original data of every resource is taken from the game's
    /// unmodified files (see [`super::integrity::pristine_path`]).
    ///
    /// Replaced resources the game does not have are skipped with a warning,
    /// as the old toolkit did; added ones become added resources of the mod
    /// (format version 2). Removing resources of the game is not something a
    /// mod can do, so a patch that removes any is refused. Returns the
    /// manifest and the warnings.
    pub fn to_mod(
        &self,
        game: &GameInstance,
        store: &PayloadStore,
    ) -> Result<(ModManifest, Vec<String>)> {
        if self.game != GAME_GODFATHER2 {
            return Err(Error::mod_error(format!(
                "{} is made for another game (game number {}; The Godfather II is {GAME_GODFATHER2})",
                self.path.display(),
                self.game
            )));
        }
        let removed: Vec<String> = self
            .files
            .iter()
            .flat_map(|f| {
                f.resources
                    .iter()
                    .filter(|r| r.operation == VpatOperation::Remove)
                    .map(|r| r.key(&f.stream).to_string())
            })
            .collect();
        if let Some(first) = removed.first() {
            return Err(Error::mod_error(format!(
                "{} removes {} resource(s) of the game (first: {first}); \
                 mods can replace and add resources but not remove them",
                self.path.display(),
                removed.len()
            )));
        }
        let info = ModInfo {
            id: super::id_from_name(&self.name),
            name: if self.name.trim().is_empty() {
                file_stem(&self.path)
            } else {
                self.name.clone()
            },
            version: self.version.to_string(),
            author: self.author.clone(),
            description: format!(
                "Converted from the .vpat patch {}.",
                self.path
                    .file_name()
                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
            ),
            url: None,
            relations: Default::default(),
        };
        let mut manifest = ModManifest::new(
            &info,
            ModGame {
                profile: game.profile().name.to_owned(),
                executable_size: None,
                executable_sha256: None,
                editions: Vec::new(),
            },
        );
        manifest.converted_from = Some("vpat".to_owned());
        let mut warnings = Vec::new();
        let mut resources = BTreeMap::new();
        let mut order = 0u64;
        for file in &self.files {
            let Some(index) = game.find_stream(&file.stream) else {
                warnings.push(format!(
                    "{} is not in this game; its {} resource(s) are skipped",
                    file.path,
                    file.resources.len()
                ));
                continue;
            };
            let game_file = &game.streams()[index];
            let original = StreamFile::open(super::integrity::pristine_path(game, game_file))?;
            let layout = original.layout();
            let mut missing = 0;
            let mut skipped = 0;
            for resource in &file.resources {
                if resource.operation == VpatOperation::None {
                    skipped += 1;
                    continue;
                }
                let key = resource.key(&file.stream);
                let found = if resource.by_type {
                    find_by_type(layout, resource.type_id).map_err(|count| {
                        Error::mod_error(format!(
                            "{}: {} has {count} resources of type {} with zero ids; \
                             a resource addressed by type must be the only one",
                            self.path.display(),
                            file.path,
                            key.type_name()
                        ))
                    })?
                } else {
                    key.find_in(layout)
                };
                let data = self.read_data(resource)?;
                let entry = match (resource.operation, found) {
                    (VpatOperation::Add, None) => {
                        order += 1;
                        ModResource {
                            payload: store.put(&data)?,
                            original: Blob::of(&[]),
                            storage: StorageChoice::Keep,
                            label: String::new(),
                            added: Some(added_like(layout, resource.type_id, order)),
                            key: key.clone(),
                        }
                    }
                    (_, None) => {
                        missing += 1;
                        continue;
                    }
                    (operation, Some(i)) => {
                        if operation == VpatOperation::Add {
                            warnings.push(format!(
                                "{key} is added by the patch but already in the game; its data is replaced"
                            ));
                        }
                        let original_blob = Blob::of(&original.resource_data(i)?);
                        if Blob::of(&data) == original_blob {
                            continue;
                        }
                        ModResource {
                            payload: store.put(&data)?,
                            original: original_blob,
                            storage: StorageChoice::Keep,
                            label: String::new(),
                            added: None,
                            key: key.clone(),
                        }
                    }
                };
                resources.insert(key, entry);
            }
            if missing > 0 {
                warnings.push(format!(
                    "{missing} of {} resource(s) for {} are not in this game and are skipped",
                    file.resources.len(),
                    file.path
                ));
            }
            if skipped > 0 {
                warnings.push(format!(
                    "{skipped} resource(s) for {} have no operation and are skipped",
                    file.path
                ));
            }
        }
        if resources.is_empty() {
            return Err(Error::mod_error(format!(
                "nothing in {} applies to this game: none of its resources exist here, \
                 or they already have the patched data in the original files",
                self.path.display()
            )));
        }
        manifest.resources = resources.into_values().collect();
        manifest.format_version = manifest.required_version();
        manifest.validate()?;
        Ok((manifest, warnings))
    }
}

/// Index of the one resource of `type_id` with zero ids in `layout`; the
/// number of such resources when there are several.
fn find_by_type(layout: &StreamLayout, type_id: u32) -> std::result::Result<Option<usize>, usize> {
    let mut found = layout
        .resources
        .iter()
        .enumerate()
        .filter(|(_, r)| r.header.type_id == type_id && r.header.ids == BY_TYPE_IDS)
        .map(|(i, _)| i);
    let first = found.next();
    match found.count() {
        0 => Ok(first),
        more => Err(more + 1),
    }
}

/// Where an added resource of `type_id` goes: at the end of the stream, with
/// the memory policy and alignment of the first resource of that type in the
/// stream, or the usual ones of the type when the stream has none.
fn added_like(layout: &StreamLayout, type_id: u32, order: u64) -> Added {
    let (mem_policy, alignment) = layout
        .resources
        .iter()
        .find(|r| r.header.type_id == type_id)
        .map_or_else(
            || {
                let d = crate::create::type_defaults(type_id);
                (d.mem_policy, d.alignment)
            },
            |r| (r.header.mem_policy, r.header.alignment),
        );
    Added {
        mem_policy,
        alignment,
        before: None,
        order,
    }
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map_or_else(|| "patch".to_owned(), |s| s.to_string_lossy().into_owned())
}

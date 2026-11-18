//! What a mod file (`.zip`, or a `.vpat` patch of the old toolkit) holds,
//! read before it is installed (stage 103): the same summary for the mod
//! manager's window and for other surfaces.

use std::path::Path;

use serde::Serialize;

use super::vpat::{self, VpatPatch};
use super::{ModArchive, ModManifest};
use crate::error::{Error, Result};

/// The kind of a mod file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FileKind {
    /// A mod package (`mod.json` and payloads in a ZIP).
    Mod,
    /// A patch of the old toolkit; installing converts it.
    Vpat,
}

/// The executable code a mod carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CodeInfo {
    /// The edition of the game the code is for.
    pub edition: String,
    /// The mod does not work without its code.
    pub required: bool,
    /// File names of the modules.
    pub modules: Vec<String>,
}

/// What a mod file holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileInfo {
    pub kind: FileKind,
    /// Empty for a patch: it gets an id when installed.
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub url: Option<String>,
    /// The game profile the mod was made for.
    pub game: String,
    pub format_version: u32,
    /// Replaced and added resources, and how many of them are added.
    pub resources: usize,
    pub added: usize,
    /// Resources stored as deltas (format version 6), which merge with
    /// other mods, and the operations in them.
    pub deltas: usize,
    pub operations: usize,
    /// Streams the mod changes.
    pub streams: Vec<String>,
    /// Streams the mod creates.
    pub new_streams: Vec<String>,
    /// Entries of `.viv` archives and other files.
    pub files: usize,
    pub payload_bytes: u64,
    /// `id condition` of the mods it needs, and of those it cannot be with.
    pub depends: Vec<String>,
    pub incompatible: Vec<String>,
    pub code: Option<CodeInfo>,
    pub warnings: Vec<String>,
    /// Every payload was read and its checksum matched.
    pub verified: bool,
}

impl FileInfo {
    fn of_manifest(m: &ModManifest) -> Self {
        Self {
            kind: FileKind::Mod,
            id: m.id.clone(),
            name: m.name.clone(),
            version: m.version.clone(),
            author: m.author.clone(),
            description: m.description.clone(),
            url: m.url.clone(),
            game: m.game.profile.clone(),
            format_version: m.format_version,
            resources: m.resources.len(),
            added: m.added().count(),
            deltas: m.deltas().count(),
            operations: 0,
            streams: m.streams().into_iter().map(str::to_owned).collect(),
            new_streams: m.streams.iter().map(|s| s.name.clone()).collect(),
            files: m.files.len(),
            payload_bytes: m.payload_bytes(),
            depends: m
                .depends
                .iter()
                .map(|(id, v)| format!("{id} {v}"))
                .collect(),
            incompatible: m
                .incompatible
                .iter()
                .map(|(id, i)| format!("{id} {}", i.versions))
                .collect(),
            code: m.native.as_ref().map(|code| CodeInfo {
                edition: code.edition.clone(),
                required: code.required,
                modules: code.modules.iter().map(|m| m.path()).collect(),
            }),
            warnings: m.warnings(),
            verified: false,
        }
    }
}

/// Reads the mod file `path` and checks it (every payload of a package).
pub fn file_info(path: &Path) -> Result<FileInfo> {
    let head = {
        use std::io::Read;
        let mut head = [0u8; 4];
        let mut file = std::fs::File::open(path).map_err(|e| Error::io(path, e))?;
        let n = file.read(&mut head).map_err(|e| Error::io(path, e))?;
        head[..n].to_vec()
    };
    if vpat::is_vpat(&head) {
        let patch = VpatPatch::open(path)?;
        return Ok(FileInfo {
            kind: FileKind::Vpat,
            id: String::new(),
            name: patch.name.clone(),
            version: patch.version.to_string(),
            author: patch.author.clone(),
            description: String::new(),
            url: None,
            game: format!("{:08x}", patch.game),
            format_version: patch.format_version,
            resources: patch.files.iter().map(|f| f.resources.len()).sum(),
            added: 0,
            deltas: 0,
            operations: 0,
            streams: patch.files.iter().map(|f| f.stream.clone()).collect(),
            new_streams: Vec::new(),
            files: 0,
            payload_bytes: patch
                .files
                .iter()
                .flat_map(|f| &f.resources)
                .map(|r| u64::from(r.size))
                .sum(),
            depends: Vec::new(),
            incompatible: Vec::new(),
            code: None,
            warnings: Vec::new(),
            verified: true,
        });
    }
    let mut archive = ModArchive::open(path)?;
    archive.verify()?;
    let mut info = FileInfo::of_manifest(archive.manifest());
    info.operations = archive
        .delta_operations()?
        .iter()
        .map(|(_, operations)| operations)
        .sum();
    info.warnings.extend(archive.warnings().iter().cloned());
    info.verified = true;
    Ok(info)
}

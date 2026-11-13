//! Mods: packages of replaced resources, their installation into a game, and
//! the checks that keep the game intact.
//!
//! A mod is a ZIP file with a manifest, `mod.json`, and the new data of every
//! resource it replaces or adds. Resources are identified by their
//! [`ResourceKey`], as in the workspace; a mod is essentially an exported
//! workspace. The manifest also records the original data each replaced
//! resource had (its SHA-256), so a mod can tell whether the game it is
//! installed into has the resources it was made for.
//!
//! Format version 2 carries what the workspace can add: new resources
//! ([`ModResource::added`]: memory policy, alignment and place in the stream)
//! and new streams ([`ModManifest::streams`]: table-of-contents entries). A
//! mod without them is still written as version 1, which older programs read.
//!
//! ```text
//! example.zip
//! ├── mod.json                 manifest: metadata and resources
//! ├── preview.png              optional picture
//! └── payloads/<sha256>        new data of the resources, one file per distinct content
//! ```
//!
//! Mod files come from strangers: [`package`] reads them with limits on every
//! size and count, never extracts by entry name, and checks every payload
//! against its hash. [`manager`] installs mods into a game and writes the
//! combined result with the same machinery as the workspace (backups, atomic
//! writes, table-of-contents sizes). [`vpat`] converts the patches of the old
//! closed toolkit; [`integrity`] tells which game files differ from their
//! originals and who changed them.
//!
//! # Mods and the workspace
//!
//! The mod manager and the editor's workspace never write into the game at
//! the same time: while one has its changes in the game files, the other
//! refuses to write and says how to proceed. An author tests a workspace
//! together with other mods by installing the workspace as a mod
//! ([`manager::ModManager::install_workspace`]).

pub mod integrity;
pub mod manager;
pub mod package;
pub mod vpat;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::game::GameInstance;
use crate::workspace::{
    Added, Blob, Deployment, GameFingerprint, NewStream, ResourceKey, StorageChoice, Workspace,
};
use crate::{Error, Result};

pub use manager::{InstalledMod, ModManager};
pub use package::ModArchive;

/// Value of the `format` field of `mod.json`.
pub const FORMAT: &str = "doctor-toolkit-mod";
/// Current version of the mod format: 2 added new resources and new streams.
pub const FORMAT_VERSION: u32 = 2;
/// The first format version that can add resources and streams.
pub const ADDING_VERSION: u32 = 2;
/// Name of the manifest inside a mod.
pub const MANIFEST_NAME: &str = "mod.json";
/// Directory of the payloads inside a mod.
pub const PAYLOAD_DIR: &str = "payloads";
/// Directory of the mod manager, relative to the game directory.
pub const MODS_DIR: &str = "_doctor_toolkit/mods";

/// Largest `mod.json` accepted.
pub const MAX_MANIFEST_SIZE: u64 = 32 << 20;
/// Largest preview picture accepted.
pub const MAX_PREVIEW_SIZE: u64 = 8 << 20;
/// Largest single resource accepted. The largest resource of Godfather II is
/// about 26 MiB (a sound bank).
pub const MAX_PAYLOAD_SIZE: u64 = 512 << 20;
/// Largest sum of all payload sizes of one mod.
pub const MAX_TOTAL_SIZE: u64 = 8 << 30;
/// Most resources one mod may replace (the whole game has about 212 000).
pub const MAX_RESOURCES: usize = 250_000;
/// Most entries a mod ZIP may contain.
pub const MAX_ENTRIES: usize = MAX_RESOURCES + 16;

const MAX_ID: usize = 64;
const MAX_NAME: usize = 200;
const MAX_VERSION: usize = 64;
const MAX_DESCRIPTION: usize = 20_000;
const MAX_URL: usize = 2_000;
const MAX_STREAM_NAME: usize = 260;
/// Most new streams one mod may create.
pub const MAX_NEW_STREAMS: usize = 10_000;
/// Most tables of contents one new stream may be listed in (the game has 25).
const MAX_STREAM_TOCS: usize = 64;
/// Largest alignment of an added resource (the game uses up to 0x1000).
const MAX_ALIGNMENT: u32 = 0x1_0000;

/// `mod.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModManifest {
    /// Always [`FORMAT`].
    pub format: String,
    /// Version of the format; a newer version than [`FORMAT_VERSION`] is refused.
    pub format_version: u32,
    /// Identifier: lower-case letters, digits, `-` and `_`. Installing a mod
    /// with the id of an installed one replaces it (an update).
    pub id: String,
    /// Name shown to players.
    pub name: String,
    /// Version of the mod, free text such as `1.2`.
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
    /// Home page of the mod, `http://` or `https://`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Name of the preview picture in the package, e.g. `preview.png`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    /// The game the mod was made for.
    pub game: ModGame,
    /// Replaced and added resources, sorted by key.
    pub resources: Vec<ModResource>,
    /// Streams the mod creates (format version 2), sorted by name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub streams: Vec<NewStream>,
    /// Where the mod came from when it was not built as a mod, e.g. `vpat`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub converted_from: Option<String>,
}

/// The game a mod was made for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModGame {
    /// Profile name, e.g. `The Godfather II (PC)`. A mod for another profile is refused.
    pub profile: String,
    /// The executable of the game the mod was made on; a difference is only reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable_size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable_sha256: Option<String>,
}

impl ModGame {
    pub fn from_fingerprint(fingerprint: &GameFingerprint) -> Self {
        let known = !fingerprint.executable_sha256.is_empty();
        Self {
            profile: fingerprint.profile.clone(),
            executable_size: known.then_some(fingerprint.executable_size),
            executable_sha256: known.then(|| fingerprint.executable_sha256.clone()),
        }
    }

    /// Whether the executable of `game` is the one the mod was made on;
    /// `None` when the mod does not say.
    pub fn same_executable(&self, game: &GameFingerprint) -> Option<bool> {
        let sha = self.executable_sha256.as_ref()?;
        Some(*sha == game.executable_sha256 && self.executable_size == Some(game.executable_size))
    }
}

/// One resource a mod replaces or adds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModResource {
    pub key: ResourceKey,
    /// The new data: `payloads/<sha256>` in the package.
    pub payload: Blob,
    /// The data of the resource in the game the mod was made for; empty data
    /// for an added resource.
    pub original: Blob,
    #[serde(default, skip_serializing_if = "StorageChoice::is_keep")]
    pub storage: StorageChoice,
    /// What the change is, e.g. the label of the workspace operation.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// Set when the mod adds the resource (format version 2): its memory
    /// policy, alignment and place in the stream, as in the workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added: Option<Added>,
}

impl ModResource {
    pub fn is_added(&self) -> bool {
        self.added.is_some()
    }
}

/// Metadata of a mod being built.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub url: Option<String>,
}

impl ModManifest {
    /// A manifest with `info` and no resources, of format version 1. Set
    /// [`Self::format_version`] to [`Self::required_version`] after adding
    /// resources or streams.
    pub fn new(info: &ModInfo, game: ModGame) -> Self {
        Self {
            format: FORMAT.to_owned(),
            format_version: 1,
            id: info.id.clone(),
            name: info.name.clone(),
            version: info.version.clone(),
            author: info.author.clone(),
            description: info.description.clone(),
            url: info.url.clone().filter(|u| !u.is_empty()),
            preview: None,
            game,
            resources: Vec::new(),
            streams: Vec::new(),
            converted_from: None,
        }
    }

    /// The lowest format version that can hold this manifest: 1 for a mod
    /// that only replaces resources, 2 when it adds resources or streams.
    pub fn required_version(&self) -> u32 {
        if self.streams.is_empty() && !self.resources.iter().any(ModResource::is_added) {
            1
        } else {
            ADDING_VERSION
        }
    }

    /// Parses and validates `mod.json`.
    pub fn parse(json: &[u8]) -> Result<Self> {
        if json.len() as u64 > MAX_MANIFEST_SIZE {
            return Err(Error::mod_error("mod.json is too large"));
        }
        // The format first, so that a newer format gets a clear message
        // instead of a complaint about a field.
        let head: serde_json::Value = serde_json::from_slice(json)
            .map_err(|e| Error::mod_error(format!("mod.json is not valid JSON: {e}")))?;
        if head.get("format").and_then(|f| f.as_str()) != Some(FORMAT) {
            return Err(Error::mod_error(
                "mod.json is not a doctor_toolkit mod manifest (wrong `format`)",
            ));
        }
        if let Some(version) = head.get("format_version").and_then(|v| v.as_u64())
            && version > u64::from(FORMAT_VERSION)
        {
            return Err(Error::mod_error(format!(
                "the mod uses format version {version}; this program reads up to \
                 {FORMAT_VERSION}. Update doctor_toolkit"
            )));
        }
        let manifest: Self =
            serde_json::from_value(head).map_err(|e| Error::mod_error(format!("mod.json: {e}")))?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// The manifest as JSON.
    pub fn to_json(&self) -> Result<Vec<u8>> {
        let mut text = serde_json::to_vec_pretty(self)
            .map_err(|e| Error::mod_error(format!("mod.json: {e}")))?;
        text.push(b'\n');
        Ok(text)
    }

    /// Checks every field against the format and the limits.
    pub fn validate(&self) -> Result<()> {
        let bad = |why: String| Err(Error::mod_error(format!("mod.json: {why}")));
        if self.format != FORMAT {
            return bad(format!("`format` must be `{FORMAT}`"));
        }
        if self.format_version == 0 || self.format_version > FORMAT_VERSION {
            return bad(format!(
                "unsupported format version {}",
                self.format_version
            ));
        }
        validate_id(&self.id)?;
        check_text("name", &self.name, MAX_NAME, false)?;
        check_text("version", &self.version, MAX_VERSION, false)?;
        check_text("author", &self.author, MAX_NAME, true)?;
        check_text("description", &self.description, MAX_DESCRIPTION, true)?;
        if let Some(url) = &self.url {
            check_text("url", url, MAX_URL, false)?;
            if !(url.starts_with("https://") || url.starts_with("http://")) {
                return bad("`url` must start with http:// or https://".into());
            }
        }
        if let Some(preview) = &self.preview
            && !is_preview_name(preview)
        {
            return bad(format!(
                "bad preview name `{}`: expected a plain file name ending in .png or .jpg",
                preview.escape_debug()
            ));
        }
        check_text("game.profile", &self.game.profile, MAX_NAME, false)?;
        if let Some(sha) = &self.game.executable_sha256
            && !is_sha256(sha)
        {
            return bad("`game.executable_sha256` is not a SHA-256".into());
        }
        if self.format_version < self.required_version() {
            return bad(format!(
                "format version {} cannot add resources or streams; version {ADDING_VERSION} \
                 is needed",
                self.format_version
            ));
        }
        if self.resources.is_empty() && self.streams.is_empty() {
            return bad("the mod replaces no resources".into());
        }
        if self.streams.len() > MAX_NEW_STREAMS {
            return bad(format!(
                "{} new streams; at most {MAX_NEW_STREAMS} are allowed",
                self.streams.len()
            ));
        }
        let new_streams = self.validate_streams()?;
        if self.resources.len() > MAX_RESOURCES {
            return bad(format!(
                "{} resources; at most {MAX_RESOURCES} are allowed",
                self.resources.len()
            ));
        }
        let mut keys = HashSet::new();
        let mut guids: HashMap<[u32; 4], &ModResource> = HashMap::new();
        let mut total = 0u64;
        for resource in &self.resources {
            let key = &resource.key;
            if !keys.insert(key) {
                return bad(format!("{key} is listed twice"));
            }
            if key.stream.len() > MAX_STREAM_NAME
                || key
                    .stream
                    .split('/')
                    .any(|part| part.is_empty() || part == "..")
                || !key.stream.bytes().all(|b| b.is_ascii_graphic())
            {
                return bad(format!(
                    "bad stream name in {}",
                    key.to_string().escape_debug()
                ));
            }
            for (what, blob) in [
                ("payload", &resource.payload),
                ("original", &resource.original),
            ] {
                if !is_sha256(&blob.sha256) {
                    return bad(format!("{key}: the {what} hash is not a SHA-256"));
                }
                if blob.size > MAX_PAYLOAD_SIZE {
                    return bad(format!(
                        "{key}: the {what} is {} bytes; at most {MAX_PAYLOAD_SIZE} are allowed",
                        blob.size
                    ));
                }
            }
            check_text("label", &resource.label, MAX_NAME, true)?;
            if let Some(added) = &resource.added {
                validate_added(resource, added, &new_streams)?;
                if key.ids != [0; 4]
                    && let Some(other) = guids.insert(key.ids, resource)
                    && other.payload != resource.payload
                {
                    return bad(format!(
                        "{key} and {} add the same GUID with different data; the game \
                         would load only one of them",
                        other.key
                    ));
                }
            } else if new_streams.contains(key.stream.as_str()) {
                return bad(format!(
                    "{key} replaces a resource of the new stream {}; only added resources \
                     can be in it",
                    key.stream
                ));
            }
            total += resource.payload.size;
        }
        if total > MAX_TOTAL_SIZE {
            return bad(format!(
                "the payloads add up to {total} bytes; at most {MAX_TOTAL_SIZE} are allowed"
            ));
        }
        Ok(())
    }

    /// Checks the new streams; returns their names.
    fn validate_streams(&self) -> Result<HashSet<&str>> {
        let bad = |why: String| Err(Error::mod_error(format!("mod.json: {why}")));
        let mut names = HashSet::new();
        let mut hashes = HashSet::new();
        for stream in &self.streams {
            let name = &stream.name;
            if !is_new_stream_name(name) {
                return bad(format!(
                    "bad new stream name `{}`: use lower-case letters, digits, `_`, `-` and \
                     `.` in parts separated by `/`",
                    name.escape_debug()
                ));
            }
            if !names.insert(name.as_str()) {
                return bad(format!("the new stream {name} is listed twice"));
            }
            if stream.hash == 0 || !hashes.insert(stream.hash) {
                return bad(format!(
                    "the new stream {name} has a zero or repeated hash {:08x}",
                    stream.hash
                ));
            }
            if stream.guid.len() != 32 || !stream.guid.bytes().all(|b| b.is_ascii_hexdigit()) {
                return bad(format!(
                    "the GUID of the new stream {name} is not 32 hexadecimal digits"
                ));
            }
            if !is_stream_name(&stream.parent) || stream.parent == *name {
                return bad(format!("bad parent of the new stream {name}"));
            }
            if stream.tocs.is_empty() || stream.tocs.len() > MAX_STREAM_TOCS {
                return bad(format!(
                    "the new stream {name} must be listed in 1 to {MAX_STREAM_TOCS} tables of \
                     contents"
                ));
            }
            let mut tocs = HashSet::new();
            for toc in &stream.tocs {
                if !is_toc_path(toc) || !tocs.insert(toc.to_ascii_lowercase()) {
                    return bad(format!(
                        "bad or repeated table of contents `{}` of the new stream {name}",
                        toc.escape_debug()
                    ));
                }
            }
            for (toc, parent) in &stream.toc_parents {
                if !tocs.contains(&toc.to_ascii_lowercase())
                    || !is_stream_name(parent)
                    || parent == name
                {
                    return bad(format!(
                        "bad parent `{}` in `{}` of the new stream {name}",
                        parent.escape_debug(),
                        toc.escape_debug()
                    ));
                }
            }
            check_text("label", &stream.label, MAX_NAME, true)?;
        }
        // A parent must be a stream of the game: tables of contents receive
        // the entries of new streams in no particular order.
        for stream in &self.streams {
            let parents = std::iter::once(&stream.parent).chain(stream.toc_parents.values());
            for parent in parents {
                if names.contains(parent.as_str()) {
                    return bad(format!(
                        "the parent of the new stream {} is another new stream ({parent})",
                        stream.name
                    ));
                }
            }
        }
        Ok(names)
    }

    /// Resources the mod adds.
    pub fn added(&self) -> impl Iterator<Item = &ModResource> {
        self.resources.iter().filter(|r| r.is_added())
    }

    /// Resources the mod replaces.
    pub fn replaced(&self) -> impl Iterator<Item = &ModResource> {
        self.resources.iter().filter(|r| !r.is_added())
    }

    /// The new stream `name`, if the mod creates it.
    pub fn new_stream(&self, name: &str) -> Option<&NewStream> {
        self.streams.iter().find(|s| s.name == name)
    }

    /// Distinct payloads, each once.
    pub fn payloads(&self) -> Vec<&Blob> {
        let mut seen = HashSet::new();
        self.resources
            .iter()
            .map(|r| &r.payload)
            .filter(|b| seen.insert(&b.sha256))
            .collect()
    }

    /// Sum of the sizes of the distinct payloads.
    pub fn payload_bytes(&self) -> u64 {
        self.payloads().iter().map(|b| b.size).sum()
    }

    /// Streams the mod changes or creates, sorted.
    pub fn streams(&self) -> Vec<&str> {
        let mut streams: Vec<&str> = self
            .resources
            .iter()
            .map(|r| r.key.stream.as_str())
            .chain(self.streams.iter().map(|s| s.name.as_str()))
            .collect();
        streams.sort_unstable();
        streams.dedup();
        streams
    }
}

/// The manifest of a mod made of the changes of a workspace: all of them, or
/// those in `keys`.
pub fn manifest_from_workspace(
    ws: &Workspace,
    info: &ModInfo,
    keys: Option<&[ResourceKey]>,
) -> Result<ModManifest> {
    let mut manifest = ModManifest::new(info, ModGame::from_fingerprint(&ws.manifest().game));
    if let Some(keys) = keys {
        for key in keys {
            if ws.change(key).is_none() {
                return Err(Error::mod_error(format!(
                    "the workspace does not change {key}"
                )));
            }
        }
    }
    let selected = || {
        ws.changes()
            .filter(|c| keys.is_none_or(|keys| keys.contains(&c.key)))
    };
    manifest.resources = selected()
        .map(|c| ModResource {
            key: c.key.clone(),
            payload: c.payload.clone(),
            original: c.original.clone(),
            storage: c.storage,
            label: c.label.chars().take(MAX_NAME).collect(),
            added: c.added.clone(),
        })
        .collect();
    // Every new stream of the workspace, or those the selected resources go into.
    manifest.streams = ws
        .new_streams()
        .filter(|s| keys.is_none() || manifest.resources.iter().any(|r| r.key.stream == s.name))
        .map(|s| NewStream {
            label: s.label.chars().take(MAX_NAME).collect(),
            ..s.clone()
        })
        .collect();
    manifest.streams.sort_by(|a, b| a.name.cmp(&b.name));
    if manifest.resources.is_empty() && manifest.streams.is_empty() {
        return Err(Error::mod_error(
            "the workspace has no changes to put into a mod",
        ));
    }
    manifest.format_version = manifest.required_version();
    manifest.validate()?;
    Ok(manifest)
}

/// A preview picture for a mod being built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    /// `preview.png` or `preview.jpg`.
    pub name: String,
    pub data: Vec<u8>,
}

impl Preview {
    /// A PNG or JPEG picture; other formats are refused.
    pub fn from_image(data: Vec<u8>) -> Result<Self> {
        let name = if data.starts_with(b"\x89PNG\r\n\x1a\n") {
            "preview.png"
        } else if data.starts_with(&[0xff, 0xd8, 0xff]) {
            "preview.jpg"
        } else {
            return Err(Error::mod_error(
                "the preview picture must be a PNG or JPEG file",
            ));
        };
        if data.len() as u64 > MAX_PREVIEW_SIZE {
            return Err(Error::mod_error(format!(
                "the preview picture is larger than {} MiB",
                MAX_PREVIEW_SIZE >> 20
            )));
        }
        Ok(Self {
            name: name.to_owned(),
            data,
        })
    }
}

/// Result of [`build_from_workspace`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildReport {
    /// Replaced and added resources.
    pub resources: usize,
    /// Added resources among them.
    pub added: usize,
    /// New streams.
    pub new_streams: usize,
    /// Bytes of distinct payload data.
    pub payload_bytes: u64,
    /// Size of the written file.
    pub file_size: u64,
}

/// Builds a mod package from the changes of a workspace (all, or those in
/// `keys`) and writes it to `out`.
pub fn build_from_workspace(
    ws: &Workspace,
    info: &ModInfo,
    keys: Option<&[ResourceKey]>,
    preview: Option<&Preview>,
    out: &Path,
) -> Result<BuildReport> {
    let mut manifest = manifest_from_workspace(ws, info, keys)?;
    manifest.preview = preview.map(|p| p.name.clone());
    manifest.validate()?;
    let file_size =
        package::write_package(out, &manifest, preview.map(|p| p.data.as_slice()), |blob| {
            ws.store().get(blob)
        })?;
    Ok(BuildReport {
        resources: manifest.resources.len(),
        added: manifest.added().count(),
        new_streams: manifest.streams.len(),
        payload_bytes: manifest.payload_bytes(),
        file_size,
    })
}

/// Checks a mod id: 1 to 64 characters of `a-z`, `0-9`, `-` and `_`,
/// starting with a letter or digit. Ids name directories, so nothing else is allowed.
pub fn validate_id(id: &str) -> Result<()> {
    let ok = !id.is_empty()
        && id.len() <= MAX_ID
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
        && id.as_bytes()[0].is_ascii_alphanumeric();
    if ok {
        Ok(())
    } else {
        Err(Error::mod_error(format!(
            "bad mod id `{}`: use 1 to {MAX_ID} lower-case letters, digits, `-` and `_`",
            id.escape_debug()
        )))
    }
}

/// An id made from a name: `Restored HQ Billboards` → `restored-hq-billboards`.
pub fn id_from_name(name: &str) -> String {
    let mut id = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            id.push(c.to_ascii_lowercase());
        } else if !id.is_empty() && !id.ends_with('-') {
            id.push('-');
        }
    }
    let mut id = id.trim_end_matches('-').to_owned();
    id.truncate(MAX_ID);
    let id = id.trim_end_matches('-').to_owned();
    if id.is_empty() { "mod".to_owned() } else { id }
}

fn check_text(field: &str, value: &str, max: usize, may_be_empty: bool) -> Result<()> {
    if value.len() > max {
        return Err(Error::mod_error(format!(
            "mod.json: `{field}` is longer than {max} bytes"
        )));
    }
    if !may_be_empty && value.trim().is_empty() {
        return Err(Error::mod_error(format!("mod.json: `{field}` is empty")));
    }
    let allowed = |c: char| !c.is_control() || (field == "description" && (c == '\n' || c == '\t'));
    if !value.chars().all(allowed) {
        return Err(Error::mod_error(format!(
            "mod.json: `{field}` contains control characters"
        )));
    }
    Ok(())
}

/// Checks an added resource.
fn validate_added(
    resource: &ModResource,
    added: &Added,
    new_streams: &HashSet<&str>,
) -> Result<()> {
    let key = &resource.key;
    let bad = |why: String| Err(Error::mod_error(format!("mod.json: {key}: {why}")));
    if resource.original != Blob::of(&[]) {
        return bad("an added resource has no original; `original` must be empty data".into());
    }
    if !added.alignment.is_power_of_two() || added.alignment > MAX_ALIGNMENT {
        return bad(format!(
            "alignment {:#x} is not a power of two up to {MAX_ALIGNMENT:#x}",
            added.alignment
        ));
    }
    if let Some(before) = &added.before {
        if new_streams.contains(key.stream.as_str()) {
            return bad("a resource of a new stream cannot be placed before another".into());
        }
        if before.stream != key.stream || before == key {
            return bad(format!("bad place: before {before}"));
        }
    }
    Ok(())
}

/// A stream name as tables of contents use it: parts of lower-case letters,
/// digits, `_`, `-` and `.`, separated by `/`.
fn is_stream_name(name: &str) -> bool {
    name.len() <= MAX_STREAM_NAME
        && !name.is_empty()
        && name.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part.bytes().all(|b| {
                    b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-' | b'.')
                })
        })
}

/// A name for a stream file the mod manager creates: a stream name outside
/// the tool directories and without device names of Windows.
fn is_new_stream_name(name: &str) -> bool {
    const DEVICES: [&str; 22] = [
        "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
        "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    ];
    is_stream_name(name)
        && !name.starts_with("_doctor_toolkit")
        && !name.starts_with("_toolkit")
        && name.split('/').all(|part| {
            let stem = part.split('.').next().unwrap_or(part);
            !DEVICES.contains(&stem) && !part.ends_with('.')
        })
}

/// A relative path of a table of contents, e.g. `gf2/frontend.toc`.
fn is_toc_path(path: &str) -> bool {
    path.strip_suffix(".toc")
        .is_some_and(|stem| is_stream_name(&stem.to_ascii_lowercase()))
}

fn is_sha256(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A preview name: a plain file name of safe characters ending in `.png` or `.jpg`.
fn is_preview_name(name: &str) -> bool {
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return false;
    };
    !stem.is_empty()
        && stem.len() <= 64
        && stem
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        && matches!(ext, "png" | "jpg" | "jpeg")
}

/// The mod manager directory of a game.
pub fn mods_dir(game_root: &Path) -> PathBuf {
    game_root.join(MODS_DIR)
}

/// Refuses while the mod manager has changes in the game files. Called before
/// the workspace writes into the game.
pub fn ensure_mods_not_in_game(game: &GameInstance) -> Result<()> {
    let record = Deployment::load(&manager::applied_path(game.root()))?;
    let held = record.held_streams(game)?;
    if held.is_empty() {
        return Ok(());
    }
    Err(Error::mod_error(format!(
        "the mod manager has mods in {} stream(s) of the game ({}). The workspace and the \
         mod manager cannot both write into the game: restore the originals in the mod \
         manager first (`doctor-toolkit mods unapply`), or install the workspace as a mod \
         (`doctor-toolkit mods install-workspace`) and apply the mods",
        held.len(),
        list_some(&held)
    )))
}

/// Refuses while the game's default workspace has changes in the game files.
/// Called before the mod manager writes into the game.
pub fn ensure_workspace_not_in_game(game: &GameInstance) -> Result<()> {
    let dir = Workspace::default_dir(game.root());
    let record = Deployment::load(&dir.join("applied.json"))?;
    let held = record.held_streams(game)?;
    if held.is_empty() {
        return Ok(());
    }
    Err(Error::mod_error(format!(
        "the editor's workspace has its changes in {} stream(s) of the game ({}). The \
         workspace and the mod manager cannot both write into the game: put the originals \
         back first (`doctor-toolkit workspace revert --game`, or Revert game in the \
         editor's Changes window), or install the workspace as a mod",
        held.len(),
        list_some(&held)
    )))
}

/// Up to five items joined by commas, then a count of the rest.
pub(crate) fn list_some(items: &[String]) -> String {
    let mut text = items.iter().take(5).cloned().collect::<Vec<_>>().join(", ");
    if items.len() > 5 {
        text += &format!(" and {} more", items.len() - 5);
    }
    text
}

#[cfg(test)]
mod tests;

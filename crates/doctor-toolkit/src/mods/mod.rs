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
//! Format version 3 adds relations between mods and the game: the mod
//! version is semantic ([`version`]), `game.editions` lists the editions of
//! the game the mod was tested on, `requires_manager` the versions of this
//! program it needs, `depends`, `recommends` and `incompatible` name other
//! mods with conditions on their versions, and `load_after`/`load_before`
//! are soft rules of the load order ([`order`]). A mod that depends on
//! another is checked against the game with that mod applied: a patch on
//! top of a mod.
//!
//! Format version 5 adds executable code ([`native`]): `.asi` modules for
//! the `scripts` folder of the game, which the mod manager installs only
//! after the player agreed to that very code, and only into the edition of
//! the game they are built for.
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

pub mod info;
pub mod integrity;
pub mod manager;
pub mod native;
pub mod order;
pub mod package;
pub mod version;
pub mod vpat;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::game::GameInstance;
use crate::game_types::MessageId;
use crate::workspace::{
    Added, Blob, Deployment, FileKey, GameFingerprint, NewStream, ResourceKey, StorageChoice,
    Workspace,
};
use crate::{Error, Result};

pub use info::{FileInfo, FileKind, file_info};
pub use manager::{InstalledMod, ModManager};
pub use native::{ModNative, NativeModule};
pub use package::ModArchive;

/// Value of the `format` field of `mod.json`.
pub const FORMAT: &str = "doctor-toolkit-mod";
/// Current version of the mod format: 2 added new resources and new
/// streams, 3 dependencies, editions and rules of the load order, 4 files of
/// the `.viv` archives (voice lines and other streamed sounds) and child
/// streams that load with their parent, 5 executable code (`native`).
pub const FORMAT_VERSION: u32 = 5;
/// The first format version that can add resources and streams.
pub const ADDING_VERSION: u32 = 2;
/// The first format version with relations: dependencies, editions, load order.
pub const RELATIONS_VERSION: u32 = 3;
/// The first format version that carries files of the archives.
pub const FILES_VERSION: u32 = 4;
/// The first format version that carries executable code. The managers that
/// read versions 3 and 4 skip fields they do not know: a mod with code in
/// one of those versions would be installed by them without its code and
/// without a word, so the code needs a version they refuse.
pub const NATIVE_VERSION: u32 = 5;
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
/// Most messages one mod may add.
pub const MAX_MESSAGES: usize = 10_000;
/// Most new streams one mod may create.
pub const MAX_NEW_STREAMS: usize = 10_000;
/// Most tables of contents one new stream may be listed in (the game has 25).
const MAX_STREAM_TOCS: usize = 64;
/// Largest alignment of an added resource (the game uses up to 0x1000).
const MAX_ALIGNMENT: u32 = 0x1_0000;
/// Most mods one list of relations (`depends`, `load_after`…) may name.
const MAX_RELATIONS: usize = 256;
/// Most editions one mod may list.
const MAX_EDITIONS: usize = 32;
const MAX_EDITION: usize = 64;
const MAX_REASON: usize = 1_000;

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
    /// Version of the mod: free text such as `1.2` up to format version 2,
    /// a semantic version from version 3 ([`version::parse_version`]).
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
    /// Versions of the mod manager the mod needs, e.g. `>=0.0.5` (format version 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_manager: Option<String>,
    /// Mods that must be installed, enabled and loaded before this one: id →
    /// condition on the version (format version 3).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub depends: BTreeMap<String, String>,
    /// Mods that go well with this one: id → condition. Only a hint.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub recommends: BTreeMap<String, String>,
    /// Mods that must not be enabled together with this one.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub incompatible: BTreeMap<String, Incompatible>,
    /// Mods this one is loaded after when they are installed (a soft rule).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub load_after: Vec<String>,
    /// Mods this one is loaded before when they are installed (a soft rule).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub load_before: Vec<String>,
    /// Messages the mod adds to the game: names its SimGroups send or hear
    /// that the game does not have, sorted. On the bus a message is
    /// `sdbm(lower(name))`, so a name here must not be a game message and
    /// must not share its hash with one ([`MessageId::custom`]). Older
    /// managers ignore the field; any format version may carry it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<String>,
    /// Entries of the `.viv` archives the mod replaces or adds (format
    /// version 4), sorted by key.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<ModFile>,
    /// Executable code of the mod (format version 5): modules for the
    /// `scripts` folder of the game.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native: Option<ModNative>,
}

/// A mod another one does not work with: the versions concerned and why.
/// Written as `"id": {"versions": ">=2", "reason": "…"}` or, without a
/// reason, as `"id": ">=2"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "IncompatibleForm", into = "IncompatibleForm")]
pub struct Incompatible {
    /// Condition on the version of the other mod; `*` for every version.
    pub versions: String,
    pub reason: String,
}

impl Default for Incompatible {
    fn default() -> Self {
        Self {
            versions: "*".into(),
            reason: String::new(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum IncompatibleForm {
    Short(String),
    Full {
        #[serde(default = "any_version")]
        versions: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        reason: String,
    },
}

fn any_version() -> String {
    "*".into()
}

impl From<IncompatibleForm> for Incompatible {
    fn from(form: IncompatibleForm) -> Self {
        match form {
            IncompatibleForm::Short(versions) => Self {
                versions,
                reason: String::new(),
            },
            IncompatibleForm::Full { versions, reason } => Self { versions, reason },
        }
    }
}

impl From<Incompatible> for IncompatibleForm {
    fn from(i: Incompatible) -> Self {
        if i.reason.is_empty() {
            Self::Short(i.versions)
        } else {
            Self::Full {
                versions: i.versions,
                reason: i.reason,
            }
        }
    }
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
    /// Editions the mod was tested on, as [`crate::edition`] names them
    /// (`1.0.789.0`); empty: any edition (format version 3).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub editions: Vec<String>,
}

impl ModGame {
    pub fn from_fingerprint(fingerprint: &GameFingerprint) -> Self {
        let known = !fingerprint.executable_sha256.is_empty();
        Self {
            profile: fingerprint.profile.clone(),
            executable_size: known.then_some(fingerprint.executable_size),
            executable_sha256: known.then(|| fingerprint.executable_sha256.clone()),
            editions: Vec::new(),
        }
    }

    /// The edition the mod was made on, when it is a known one (`1.0.789.0`).
    pub fn made_on_edition(&self) -> Option<&'static str> {
        let known = crate::edition::known_edition(
            self.executable_size?,
            self.executable_sha256.as_deref()?,
        )?;
        Some(known.version())
    }

    /// Whether the game of `edition` (as [`crate::edition::game_edition`]
    /// gives it) is one the mod was tested on; always when the mod lists none.
    pub fn fits_edition(&self, edition: Option<&str>) -> bool {
        self.editions.is_empty()
            || edition.is_some_and(|e| self.editions.iter().any(|x| edition_matches(x, e)))
    }

    /// Whether the executable of `game` is the one the mod was made on;
    /// `None` when the mod does not say.
    pub fn same_executable(&self, game: &GameFingerprint) -> Option<bool> {
        let sha = self.executable_sha256.as_ref()?;
        Some(*sha == game.executable_sha256 && self.executable_size == Some(game.executable_size))
    }
}

/// A file of the `.viv` archives a mod replaces or adds (format version 4):
/// a streamed sound, such as a voice line. The mod manager puts it into its
/// overlay archive, as the workspace does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModFile {
    /// `viv:audiostreams\…`; loose files are not carried by mods.
    pub key: FileKey,
    /// The new data: `payloads/<sha256>` in the package.
    pub payload: Blob,
    /// What the change is, e.g. the label of the workspace operation.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
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
    /// Dependencies, editions and the load order (format version 3).
    pub relations: Relations,
}

/// What a mod being built says about other mods and the game (format
/// version 3). With every list empty and `requires_manager` set to
/// `Some("")`, the mod is written in an older format.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Relations {
    /// id → condition on its version.
    pub depends: BTreeMap<String, String>,
    pub recommends: BTreeMap<String, String>,
    pub incompatible: BTreeMap<String, Incompatible>,
    pub load_after: Vec<String>,
    pub load_before: Vec<String>,
    /// Editions the mod was tested on; empty: any edition (the resources
    /// are checked one by one). [`ModGame::made_on_edition`] gives the
    /// edition of the game the mod is made on, for "only this edition".
    pub editions: Vec<String>,
    /// Condition on the version of the mod manager; `None`: this
    /// program's version or newer ([`default_requires_manager`]); `Some("")`: none.
    pub requires_manager: Option<String>,
    /// Replaced resources whose change was made on the data of a dependency
    /// rather than on the game's original: that data becomes the
    /// resource's `original` ([`add_suggested_dependencies`] fills it in).
    pub made_on: BTreeMap<ResourceKey, Blob>,
    /// Executable code to put into the mod (format version 5).
    pub native: Option<NativeBuild>,
}

/// The executable code of a mod being built ([`native`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeBuild {
    /// File name in `scripts/` (`my_mod.asi`) and the DLL.
    pub modules: Vec<(String, Vec<u8>)>,
    /// The ABI of the platform module the modules were built with; `None`
    /// for plain ASI plug-ins.
    pub platform_abi: Option<u32>,
    /// The mod makes no sense without its code.
    pub required: bool,
}

impl NativeBuild {
    /// The data of the module with the hash of `blob`.
    fn payload(&self, blob: &Blob) -> Option<&[u8]> {
        self.modules
            .iter()
            .map(|(_, data)| data.as_slice())
            .find(|data| data.len() as u64 == blob.size && Blob::of(data) == *blob)
    }
}

/// The data of a payload of a mod built from `ws` with `info`: a module of
/// its code, or a payload of the workspace.
pub(crate) fn built_payload(ws: &Workspace, info: &ModInfo, blob: &Blob) -> Result<Vec<u8>> {
    match info.relations.native.as_ref().and_then(|n| n.payload(blob)) {
        Some(data) => Ok(data.to_vec()),
        None => ws.store().get(blob),
    }
}

impl Relations {
    /// No relations at all: the mod is written in format version 1 or 2.
    pub fn none() -> Self {
        Self {
            requires_manager: Some(String::new()),
            ..Self::default()
        }
    }
}

/// The condition on the mod manager a built mod gets by default: the version
/// of this program or newer, the first that reads format version 3.
pub fn default_requires_manager() -> String {
    format!(">={}", version::manager_version())
}

/// Whether the edition `entry` of a mod's list names the edition `game`
/// (`1.0.789.0`): the same version, or the full id of a known edition.
pub fn edition_matches(entry: &str, game: &str) -> bool {
    entry.eq_ignore_ascii_case(game)
        || crate::edition::KNOWN
            .iter()
            .any(|k| k.id.eq_ignore_ascii_case(entry) && k.version() == game)
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
            requires_manager: None,
            depends: BTreeMap::new(),
            recommends: BTreeMap::new(),
            incompatible: BTreeMap::new(),
            load_after: Vec::new(),
            load_before: Vec::new(),
            messages: Vec::new(),
            files: Vec::new(),
            native: None,
        }
    }

    /// The lowest format version that can hold this manifest: 1 for a mod
    /// that only replaces resources, 2 when it adds resources or streams, 3
    /// when it has relations ([`Self::has_relations`]), 4 when it carries
    /// files of the archives or a child stream that loads with its parent
    /// (`auto_load`: a manager older than that field would install the
    /// stream disabled without a word), 5 when it has executable code (the
    /// managers of versions 3 and 4 would install it without the code).
    pub fn required_version(&self) -> u32 {
        if self.native.is_some() {
            NATIVE_VERSION
        } else if !self.files.is_empty() || self.streams.iter().any(|s| s.auto_load) {
            FILES_VERSION
        } else if self.has_relations() {
            RELATIONS_VERSION
        } else if self.streams.is_empty() && !self.resources.iter().any(ModResource::is_added) {
            1
        } else {
            ADDING_VERSION
        }
    }

    /// Whether the manifest has a field of format version 3: editions, a
    /// condition on the mod manager, dependencies or rules of the load order.
    pub fn has_relations(&self) -> bool {
        !self.game.editions.is_empty()
            || self.requires_manager.is_some()
            || !self.depends.is_empty()
            || !self.recommends.is_empty()
            || !self.incompatible.is_empty()
            || !self.load_after.is_empty()
            || !self.load_before.is_empty()
    }

    /// The version of the mod as a semantic version, if it reads as one
    /// (always from format version 3).
    pub fn semver(&self) -> Option<semver::Version> {
        version::parse_version(&self.version).ok()
    }

    /// Sets the relations of `relations` (format version 3). No editions
    /// listed means any edition: a mod of data fits wherever its resources do.
    pub fn set_relations(&mut self, relations: &Relations) {
        self.depends = relations.depends.clone();
        self.recommends = relations.recommends.clone();
        self.incompatible = relations.incompatible.clone();
        self.load_after = relations.load_after.clone();
        self.load_before = relations.load_before.clone();
        self.game.editions = relations.editions.clone();
        self.requires_manager = match &relations.requires_manager {
            Some(text) if text.trim().is_empty() => None,
            Some(text) => Some(text.trim().to_owned()),
            None => Some(default_requires_manager()),
        };
    }

    /// Remarks that do not stop the mod: editions this program does not know.
    pub fn warnings(&self) -> Vec<String> {
        let known = || {
            crate::edition::KNOWN
                .iter()
                .map(|k| k.version())
                .collect::<Vec<_>>()
                .join(", ")
        };
        self.game
            .editions
            .iter()
            .filter(|e| {
                !crate::edition::KNOWN
                    .iter()
                    .any(|k| k.version().eq_ignore_ascii_case(e) || k.id.eq_ignore_ascii_case(e))
            })
            .map(|e| {
                format!(
                    "mod.json: the edition `{}` is not known to this program (known: {})",
                    e.escape_debug(),
                    known()
                )
            })
            .collect()
    }

    /// Parses and validates `mod.json`.
    pub fn parse(json: &[u8]) -> Result<Self> {
        Self::parse_up_to(json, FORMAT_VERSION)
    }

    /// [`Self::parse`] by a program that reads format versions up to
    /// `newest`: what an older mod manager says about a newer mod.
    pub fn parse_up_to(json: &[u8], newest: u32) -> Result<Self> {
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
            && version > u64::from(newest)
        {
            return Err(Error::mod_error(format!(
                "the mod uses format version {version}; this program reads up to \
                 {newest}. Update Open Mod Manager (doctor_toolkit) to install it"
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
        if self.format_version < ADDING_VERSION
            && (!self.streams.is_empty() || self.resources.iter().any(ModResource::is_added))
        {
            return bad(format!(
                "format version {} cannot add resources or streams; version {ADDING_VERSION} \
                 is needed",
                self.format_version
            ));
        }
        if self.format_version < FILES_VERSION && !self.files.is_empty() {
            return bad(format!(
                "format version {} cannot carry files of the archives; version \
                 {FILES_VERSION} is needed",
                self.format_version
            ));
        }
        if self.format_version < FILES_VERSION && self.streams.iter().any(|s| s.auto_load) {
            return bad(format!(
                "format version {} cannot hold a child stream that loads with its parent; \
                 version {FILES_VERSION} is needed",
                self.format_version
            ));
        }
        if let Some(native) = &self.native {
            if self.format_version < NATIVE_VERSION {
                return bad(format!(
                    "format version {} cannot carry executable code (`native`); version \
                     {NATIVE_VERSION} is needed",
                    self.format_version
                ));
            }
            native.validate()?;
        }
        if self.files.len() > MAX_RESOURCES {
            return bad(format!("more than {MAX_RESOURCES} files"));
        }
        let mut file_keys = HashSet::new();
        for file in &self.files {
            let FileKey::Archive(name) = &file.key else {
                return bad(format!(
                    "{}: only archive entries can be in a mod",
                    file.key
                ));
            };
            let safe = !name.is_empty()
                && name.len() <= MAX_STREAM_NAME
                && name.bytes().all(|b| b.is_ascii_graphic())
                && *name == name.to_ascii_lowercase()
                && !name.contains('/')
                && !name.starts_with("_doctor_toolkit")
                && name
                    .split('\\')
                    .all(|part| !part.is_empty() && part != ".." && part != ".");
            if !safe {
                return bad(format!("bad archive entry name `{}`", name.escape_debug()));
            }
            if !file_keys.insert(&file.key) {
                return bad(format!("{} is listed twice", file.key));
            }
            if !is_sha256(&file.payload.sha256) {
                return bad(format!("{}: the payload hash is not a SHA-256", file.key));
            }
            if file.payload.size > MAX_PAYLOAD_SIZE {
                return bad(format!(
                    "{}: the payload is {} bytes; at most {MAX_PAYLOAD_SIZE} are allowed",
                    file.key, file.payload.size
                ));
            }
            check_text("label", &file.label, MAX_NAME, true)?;
        }
        if self.format_version < RELATIONS_VERSION && self.has_relations() {
            return bad(format!(
                "format version {} cannot have editions, dependencies or rules of the load \
                 order; version {RELATIONS_VERSION} is needed",
                self.format_version
            ));
        }
        if self.format_version >= RELATIONS_VERSION {
            self.validate_relations()?;
        }
        // A mod of files alone is a mod too: a replaced sound, a retimed
        // subtitle of a cut-scene.
        if self.resources.is_empty()
            && self.streams.is_empty()
            && self.native.is_none()
            && self.files.is_empty()
        {
            return bad("the mod replaces no resources".into());
        }
        if self.messages.len() > MAX_MESSAGES {
            return bad(format!(
                "{} messages; at most {MAX_MESSAGES} are allowed",
                self.messages.len()
            ));
        }
        let mut hashes: HashMap<u32, &str> = HashMap::new();
        for name in &self.messages {
            check_text("messages", name, MAX_NAME, false)?;
            // Not a message of the game, and not the hash of one: the game
            // could not tell the two apart.
            if let Err(e) = MessageId::custom(&self.id, name) {
                return bad(format!("`messages`: {e}"));
            }
            if let Some(other) = hashes.insert(crate::hash::string_id(name), name)
                && !other.eq_ignore_ascii_case(name)
            {
                return bad(format!(
                    "`messages`: `{name}` and `{other}` have one hash; the game cannot tell them apart"
                ));
            }
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

    /// Checks the fields of format version 3.
    fn validate_relations(&self) -> Result<()> {
        let bad = |why: String| Err(Error::mod_error(format!("mod.json: {why}")));
        version::parse_version(&self.version)
            .map_err(|e| Error::mod_error(format!("mod.json: `version`: {e}")))?;
        let condition = |field: &str, text: &str| {
            version::Condition::parse(text)
                .map(|_| ())
                .map_err(|e| Error::mod_error(format!("mod.json: `{field}`: {e}")))
        };
        if let Some(text) = &self.requires_manager {
            condition("requires_manager", text)?;
        }
        if self.game.editions.len() > MAX_EDITIONS {
            return bad(format!("more than {MAX_EDITIONS} editions"));
        }
        let mut editions = HashSet::new();
        for edition in &self.game.editions {
            if edition.is_empty()
                || edition.len() > MAX_EDITION
                || !edition.bytes().all(|b| b.is_ascii_graphic())
            {
                return bad(format!(
                    "bad edition `{}` in `game.editions`",
                    edition.escape_debug()
                ));
            }
            if !editions.insert(edition.to_ascii_lowercase()) {
                return bad(format!("the edition {edition} is listed twice"));
            }
        }
        let lists: [(&str, Vec<&String>); 5] = [
            ("depends", self.depends.keys().collect()),
            ("recommends", self.recommends.keys().collect()),
            ("incompatible", self.incompatible.keys().collect()),
            ("load_after", self.load_after.iter().collect()),
            ("load_before", self.load_before.iter().collect()),
        ];
        for (field, ids) in &lists {
            if ids.len() > MAX_RELATIONS {
                return bad(format!("`{field}` names more than {MAX_RELATIONS} mods"));
            }
            let mut seen = HashSet::new();
            for id in ids {
                validate_id(id)
                    .map_err(|e| Error::mod_error(format!("mod.json: `{field}`: {e}")))?;
                if **id == self.id {
                    return bad(format!("`{field}` names the mod itself"));
                }
                if !seen.insert(*id) {
                    return bad(format!("`{field}` lists `{id}` twice"));
                }
            }
        }
        for (id, text) in &self.depends {
            condition(&format!("depends.{id}"), text)?;
            if self.incompatible.contains_key(id) {
                return bad(format!("`{id}` is both a dependency and incompatible"));
            }
            if self.load_before.contains(id) {
                return bad(format!(
                    "`{id}` is a dependency, so it is loaded before this mod, not after it \
                     (`load_before`)"
                ));
            }
        }
        for (id, text) in &self.recommends {
            condition(&format!("recommends.{id}"), text)?;
        }
        for (id, incompatible) in &self.incompatible {
            condition(&format!("incompatible.{id}"), &incompatible.versions)?;
            if incompatible.reason.len() > MAX_REASON
                || incompatible
                    .reason
                    .chars()
                    .any(|c| c.is_control() && c != '\n')
            {
                return bad(format!(
                    "the reason of `incompatible.{id}` is longer than {MAX_REASON} bytes or \
                     contains control characters"
                ));
            }
        }
        if let Some(id) = self
            .load_after
            .iter()
            .find(|id| self.load_before.contains(id))
        {
            return bad(format!(
                "`{id}` is both in `load_after` and in `load_before`"
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

    /// Distinct payloads of resources, files and modules of code, each once.
    pub fn payloads(&self) -> Vec<&Blob> {
        let mut seen = HashSet::new();
        self.resources
            .iter()
            .map(|r| &r.payload)
            .chain(self.files.iter().map(|f| &f.payload))
            .chain(
                self.native
                    .iter()
                    .flat_map(|n| n.modules.iter().map(|m| &m.payload)),
            )
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
    manifest.set_relations(&info.relations);
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
            original: match (&c.added, info.relations.made_on.get(&c.key)) {
                (None, Some(base)) => base.clone(),
                _ => c.original.clone(),
            },
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
    // Changed and new entries of the archives (voice lines, replaced
    // sounds): all of them, or those recorded by the operations of the
    // selected resources. Loose files (movies) stay out of mods.
    let labels: HashSet<&str> = selected().map(|c| c.label.as_str()).collect();
    manifest.files = ws
        .file_changes()
        .filter(|c| matches!(c.key, FileKey::Archive(_)))
        .filter(|c| keys.is_none() || labels.contains(c.label.as_str()))
        .map(|c| ModFile {
            key: c.key.clone(),
            payload: c.payload.clone(),
            label: c.label.chars().take(MAX_NAME).collect(),
        })
        .collect();
    if let Some(code) = &info.relations.native {
        manifest.native = Some(ModNative::of_modules(
            &code.modules,
            code.platform_abi,
            code.required,
        )?);
    }
    if manifest.resources.is_empty()
        && manifest.streams.is_empty()
        && manifest.files.is_empty()
        && manifest.native.is_none()
    {
        return Err(Error::mod_error(
            "the workspace has no changes to put into a mod",
        ));
    }
    // The messages the selected SimGroups add; `validate` refuses a name
    // with the hash of a game message.
    manifest.messages = crate::logic_graph::check::check_workspace_keys(ws, |key| {
        keys.is_none_or(|keys| keys.contains(key))
    })
    .custom;
    manifest.format_version = manifest.required_version();
    manifest.validate()?;
    Ok(manifest)
}

/// What a mod built from a workspace needs of the mods the mod manager has
/// written into the game ([`suggested_dependencies`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Suggested {
    /// id → `>=its version` (`*` for a version that is no version number).
    pub depends: BTreeMap<String, String>,
    /// For each replaced resource such a mod changes: its data, which the
    /// change was made on, and the id of the mod.
    pub made_on: BTreeMap<ResourceKey, (Blob, String)>,
}

/// The dependencies of a mod built from the changes of `ws` (all, or those
/// in `keys`) on the mods the mod manager has written into the game: a mod
/// is one when a change replaces a resource it changes (the editor shows
/// the game with the mod, so the change is made on the mod's data) or goes
/// into a stream it creates. The mod `own_id` itself is left out.
pub fn suggested_dependencies(
    game: &GameInstance,
    ws: &Workspace,
    keys: Option<&[ResourceKey]>,
    own_id: &str,
) -> Result<Suggested> {
    let manager = ModManager::open(game)?;
    let written = manager.written_streams();
    let mut out = Suggested::default();
    if written.is_empty() {
        return Ok(out);
    }
    let plan = manager.plan();
    let streams = manager.plan_streams();
    let mut found: Vec<usize> = Vec::new();
    for change in ws
        .changes()
        .filter(|c| keys.is_none_or(|keys| keys.contains(&c.key)))
    {
        if !written.contains(change.key.stream.as_str()) {
            continue;
        }
        if let Some((_, index)) = streams.get(&change.key.stream)
            && manager.mods()[*index].id != own_id
        {
            found.push(*index);
        }
        if let Some((planned, index)) = plan.get(&change.key)
            && manager.mods()[*index].id != own_id
        {
            found.push(*index);
            if change.added.is_none() {
                let id = manager.mods()[*index].id.clone();
                out.made_on
                    .insert(change.key.clone(), (planned.payload.clone(), id));
            }
        }
    }
    for index in found {
        let m = &manager.mods()[index];
        let condition = m.manifest.semver().map_or_else(
            || "*".to_owned(),
            |v| version::Condition::at_least(&v).to_string(),
        );
        out.depends.insert(m.id.clone(), condition);
    }
    Ok(out)
}

/// Adds [`suggested_dependencies`] to the dependencies of `info` that it
/// does not name yet (a mod it names as incompatible stays out), and
/// records the data of the dependencies the changes were made on, so that
/// the mod is checked against them ([`Relations::made_on`]). Returns the ids
/// added.
pub fn add_suggested_dependencies(
    info: &mut ModInfo,
    game: &GameInstance,
    ws: &Workspace,
    keys: Option<&[ResourceKey]>,
) -> Result<Vec<String>> {
    let suggested = suggested_dependencies(game, ws, keys, &info.id)?;
    let mut added = Vec::new();
    for (id, condition) in suggested.depends {
        if !info.relations.depends.contains_key(&id)
            && !info.relations.incompatible.contains_key(&id)
        {
            info.relations.depends.insert(id.clone(), condition);
            added.push(id);
        }
    }
    for (key, (blob, id)) in suggested.made_on {
        if info.relations.depends.contains_key(&id) {
            info.relations.made_on.insert(key, blob);
        }
    }
    Ok(added)
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
    /// Version of the format the mod is written in.
    pub format_version: u32,
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
    /// Modules of executable code.
    pub native_modules: usize,
    /// What the author should know about the mod ([`whole_sound_notes`]).
    pub notes: Vec<String>,
}

/// A sound of the archives from this size on is worth a note when a mod
/// carries it for its subtitles.
const BIG_SOUND: u64 = 1 << 20;

/// Notes on the streamed sounds a mod carries whole. The times of a
/// cut-scene's subtitles are in the header of its sound file, and a mod
/// replaces a file of the archives as one piece: a mod that only retimes a
/// subtitle still carries the whole sound (megabytes). The format has no
/// partial file; the note tells the author where the size comes from.
pub fn whole_sound_notes(ws: &Workspace, manifest: &ModManifest) -> Vec<String> {
    manifest
        .files
        .iter()
        .filter(|f| matches!(f.key, FileKey::Archive(_)) && f.payload.size >= BIG_SOUND)
        .filter(|f| {
            ws.store()
                .get(&f.payload)
                .is_ok_and(|data| crate::resource::tob::Tob::from_snu(&data).is_some())
        })
        .map(|f| {
            format!(
                "{} ({:.1} MB) is in the mod whole: its subtitle table is in the header of the \
                 sound file and a file of the archives is replaced as one piece, also when \
                 only the times of subtitles changed",
                f.key.name(),
                f.payload.size as f64 / 1e6
            )
        })
        .collect()
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
            built_payload(ws, info, blob)
        })?;
    Ok(BuildReport {
        format_version: manifest.format_version,
        resources: manifest.resources.len(),
        added: manifest.added().count(),
        new_streams: manifest.streams.len(),
        payload_bytes: manifest.payload_bytes(),
        file_size,
        native_modules: manifest.native.as_ref().map_or(0, |n| n.modules.len()),
        notes: whole_sound_notes(ws, &manifest),
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

pub(crate) fn check_text(field: &str, value: &str, max: usize, may_be_empty: bool) -> Result<()> {
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

pub(crate) fn is_sha256(text: &str) -> bool {
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

/// Who wrote into the game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOwner {
    ModManager,
    /// The game's default workspace of the editor.
    Workspace,
}

/// Writes into the game that did not finish (see
/// [`Deployment::interrupted`]), of the mod manager and of the game's
/// default workspace: the owner and the streams. "Restore originals" of the
/// owner ([`manager::ModManager::unapply`], [`Workspace::unapply`]) or the
/// editor's restore from the backups puts the originals back.
pub fn interrupted_writes(game: &GameInstance) -> Result<Vec<(WriteOwner, Vec<String>)>> {
    let mut out = Vec::new();
    for (owner, path) in [
        (WriteOwner::ModManager, manager::applied_path(game.root())),
        (
            WriteOwner::Workspace,
            Workspace::default_dir(game.root()).join("applied.json"),
        ),
    ] {
        let streams = Deployment::load(&path)?.interrupted(game)?;
        if !streams.is_empty() {
            out.push((owner, streams));
        }
    }
    Ok(out)
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

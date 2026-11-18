//! The mod manager: installed mods of a game, their order, and writing their
//! combined result into the game.
//!
//! ```text
//! <game>/_doctor_toolkit/mods/
//! ├── registry.json     installed mods in load order, enabled or not
//! ├── applied.json      what the mod manager has written into the game
//! └── <id>/             one installed mod
//!     ├── mod.json
//!     ├── preview.png
//!     └── payloads/ab/ab12…
//! ```
//!
//! Mods are applied in load order: when two enabled mods replace the same
//! resource, the later one wins. Applying rebuilds every affected stream from
//! its original with the winning data of all enabled mods; a stream no
//! enabled mod touches any more is put back to its original. Before writing,
//! every enabled mod is checked against the game: each replaced resource must
//! exist, and its original data must be what the mod was made for.
//!
//! Mods of format version 2 also add resources and create streams. An added
//! resource must not be in its stream yet, and its GUID must not belong to
//! another resource of the game with other data (the game loads one resource
//! per GUID). A new stream must not exist, its hash must be free in every
//! table of contents it goes into, and its parent must be listed there.
//! Between mods, the same new stream defined differently, two new streams
//! with one hash, or one GUID added with different data are [`Clash`]es that
//! stop [`ModManager::apply`]. A created stream leaves the game, with its
//! table-of-contents entries, when no enabled mod creates it any more.
//!
//! Mods of format version 3 name other mods. A dependency (`depends`) must
//! be installed, enabled, of an allowed version and earlier in the load
//! order; the dependent mod is checked against the game with its
//! dependencies applied, so it may change what they changed (a patch on top
//! of a mod). An `incompatible` mod is a [`Clash`]; a mod of an edition of
//! the game the mod was not tested on (`game.editions`) does not fit unless
//! compatibility is ignored. Installing and enabling put a mod in the
//! nearest place its rules allow; [`ModManager::sort`] sorts the whole list
//! ([`super::order`]).
//!
//! Mods of format version 6 may change a resource by a delta instead of
//! replacing it. The data of such a resource is the game's original, or the
//! last whole resource an enabled mod gives it, with the deltas of the
//! enabled mods above applied in load order ([`super::fold`]): the mods
//! merge. A whole resource above a delta still replaces it — that is the
//! conflict it always was; two deltas that set one attribute of one object
//! are a conflict by element ([`Conflict::elements`]), which the later mod
//! wins. An operation whose target is gone is left out and said
//! ([`Problem::OperationsSkipped`]); a delta that adds an element another
//! mod has added stops the later mod ([`Problem::DeltaRefused`]).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use super::fold::{Entry, FoldCache, KeyFold, Originals, Outcome, Step, Want};
use super::lock::{self, RegistryLock};
use super::native::{self, NativeBlock, NativeHost, NativeReport};
use super::order::{self, Rule, Strength};
use super::package::ModArchive;
use super::version::{self, Condition};
use super::vpat::{self, VpatPatch};
use super::{
    MANIFEST_NAME, MAX_PREVIEW_SIZE, ModGame, ModInfo, ModManifest, ModResource, PAYLOAD_DIR,
    ensure_workspace_not_in_game, list_some, mods_dir, validate_id,
};
use crate::game::{GameInstance, StreamFile};
use crate::toc::Toc;
use crate::workspace::{
    ApplyReport, Blob, Change, DeployStatus, Deployment, FileChange, FileKey, GameFingerprint,
    NewStream, PayloadStore, ResourceKey, StreamState, SyncProgress, Workspace, files,
};
use crate::{Error, Result};

pub(super) const REGISTRY_FILE: &str = "registry.json";
const APPLIED_FILE: &str = "applied.json";
const REGISTRY_FORMAT: &str = "doctor-toolkit-mods";
const REGISTRY_VERSION: u32 = 1;
const STAGING_PREFIX: &str = ".staging-";
const OLD_PREFIX: &str = ".old-";
const REMOVED_PREFIX: &str = ".removed-";
const SETS_FILE: &str = "sets.json";
const SETS_FORMAT: &str = "doctor-toolkit-mod-sets";

/// Path of the mod manager's record of what it wrote into the game.
pub fn applied_path(game_root: &Path) -> PathBuf {
    mods_dir(game_root).join(APPLIED_FILE)
}

/// `registry.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Registry {
    format: String,
    version: u32,
    /// In load order: later mods win.
    mods: Vec<RegistryEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RegistryEntry {
    id: String,
    enabled: bool,
    /// Seconds since the Unix epoch.
    #[serde(default)]
    installed: u64,
    /// File name the mod was installed from.
    #[serde(default)]
    source: String,
    /// The folder of the workspace the mod was installed from
    /// ([`InstalledMod::workspace`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    workspace: Option<PathBuf>,
}

/// An installed mod.
#[derive(Debug, Clone)]
pub struct InstalledMod {
    pub id: String,
    pub enabled: bool,
    /// Seconds since the Unix epoch.
    pub installed: u64,
    /// File name the mod was installed from.
    pub source: String,
    /// The folder of the workspace the mod was installed from
    /// ([`ModManager::install_workspace`]), on this computer: the mod is
    /// that workspace's own. Written into the game, it is in the files the
    /// workspace's edits are compared with, and it is not what they stand
    /// on (`super::game_without_mod`, stage 111). `None` for a mod
    /// installed from a file.
    pub workspace: Option<PathBuf>,
    pub manifest: ModManifest,
    /// Why the mod cannot be used, e.g. a damaged `mod.json`. Such a mod can
    /// only be uninstalled.
    pub problem: Option<String>,
    dir: PathBuf,
}

impl InstalledMod {
    /// Directory of the installed mod.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The payloads of the mod.
    pub fn store(&self) -> PayloadStore {
        PayloadStore::new(self.dir.join(PAYLOAD_DIR))
    }

    /// The preview picture, if the mod has one.
    pub fn preview(&self) -> Option<Vec<u8>> {
        let name = self.manifest.preview.as_ref()?;
        let path = self.dir.join(name);
        let len = fs::metadata(&path).ok()?.len();
        if len > MAX_PREVIEW_SIZE {
            return None;
        }
        fs::read(path).ok()
    }
}

/// A resource or a new stream of an enabled mod that does not fit the game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatIssue {
    pub mod_id: String,
    pub subject: Subject,
    pub problem: Problem,
}

/// What a [`CompatIssue`] is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subject {
    /// A replaced or added resource.
    Resource(ResourceKey),
    /// A stream the mod creates.
    Stream(String),
    /// The mod as a whole: its edition, the mod manager it needs, its
    /// dependencies (format version 3).
    Mod,
}

impl fmt::Display for Subject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Resource(key) => write!(f, "{key}"),
            Self::Stream(name) => write!(f, "new stream {name}"),
            Self::Mod => write!(f, "the mod"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// The game has no such stream, and no enabled mod creates it.
    StreamMissing,
    /// The stream has no such resource.
    ResourceMissing,
    /// The game's original data of the resource is not what the mod was made for.
    OriginalDiffers { expected: Blob, found: Blob },
    /// The resource replaces one in a stream another mod creates; only added
    /// resources can be in a new stream.
    InNewStream { creator: String },
    /// The added resource is already in the stream.
    ResourceExists,
    /// The GUID of the added resource belongs to another resource of the game
    /// with other data; the game would load only one of them.
    GuidInUse { other: ResourceKey },
    /// The resource the added one is placed before is not in the stream; it
    /// is added at the end.
    AnchorMissing { before: ResourceKey },
    /// The game already has a stream of this name.
    StreamExists,
    /// The table of contents the new stream goes into is not in the game.
    TocMissing { toc: String },
    /// The parent of the new stream is not listed in the table of contents.
    ParentMissing { toc: String, parent: String },
    /// A stream of the table of contents already has the hash of the new stream.
    StreamHashUsed { toc: String },
    /// The game is not one of the editions the mod was tested on; `game` is
    /// the edition of the game, `None` when it is not known.
    Edition {
        game: Option<String>,
        tested: Vec<String>,
    },
    /// The mod needs a newer mod manager (`requires_manager`).
    ManagerTooOld { required: String },
    /// A dependency is not installed.
    DependencyMissing { id: String, versions: String },
    /// A dependency is installed, but switched off or damaged.
    DependencyDisabled { id: String, versions: String },
    /// A dependency is not of a version the mod allows.
    DependencyVersion {
        id: String,
        versions: String,
        found: String,
    },
    /// A dependency is loaded after the mod; sorting puts it before.
    DependencyLater { id: String },
    /// A mod the author recommends is not enabled (or not of an allowed
    /// version). Only a hint.
    Recommended { id: String, versions: String },
    /// The executable code of the mod (format version 5) is not installed:
    /// why, and whether the mod says it makes no sense without its code
    /// (`required`). With `required` the mod cannot be applied; without, its
    /// data is written and this is only said.
    Native { block: NativeBlock, required: bool },
    /// The delta of the resource (format version 6) cannot be applied: it
    /// adds an element that is there already — an object GUID, a string
    /// hash, the name of a part or a variant, the cell of a stream that a
    /// mod below, or the game, has — or the document, or the data it
    /// applies to, does not read. Addresses are unique, so the later mod
    /// cannot be written.
    DeltaRefused { error: String },
    /// Operations of the delta were left out because what they change is
    /// not there: a mod below removed the object, or the game is another
    /// edition. The other `applied` operations are written. Only said.
    OperationsSkipped {
        /// Each as `<what it addresses>: <why>`.
        skipped: Vec<String>,
        applied: usize,
    },
    /// Operations of the delta set values that were not what the author of
    /// the mod saw, and no mod below set them: the game's own data differs.
    /// The mod's values are written. Only said.
    ValuesDiffer {
        /// Each as `<what it addresses>: <what was there>`.
        elements: Vec<String>,
    },
}

impl Problem {
    /// Whether the mod cannot be applied at all. The others stop applying
    /// unless compatibility is ignored, except notices ([`Self::is_notice`]).
    pub fn is_fatal(&self) -> bool {
        if let Self::Native { block, required } = self {
            return native::is_fatal(block, *required);
        }
        !matches!(
            self,
            Self::OriginalDiffers { .. }
                | Self::AnchorMissing { .. }
                | Self::Edition { .. }
                | Self::DependencyVersion { .. }
                | Self::Recommended { .. }
                | Self::OperationsSkipped { .. }
                | Self::ValuesDiffer { .. }
        )
    }

    /// Whether the problem is only reported and never stops applying.
    pub fn is_notice(&self) -> bool {
        match self {
            Self::Native { .. } => !self.is_fatal(),
            _ => matches!(
                self,
                Self::AnchorMissing { .. }
                    | Self::Recommended { .. }
                    | Self::OperationsSkipped { .. }
                    | Self::ValuesDiffer { .. }
            ),
        }
    }

    /// Whether the problem is about another mod the mod needs (format version 3).
    pub fn is_dependency(&self) -> bool {
        matches!(
            self,
            Self::DependencyMissing { .. }
                | Self::DependencyDisabled { .. }
                | Self::DependencyVersion { .. }
                | Self::DependencyLater { .. }
        )
    }
}

/// `version 1.2` or `any version` for a condition.
fn versions_words(versions: &str) -> String {
    match Condition::parse(versions) {
        Ok(c) if c.is_any() => "any version".to_owned(),
        Ok(c) => format!("version {c}"),
        Err(_) => format!("version {versions}"),
    }
}

impl CompatIssue {
    /// Whether the mod cannot be applied at all.
    pub fn is_fatal(&self) -> bool {
        self.problem.is_fatal()
    }

    pub fn describe(&self) -> String {
        let what = match &self.problem {
            Problem::StreamMissing => "the stream is not in the game".to_owned(),
            Problem::ResourceMissing => "the resource is not in the game".to_owned(),
            Problem::OriginalDiffers { expected, found } => format!(
                "the game's original differs from what the mod expects ({} bytes, expected {})",
                found.size, expected.size
            ),
            Problem::InNewStream { creator } => format!(
                "the stream is created by mod `{creator}`; only added resources can go into it"
            ),
            Problem::ResourceExists => "the added resource is already in the stream".to_owned(),
            Problem::GuidInUse { other } => format!(
                "the GUID is already used by {other} with other data; the game would load only \
                 one of them"
            ),
            Problem::AnchorMissing { before } => {
                format!("{before} is not in the stream; the resource is added at the end")
            }
            Problem::StreamExists => "the game already has a stream of this name".to_owned(),
            Problem::TocMissing { toc } => {
                format!("the table of contents {toc} is not in the game")
            }
            Problem::ParentMissing { toc, parent } => {
                format!("{toc} does not list the parent stream {parent}")
            }
            Problem::StreamHashUsed { toc } => {
                format!("{toc} already has a stream with the same hash")
            }
            Problem::Edition { game, tested } => format!(
                "the mod is tested on the editions {} of the game, this game is {}",
                tested.join(", "),
                game.as_deref().unwrap_or("of an unknown edition")
            ),
            Problem::ManagerTooOld { required } => format!(
                "the mod needs mod manager {required}, this is {}; update Open Mod Manager \
                 (doctor_toolkit)",
                version::manager_version()
            ),
            Problem::DependencyMissing { id, versions } => format!(
                "needs mod `{id}` ({}), which is not installed",
                versions_words(versions)
            ),
            Problem::DependencyDisabled { id, versions } => format!(
                "needs mod `{id}` ({}), which is switched off",
                versions_words(versions)
            ),
            Problem::DependencyVersion {
                id,
                versions,
                found,
            } => format!(
                "needs mod `{id}` of {}, the installed one is {found}",
                versions_words(versions)
            ),
            Problem::DependencyLater { id } => format!(
                "needs mod `{id}`, which is loaded after it; sort the mods or move `{id}` up"
            ),
            Problem::Recommended { id, versions } => {
                format!("works best with mod `{id}` ({})", versions_words(versions))
            }
            Problem::Native {
                block,
                required: true,
            } => format!(
                "{}; the mod does not work without its code, so it is not written at all",
                block.describe()
            ),
            Problem::Native {
                block,
                required: false,
            } => format!("{}; only its data is written", block.describe()),
            Problem::DeltaRefused { error } => format!(
                "its changes cannot be merged into the resource ({error}); disable this mod or \
                 the one that has added the same"
            ),
            Problem::OperationsSkipped { skipped, applied } => format!(
                "{} of its {} change(s) are left out because what they change is not there; \
                 the rest is written: {}",
                skipped.len(),
                skipped.len() + applied,
                list_some(skipped)
            ),
            Problem::ValuesDiffer { elements } => format!(
                "{} value(s) were not what the mod was made on; the mod's values are written: {}",
                elements.len(),
                list_some(elements)
            ),
        };
        match self.subject {
            Subject::Mod => format!("{}: {what}", self.mod_id),
            _ => format!("{}: {}: {what}", self.mod_id, self.subject),
        }
    }
}

/// Mods that cannot both be applied as they are, beyond a resource replaced
/// by several mods ([`Conflict`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Clash {
    /// Several mods create a stream of the same name. Harmless when they
    /// define it the same way (their resources go into the one stream).
    Stream {
        name: String,
        /// Ids of the mods in load order.
        mods: Vec<String>,
        same_definition: bool,
    },
    /// Several mods create different streams with the same table-of-contents hash.
    StreamHash {
        hash: u32,
        /// (mod id, stream name) in load order.
        streams: Vec<(String, String)>,
    },
    /// Several mods add resources with the same GUID under different keys.
    /// Harmless with the same data: the game shares resources by GUID.
    Guid {
        ids: [u32; 4],
        /// (mod id, key) in load order.
        resources: Vec<(String, ResourceKey)>,
        same_data: bool,
    },
    /// Mods add resources to a stream of the game that other mods replace
    /// resources of. Both are written; reported so that an author can check
    /// that the added resources still fit the replaced ones.
    SharedStream {
        stream: String,
        adding: Vec<String>,
        replacing: Vec<String>,
    },
    /// A mod says it does not work with another (`incompatible`, format
    /// version 3), and that mod is of a version it names.
    Incompatible {
        /// The mod whose manifest says so.
        declared_by: String,
        other: String,
        /// Condition on the version of `other`.
        versions: String,
        /// Why, in the words of the author; may be empty.
        reason: String,
    },
}

impl Clash {
    /// Whether the mods cannot be applied together.
    pub fn is_fatal(&self) -> bool {
        match self {
            Self::Stream {
                same_definition, ..
            } => !same_definition,
            Self::StreamHash { .. } | Self::Incompatible { .. } => true,
            Self::Guid { same_data, .. } => !same_data,
            Self::SharedStream { .. } => false,
        }
    }

    /// Ids of the mods involved, in load order.
    pub fn mods(&self) -> Vec<&str> {
        let mut mods: Vec<&str> = match self {
            Self::Stream { mods, .. } => mods.iter().map(String::as_str).collect(),
            Self::StreamHash { streams, .. } => streams.iter().map(|(m, _)| m.as_str()).collect(),
            Self::Guid { resources, .. } => resources.iter().map(|(m, _)| m.as_str()).collect(),
            Self::SharedStream {
                adding, replacing, ..
            } => adding.iter().chain(replacing).map(String::as_str).collect(),
            Self::Incompatible {
                declared_by, other, ..
            } => vec![declared_by.as_str(), other.as_str()],
        };
        let mut seen = BTreeSet::new();
        mods.retain(|m| seen.insert(*m));
        mods
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Stream {
                name,
                mods,
                same_definition: true,
            } => format!(
                "new stream {name} is created by {} in the same way; their resources go into it",
                mods.join(", ")
            ),
            Self::Stream { name, mods, .. } => format!(
                "new stream {name} is created differently by {}; enable only one of them",
                mods.join(", ")
            ),
            Self::StreamHash { hash, streams } => format!(
                "new streams {} have the same hash {hash:08x}; enable only one of them",
                streams
                    .iter()
                    .map(|(m, s)| format!("{s} ({m})"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Guid {
                ids,
                resources,
                same_data,
            } => {
                let list = resources
                    .iter()
                    .map(|(m, k)| format!("{k} ({m})"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let guid = ids.map(|i| format!("{i:08x}")).join(".");
                if *same_data {
                    format!("{list} add GUID {guid} with the same data; the game shares it")
                } else {
                    format!(
                        "{list} add GUID {guid} with different data; the game would load only \
                         one of them: enable only one of the mods"
                    )
                }
            }
            Self::SharedStream {
                stream,
                adding,
                replacing,
            } => format!(
                "{} add resources to {stream}, which {} change",
                adding.join(", "),
                replacing.join(", ")
            ),
            Self::Incompatible {
                declared_by,
                other,
                versions,
                reason,
            } => {
                let mut text = format!(
                    "{declared_by} does not work with {other} ({}); enable only one of them",
                    versions_words(versions)
                );
                if !reason.is_empty() {
                    text += &format!(": {reason}");
                }
                text
            }
        }
    }
}

/// Two or more mods replacing the same resource, or setting the same
/// elements of it by deltas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub key: ResourceKey,
    /// Ids of the mods in load order; the last one wins.
    pub mods: Vec<String>,
    /// Whether all of them have the same data (a harmless overlap).
    pub same_data: bool,
    /// Whether the winner depends on all the others: a patch on top of
    /// them, which is meant to win (format version 3).
    pub patch: bool,
    /// Empty when the last mod replaces the resource whole. Otherwise the
    /// mods merge (format version 6) and the conflict is only these
    /// elements, which the delta of the last mod set over a value another
    /// mod gave them: each as `<object and attribute>: <what was there>`.
    pub elements: Vec<String>,
}

impl Conflict {
    /// The mod whose data is used.
    pub fn winner(&self) -> &str {
        self.mods.last().map_or("", String::as_str)
    }
}

/// Result of an installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallReport {
    pub id: String,
    pub name: String,
    /// Version of the mod this one replaced, when it updated an installed mod.
    pub replaced_version: Option<String>,
    /// Replaced and added resources.
    pub resources: usize,
    /// Added resources among them.
    pub added: usize,
    /// Streams the mod creates.
    pub new_streams: usize,
    pub warnings: Vec<String>,
    /// How the new mod fits the game.
    pub issues: Vec<CompatIssue>,
}

/// How the data of a workspace gets into the folder of the mod installed
/// from it ([`ModManager::install_workspace_with`]). Either way a payload
/// the installed state of the mod already has is taken from there (a hard
/// link: the files are named by the hash of their contents and never
/// rewritten), so installing a project again costs what changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Payloads {
    /// Copied: the mod is whole without the workspace. For the mod manager
    /// of the game.
    #[default]
    Copied,
    /// Symbolic links to the payload files of the workspace: nothing is
    /// copied, and the mod is good as long as the workspace keeps those
    /// files. For the mod manager of a staged copy of the game
    /// ([`crate::play::overlay`]), which lasts one launch — never for the
    /// game's own.
    Linked,
}

/// Gives the payload `blob` of a mod being installed the file `from` of
/// another store instead of a copy of its data: a symbolic link to it
/// (`symbolic`) or a second name of the same file. Whether it did; a
/// failure (no such file, another size, another file system, no right to
/// make links) leaves the payload to be copied.
fn take_payload(from: &Path, to: &Path, blob: &Blob, symbolic: bool) -> bool {
    if !fs::metadata(from).is_ok_and(|m| m.is_file() && m.len() == blob.size) {
        return false;
    }
    if fs::symlink_metadata(to).is_ok() {
        return true;
    }
    let Some(parent) = to.parent() else {
        return false;
    };
    if fs::create_dir_all(parent).is_err() {
        return false;
    }
    if !symbolic {
        return fs::hard_link(from, to).is_ok();
    }
    let Ok(from) = std::path::absolute(from) else {
        return false;
    };
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(from, to).is_ok()
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(from, to).is_ok()
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = from;
        false
    }
}

/// Options for [`ModManager::apply`].
#[derive(Debug, Clone, Copy, Default)]
pub struct ModApplyOptions {
    /// Overwrite streams changed by another program; their changes are lost.
    pub force: bool,
    /// Apply mods although the game's originals differ from what they expect.
    pub ignore_compatibility: bool,
}

/// The installed mods of one game.
#[derive(Debug)]
pub struct ModManager {
    root: PathBuf,
    dir: PathBuf,
    mods: Vec<InstalledMod>,
    deployment: Deployment,
    /// What installing executable code depends on; looked up when first
    /// needed and after each write.
    native_host: OnceLock<NativeHost>,
    /// The host is given ([`Self::set_native_host`]) and never looked up.
    native_host_fixed: bool,
    /// What the last write or removal did with the code of the mods.
    native_report: NativeReport,
    /// The lock of the folder while a change is being made ([`Self::changing`]).
    lock: Option<RegistryLock>,
    /// The files of the folder as this manager knows them.
    seen: Seen,
    /// The folds of the deltas of the mods ([`super::fold`]).
    folds: FoldCache,
}

/// The combined changes of the enabled mods ([`ModManager::plan`]) with
/// what folding the deltas told.
#[derive(Debug, Clone, Default)]
pub(super) struct FoldedPlan {
    /// For each resource its change and the index of the last mod in it.
    pub changes: BTreeMap<ResourceKey, (Change, usize)>,
    /// The resources whose data is a fold, of no single mod: the indices of
    /// the mods in it, in load order — the mod of the whole resource the
    /// fold starts from, if one, and the mods whose deltas applied.
    pub folded: BTreeMap<ResourceKey, Vec<usize>>,
    /// The steps of the folds the plan stands on.
    steps: HashSet<String>,
}

/// The entries of the mods for one resource, each with the index of its
/// mod, in load order.
type Chain<'a> = Vec<(usize, &'a ModResource)>;

/// The registry and the record of what is written into the game as a mod
/// manager read or last wrote them (the SHA-256 of each; `None`: no such
/// file). When they are something else on disk, another program changed
/// the mods, and what this manager has in memory is old.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Seen {
    registry: Option<String>,
    applied: Option<String>,
}

impl Seen {
    fn of(dir: &Path) -> Self {
        let hash = |name: &str| {
            fs::read(dir.join(name))
                .ok()
                .map(|data| crate::workspace::store::sha256_hex(&data))
        };
        Self {
            registry: hash(REGISTRY_FILE),
            applied: hash(APPLIED_FILE),
        }
    }
}

impl ModManager {
    /// Opens the mod manager of `game`. Nothing is written until a mod is
    /// installed or applied.
    pub fn open(game: &GameInstance) -> Result<Self> {
        let root = game.root().to_owned();
        let dir = mods_dir(&root);
        // Before the files are read: a change in between is then found
        // later, not missed.
        let seen = Seen::of(&dir);
        let (mods, deployment) = Self::read(&dir)?;
        Ok(Self {
            root,
            folds: FoldCache::new(&dir),
            dir,
            mods,
            deployment,
            native_host: OnceLock::new(),
            native_host_fixed: false,
            native_report: NativeReport::default(),
            lock: None,
            seen,
        })
    }

    /// The installed mods in load order and what is written into the game,
    /// as the folder `dir` has them.
    fn read(dir: &Path) -> Result<(Vec<InstalledMod>, Deployment)> {
        let registry_path = dir.join(REGISTRY_FILE);
        let registry = if registry_path.is_file() {
            let text = fs::read(&registry_path).map_err(|e| Error::io(&registry_path, e))?;
            let registry: Registry = serde_json::from_slice(&text)
                .map_err(|e| Error::mod_error(format!("{}: {e}", registry_path.display())))?;
            if registry.format != REGISTRY_FORMAT || registry.version > REGISTRY_VERSION {
                return Err(Error::mod_error(format!(
                    "{} was written by a newer program or is not a mod registry",
                    registry_path.display()
                )));
            }
            registry
        } else {
            Registry {
                format: REGISTRY_FORMAT.to_owned(),
                version: REGISTRY_VERSION,
                mods: Vec::new(),
            }
        };
        let mut mods = Vec::new();
        let mut seen = BTreeSet::new();
        for entry in registry.mods {
            if validate_id(&entry.id).is_err() || !seen.insert(entry.id.clone()) {
                continue;
            }
            mods.push(load_installed(dir, entry));
        }
        let deployment = Deployment::load(&dir.join(APPLIED_FILE))?;
        Ok((mods, deployment))
    }

    /// Reads the mods again when another program changed them since this
    /// manager read or wrote them (it installed a mod, changed the order,
    /// wrote into the game). Returns whether it did. A window that keeps a
    /// manager calls this before it shows the mods; every change of the
    /// manager does it by itself ([`Self::changing`]).
    pub fn reload_if_changed(&mut self) -> Result<bool> {
        let seen = Seen::of(&self.dir);
        if seen == self.seen {
            return Ok(false);
        }
        let (mods, deployment) = Self::read(&self.dir)?;
        self.mods = mods;
        self.deployment = deployment;
        self.seen = seen;
        self.forget_native_host();
        self.folds.forget();
        Ok(true)
    }

    /// Makes a change of the mods — of the registry, of a mod's folder, of
    /// the game files — as the only program doing so ([`RegistryLock`]):
    /// refused, with the holder named, while another program makes one. The
    /// mods are read again first when another program changed them since
    /// ([`Self::reload_if_changed`]), so that the change is made to what is
    /// there now and loses nothing. A change made of other changes holds
    /// the lock once.
    fn changing<T>(&mut self, change: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        if self.lock.is_some() {
            return change(self);
        }
        self.lock = Some(RegistryLock::acquire(&self.dir, &lock::program_name())?);
        let result = self.reload_if_changed().and_then(|_| change(self));
        if result.is_ok() {
            // Nobody else wrote meanwhile: the files are what this manager
            // has. After a failure they are compared again by the next
            // change, which reads them when a part was written.
            self.seen = Seen::of(&self.dir);
        }
        self.lock = None;
        result
    }

    /// The lock for a change that needs no more than `&self` (the saved
    /// sets); nothing when a change of this manager holds it already.
    fn hold(&self) -> Result<Option<RegistryLock>> {
        match self.lock {
            Some(_) => Ok(None),
            None => RegistryLock::acquire(&self.dir, &lock::program_name()).map(Some),
        }
    }

    /// The game and the files of the toolkit, as installing executable code
    /// sees them ([`NativeHost::detect`]).
    pub fn native_host(&self) -> &NativeHost {
        self.native_host
            .get_or_init(|| NativeHost::detect(&self.root))
    }

    /// Gives the host instead of looking at the game and next to the
    /// program: for tests and for a caller that knows better.
    pub fn set_native_host(&mut self, host: NativeHost) {
        self.native_host = OnceLock::from(host);
        self.native_host_fixed = true;
    }

    /// The game folder changed: the host is looked up anew.
    fn forget_native_host(&mut self) {
        if !self.native_host_fixed {
            self.native_host = OnceLock::new();
        }
    }

    /// What the last [`Self::apply`], [`Self::unapply`] or
    /// [`Self::sync_native`] did with the executable code of the mods.
    pub fn native_report(&self) -> &NativeReport {
        &self.native_report
    }

    /// Brings the executable code in the game folder in line with the
    /// enabled mods and the player's answers ([`native::sync`]) without
    /// writing data: for a launch that does not write the mods into the
    /// game. Never fails: what went wrong is in the report.
    pub fn sync_native(&mut self) -> &NativeReport {
        self.native_report =
            native::sync(self, self.native_host()).unwrap_or_else(|e| NativeReport {
                warnings: vec![format!("native code: {e}")],
                ..NativeReport::default()
            });
        self.forget_native_host();
        &self.native_report
    }

    /// The mod manager directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Installed mods in load order.
    pub fn mods(&self) -> &[InstalledMod] {
        &self.mods
    }

    pub fn get(&self, id: &str) -> Option<&InstalledMod> {
        self.mods.iter().find(|m| m.id == id)
    }

    fn position(&self, id: &str) -> Result<usize> {
        self.mods
            .iter()
            .position(|m| m.id == id)
            .ok_or_else(|| Error::mod_error(format!("no mod `{id}` is installed")))
    }

    fn save_registry(&self) -> Result<()> {
        let registry = Registry {
            format: REGISTRY_FORMAT.to_owned(),
            version: REGISTRY_VERSION,
            mods: self
                .mods
                .iter()
                .map(|m| RegistryEntry {
                    id: m.id.clone(),
                    enabled: m.enabled,
                    installed: m.installed,
                    source: m.source.clone(),
                    workspace: m.workspace.clone(),
                })
                .collect(),
        };
        fs::create_dir_all(&self.dir).map_err(|e| Error::io(&self.dir, e))?;
        let mut text = serde_json::to_vec_pretty(&registry)
            .map_err(|e| Error::mod_error(format!("registry: {e}")))?;
        text.push(b'\n');
        crate::atomic::write(&self.dir.join(REGISTRY_FILE), &text)?;
        // Code of a mod that is switched off, removed or replaced by a
        // build nobody agreed to leaves the game at once, not at the next
        // write. What cannot be removed now (the game is running) is said
        // by the state of the mod and removed the next time.
        let _ = native::prune(self);
        Ok(())
    }

    // ---------------------------------------------------------- installation

    /// Installs a mod package (`.zip`) or an old toolkit patch (`.vpat`).
    /// A mod with the id of an installed one replaces it and keeps its place
    /// and its enabled state; a new mod goes last, enabled when `enable`.
    /// The game is not changed until [`Self::apply`].
    pub fn install_file(
        &mut self,
        game: &GameInstance,
        path: &Path,
        enable: bool,
    ) -> Result<InstallReport> {
        self.changing(|manager| manager.install_file_locked(game, path, enable))
    }

    /// [`Self::install_file`], with the lock held.
    fn install_file_locked(
        &mut self,
        game: &GameInstance,
        path: &Path,
        enable: bool,
    ) -> Result<InstallReport> {
        let mut head = [0u8; 4];
        {
            use std::io::Read;
            let mut file = fs::File::open(path).map_err(|e| Error::io(path, e))?;
            let n = file.read(&mut head).map_err(|e| Error::io(path, e))?;
            if n < 4 {
                return Err(Error::mod_error(format!(
                    "{} is not a mod: the file is too short",
                    path.display()
                )));
            }
        }
        let source = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let staging = self.new_staging()?;
        let result = if vpat::is_vpat(&head) {
            self.stage_vpat(game, path, &staging)
        } else if head.starts_with(b"PK") {
            self.stage_zip(game, path, &staging)
        } else {
            Err(Error::mod_error(format!(
                "{} is neither a mod (.zip) nor an old toolkit patch (.vpat)",
                path.display()
            )))
        };
        match result {
            Ok((manifest, warnings)) => {
                self.commit_staged(game, &staging, manifest, source, warnings, enable)
            }
            Err(error) => {
                let _ = fs::remove_dir_all(&staging);
                Err(error)
            }
        }
    }

    /// Installs the changes of a workspace as a mod (all of them, or those in
    /// `keys`). This is how an author tests a workspace together with other
    /// mods. The installed mod is enabled and moved to the end of the load
    /// order, so it wins over the others.
    pub fn install_workspace(
        &mut self,
        game: &GameInstance,
        ws: &Workspace,
        info: &ModInfo,
        keys: Option<&[ResourceKey]>,
    ) -> Result<InstallReport> {
        self.install_workspace_with(game, ws, info, keys, Payloads::Copied)
    }

    /// [`Self::install_workspace`], saying how the data gets into the
    /// folder of the mod ([`Payloads`]).
    pub fn install_workspace_with(
        &mut self,
        game: &GameInstance,
        ws: &Workspace,
        info: &ModInfo,
        keys: Option<&[ResourceKey]>,
        payloads: Payloads,
    ) -> Result<InstallReport> {
        self.changing(|manager| manager.install_workspace_locked(game, ws, info, keys, payloads))
    }

    /// [`Self::install_workspace_with`], with the lock held.
    fn install_workspace_locked(
        &mut self,
        game: &GameInstance,
        ws: &Workspace,
        info: &ModInfo,
        keys: Option<&[ResourceKey]>,
        payloads: Payloads,
    ) -> Result<InstallReport> {
        let built = super::mod_from_workspace(ws, info, keys)?;
        let manifest = built.manifest.clone();
        let staging = self.new_staging()?;
        // The state of this mod installed before: what it has is not
        // copied again.
        let installed = self
            .get(&manifest.id)
            .filter(|m| m.problem.is_none())
            .map(InstalledMod::store);
        let result = (|| {
            let store = PayloadStore::new(staging.join(PAYLOAD_DIR));
            for blob in manifest.payloads() {
                let to = store.path(&blob.sha256);
                let sources = [
                    (payloads == Payloads::Linked).then(|| (ws.store().path(&blob.sha256), true)),
                    installed
                        .as_ref()
                        .map(|store| (store.path(&blob.sha256), false)),
                ];
                if sources
                    .into_iter()
                    .flatten()
                    .any(|(from, symbolic)| take_payload(&from, &to, blob, symbolic))
                {
                    continue;
                }
                let data = built.payload(ws, info, blob)?;
                store.put(&data)?;
            }
            write_manifest(&staging, &manifest)?;
            Ok(())
        })();
        if let Err(error) = result {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
        let id = manifest.id.clone();
        // Not silently: a mod of changes made on other data than it is
        // built on takes back what the mods below changed since.
        let warnings = built.stale_note().into_iter().collect();
        let report =
            self.commit_staged(game, &staging, manifest, "workspace".into(), warnings, true)?;
        let index = self.position(&id)?;
        let mut m = self.mods.remove(index);
        m.enabled = true;
        m.workspace = Some(super::workspace_folder(ws));
        self.mods.push(m);
        self.place(&id);
        self.save_registry()?;
        Ok(report)
    }

    fn new_staging(&self) -> Result<PathBuf> {
        fs::create_dir_all(&self.dir).map_err(|e| Error::io(&self.dir, e))?;
        // Leftovers of an interrupted installation.
        if let Ok(entries) = fs::read_dir(&self.dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if (name.starts_with(STAGING_PREFIX) || name.starts_with(OLD_PREFIX))
                    && entry.file_type().is_ok_and(|t| t.is_dir())
                {
                    let _ = fs::remove_dir_all(entry.path());
                }
            }
        }
        let staging = self
            .dir
            .join(format!("{STAGING_PREFIX}{}", std::process::id()));
        fs::create_dir_all(&staging).map_err(|e| Error::io(&staging, e))?;
        Ok(staging)
    }

    fn stage_zip(
        &self,
        game: &GameInstance,
        path: &Path,
        staging: &Path,
    ) -> Result<(ModManifest, Vec<String>)> {
        let mut archive = ModArchive::open(path)?;
        let mut manifest = archive.manifest().clone();
        check_profile(game, &manifest.game)?;
        let mut warnings = archive.warnings().to_vec();
        if let Some(note) = executable_note(game, &manifest.game)? {
            warnings.push(note);
        }
        let store = PayloadStore::new(staging.join(PAYLOAD_DIR));
        let blobs: Vec<Blob> = manifest.payloads().into_iter().cloned().collect();
        let modules: HashMap<&str, &str> = manifest
            .native
            .iter()
            .flat_map(|n| &n.modules)
            .map(|m| (m.payload.sha256.as_str(), m.file.as_str()))
            .collect();
        let deltas: HashMap<&str, &ModResource> = manifest
            .deltas()
            .map(|r| (r.payload.sha256.as_str(), r))
            .collect();
        for blob in &blobs {
            // Checked against its hash by the archive.
            let data = archive.read_payload(blob)?;
            if let Some(resource) = deltas.get(blob.sha256.as_str()) {
                super::read_delta(resource, &data)
                    .map_err(|e| Error::mod_error(format!("{}: {e}", path.display())))?;
            }
            if let Some(file) = modules.get(blob.sha256.as_str()) {
                native::check_module(&data).map_err(|why| {
                    Error::mod_error(format!("{}: native module `{file}`: {why}", path.display()))
                })?;
            }
            store.put(&data)?;
        }
        if let Some(native) = &manifest.native {
            warnings.push(format!(
                "the mod contains executable code ({}): it is installed into the game only \
                 after you agree to it",
                native
                    .modules
                    .iter()
                    .map(|m| m.file.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if let Some(picture) = archive.read_preview()? {
            let name = manifest.preview.clone().expect("a preview has a name");
            crate::atomic::write(&staging.join(name), &picture)?;
        } else {
            manifest.preview = None;
        }
        write_manifest(staging, &manifest)?;
        Ok((manifest, warnings))
    }

    fn stage_vpat(
        &self,
        game: &GameInstance,
        path: &Path,
        staging: &Path,
    ) -> Result<(ModManifest, Vec<String>)> {
        let patch = VpatPatch::open(path)?;
        let store = PayloadStore::new(staging.join(PAYLOAD_DIR));
        let (manifest, warnings) = patch.to_mod(game, &store)?;
        write_manifest(staging, &manifest)?;
        Ok((manifest, warnings))
    }

    /// Moves a staged mod into place and records it in the registry.
    fn commit_staged(
        &mut self,
        game: &GameInstance,
        staging: &Path,
        manifest: ModManifest,
        source: String,
        warnings: Vec<String>,
        enable: bool,
    ) -> Result<InstallReport> {
        let id = manifest.id.clone();
        let target = self.dir.join(&id);
        let existing = self.mods.iter().position(|m| m.id == id);
        let replaced_version = existing.map(|i| self.mods[i].manifest.version.clone());
        if target.exists() {
            let old = self
                .dir
                .join(format!("{OLD_PREFIX}{id}-{}", std::process::id()));
            fs::rename(&target, &old).map_err(|e| Error::io(&target, e))?;
            if let Err(error) = fs::rename(staging, &target) {
                let _ = fs::rename(&old, &target);
                let _ = fs::remove_dir_all(staging);
                return Err(Error::io(&target, error));
            }
            let _ = fs::remove_dir_all(&old);
        } else {
            fs::rename(staging, &target).map_err(|e| Error::io(&target, e))?;
        }
        let installed = InstalledMod {
            id: id.clone(),
            enabled: existing.map_or(enable, |i| self.mods[i].enabled),
            installed: now(),
            source,
            workspace: None,
            manifest,
            problem: None,
            dir: target,
        };
        match existing {
            Some(i) => self.mods[i] = installed,
            None => self.mods.push(installed),
        }
        self.place(&id);
        self.save_registry()?;
        let index = self.position(&id)?;
        let issues = self.check_mods(game, &[index])?;
        let m = &self.mods[index];
        Ok(InstallReport {
            id,
            name: m.manifest.name.clone(),
            replaced_version,
            resources: m.manifest.resources.len(),
            added: m.manifest.added().count(),
            new_streams: m.manifest.streams.len(),
            warnings,
            issues,
        })
    }

    /// Writes an installed mod as a package, e.g. a converted `.vpat` patch.
    /// Returns the size of the file.
    pub fn export(&self, id: &str, out: &Path) -> Result<u64> {
        let m = &self.mods[self.position(id)?];
        if let Some(problem) = &m.problem {
            return Err(Error::mod_error(format!(
                "mod `{id}` is damaged: {problem}"
            )));
        }
        let preview = m.preview();
        let mut manifest = m.manifest.clone();
        if preview.is_none() {
            manifest.preview = None;
        }
        let store = m.store();
        super::package::write_package(out, &manifest, preview.as_deref(), |blob| store.get(blob))
    }

    /// Removes a mod. The game keeps its data until [`Self::apply`].
    pub fn uninstall(&mut self, id: &str) -> Result<()> {
        self.changing(|manager| manager.uninstall_locked(id))
    }

    /// [`Self::uninstall`], with the lock held.
    fn uninstall_locked(&mut self, id: &str) -> Result<()> {
        let index = self.position(id)?;
        let removed = self.mods.remove(index);
        self.save_registry()?;
        if removed.dir.is_dir() {
            fs::remove_dir_all(&removed.dir).map_err(|e| Error::io(&removed.dir, e))?;
        }
        Ok(())
    }

    /// Switches a mod on or off. A mod switched on moves to the nearest
    /// place its rules of the load order allow. Its dependencies are not
    /// switched on ([`Self::enable_with_dependencies`] does that), and the
    /// mods that need a mod switched off are not switched off
    /// ([`Self::dependents`] lists them).
    pub fn set_enabled(&mut self, id: &str, enabled: bool) -> Result<()> {
        self.changing(|manager| manager.set_enabled_locked(id, enabled))
    }

    /// [`Self::set_enabled`], with the lock held.
    fn set_enabled_locked(&mut self, id: &str, enabled: bool) -> Result<()> {
        let index = self.position(id)?;
        if enabled && let Some(problem) = &self.mods[index].problem {
            return Err(Error::mod_error(format!(
                "mod `{id}` cannot be enabled: {problem}"
            )));
        }
        self.mods[index].enabled = enabled;
        if enabled {
            self.place(id);
        }
        self.save_registry()
    }

    /// Switches on mod `id` and every installed dependency of it that is
    /// off; returns the ids of those dependencies.
    pub fn enable_with_dependencies(&mut self, id: &str) -> Result<Vec<String>> {
        self.changing(|manager| manager.enable_with_dependencies_locked(id))
    }

    /// [`Self::enable_with_dependencies`], with the lock held.
    fn enable_with_dependencies_locked(&mut self, id: &str) -> Result<Vec<String>> {
        let deps = self.disabled_dependencies(id);
        for dep in &deps {
            self.set_enabled(dep, true)?;
        }
        self.set_enabled(id, true)?;
        Ok(deps)
    }

    /// Moves a mod to `position` in the load order (0 is applied first,
    /// the last one wins). Refused when the move breaks a dependency (a
    /// mod would be loaded before a mod it needs); soft rules
    /// (`load_after`, `load_before`) the move breaks are returned as
    /// warnings.
    pub fn move_mod(&mut self, id: &str, position: usize) -> Result<Vec<String>> {
        self.changing(|manager| manager.move_mod_locked(id, position))
    }

    /// [`Self::move_mod`], with the lock held.
    fn move_mod_locked(&mut self, id: &str, position: usize) -> Result<Vec<String>> {
        self.position(id)?;
        let rules = self.rules();
        let ids: Vec<&str> = self.mods.iter().map(|m| m.id.as_str()).collect();
        let before = order::broken(&ids, &rules);
        let broken: Vec<&Rule> = order::broken_by_move(&ids, &rules, id, position)
            .into_iter()
            .filter(|r| !before.contains(r))
            .collect();
        if let Some(rule) = broken.iter().find(|r| r.strength == Strength::Hard) {
            return Err(Error::mod_error(format!(
                "cannot move `{id}` there: {}",
                rule.describe()
            )));
        }
        let warnings = broken.iter().map(|r| r.describe()).collect();
        self.move_to(id, position)?;
        self.save_registry()?;
        Ok(warnings)
    }

    /// Moves a mod without checking the rules.
    fn move_to(&mut self, id: &str, position: usize) -> Result<()> {
        let index = self.position(id)?;
        let item = self.mods.remove(index);
        let position = position.min(self.mods.len());
        self.mods.insert(position, item);
        Ok(())
    }

    // ------------------------------------------- relations and load order

    /// The rules of the load order among the installed mods ([`order::rules`]).
    pub fn rules(&self) -> Vec<Rule> {
        let mods: Vec<(&str, &ModManifest)> = self
            .mods
            .iter()
            .map(|m| (m.id.as_str(), &m.manifest))
            .collect();
        order::rules(&mods)
    }

    /// Moves mod `id` to the nearest place its rules allow ([`order::place`]).
    /// The registry is not saved.
    fn place(&mut self, id: &str) {
        let rules = self.rules();
        let ids: Vec<&str> = self.mods.iter().map(|m| m.id.as_str()).collect();
        if let Some(to) = order::place(&ids, &rules, id) {
            let _ = self.move_to(id, to);
        }
    }

    /// Sorts the load order by the rules of the mods, keeping the present
    /// order where the rules allow ([`order::sort`]); with `dry_run`, only
    /// tells what the order would be. A circle of dependencies is an error.
    pub fn sort(&mut self, dry_run: bool) -> Result<SortReport> {
        if dry_run {
            // Nothing is written.
            return self.sort_locked(true);
        }
        self.changing(|manager| manager.sort_locked(false))
    }

    /// [`Self::sort`], with the lock held.
    fn sort_locked(&mut self, dry_run: bool) -> Result<SortReport> {
        let rules = self.rules();
        let ids: Vec<&str> = self.mods.iter().map(|m| m.id.as_str()).collect();
        let sorted = order::sort(&ids, &rules).map_err(|cycle| {
            Error::mod_error(format!(
                "the mods {} need each other in a circle, so no order loads each after the \
                 mods it needs; disable or uninstall one of them",
                cycle.join(" → ")
            ))
        })?;
        let changed = sorted
            .order
            .iter()
            .map(String::as_str)
            .ne(ids.iter().copied());
        let report = SortReport {
            changed,
            dropped: sorted.dropped.iter().map(Rule::describe).collect(),
            order: sorted.order,
        };
        if changed && !dry_run {
            let mut rest = std::mem::take(&mut self.mods);
            for id in &report.order {
                if let Some(i) = rest.iter().position(|m| m.id == *id) {
                    self.mods.push(rest.remove(i));
                }
            }
            self.mods.append(&mut rest);
            self.save_registry()?;
        }
        Ok(report)
    }

    /// Whether mod `a` needs mod `b`, directly or through other mods.
    pub fn depends_on(&self, a: &str, b: &str) -> bool {
        self.dependencies(a).iter().any(|d| d == b)
    }

    /// The installed mods mod `id` needs, directly or through other mods,
    /// nearest first.
    pub fn dependencies(&self, id: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut queue = std::collections::VecDeque::from([id.to_owned()]);
        while let Some(next) = queue.pop_front() {
            let Some(m) = self.get(&next) else { continue };
            for dep in m.manifest.depends.keys() {
                if dep != id && self.get(dep).is_some() && !out.contains(dep) {
                    out.push(dep.clone());
                    queue.push_back(dep.clone());
                }
            }
        }
        out
    }

    /// Installed dependencies of mod `id` (also indirect ones) that are
    /// switched off, in load order; damaged ones are left out.
    pub fn disabled_dependencies(&self, id: &str) -> Vec<String> {
        let deps = self.dependencies(id);
        self.mods
            .iter()
            .filter(|m| !m.enabled && m.problem.is_none() && deps.contains(&m.id))
            .map(|m| m.id.clone())
            .collect()
    }

    /// The enabled mods that need mod `id`, directly or through other mods,
    /// in load order: switching it off or removing it leaves them without it.
    pub fn dependents(&self, id: &str) -> Vec<String> {
        self.mods
            .iter()
            .filter(|m| m.enabled && m.id != id && self.depends_on(&m.id, id))
            .map(|m| m.id.clone())
            .collect()
    }

    /// What mod `id` says about other mods, each with its state now, and
    /// the installed mods that need it.
    pub fn relations(&self, id: &str) -> Vec<Relation> {
        let Some(m) = self.get(id) else {
            return Vec::new();
        };
        let at = |x: &str| self.mods.iter().position(|m| m.id == x);
        let me = at(id);
        let fits = |other: &InstalledMod, versions: &str| {
            Condition::parse(versions)
                .unwrap_or_else(|_| Condition::any())
                .matches_text(&other.manifest.version)
        };
        let mut out = Vec::new();
        let mut push = |kind, other: &str, versions: &str, reason: &str, state| {
            out.push(Relation {
                kind,
                id: other.to_owned(),
                name: self.get(other).map(|o| o.manifest.name.clone()),
                versions: versions.to_owned(),
                reason: reason.to_owned(),
                state,
            });
        };
        for (dep, versions) in &m.manifest.depends {
            let state = match self.get(dep) {
                None => RelationState::Missing,
                Some(d) if !d.enabled || d.problem.is_some() => RelationState::Off,
                Some(d) if !fits(d, versions) => RelationState::Version(d.manifest.version.clone()),
                Some(_) if at(dep) > me => RelationState::OutOfOrder,
                Some(_) => RelationState::Fine,
            };
            push(RelationKind::Depends, dep, versions, "", state);
        }
        for (rec, versions) in &m.manifest.recommends {
            let state = match self.get(rec) {
                None => RelationState::Missing,
                Some(r) if !r.enabled || r.problem.is_some() => RelationState::Off,
                Some(r) if !fits(r, versions) => RelationState::Version(r.manifest.version.clone()),
                Some(_) => RelationState::Fine,
            };
            push(RelationKind::Recommends, rec, versions, "", state);
        }
        for (other, inc) in &m.manifest.incompatible {
            let state = match self.get(other) {
                None => RelationState::Missing,
                Some(o) if !fits(o, &inc.versions) => {
                    RelationState::Version(o.manifest.version.clone())
                }
                Some(o) if o.enabled && m.enabled => RelationState::Clashing,
                Some(_) => RelationState::Off,
            };
            push(
                RelationKind::Incompatible,
                other,
                &inc.versions,
                &inc.reason,
                state,
            );
        }
        for (kind, list) in [
            (RelationKind::LoadAfter, &m.manifest.load_after),
            (RelationKind::LoadBefore, &m.manifest.load_before),
        ] {
            for other in list {
                let state = match at(other) {
                    None => RelationState::Missing,
                    Some(o) if (kind == RelationKind::LoadAfter) == (Some(o) > me) => {
                        RelationState::OutOfOrder
                    }
                    Some(_) => RelationState::Fine,
                };
                push(kind, other, "*", "", state);
            }
        }
        for other in &self.mods {
            if let Some(versions) = other.manifest.depends.get(id) {
                let state = if other.enabled {
                    RelationState::Fine
                } else {
                    RelationState::Off
                };
                push(RelationKind::NeededBy, &other.id, versions, "", state);
            }
        }
        out
    }

    /// For each mod of `indices`, the data its enabled dependencies (also
    /// indirect ones) give each resource, applied in load order: what the
    /// mod was made on instead of the game's original. Where a dependency
    /// changes a resource by a delta, that is the fold of the dependencies
    /// alone.
    fn dependency_bases(
        &self,
        originals: &mut Originals<'_>,
        indices: &[usize],
    ) -> Result<HashMap<usize, HashMap<&ResourceKey, Blob>>> {
        let mut out = HashMap::new();
        for &i in indices {
            let deps = self.dependencies(&self.mods[i].id);
            if deps.is_empty() {
                continue;
            }
            let chains = self.chains(|m| m.enabled && m.problem.is_none() && deps.contains(&m.id));
            let mut base = HashMap::new();
            for (key, chain) in chains {
                let data = match chain.iter().any(|(_, r)| r.is_delta()) {
                    true => self.fold(originals, key, &chain, Want::Hash)?.result,
                    false => chain.last().map(|(_, r)| r.payload.clone()),
                };
                if let Some(data) = data {
                    base.insert(key, data);
                }
            }
            out.insert(i, base);
        }
        Ok(out)
    }

    // ----------------------------------------------------------------- folds

    /// The entries of the mods `included` for each resource, in load order.
    fn chains(
        &self,
        included: impl Fn(&InstalledMod) -> bool,
    ) -> BTreeMap<&ResourceKey, Chain<'_>> {
        let mut chains: BTreeMap<&ResourceKey, Chain<'_>> = BTreeMap::new();
        for (index, m) in self.mods.iter().enumerate().filter(|(_, m)| included(m)) {
            for resource in &m.manifest.resources {
                chains
                    .entry(&resource.key)
                    .or_default()
                    .push((index, resource));
            }
        }
        chains
    }

    /// The resources of the unmodified game, for the folds.
    pub(super) fn originals<'a>(&self, game: &'a GameInstance) -> Originals<'a> {
        let created = self
            .created_streams()
            .into_iter()
            .map(str::to_owned)
            .collect();
        Originals::new(game, created)
    }

    /// The fold of the resource `key` over the entries `chain`.
    fn fold(
        &self,
        originals: &mut Originals<'_>,
        key: &ResourceKey,
        chain: &[(usize, &ModResource)],
        want: Want,
    ) -> Result<KeyFold> {
        let entries: Vec<Entry<'_>> = chain
            .iter()
            .map(|(index, resource)| Entry {
                resource,
                store: self.mods[*index].store(),
            })
            .collect();
        self.folds.fold(originals, key, &entries, want)
    }

    /// For each value the last delta of `chain` set over one its author did
    /// not see ([`Outcome::conflicts`]), the mod that gave the value: the
    /// nearest mod below whose delta addresses the same element, else the
    /// mod of the whole resource the fold starts from, else none — the
    /// value is the game's.
    fn conflict_sources(
        &self,
        chain: &[(usize, &ModResource)],
        steps: &[Step],
        outcome: &Outcome,
    ) -> Result<Vec<(String, Option<usize>)>> {
        let below = chain.len() - 1;
        let whole = chain[..below].iter().rposition(|(_, r)| !r.is_delta());
        // Later mods last: they are the ones whose value was there.
        let mut set_by: HashMap<String, usize> = HashMap::new();
        for (at, (index, resource)) in chain[..below].iter().enumerate() {
            if !matches!(&steps[at], Step::Delta(o) if o.data.is_some()) {
                continue;
            }
            let document = self.mods[*index].store().get(&resource.payload)?;
            for target in super::read_delta(resource, &document)?.targets() {
                set_by.insert(target, *index);
            }
        }
        Ok(outcome
            .conflicts
            .iter()
            .map(|note| {
                let source = set_by
                    .get(&note.target)
                    .copied()
                    .or(whole.map(|w| chain[w].0));
                (format!("{}: {}", note.target, note.reason), source)
            })
            .collect())
    }

    /// Problems of mod `index` as a whole: the edition of the game, the
    /// version of the mod manager, dependencies and recommended mods.
    fn check_relations(
        &self,
        index: usize,
        edition: &mut Option<Option<String>>,
        game: &GameInstance,
    ) -> Vec<Problem> {
        let m = &self.mods[index];
        let mut problems = Vec::new();
        if let Some(required) = &m.manifest.requires_manager
            && let Ok(condition) = Condition::parse(required)
            && !condition.matches(&version::manager_version())
        {
            problems.push(Problem::ManagerTooOld {
                required: condition.to_string(),
            });
        }
        if !m.manifest.game.editions.is_empty() {
            let edition = edition.get_or_insert_with(|| crate::edition::game_edition(game.root()));
            if !m.manifest.game.fits_edition(edition.as_deref()) {
                problems.push(Problem::Edition {
                    game: edition.clone(),
                    tested: m.manifest.game.editions.clone(),
                });
            }
        }
        for relation in self.relations(&m.id) {
            let (id, versions) = (relation.id, relation.versions);
            match (relation.kind, relation.state) {
                (RelationKind::Depends, RelationState::Missing) => {
                    problems.push(Problem::DependencyMissing { id, versions });
                }
                (RelationKind::Depends, RelationState::Off) => {
                    problems.push(Problem::DependencyDisabled { id, versions });
                }
                (RelationKind::Depends, RelationState::Version(found)) => {
                    problems.push(Problem::DependencyVersion {
                        id,
                        versions,
                        found,
                    });
                }
                (RelationKind::Depends, RelationState::OutOfOrder) => {
                    problems.push(Problem::DependencyLater { id });
                }
                (RelationKind::Recommends, state) if state != RelationState::Fine => {
                    problems.push(Problem::Recommended { id, versions });
                }
                _ => {}
            }
        }
        if let Some(code) = &m.manifest.native {
            let block = if m.enabled {
                native::blocks(self, self.native_host())
                    .map(|mut blocks| blocks.remove(&m.id))
                    .unwrap_or_else(|e| Some(NativeBlock::Damaged(e.to_string())))
            } else {
                None
            };
            if let Some(block) = block {
                problems.push(Problem::Native {
                    block,
                    required: code.required,
                });
            }
        }
        problems
    }

    // ---------------------------------------------------------------- checks

    fn enabled(&self) -> impl Iterator<Item = (usize, &InstalledMod)> {
        self.mods.iter().enumerate().filter(|(_, m)| m.enabled)
    }

    /// Resources more than one mod changes in a way that makes one of them
    /// lose: of the enabled mods, or of all installed mods.
    ///
    /// A resource replaced whole by several mods, or replaced whole above
    /// the deltas of others, is the data of the last of them
    /// ([`Conflict::elements`] is empty). Deltas above that merge, and are
    /// a conflict only where a delta set a value another mod had given the
    /// same element: one conflict for each pair of such mods, with the
    /// elements. The game is read for the resources that have deltas.
    pub fn conflicts(&self, game: &GameInstance, enabled_only: bool) -> Result<Vec<Conflict>> {
        let chains = self.chains(|m| (m.enabled || !enabled_only) && m.problem.is_none());
        let mut originals = self.originals(game);
        let mut deps: HashMap<usize, Vec<String>> = HashMap::new();
        let mut needs = |winner: usize, others: &[usize]| {
            let needs = deps
                .entry(winner)
                .or_insert_with(|| self.dependencies(&self.mods[winner].id));
            others
                .iter()
                .all(|other| needs.contains(&self.mods[*other].id))
        };
        let id = |index: &usize| self.mods[*index].id.clone();
        let mut conflicts = Vec::new();
        for (key, chain) in chains.into_iter().filter(|(_, chain)| chain.len() > 1) {
            let whole = chain.iter().rposition(|(_, r)| !r.is_delta());
            if let Some(w) = whole.filter(|w| *w > 0) {
                let below: Vec<usize> = chain[..w].iter().map(|(index, _)| *index).collect();
                conflicts.push(Conflict {
                    key: key.clone(),
                    same_data: chain[..=w]
                        .iter()
                        .all(|(_, r)| !r.is_delta() && r.payload == chain[0].1.payload),
                    patch: needs(chain[w].0, &below),
                    mods: chain[..=w].iter().map(|(index, _)| id(index)).collect(),
                    elements: Vec::new(),
                });
            }
            let first = whole.map_or(0, |w| w + 1);
            if first == chain.len() {
                continue;
            }
            let fold = self.fold(&mut originals, key, &chain, Want::Hash)?;
            // The first mod of a chain has no mod below to be in conflict
            // with: what its delta finds changed is the game's own data.
            for at in first.max(1)..chain.len() {
                let Step::Delta(outcome) = &fold.steps[at] else {
                    continue;
                };
                if outcome.conflicts.is_empty() {
                    continue;
                }
                let sources = self.conflict_sources(&chain[..=at], &fold.steps[..=at], outcome)?;
                let mut by_other: BTreeMap<usize, Vec<String>> = BTreeMap::new();
                for (element, source) in sources {
                    if let Some(other) = source {
                        by_other.entry(other).or_default().push(element);
                    }
                }
                let winner = chain[at].0;
                for (other, elements) in by_other {
                    conflicts.push(Conflict {
                        key: key.clone(),
                        mods: vec![id(&other), id(&winner)],
                        same_data: false,
                        patch: needs(winner, &[other]),
                        elements,
                    });
                }
            }
        }
        Ok(conflicts)
    }

    /// Checks every enabled mod against the game.
    pub fn check(&self, game: &GameInstance) -> Result<Vec<CompatIssue>> {
        let indices: Vec<usize> = self.enabled().map(|(i, _)| i).collect();
        self.check_mods(game, &indices)
    }

    /// Streams, other than those of the game, whose files the mod manager
    /// has created.
    fn created_streams(&self) -> BTreeSet<&str> {
        self.deployment
            .streams
            .iter()
            .filter(|(_, r)| r.new_stream.is_some())
            .map(|(name, _)| name.as_str())
            .collect()
    }

    /// Checks the mods at `indices` against the game: every replaced
    /// resource must exist and have the original data the mod was made for;
    /// every added resource and new stream must fit (see the module
    /// documentation). The original is what applying rebuilds from: the
    /// backup, or the file itself. Streams created by other enabled mods
    /// count as present for added resources.
    ///
    /// A mod with dependencies is checked against the game with its enabled
    /// dependencies applied: where a dependency changes or adds a resource,
    /// the mod's `original` must be the dependency's data. The mod as a
    /// whole is checked too ([`Subject::Mod`]): the edition of the game, the
    /// version of the mod manager, its dependencies and recommendations.
    ///
    /// A resource the mod changes by a delta (format version 6) has no
    /// original to compare: the delta is applied to what the enabled mods
    /// below give the resource, and what that tells is the check — a delta
    /// that is refused, operations left out, values that were not what the
    /// author saw.
    pub fn check_mods(&self, game: &GameInstance, indices: &[usize]) -> Result<Vec<CompatIssue>> {
        let created = self.created_streams();
        let mut originals = self.originals(game);
        let bases = self.dependency_bases(&mut originals, indices)?;
        let enabled = self.chains(|m| m.enabled && m.problem.is_none());
        // Streams of the game, as the game was before the mod manager. (A
        // game opened earlier may still list a created stream deleted since.)
        let game_stream = |name: &str| {
            game.find_stream(name).filter(|&i| {
                !created.contains(name) && game.original_path(&game.streams()[i]).is_file()
            })
        };
        // New streams of the enabled mods and of the checked ones: name → creator.
        let mut new_streams: BTreeMap<&str, &str> = BTreeMap::new();
        for (_, m) in self
            .enabled()
            .chain(indices.iter().map(|&i| (i, &self.mods[i])))
            .filter(|(_, m)| m.problem.is_none())
        {
            for stream in &m.manifest.streams {
                new_streams.entry(&stream.name).or_insert(&m.id);
            }
        }

        let mut issues = Vec::new();
        let mut tocs: HashMap<usize, Toc> = HashMap::new();
        let mut by_stream: BTreeMap<&str, Vec<(usize, &str, &ModResource)>> = BTreeMap::new();
        let mut edition = None;
        for &i in indices {
            let m = &self.mods[i];
            if m.problem.is_none() {
                issues.extend(self.check_relations(i, &mut edition, game).into_iter().map(
                    |problem| CompatIssue {
                        mod_id: m.id.clone(),
                        subject: Subject::Mod,
                        problem,
                    },
                ));
            }
            for stream in &m.manifest.streams {
                issues.extend(
                    self.check_new_stream(game, stream, game_stream(&stream.name), &mut tocs)?
                        .into_iter()
                        .map(|problem| CompatIssue {
                            mod_id: m.id.clone(),
                            subject: Subject::Stream(stream.name.clone()),
                            problem,
                        }),
                );
            }
            for resource in &m.manifest.resources {
                by_stream
                    .entry(&resource.key.stream)
                    .or_default()
                    .push((i, &m.id, resource));
            }
        }

        let mut guids: Option<GameGuids> = None;
        for (stream, resources) in by_stream {
            let mut issue = |id: &str, key: &ResourceKey, problem| {
                issues.push(CompatIssue {
                    mod_id: id.to_owned(),
                    subject: Subject::Resource(key.clone()),
                    problem,
                })
            };
            let original = match game_stream(stream) {
                Some(index) => Some(StreamFile::open(
                    game.original_path(&game.streams()[index]),
                )?),
                None => None,
            };
            let mut hashes: HashMap<usize, Blob> = HashMap::new();
            for (index, id, resource) in resources {
                let key = &resource.key;
                if resource.is_delta() {
                    // On what the enabled mods below this one give it.
                    let mut chain: Chain<'_> = enabled
                        .get(key)
                        .map(|chain| chain.iter().filter(|(i, _)| *i < index).copied().collect())
                        .unwrap_or_default();
                    chain.push((index, resource));
                    let fold = self.fold(&mut originals, key, &chain, Want::Hash)?;
                    match fold.steps.last() {
                        Some(Step::NoBase) => {
                            let problem = match (&original, new_streams.get(stream)) {
                                (Some(_), _) => Problem::ResourceMissing,
                                (None, Some(creator)) => Problem::InNewStream {
                                    creator: (*creator).to_owned(),
                                },
                                (None, None) => Problem::StreamMissing,
                            };
                            issue(id, key, problem);
                        }
                        Some(Step::Delta(outcome)) => {
                            if let Some(error) = &outcome.error {
                                let error = error.clone();
                                issue(id, key, Problem::DeltaRefused { error });
                            }
                            if !outcome.skipped.is_empty() {
                                let skipped = outcome
                                    .skipped
                                    .iter()
                                    .map(|n| format!("{}: {}", n.target, n.reason))
                                    .collect();
                                let applied = outcome.applied;
                                issue(id, key, Problem::OperationsSkipped { skipped, applied });
                            }
                            if !outcome.conflicts.is_empty() {
                                // Values other mods gave are conflicts
                                // between the mods; the game's are said here.
                                let elements: Vec<String> = self
                                    .conflict_sources(&chain, &fold.steps, outcome)?
                                    .into_iter()
                                    .filter(|(_, source)| source.is_none())
                                    .map(|(element, _)| element)
                                    .collect();
                                if !elements.is_empty() {
                                    issue(id, key, Problem::ValuesDiffer { elements });
                                }
                            }
                        }
                        _ => {}
                    }
                    continue;
                }
                let Some(added) = &resource.added else {
                    // Made on top of a dependency that changes the resource.
                    if let Some(base) = bases.get(&index).and_then(|b| b.get(key)) {
                        if *base != resource.original {
                            issue(
                                id,
                                key,
                                Problem::OriginalDiffers {
                                    expected: resource.original.clone(),
                                    found: base.clone(),
                                },
                            );
                        }
                        continue;
                    }
                    let Some(original) = &original else {
                        let problem = match new_streams.get(stream) {
                            Some(creator) => Problem::InNewStream {
                                creator: (*creator).to_owned(),
                            },
                            None => Problem::StreamMissing,
                        };
                        issue(id, key, problem);
                        continue;
                    };
                    let Some(i) = key.find_in(original.layout()) else {
                        issue(id, key, Problem::ResourceMissing);
                        continue;
                    };
                    let found = match hashes.get(&i) {
                        Some(blob) => blob.clone(),
                        None => {
                            let blob = Blob::of(&original.resource_data(i)?);
                            hashes.insert(i, blob.clone());
                            blob
                        }
                    };
                    if found != resource.original {
                        issue(
                            id,
                            key,
                            Problem::OriginalDiffers {
                                expected: resource.original.clone(),
                                found,
                            },
                        );
                    }
                    continue;
                };
                match &original {
                    Some(original) => {
                        if key.find_in(original.layout()).is_some() {
                            issue(id, key, Problem::ResourceExists);
                            continue;
                        }
                        if let Some(before) = &added.before
                            && before.find_in(original.layout()).is_none()
                        {
                            let before = before.clone();
                            issue(id, key, Problem::AnchorMissing { before });
                        }
                    }
                    None if new_streams.contains_key(stream) => {}
                    None => {
                        issue(id, key, Problem::StreamMissing);
                        continue;
                    }
                }
                if key.ids == [0; 4] {
                    continue;
                }
                if guids.is_none() {
                    guids = Some(GameGuids::of(game, &created)?);
                }
                let guids = guids.as_ref().expect("built above");
                if let Some(other) = guids.other_data(game, key, &resource.payload)? {
                    issue(id, key, Problem::GuidInUse { other });
                }
            }
        }
        Ok(issues)
    }

    /// Problems of a new stream: `existing` is the stream of the game with its name.
    fn check_new_stream(
        &self,
        game: &GameInstance,
        stream: &NewStream,
        existing: Option<usize>,
        tocs: &mut HashMap<usize, Toc>,
    ) -> Result<Vec<Problem>> {
        let mut problems = Vec::new();
        let ours = self
            .deployment
            .streams
            .get(&stream.name)
            .is_some_and(|r| r.new_stream.is_some());
        if existing.is_some() || (!ours && game.root().join(stream.rel_path()).exists()) {
            problems.push(Problem::StreamExists);
        }
        for rel in &stream.tocs {
            let Some(index) = game
                .tocs()
                .iter()
                .position(|t| t.rel_path.eq_ignore_ascii_case(rel))
            else {
                problems.push(Problem::TocMissing { toc: rel.clone() });
                continue;
            };
            if let std::collections::hash_map::Entry::Vacant(entry) = tocs.entry(index) {
                // The table as it was before the mod manager added streams to it.
                let path = game.original_path(&game.tocs()[index]);
                let data = fs::read(&path).map_err(|e| Error::io(&path, e))?;
                entry.insert(Toc::parse(&data)?);
            }
            let table = &tocs[&index];
            let parent = stream.parent_in(rel);
            if table.find_file(parent).is_none() {
                problems.push(Problem::ParentMissing {
                    toc: rel.clone(),
                    parent: parent.to_owned(),
                });
            }
            if table.find_name(&stream.name).is_some() {
                problems.push(Problem::StreamExists);
            } else if table.names.iter().any(|n| n.hash == stream.hash) {
                problems.push(Problem::StreamHashUsed { toc: rel.clone() });
            }
        }
        problems.dedup();
        Ok(problems)
    }

    /// Structural clashes between mods: of the enabled mods, or of all
    /// installed mods. Fatal ones stop [`Self::apply`].
    pub fn clashes(&self, enabled_only: bool) -> Vec<Clash> {
        let mods: Vec<&InstalledMod> = self
            .mods
            .iter()
            .filter(|m| (m.enabled || !enabled_only) && m.problem.is_none())
            .collect();
        let mut clashes = Vec::new();

        let mut by_name: BTreeMap<&str, Vec<(&str, &NewStream)>> = BTreeMap::new();
        let mut by_hash: BTreeMap<u32, Vec<(&str, &str)>> = BTreeMap::new();
        for m in &mods {
            for stream in &m.manifest.streams {
                by_name
                    .entry(&stream.name)
                    .or_default()
                    .push((&m.id, stream));
                let list = by_hash.entry(stream.hash).or_default();
                if !list.iter().any(|(_, name)| *name == stream.name) {
                    list.push((&m.id, &stream.name));
                }
            }
        }
        for (name, list) in by_name.into_iter().filter(|(_, l)| l.len() > 1) {
            let same = |a: &NewStream, b: &NewStream| {
                NewStream {
                    label: String::new(),
                    ..a.clone()
                } == NewStream {
                    label: String::new(),
                    ..b.clone()
                }
            };
            clashes.push(Clash::Stream {
                name: name.to_owned(),
                mods: list.iter().map(|(m, _)| (*m).to_owned()).collect(),
                same_definition: list.iter().all(|(_, s)| same(s, list[0].1)),
            });
        }
        for (hash, list) in by_hash.into_iter().filter(|(_, l)| l.len() > 1) {
            clashes.push(Clash::StreamHash {
                hash,
                streams: list
                    .iter()
                    .map(|(m, s)| ((*m).to_owned(), (*s).to_owned()))
                    .collect(),
            });
        }

        let mut by_guid: BTreeMap<[u32; 4], Vec<(&str, &ModResource)>> = BTreeMap::new();
        for m in &mods {
            for resource in m.manifest.added().filter(|r| r.key.ids != [0; 4]) {
                by_guid
                    .entry(resource.key.ids)
                    .or_default()
                    .push((&m.id, resource));
            }
        }
        for (ids, list) in by_guid {
            let keys: BTreeSet<&ResourceKey> = list.iter().map(|(_, r)| &r.key).collect();
            // The same key in several mods is a resource conflict: one wins.
            if keys.len() < 2 {
                continue;
            }
            clashes.push(Clash::Guid {
                ids,
                resources: list
                    .iter()
                    .map(|(m, r)| ((*m).to_owned(), r.key.clone()))
                    .collect(),
                same_data: list.iter().all(|(_, r)| r.payload == list[0].1.payload),
            });
        }

        let mut adding: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        let mut replacing: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        for m in &mods {
            let new: BTreeSet<&str> = m.manifest.streams.iter().map(|s| s.name.as_str()).collect();
            for resource in &m.manifest.resources {
                let stream = resource.key.stream.as_str();
                let map = if resource.is_added() {
                    if new.contains(stream) {
                        continue;
                    }
                    &mut adding
                } else {
                    &mut replacing
                };
                let list = map.entry(stream).or_default();
                if !list.contains(&m.id) {
                    list.push(m.id.clone());
                }
            }
        }
        for (stream, adders) in adding {
            let Some(replacers) = replacing.get(stream) else {
                continue;
            };
            let others: Vec<String> = replacers
                .iter()
                .filter(|r| !adders.contains(r))
                .cloned()
                .collect();
            // A patch on top of the adding mod, or the other way round, is meant.
            let others: Vec<String> = others
                .into_iter()
                .filter(|r| {
                    !adders
                        .iter()
                        .any(|a| self.depends_on(r, a) || self.depends_on(a, r))
                })
                .collect();
            if !others.is_empty() {
                clashes.push(Clash::SharedStream {
                    stream: stream.to_owned(),
                    adding: adders,
                    replacing: others,
                });
            }
        }

        for m in &mods {
            for (other, incompatible) in &m.manifest.incompatible {
                let Some(o) = mods.iter().find(|o| o.id == *other) else {
                    continue;
                };
                let condition =
                    Condition::parse(&incompatible.versions).unwrap_or_else(|_| Condition::any());
                if condition.matches_text(&o.manifest.version) {
                    clashes.push(Clash::Incompatible {
                        declared_by: m.id.clone(),
                        other: o.id.clone(),
                        versions: incompatible.versions.clone(),
                        reason: incompatible.reason.clone(),
                    });
                }
            }
        }
        clashes
    }

    // ------------------------------------------------------------ the game

    /// The combined changes of the enabled mods: for each resource its data
    /// and the index of the last enabled mod that has a part in it. The
    /// data is that of the last enabled mod that replaces or adds the
    /// resource whole — else the game's own — with the deltas of the
    /// enabled mods above applied in load order ([`super::fold`]); the game
    /// is read for the resources that have deltas.
    /// A later mod that replaces or changes a resource an earlier one adds
    /// (a patch on top of a mod) keeps it added; the original is the game's.
    pub fn plan(&self, game: &GameInstance) -> Result<BTreeMap<ResourceKey, (Change, usize)>> {
        Ok(self
            .folded_plan(game, Want::Hash, |m| m.enabled && m.problem.is_none())?
            .changes)
    }

    /// [`Self::plan`] of the mods `included`, with what the folds told.
    pub(super) fn folded_plan(
        &self,
        game: &GameInstance,
        want: Want,
        included: impl Fn(&InstalledMod) -> bool,
    ) -> Result<FoldedPlan> {
        let mut plan = FoldedPlan::default();
        let mut originals = self.originals(game);
        for (key, chain) in self.chains(included) {
            let fold = match chain.iter().any(|(_, r)| r.is_delta()) {
                true => Some(self.fold(&mut originals, key, &chain, want)?),
                false => None,
            };
            let mut planned: Option<(Change, usize)> = None;
            let mut mods = Vec::new();
            for (at, (index, resource)) in chain.iter().enumerate() {
                let label = self.mods[*index].manifest.name.clone();
                match fold.as_ref().map(|fold| &fold.steps[at]) {
                    None | Some(Step::Whole) => {
                        let mut change = Change {
                            key: key.clone(),
                            payload: resource.payload.clone(),
                            original: resource.original.clone(),
                            storage: resource.storage,
                            label,
                            added: resource.added.clone(),
                            made_on: None,
                        };
                        if let Some((earlier, _)) = &planned {
                            change.original = earlier.original.clone();
                            if change.added.is_none() {
                                change.added = earlier.added.clone();
                            }
                        }
                        planned = Some((change, *index));
                        mods = vec![*index];
                    }
                    Some(Step::Delta(Outcome {
                        data: Some(data), ..
                    })) => {
                        let below = match planned.take() {
                            Some((change, _)) => change,
                            // The fold starts from the game's own data.
                            None => Change {
                                key: key.clone(),
                                payload: data.clone(),
                                original: fold
                                    .as_ref()
                                    .and_then(|fold| fold.original.clone())
                                    .unwrap_or_else(|| resource.original.clone()),
                                storage: resource.storage,
                                label: String::new(),
                                added: None,
                                made_on: None,
                            },
                        };
                        let storage = match resource.storage.is_keep() {
                            true => below.storage,
                            false => resource.storage,
                        };
                        let change = Change {
                            payload: data.clone(),
                            storage,
                            label,
                            ..below
                        };
                        planned = Some((change, *index));
                        mods.push(*index);
                    }
                    // A delta that is replaced, refused or has nothing to
                    // be applied to gives the resource nothing.
                    Some(_) => {}
                }
            }
            let Some((change, index)) = planned else {
                continue;
            };
            if let Some(fold) = fold.filter(|fold| fold.folded) {
                // Deltas that left the game's data as it is change nothing.
                if change.added.is_none() && fold.original.as_ref() == Some(&change.payload) {
                    continue;
                }
                plan.steps.extend(fold.ids);
                plan.folded.insert(key.clone(), mods);
            }
            plan.changes.insert(key.clone(), (change, index));
        }
        Ok(plan)
    }

    /// The data of the resource `key` as `plan` (made with [`Want::Data`])
    /// has it.
    pub(super) fn planned_data(&self, plan: &FoldedPlan, key: &ResourceKey) -> Result<Vec<u8>> {
        let (change, index) = plan
            .changes
            .get(key)
            .ok_or_else(|| Error::mod_error(format!("{key} is not planned")))?;
        match plan.folded.contains_key(key) {
            true => self.folds.data(&change.payload),
            false => self.mods[*index].store().get(&change.payload),
        }
    }

    /// The files of the archives the enabled mods carry: for each entry, the
    /// file of the last enabled mod that has it, with that mod's index.
    pub fn plan_files(&self) -> BTreeMap<FileKey, (FileChange, usize)> {
        let mut plan = BTreeMap::new();
        for (index, m) in self.enabled().filter(|(_, m)| m.problem.is_none()) {
            for file in &m.manifest.files {
                let change = FileChange {
                    key: file.key.clone(),
                    payload: file.payload.clone(),
                    // The archives of the game are never written: nothing is
                    // compared with the original of an entry.
                    original: Blob::of(&[]),
                    label: m.manifest.name.clone(),
                };
                plan.insert(file.key.clone(), (change, index));
            }
        }
        plan
    }

    /// Writes the files of [`Self::plan_files`] into the overlay archive, or
    /// takes the overlay away when no enabled mod has files.
    fn sync_files(
        &mut self,
        game: &GameInstance,
        wanted: bool,
        force: bool,
    ) -> Result<ApplyReport> {
        let plan = if wanted {
            self.plan_files()
        } else {
            BTreeMap::new()
        };
        if plan.is_empty() && self.deployment.files.is_empty() {
            return Ok(ApplyReport::default());
        }
        let stores: Vec<PayloadStore> = self.mods.iter().map(InstalledMod::store).collect();
        let changes: BTreeMap<FileKey, FileChange> = plan
            .iter()
            .map(|(key, (change, _))| (key.clone(), change.clone()))
            .collect();
        let path = self.dir.join(APPLIED_FILE);
        files::sync(
            game,
            &mut self.deployment,
            &path,
            &changes,
            &|change| {
                let (_, index) = plan
                    .get(&change.key)
                    .ok_or_else(|| Error::mod_error(format!("{} is not planned", change.key)))?;
                stores[*index].get(&change.payload)
            },
            files::MODS_OWNER,
            force,
        )
    }

    /// The streams the enabled mods create: for each name, the definition of
    /// the last enabled mod that creates it, with that mod's index.
    pub fn plan_streams(&self) -> BTreeMap<String, (NewStream, usize)> {
        let mut plan = BTreeMap::new();
        for (index, m) in self.enabled().filter(|(_, m)| m.problem.is_none()) {
            for stream in &m.manifest.streams {
                plan.insert(stream.name.clone(), (stream.clone(), index));
            }
        }
        plan
    }

    fn new_streams(&self) -> BTreeMap<String, NewStream> {
        self.plan_streams()
            .into_iter()
            .map(|(name, (stream, _))| (name, stream))
            .collect()
    }

    /// Changes grouped by stream; created streams are present even when
    /// nothing goes into them.
    fn desired<'a>(
        plan: &'a BTreeMap<ResourceKey, (Change, usize)>,
        new_streams: &BTreeMap<String, NewStream>,
    ) -> BTreeMap<String, Vec<&'a Change>> {
        let mut by_stream: BTreeMap<String, Vec<&Change>> = BTreeMap::new();
        for name in new_streams.keys() {
            by_stream.entry(name.clone()).or_default();
        }
        for (change, _) in plan.values() {
            by_stream
                .entry(change.key.stream.clone())
                .or_default()
                .push(change);
        }
        by_stream
    }

    /// Compares the combined result of the enabled mods with the game.
    pub fn status(&self, game: &GameInstance) -> Result<DeployStatus> {
        let plan = self.plan(game)?;
        let new_streams = self.new_streams();
        self.deployment
            .status_with(game, &Self::desired(&plan, &new_streams), &new_streams)
    }

    /// Streams whose game files hold mods written by the manager.
    pub fn held_streams(&self, game: &GameInstance) -> Result<Vec<String>> {
        self.deployment.held_streams(game)
    }

    /// Streams whose last write into the game did not finish: the game is
    /// not as the mods were written nor as it was, and its tables of
    /// contents may not match. [`Self::unapply`] puts the originals back
    /// from the backups; [`Self::apply`] writes the mods anew.
    pub fn interrupted(&self, game: &GameInstance) -> Result<Vec<String>> {
        self.deployment.interrupted(game)
    }

    /// The streams the mod manager has written into the game (by its record).
    pub fn written_streams(&self) -> BTreeSet<&str> {
        self.deployment.streams.keys().map(String::as_str).collect()
    }

    /// Whether the mod manager has written anything into the game that is still there.
    pub fn has_written(&self) -> bool {
        !self.deployment.streams.is_empty()
            || !self.deployment.files.is_empty()
            // Code too; a loader that stays for other modules is not the mods'.
            || native::written(self)
                .is_ok_and(|w| w.iter().any(|(_, owner)| owner != native::OWNER_LOADER))
    }

    /// The installed mods against the files of the game: which of them the
    /// game holds now ([`ModInGame::written`]), and how much it still holds
    /// of mods that are switched off or removed. This is what a project
    /// compares its base with (`crate::project::base`): the editor reads
    /// the files of the game, so a mod that is enabled but not written is
    /// not in what its author sees.
    pub fn in_game(&self, game: &GameInstance) -> Result<GameMods> {
        let status = self.status(game)?;
        // The streams whose file is, right now, what the record says.
        let records: BTreeMap<&str, &crate::workspace::AppliedStream> = status
            .streams
            .iter()
            .filter(|s| s.state == StreamState::Applied)
            .filter_map(|s| {
                let record = self.deployment.streams.get(&s.stream)?;
                record
                    .in_flight
                    .is_none()
                    .then_some((s.stream.as_str(), record))
            })
            .collect();
        let plan = self.plan(game)?;
        let files = self.plan_files();
        // The game holds the data of the mod, or the data of the enabled
        // mod that wins the resource over it: a mod is not less written for
        // being overridden, nor for a later mod waiting to be written into
        // the same stream.
        let holds = |sha: &String, own: &Blob, winner: Option<&Blob>| {
            *sha == own.sha256 || winner.is_some_and(|w| *sha == w.sha256)
        };
        let resource_written = |key: &ResourceKey, payload: &Blob| {
            records
                .get(key.stream.as_str())
                .and_then(|record| record.resources.get(key))
                .is_some_and(|sha| holds(sha, payload, plan.get(key).map(|(c, _)| &c.payload)))
        };
        let file_written = |key: &FileKey, payload: &Blob| {
            self.deployment.files.values().any(|record| {
                record.in_flight.is_none()
                    && record.changes.get(key).is_some_and(|sha| {
                        holds(sha, payload, files.get(key).map(|(c, _)| &c.payload))
                    })
            })
        };
        let mods = self
            .mods
            .iter()
            .map(|m| {
                let manifest = &m.manifest;
                // Executable code is not data the editor reads: a mod is
                // written by its resources, streams and files.
                let written = m.enabled
                    && m.problem.is_none()
                    && manifest
                        .resources
                        .iter()
                        .all(|r| resource_written(&r.key, &r.payload))
                    && manifest.streams.iter().all(|s| {
                        records
                            .get(s.name.as_str())
                            .is_some_and(|record| record.new_stream.is_some())
                    })
                    && manifest
                        .files
                        .iter()
                        .all(|f| file_written(&f.key, &f.payload));
                ModInGame {
                    id: m.id.clone(),
                    name: manifest.name.clone(),
                    version: manifest.version.clone(),
                    sha256: manifest.sha256(),
                    enabled: m.enabled,
                    damaged: m.problem.is_some(),
                    written,
                }
            })
            .collect();
        Ok(GameMods {
            mods,
            leftover: status.pending_reverts.len(),
        })
    }

    /// Writes the combined result of the enabled mods into the game. Streams
    /// that no enabled mod changes any more are put back to their originals.
    ///
    /// Refused when the editor's workspace has changes in the game, when an
    /// enabled mod does not fit the game (unless
    /// `options.ignore_compatibility` for different originals), and for
    /// streams changed by another program (unless `options.force`).
    pub fn apply(&mut self, game: &GameInstance, options: ModApplyOptions) -> Result<ApplyReport> {
        self.apply_with_progress(game, options, &mut |_| {})
    }

    /// [`Self::apply`], telling `progress` before each stream is written
    /// (after every check passed) and when all are.
    pub fn apply_with_progress(
        &mut self,
        game: &GameInstance,
        options: ModApplyOptions,
        progress: &mut dyn FnMut(SyncProgress<'_>),
    ) -> Result<ApplyReport> {
        self.changing(|manager| manager.apply_with_progress_locked(game, options, progress))
    }

    /// [`Self::apply_with_progress`], with the lock held.
    fn apply_with_progress_locked(
        &mut self,
        game: &GameInstance,
        options: ModApplyOptions,
        progress: &mut dyn FnMut(SyncProgress<'_>),
    ) -> Result<ApplyReport> {
        if let Some(m) = self.enabled().map(|(_, m)| m).find(|m| m.problem.is_some()) {
            return Err(Error::mod_error(format!(
                "mod `{}` is damaged ({}); disable or uninstall it",
                m.id,
                m.problem.as_deref().unwrap_or_default()
            )));
        }
        let folded = self.folded_plan(game, Want::Data, |m| m.enabled && m.problem.is_none())?;
        let plan = &folded.changes;
        let new_streams = self.new_streams();
        if !plan.is_empty() || !new_streams.is_empty() || !self.plan_files().is_empty() {
            ensure_workspace_not_in_game(game)?;
        }
        let clashes: Vec<String> = self
            .clashes(true)
            .iter()
            .filter(|c| c.is_fatal())
            .map(Clash::describe)
            .collect();
        if !clashes.is_empty() {
            return Err(Error::mod_error(format!(
                "the enabled mods cannot be applied together: {}",
                list_some(&clashes)
            )));
        }
        let issues = self.check(game)?;
        let fatal: Vec<String> = issues
            .iter()
            .filter(|i| i.is_fatal())
            .map(CompatIssue::describe)
            .collect();
        if !fatal.is_empty() {
            return Err(Error::mod_error(format!(
                "{} resource(s) or stream(s) of the enabled mods do not fit this game: {}. \
                 Disable those mods",
                fatal.len(),
                list_some(&fatal)
            )));
        }
        let differ: Vec<String> = issues
            .iter()
            .filter(|i| !i.problem.is_notice())
            .map(CompatIssue::describe)
            .collect();
        if !differ.is_empty() && !options.ignore_compatibility {
            return Err(Error::mod_error(format!(
                "{} resource(s) in the game are not the originals the mods were made for: {}. \
                 The game files may be changed by another tool (run the integrity check) or \
                 be a different version of the game. Apply anyway with \
                 `--ignore-compatibility` if you know the mods still fit",
                differ.len(),
                list_some(&differ)
            )));
        }
        // What the folds left out or wrote over is said with the result.
        let told: Vec<String> = issues
            .iter()
            .filter(|i| {
                matches!(
                    i.problem,
                    Problem::OperationsSkipped { .. } | Problem::ValuesDiffer { .. }
                )
            })
            .map(CompatIssue::describe)
            .collect();
        let mut names: BTreeSet<String> = plan.keys().map(|k| k.stream.clone()).collect();
        names.extend(new_streams.keys().cloned());
        names.extend(self.deployment.streams.keys().cloned());
        let stores: Vec<PayloadStore> = self.mods.iter().map(InstalledMod::store).collect();
        let by_stream: BTreeMap<String, Vec<Change>> = Self::desired(plan, &new_streams)
            .into_iter()
            .map(|(name, changes)| (name, changes.into_iter().cloned().collect()))
            .collect();
        // The folded resources are data of no mod: they go into the store
        // of the folds before the write, which reads them by the hash the
        // plan names.
        let results = || folded.folded.keys().map(|key| &plan[key].0.payload);
        self.folds.persist(results())?;
        let fold_store = self.folds.store();
        let path = self.dir.join(APPLIED_FILE);
        let mut report = self.deployment.sync_with_progress(
            &path,
            game,
            &names,
            options.force,
            &new_streams,
            |name| by_stream.get(name).cloned().unwrap_or_default(),
            |change| {
                let (_, index) = plan
                    .get(&change.key)
                    .ok_or_else(|| Error::mod_error(format!("{} is not planned", change.key)))?;
                match folded.folded.contains_key(&change.key) {
                    true => fold_store.get(&change.payload),
                    false => stores[*index].get(&change.payload),
                }
            },
            progress,
        )?;
        report.warnings.extend(told);
        // The folds of mods that are no longer in the game are not kept.
        if let Err(error) = self.folds.settle(results(), &folded.steps) {
            report
                .warnings
                .push(format!("the folded resources are not tidied: {error}"));
        }
        // Files of the archives go after the streams, into the overlay archive.
        report.merge(self.sync_files(game, true, options.force)?);
        // Executable code goes last, and only what the player agreed to.
        let code = self.sync_native().clone();
        report.written.extend(code.written);
        report
            .warnings
            .extend(code.blocked.iter().map(|(id, block)| {
                format!(
                    "native code of mod `{id}` is not installed: {}",
                    block.describe()
                )
            }));
        report.warnings.extend(code.warnings);
        Ok(report)
    }

    /// Puts back the original of every stream the mod manager has written.
    /// The mods stay installed.
    pub fn unapply(&mut self, game: &GameInstance, force: bool) -> Result<ApplyReport> {
        self.unapply_with_progress(game, force, &mut |_| {})
    }

    /// [`Self::unapply`], telling `progress` before each stream is put back.
    pub fn unapply_with_progress(
        &mut self,
        game: &GameInstance,
        force: bool,
        progress: &mut dyn FnMut(SyncProgress<'_>),
    ) -> Result<ApplyReport> {
        self.changing(|manager| manager.unapply_with_progress_locked(game, force, progress))
    }

    /// [`Self::unapply_with_progress`], with the lock held.
    fn unapply_with_progress_locked(
        &mut self,
        game: &GameInstance,
        force: bool,
        progress: &mut dyn FnMut(SyncProgress<'_>),
    ) -> Result<ApplyReport> {
        let names: BTreeSet<String> = self.deployment.streams.keys().cloned().collect();
        let path = self.dir.join(APPLIED_FILE);
        let mut report = self.deployment.sync_with_progress(
            &path,
            game,
            &names,
            force,
            &BTreeMap::new(),
            |_| Vec::new(),
            |change| Err(Error::mod_error(format!("{} is not planned", change.key))),
            progress,
        )?;
        report.merge(self.sync_files(game, false, force)?);
        // The code the mod manager installed leaves with the data.
        self.native_report = native::remove_all(self).unwrap_or_else(|e| NativeReport {
            warnings: vec![format!("native code: {e}")],
            ..NativeReport::default()
        });
        self.forget_native_host();
        report
            .warnings
            .extend(self.native_report.warnings.iter().cloned());
        Ok(report)
    }

    /// The game directory.
    pub fn game_root(&self) -> &Path {
        &self.root
    }

    /// Takes over a game patched by the old toolkit: its backups become the
    /// originals ([`super::integrity::adopt_old_backups`]) and its installed
    /// patches are installed as mods in the order it applied them. The game
    /// files are not changed; applying afterwards needs `force`, because the
    /// patched files differ from the adopted originals.
    pub fn import_old_toolkit(&mut self, game: &GameInstance) -> Result<OldToolkitImport> {
        self.changing(|manager| manager.import_old_toolkit_locked(game))
    }

    /// [`Self::import_old_toolkit`], with the lock held.
    fn import_old_toolkit_locked(&mut self, game: &GameInstance) -> Result<OldToolkitImport> {
        let old = super::integrity::detect_old_toolkit(game.root()).ok_or_else(|| {
            Error::mod_error("the old toolkit's directory `_toolkit` is not in the game directory")
        })?;
        let adopted = super::integrity::adopt_old_backups(game)?;
        let mut installed = Vec::new();
        for path in &old.patch_files {
            let name = path
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            installed.push((
                name,
                self.install_file(game, path, true)
                    .map_err(|e| e.to_string()),
            ));
        }
        Ok(OldToolkitImport { adopted, installed })
    }

    // ------------------------------------------------- order, sets and undo

    /// The load order with the enabled state of every installed mod.
    pub fn arrangement(&self) -> Vec<SetEntry> {
        self.mods
            .iter()
            .map(|m| SetEntry {
                id: m.id.clone(),
                enabled: m.enabled,
            })
            .collect()
    }

    /// The arrangement [`Self::arrange`] would make of `entries`, and the
    /// ids of `entries` that are not installed.
    fn arranged(&self, entries: &[SetEntry]) -> (Vec<SetEntry>, Vec<String>) {
        let mut out: Vec<SetEntry> = Vec::new();
        let mut missing = Vec::new();
        for entry in entries {
            if out.iter().any(|e| e.id == entry.id) || missing.contains(&entry.id) {
                continue;
            }
            match self.get(&entry.id) {
                Some(m) => out.push(SetEntry {
                    id: m.id.clone(),
                    enabled: entry.enabled && m.problem.is_none(),
                }),
                None => missing.push(entry.id.clone()),
            }
        }
        for m in &self.mods {
            if !out.iter().any(|e| e.id == m.id) {
                out.push(SetEntry {
                    id: m.id.clone(),
                    enabled: false,
                });
            }
        }
        (out, missing)
    }

    /// Puts the mods in the order of `entries` and enables them as listed
    /// there; installed mods that `entries` does not list follow in their
    /// order, disabled. A damaged mod stays disabled. Returns the ids of
    /// `entries` that are not installed. The game is not changed until
    /// [`Self::apply`].
    pub fn arrange(&mut self, entries: &[SetEntry]) -> Result<Vec<String>> {
        self.changing(|manager| manager.arrange_locked(entries))
    }

    /// [`Self::arrange`], with the lock held.
    fn arrange_locked(&mut self, entries: &[SetEntry]) -> Result<Vec<String>> {
        let (order, missing) = self.arranged(entries);
        let mut rest = std::mem::take(&mut self.mods);
        for entry in order {
            if let Some(i) = rest.iter().position(|m| m.id == entry.id) {
                let mut m = rest.remove(i);
                m.enabled = entry.enabled;
                self.mods.push(m);
            }
        }
        self.mods.append(&mut rest);
        self.save_registry()?;
        Ok(missing)
    }

    /// Arranges the mods as `entries` ([`Self::arrange`]) and writes them
    /// into the game ([`Self::apply_with_progress`]) as one change. When the
    /// write is refused or fails, the arrangement before comes back, so
    /// that the mod manager does not show an arrangement the game was never
    /// brought to. Returns the ids of `entries` that are not installed, and
    /// what was written.
    pub fn arrange_and_apply(
        &mut self,
        game: &GameInstance,
        entries: &[SetEntry],
        options: ModApplyOptions,
        progress: &mut dyn FnMut(SyncProgress<'_>),
    ) -> Result<(Vec<String>, ApplyReport)> {
        self.changing(|manager| {
            let before = manager.arrangement();
            let missing = manager.arrange_locked(entries)?;
            match manager.apply_with_progress_locked(game, options, progress) {
                Ok(report) => Ok((missing, report)),
                Err(error) => {
                    // The first error is the one to tell.
                    let _ = manager.arrange_locked(&before);
                    Err(error)
                }
            }
        })
    }

    /// A name for a new set of mods that no saved set has: `wanted`, or
    /// `wanted (2)`, `wanted (3)`… (shortened to fit the limit of a name).
    pub fn free_set_name(&self, wanted: &str) -> Result<String> {
        let taken: Vec<String> = self.sets()?.into_iter().map(|s| s.name).collect();
        let clean: String = wanted.chars().filter(|c| !c.is_control()).collect();
        let clean = clean.trim();
        for n in 1usize.. {
            let suffix = if n == 1 {
                String::new()
            } else {
                format!(" ({n})")
            };
            let room = MAX_SET_NAME - suffix.chars().count();
            let head: String = clean.chars().take(room).collect();
            let name = format!("{}{suffix}", head.trim_end());
            if !taken.contains(&name) {
                return validate_set_name(&name).map(str::to_owned);
            }
        }
        unreachable!("one of more names than there are sets is free")
    }

    /// The saved sets of mods (`sets.json` of the mod manager directory).
    pub fn sets(&self) -> Result<Vec<ModSet>> {
        let path = self.dir.join(SETS_FILE);
        if !path.is_file() {
            return Ok(Vec::new());
        }
        let text = fs::read(&path).map_err(|e| Error::io(&path, e))?;
        let file: SetsFile = serde_json::from_slice(&text)
            .map_err(|e| Error::mod_error(format!("{}: {e}", path.display())))?;
        if file.format != SETS_FORMAT || file.version > REGISTRY_VERSION {
            return Err(Error::mod_error(format!(
                "{} was written by a newer program or is not a list of mod sets",
                path.display()
            )));
        }
        Ok(file.sets)
    }

    fn save_sets(&self, sets: Vec<ModSet>) -> Result<()> {
        let file = SetsFile {
            format: SETS_FORMAT.to_owned(),
            version: REGISTRY_VERSION,
            sets,
        };
        fs::create_dir_all(&self.dir).map_err(|e| Error::io(&self.dir, e))?;
        let mut text =
            serde_json::to_vec_pretty(&file).map_err(|e| Error::mod_error(format!("sets: {e}")))?;
        text.push(b'\n');
        crate::atomic::write(&self.dir.join(SETS_FILE), &text)
    }

    /// Saves the current order and enabled mods as the set `name`,
    /// replacing a set of that name.
    pub fn save_set(&self, name: &str) -> Result<()> {
        let name = validate_set_name(name)?;
        let _lock = self.hold()?;
        let mut sets = self.sets()?;
        let set = ModSet {
            name: name.to_owned(),
            mods: self.arrangement(),
        };
        match sets.iter_mut().find(|s| s.name == name) {
            Some(old) => *old = set,
            None => sets.push(set),
        }
        self.save_sets(sets)
    }

    /// Forgets the set `name`; returns whether there was one.
    pub fn delete_set(&self, name: &str) -> Result<bool> {
        let _lock = self.hold()?;
        let mut sets = self.sets()?;
        let before = sets.len();
        sets.retain(|s| s.name != name);
        if sets.len() == before {
            return Ok(false);
        }
        self.save_sets(sets)?;
        Ok(true)
    }

    /// Arranges the mods as the saved set `name` ([`Self::arrange`]);
    /// returns the ids of its mods that are not installed.
    pub fn use_set(&mut self, name: &str) -> Result<Vec<String>> {
        let set = self
            .sets()?
            .into_iter()
            .find(|s| s.name == name)
            .ok_or_else(|| Error::mod_error(format!("no set of mods `{name}`")))?;
        self.arrange(&set.mods)
    }

    /// The saved set the mods are arranged as now, if any.
    pub fn current_set(&self) -> Option<String> {
        let now = self.arrangement();
        self.sets()
            .ok()?
            .into_iter()
            .find(|s| self.arranged(&s.mods).0 == now)
            .map(|s| s.name)
    }

    /// Removes a mod like [`Self::uninstall`], but keeps its files aside so
    /// that [`Self::restore_removed`] can bring it back to its place.
    /// [`Self::discard_removed`] deletes them.
    pub fn remove_undoable(&mut self, id: &str) -> Result<RemovedMod> {
        self.changing(|manager| manager.remove_undoable_locked(id))
    }

    /// [`Self::remove_undoable`], with the lock held.
    fn remove_undoable_locked(&mut self, id: &str) -> Result<RemovedMod> {
        let index = self.position(id)?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let kept = self.dir.join(format!(
            "{REMOVED_PREFIX}{id}-{}-{stamp}",
            std::process::id()
        ));
        let dir = self.mods[index].dir.clone();
        let moved = dir.is_dir();
        if moved {
            fs::rename(&dir, &kept).map_err(|e| Error::io(&dir, e))?;
        }
        let m = self.mods.remove(index);
        if let Err(error) = self.save_registry() {
            if moved {
                let _ = fs::rename(&kept, &dir);
            }
            self.mods.insert(index, m);
            return Err(error);
        }
        Ok(RemovedMod {
            id: m.id.clone(),
            name: m.manifest.name.clone(),
            position: index,
            entry: RegistryEntry {
                id: m.id,
                enabled: m.enabled,
                installed: m.installed,
                source: m.source,
                workspace: m.workspace,
            },
            kept: moved.then_some(kept),
        })
    }

    /// Brings back a mod taken out by [`Self::remove_undoable`], in its
    /// place and with its enabled state. Refused when a mod of that id was
    /// installed since.
    pub fn restore_removed(&mut self, removed: &RemovedMod) -> Result<()> {
        self.changing(|manager| manager.restore_removed_locked(removed))
    }

    /// [`Self::restore_removed`], with the lock held.
    fn restore_removed_locked(&mut self, removed: &RemovedMod) -> Result<()> {
        if self.get(&removed.id).is_some() {
            return Err(Error::mod_error(format!(
                "a mod `{}` is installed again",
                removed.id
            )));
        }
        let dir = self.dir.join(&removed.id);
        if let Some(kept) = &removed.kept {
            if !kept.is_dir() {
                return Err(Error::mod_error(format!(
                    "the files of mod `{}` are gone",
                    removed.id
                )));
            }
            if dir.exists() {
                return Err(Error::mod_error(format!("{} is in the way", dir.display())));
            }
            fs::rename(kept, &dir).map_err(|e| Error::io(kept, e))?;
        }
        let m = load_installed(&self.dir, removed.entry.clone());
        let position = removed.position.min(self.mods.len());
        self.mods.insert(position, m);
        self.save_registry()
    }

    /// Deletes the files of a mod taken out by [`Self::remove_undoable`].
    pub fn discard_removed(&self, removed: &RemovedMod) -> Result<()> {
        match &removed.kept {
            Some(kept) if kept.is_dir() => fs::remove_dir_all(kept).map_err(|e| Error::io(kept, e)),
            _ => Ok(()),
        }
    }

    /// Deletes the files of every mod kept aside for undo, e.g. left by a
    /// program that stopped. Returns how many there were.
    pub fn purge_removed(&self) -> Result<usize> {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return Ok(0);
        };
        let mut count = 0;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(REMOVED_PREFIX) && entry.file_type().is_ok_and(|t| t.is_dir()) {
                let path = entry.path();
                fs::remove_dir_all(&path).map_err(|e| Error::io(&path, e))?;
                count += 1;
            }
        }
        Ok(count)
    }
}

/// Result of [`ModManager::sort`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SortReport {
    /// Ids in the sorted order.
    pub order: Vec<String>,
    /// Whether the order differs from the one before.
    pub changed: bool,
    /// Soft rules that could not be kept, in words.
    pub dropped: Vec<String>,
}

/// What a mod says about another one ([`ModManager::relations`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationKind {
    /// `depends`: needed.
    Depends,
    /// `recommends`: goes well with it.
    Recommends,
    /// `incompatible`: does not work with it.
    Incompatible,
    /// `load_after`.
    LoadAfter,
    /// `load_before`.
    LoadBefore,
    /// The other mod depends on this one.
    NeededBy,
}

/// How a [`Relation`] stands now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelationState {
    /// As the mod wants it: a dependency enabled, of an allowed version and
    /// earlier; an incompatible mod switched off; a rule of the order kept;
    /// a mod that needs this one enabled.
    Fine,
    /// The other mod is not installed.
    Missing,
    /// The other mod is switched off or damaged.
    Off,
    /// The other mod is of this version, which the condition does not allow
    /// (for an incompatible mod: not one of the versions it is incompatible with).
    Version(String),
    /// The load order does not keep the rule.
    OutOfOrder,
    /// Both mods are enabled although they do not work together.
    Clashing,
}

/// One relation of a mod to another ([`ModManager::relations`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relation {
    pub kind: RelationKind,
    /// Id of the other mod.
    pub id: String,
    /// Name of the other mod when it is installed.
    pub name: Option<String>,
    /// Condition on its version (`*` for the rules of the order).
    pub versions: String,
    /// Why, for an incompatible mod; may be empty.
    pub reason: String,
    pub state: RelationState,
}

/// One mod of a [`ModSet`] or of [`ModManager::arrangement`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetEntry {
    pub id: String,
    pub enabled: bool,
}

/// A named set of mods: which are enabled and in which order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModSet {
    pub name: String,
    /// In load order.
    pub mods: Vec<SetEntry>,
}

/// `sets.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SetsFile {
    format: String,
    version: u32,
    sets: Vec<ModSet>,
}

/// One installed mod against the files of the game ([`ModManager::in_game`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModInGame {
    pub id: String,
    pub name: String,
    pub version: String,
    /// SHA-256 of its manifest ([`ModManifest::sha256`]): which build of
    /// that version it is.
    pub sha256: String,
    pub enabled: bool,
    /// It cannot be used (a damaged `mod.json`), and so is never enabled.
    pub damaged: bool,
    /// It is enabled and the files of the game hold it: each of its
    /// resources and files of the archives with its data (or with the data
    /// of the enabled mod that wins it), each stream it creates.
    pub written: bool,
}

/// [`ModManager::in_game`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GameMods {
    /// Every installed mod, in load order.
    pub mods: Vec<ModInGame>,
    /// Resources the files of the game still hold changed although no
    /// enabled mod changes them any more: of mods switched off or removed
    /// since the mods were written.
    pub leftover: usize,
}

impl GameMods {
    pub fn get(&self, id: &str) -> Option<&ModInGame> {
        self.mods.iter().find(|m| m.id == id)
    }

    /// The mods the files of the game hold, in load order.
    pub fn written(&self) -> impl Iterator<Item = &ModInGame> {
        self.mods.iter().filter(|m| m.written)
    }
}

/// Longest name of a set of mods, in characters.
const MAX_SET_NAME: usize = 64;

/// The name of a set: trimmed, not empty, at most 64 characters, no
/// control characters.
fn validate_set_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_SET_NAME || name.chars().any(char::is_control)
    {
        return Err(Error::mod_error(format!(
            "`{name}` is not a name for a set of mods: 1 to 64 characters, no control characters"
        )));
    }
    Ok(name)
}

/// A mod taken out by [`ModManager::remove_undoable`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovedMod {
    pub id: String,
    pub name: String,
    position: usize,
    entry: RegistryEntry,
    /// Where its files are kept; `None` for a mod without files (damaged).
    kept: Option<PathBuf>,
}

/// Result of [`ModManager::import_old_toolkit`].
#[derive(Debug, Clone)]
pub struct OldToolkitImport {
    /// Game files whose old toolkit backup became the original.
    pub adopted: Vec<String>,
    /// Every patch file with the result of its installation.
    pub installed: Vec<(String, std::result::Result<InstallReport, String>)>,
}

/// Reads an installed mod; a damaged one is kept with its problem.
fn load_installed(dir: &Path, entry: RegistryEntry) -> InstalledMod {
    let mod_dir = dir.join(&entry.id);
    let path = mod_dir.join(MANIFEST_NAME);
    let manifest = fs::read(&path)
        .map_err(|e| Error::io(&path, e))
        .and_then(|json| ModManifest::parse(&json))
        .and_then(|m| {
            if m.id == entry.id {
                Ok(m)
            } else {
                Err(Error::mod_error(format!(
                    "{} has the id `{}`",
                    path.display(),
                    m.id
                )))
            }
        });
    let (manifest, problem) = match manifest {
        Ok(manifest) => (manifest, None),
        Err(error) => (
            ModManifest::new(
                &ModInfo {
                    id: entry.id.clone(),
                    name: entry.id.clone(),
                    version: "?".into(),
                    ..ModInfo::default()
                },
                ModGame {
                    profile: String::new(),
                    executable_size: None,
                    executable_sha256: None,
                    editions: Vec::new(),
                },
            ),
            Some(error.to_string()),
        ),
    };
    InstalledMod {
        enabled: entry.enabled && problem.is_none(),
        id: entry.id,
        installed: entry.installed,
        source: entry.source,
        workspace: entry.workspace,
        manifest,
        problem,
        dir: mod_dir,
    }
}

/// Where each GUID of the unmodified game is: the stream (index into
/// [`GameInstance::streams`]) and the resource of one occurrence.
struct GameGuids {
    guids: HashMap<[u32; 4], (usize, usize)>,
}

impl GameGuids {
    /// Reads the headers of the original of every stream of the game, except
    /// the streams in `created` (made by the mod manager).
    fn of(game: &GameInstance, created: &BTreeSet<&str>) -> Result<Self> {
        let lists = game
            .streams()
            .par_iter()
            .enumerate()
            .filter(|(_, file)| {
                !created.contains(file.stream_name().as_str()) && game.original_path(file).is_file()
            })
            .map(|(index, file)| {
                let stream = StreamFile::open(game.original_path(file))?;
                Ok(stream
                    .layout()
                    .resources
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| r.header.ids != [0; 4])
                    .map(|(i, r)| (r.header.ids, (index, i)))
                    .collect::<Vec<_>>())
            })
            .collect::<Result<Vec<_>>>()?;
        let mut guids = HashMap::new();
        for (ids, place) in lists.into_iter().flatten() {
            guids.entry(ids).or_insert(place);
        }
        Ok(Self { guids })
    }

    /// The resource of the game with the GUID of `key` when its data is not
    /// `payload`; `None` when the GUID is free or shared with the same data.
    fn other_data(
        &self,
        game: &GameInstance,
        key: &ResourceKey,
        payload: &Blob,
    ) -> Result<Option<ResourceKey>> {
        let Some(&(stream_index, index)) = self.guids.get(&key.ids) else {
            return Ok(None);
        };
        let file = &game.streams()[stream_index];
        let stream = StreamFile::open(game.original_path(file))?;
        if Blob::of(&stream.resource_data(index)?) == *payload {
            return Ok(None);
        }
        Ok(Some(ResourceKey::new(
            &file.stream_name(),
            &stream.layout().resources[index].header,
        )))
    }
}

fn write_manifest(dir: &Path, manifest: &ModManifest) -> Result<()> {
    crate::atomic::write(&dir.join(MANIFEST_NAME), &manifest.to_json()?)
}

fn check_profile(game: &GameInstance, mod_game: &ModGame) -> Result<()> {
    if mod_game.profile != game.profile().name {
        return Err(Error::mod_error(format!(
            "the mod is made for {}, not for {}",
            mod_game.profile,
            game.profile().name
        )));
    }
    Ok(())
}

/// A note when the mod was made on another build of the game.
fn executable_note(game: &GameInstance, mod_game: &ModGame) -> Result<Option<String>> {
    let fingerprint = GameFingerprint::of(game)?;
    Ok(match mod_game.same_executable(&fingerprint) {
        Some(false) => {
            let name = |size: Option<u64>, sha: Option<&str>| {
                size.zip(sha)
                    .and_then(|(size, sha)| crate::edition::edition_name(size, sha))
                    .map_or_else(|| "an unknown build".to_owned(), str::to_owned)
            };
            let made_for = name(
                mod_game.executable_size,
                mod_game.executable_sha256.as_deref(),
            );
            let this = name(
                Some(fingerprint.executable_size),
                Some(&fingerprint.executable_sha256),
            );
            Some(format!(
                "the mod was made for another edition of the game ({made_for}; this game is \
                 {this}); it may still work, the resources are checked one by one"
            ))
        }
        _ => None,
    })
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

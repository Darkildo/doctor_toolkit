//! The workspace: edits kept on disk with their history, applied to the game
//! on demand.
//!
//! Edits are not written into the game files directly. Each edit replaces the
//! data of one resource, identified by a [`ResourceKey`]; the new data is
//! stored in the workspace directory together with the original data and a
//! log of operations. The log gives undo and redo; the stored originals give a
//! diff without the game. [`Workspace::apply`] writes the edits into the game,
//! one rewrite per stream; [`Workspace::unapply`] puts the original files back.
//! Writing straight into the game ("direct mode") is recording an edit and
//! applying its stream at once.
//!
//! # Layout
//!
//! ```text
//! <game>/_doctor_toolkit/workspace/      default location, any directory works
//! ├── manifest.json     format, version, game fingerprint, current changes
//! ├── log.json          operations with their before/after state, undo position
//! ├── applied.json      what has been written into the game files
//! └── payloads/ab/ab12…  resource data, named by SHA-256
//! ```
//!
//! `manifest.json` alone describes the result of the edits: which resources
//! are replaced and by which payload. A mod is that manifest plus the payload
//! files it names. See `docs/workspace.md` for the formats.
//!
//! Every file is replaced atomically. The manifest records the sequence
//! number of the last operation in effect, so a crash between writing the log
//! and the manifest is repaired when the workspace is opened again.

pub mod deploy;
pub mod diff;
pub mod files;
pub mod key;
pub mod lock;
pub mod store;

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::container::ustream::{ResourceHeader, Storage, StreamLayout};
use crate::game::{GameFile, GameInstance, StreamFile};
use crate::resource::lh2::StringTable;
use crate::resource::texture::{self, Texture, TexturePair};
use crate::{Error, Result};

pub use deploy::{AppliedStream, DeployStatus, Deployment};
pub use diff::{
    ApxDiff, BlockChange, ByteDiff, LogicDiff, SemanticDiff, StringChange, StringTableDiff,
    TextureDiff, ValueChange,
};
pub use files::{FileChange, FileEdit, FileKey};
pub use key::ResourceKey;
pub use lock::{LockHolder, WorkspaceLock};
pub use store::{Blob, FileStamp, PayloadStore};

/// Value of the `format` field of `manifest.json`.
pub const FORMAT: &str = "doctor-toolkit-workspace";
/// Current version of the workspace format. Version 2 added new resources
/// ([`Change::added`]) and new streams ([`Manifest::streams`]); version 1
/// workspaces are read as they are and saved as version 2.
pub const VERSION: u32 = 2;
/// Default location of the workspace, relative to the game directory.
pub const DEFAULT_DIR: &str = "_doctor_toolkit/workspace";

const MANIFEST_FILE: &str = "manifest.json";
const LOG_FILE: &str = "log.json";
const APPLIED_FILE: &str = "applied.json";
const PAYLOAD_DIR: &str = "payloads";

/// How the new data of a resource is stored in its stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StorageChoice {
    /// Like the original resource: compressed if it was compressed.
    #[default]
    Keep,
    /// Uncompressed `SDAT` blocks.
    Raw,
    /// RefPack-compressed `Rpak` blocks.
    Refpack,
}

impl StorageChoice {
    pub fn is_keep(&self) -> bool {
        *self == Self::Keep
    }

    fn resolve(self, original_compressed: bool) -> Storage {
        match self {
            Self::Keep if original_compressed => Storage::RefPack,
            Self::Keep | Self::Raw => Storage::Raw,
            Self::Refpack => Storage::RefPack,
        }
    }
}

/// One resource whose data the workspace replaces or adds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub key: ResourceKey,
    /// The new data.
    pub payload: Blob,
    /// The data of the resource in the unmodified game; empty for an added resource.
    pub original: Blob,
    #[serde(default, skip_serializing_if = "StorageChoice::is_keep")]
    pub storage: StorageChoice,
    /// Label of the operation that made this change.
    #[serde(default)]
    pub label: String,
    /// Set when the resource is not in the game: the workspace adds it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added: Option<Added>,
}

impl Change {
    pub fn is_added(&self) -> bool {
        self.added.is_some()
    }
}

/// Where and how an added resource goes into its stream. The type and the
/// ids are in the key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Added {
    /// SDBM hash of the memory policy name.
    pub mem_policy: u32,
    pub alignment: u32,
    /// The resource of the original stream the new one is placed before; at
    /// the end of the stream when absent or no longer there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<ResourceKey>,
    /// Order among resources added at the same place: lower first.
    pub order: u64,
}

impl Added {
    /// The resource header of an added resource with key `key`.
    pub fn header(&self, key: &ResourceKey, data_size: u64) -> ResourceHeader {
        ResourceHeader {
            mem_policy: self.mem_policy,
            alignment: self.alignment,
            type_id: key.type_id,
            ids: key.ids,
            data_size: data_size as u32,
        }
    }
}

/// A stream the workspace creates, with its entries in the tables of contents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewStream {
    /// Stream name as in [`ResourceKey::stream`], e.g. `gf2/doctor/props`.
    pub name: String,
    /// 128-bit GUID as 32 hexadecimal digits; its fold is the table-of-contents hash.
    pub guid: String,
    /// [`crate::toc::guid_hash`] of the GUID.
    pub hash: u32,
    /// The stream it depends on: loaded before it, and, when `with_parent`
    /// is set, the stream that loads it.
    pub parent: String,
    /// A "parented stream" of `parent` (table-of-contents flags `0,0,5,0`,
    /// a child link in the parent's entry), as the game's mission streams.
    /// The game does not open such a stream with every load of the parent
    /// (stage 24, `docs/analysis/new-resources.md` §3.1). Otherwise the
    /// stream is loaded when game data asks for its hash (flags `0,0,1,0`).
    #[serde(default)]
    pub with_parent: bool,
    /// Tables of contents that list the stream, relative paths.
    pub tocs: Vec<String>,
    /// A different parent in some tables of contents (relative path → stream
    /// name): text streams depend on `gf2/frontend` in `frontend.toc` and on
    /// `gf2/globals_global` in `globals_global.toc`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub toc_parents: BTreeMap<String, String>,
    #[serde(default)]
    pub label: String,
}

impl NewStream {
    /// Path of the stream file relative to the game directory.
    pub fn rel_path(&self) -> String {
        format!("{}.str", self.name)
    }

    /// The parent in table of contents `toc` (a relative path).
    pub fn parent_in(&self, toc: &str) -> &str {
        self.toc_parents
            .iter()
            .find(|(t, _)| t.eq_ignore_ascii_case(toc))
            .map_or(&self.parent, |(_, p)| p)
    }

    /// The name as tables of contents spell it.
    pub fn toc_name(&self) -> String {
        self.name.replace('/', "\\")
    }
}

/// Sorts the resources of a new stream into the order of its file: in the
/// order they were added, SimGroups last. The game's streams hold the
/// targets of references (textures, materials, meshes, collision, models)
/// before the SimGroups that place them (`docs/analysis/new-resources.md`
/// §2), so an object added before its model was imported still comes after
/// it.
pub fn file_order(changes: &mut [&Change]) {
    changes.sort_by_key(|c| {
        (
            c.key.type_id == crate::level::TYPE_SIMGROUP,
            c.added.as_ref().map_or(0, |a| a.order),
        )
    });
}

/// The change of one new stream made by an operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamEdit {
    pub name: String,
    pub before: Option<NewStream>,
    pub after: Option<NewStream>,
}

/// Identifies the game a workspace was made for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameFingerprint {
    /// Profile name, e.g. `The Godfather II (PC)`.
    pub profile: String,
    /// File name of the game executable.
    pub executable: String,
    pub executable_size: u64,
    pub executable_sha256: String,
}

impl GameFingerprint {
    /// Fingerprint of an installed game: its profile and executable.
    pub fn of(game: &GameInstance) -> Result<Self> {
        let profile = game.profile();
        let executable = fs::read_dir(game.root())
            .map_err(|e| Error::io(game.root(), e))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.eq_ignore_ascii_case(profile.executable))
            });
        let (executable_size, executable_sha256) = match &executable {
            Some(path) => {
                let stamp = FileStamp::of_file(path)?;
                (stamp.size, stamp.sha256)
            }
            None => (0, String::new()),
        };
        Ok(Self {
            profile: profile.name.to_owned(),
            executable: profile.executable.to_owned(),
            executable_size,
            executable_sha256,
        })
    }
}

/// `manifest.json`: the current result of all edits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: String,
    pub version: u32,
    pub game: GameFingerprint,
    /// Incremented on every save; detects a second program editing the same workspace.
    pub revision: u64,
    /// Sequence number of the last operation in effect, 0 when none is.
    pub log_seq: u64,
    /// Replaced and added resources, sorted by key.
    pub resources: Vec<Change>,
    /// Streams the workspace creates, sorted by name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub streams: Vec<NewStream>,
    /// Replaced files outside the streams (archive entries, movies), sorted by key.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<FileChange>,
}

/// One undoable step: one or more resources changed together.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Operation {
    /// Unique, increasing number.
    pub seq: u64,
    /// Seconds since the Unix epoch.
    pub time: u64,
    pub label: String,
    pub edits: Vec<Edit>,
    /// New streams created or removed by the operation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stream_edits: Vec<StreamEdit>,
    /// Files outside the streams changed by the operation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub file_edits: Vec<FileEdit>,
}

/// The change of one resource made by an operation. `None` means the resource
/// is not replaced (it has its original data).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edit {
    pub key: ResourceKey,
    pub before: Option<Change>,
    pub after: Option<Change>,
}

/// `log.json`. `operations[..head]` are in effect; the rest can be redone.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Log {
    head: usize,
    next_seq: u64,
    operations: Vec<Operation>,
}

/// A resource to add, for [`Workspace::add_resources`].
#[derive(Debug, Clone)]
pub struct NewResourceEdit {
    /// Stream name, as in [`ResourceKey::stream`].
    pub stream: String,
    /// Type, ids, memory policy and alignment; `data_size` is ignored.
    pub header: ResourceHeader,
    pub data: Vec<u8>,
    pub storage: StorageChoice,
    /// Place it before this resource of the original stream; at the end when `None`.
    pub before: Option<ResourceKey>,
}

/// New data for one resource of a stream, for [`Workspace::set_resources`].
#[derive(Debug, Clone)]
pub struct ResourceEdit {
    /// Index of the resource in the stream.
    pub index: usize,
    pub data: Vec<u8>,
    pub storage: StorageChoice,
}

/// State of a game stream relative to the workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamState {
    /// The file is the unmodified original.
    Original,
    /// The file is what the workspace wrote.
    Applied,
    /// The file was changed by something else since it was original or applied.
    ChangedExternally,
    /// The stream is not in the game.
    Missing,
}

/// State of one recorded change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeState {
    /// Only in the workspace; the game has other data.
    Pending,
    /// The game has this data.
    Applied,
    /// The stream was changed outside the workspace; applying needs `force`.
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamStatus {
    pub stream: String,
    pub state: StreamState,
    /// The game file matches the workspace: nothing to apply.
    pub up_to_date: bool,
}

/// Result of [`Workspace::status`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// Whether the workspace was made for this game.
    pub fingerprint_matches: bool,
    pub streams: Vec<StreamStatus>,
    pub changes: Vec<(ResourceKey, ChangeState)>,
    /// Resources the game still has edited although the workspace no longer replaces them.
    pub pending_reverts: Vec<ResourceKey>,
    /// Files outside the streams and whether the game has them.
    pub files: Vec<(FileKey, ChangeState)>,
    /// Game files (relative paths) still written although no change wants them.
    pub pending_file_reverts: Vec<String>,
}

impl Status {
    /// Changes of resources and files in this state.
    pub fn count(&self, state: ChangeState) -> usize {
        self.changes
            .iter()
            .map(|(_, s)| s)
            .chain(self.files.iter().map(|(_, s)| s))
            .filter(|s| **s == state)
            .count()
    }

    /// Whether the game matches the workspace.
    pub fn is_up_to_date(&self) -> bool {
        self.streams.iter().all(|s| s.up_to_date)
            && self.files.iter().all(|(_, s)| *s == ChangeState::Applied)
            && self.pending_file_reverts.is_empty()
    }

    /// Streams changed outside the workspace that hold edits.
    pub fn conflicts(&self) -> Vec<&str> {
        self.streams
            .iter()
            .filter(|s| s.state == StreamState::ChangedExternally && !s.up_to_date)
            .map(|s| s.stream.as_str())
            .collect()
    }

    pub fn state_of(&self, key: &ResourceKey) -> Option<ChangeState> {
        self.changes.iter().find(|(k, _)| k == key).map(|(_, s)| *s)
    }
}

/// Options for [`Workspace::apply`].
#[derive(Debug, Clone, Default)]
pub struct ApplyOptions {
    /// Overwrite streams changed outside the workspace; their foreign changes are lost.
    pub force: bool,
    /// Only these streams (names as in [`ResourceKey::stream`]); all when `None`.
    /// Files outside the streams ([`FileChange`]) are applied when this is
    /// `None` or names [`FILES`].
    pub streams: Option<Vec<String>>,
}

/// Name for [`ApplyOptions::streams`] that selects the files outside the streams.
pub const FILES: &str = "files";

/// What happened to one stream during apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamAction {
    /// Rewritten with this many replaced resources.
    Rebuilt(usize),
    /// Put back to the original file.
    Restored,
}

/// Result of [`Workspace::apply`] and [`Workspace::unapply`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApplyReport {
    pub streams: Vec<(String, StreamAction)>,
    /// Relative paths of the files written.
    pub written: Vec<String>,
    /// Relative paths of the files backed up.
    pub backed_up: Vec<String>,
    /// Problems that did not stop the operation.
    pub warnings: Vec<String>,
}

impl ApplyReport {
    /// Adds the actions of another report.
    pub fn merge(&mut self, other: ApplyReport) {
        self.streams.extend(other.streams);
        self.written.extend(other.written);
        self.backed_up.extend(other.backed_up);
        self.warnings.extend(other.warnings);
    }
}

/// One line of [`Workspace::diff`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffEntry {
    pub key: ResourceKey,
    pub label: String,
    pub original_size: u64,
    pub new_size: u64,
    /// The resource is new, not in the game.
    pub added: bool,
}

/// Detailed comparison of one change, from [`Workspace::detail`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeDetail {
    pub bytes: ByteDiff,
    pub semantic: Option<SemanticDiff>,
}

/// An open workspace.
#[derive(Debug)]
pub struct Workspace {
    dir: PathBuf,
    manifest: Manifest,
    state: BTreeMap<ResourceKey, Change>,
    new_streams: BTreeMap<String, NewStream>,
    files: BTreeMap<FileKey, FileChange>,
    log: Log,
    applied: Deployment,
    store: PayloadStore,
    /// Whether the workspace is on disk. One made by [`Self::for_game`] is
    /// written by its first operation, so that a command that records
    /// nothing leaves no workspace behind.
    saved: bool,
}

impl Workspace {
    /// The default workspace directory of a game.
    pub fn default_dir(game_root: &Path) -> PathBuf {
        game_root.join(DEFAULT_DIR)
    }

    /// Whether `dir` holds a workspace.
    pub fn exists(dir: &Path) -> bool {
        dir.join(MANIFEST_FILE).is_file()
    }

    /// Creates an empty workspace for `game` in `dir`.
    pub fn create(dir: impl Into<PathBuf>, game: &GameInstance) -> Result<Self> {
        let mut workspace = Self::unsaved(dir.into(), game)?;
        workspace.write_new()?;
        Ok(workspace)
    }

    /// An empty workspace for `game` in `dir` that is not written yet: the
    /// first recorded operation writes it.
    fn unsaved(dir: PathBuf, game: &GameInstance) -> Result<Self> {
        if Self::exists(&dir) {
            return Err(Error::InvalidArgument(format!(
                "{} already holds a workspace",
                dir.display()
            )));
        }
        Ok(Self {
            store: PayloadStore::staged(dir.join(PAYLOAD_DIR)),
            manifest: Manifest {
                format: FORMAT.to_owned(),
                version: VERSION,
                game: GameFingerprint::of(game)?,
                revision: 0,
                log_seq: 0,
                resources: Vec::new(),
                streams: Vec::new(),
                files: Vec::new(),
            },
            dir,
            state: BTreeMap::new(),
            new_streams: BTreeMap::new(),
            files: BTreeMap::new(),
            log: Log {
                head: 0,
                next_seq: 1,
                operations: Vec::new(),
            },
            applied: Deployment::default(),
            saved: false,
        })
    }

    /// Writes a workspace made by [`Self::unsaved`].
    fn write_new(&mut self) -> Result<()> {
        if Self::exists(&self.dir) {
            return Err(Error::InvalidArgument(format!(
                "another program created a workspace in {}; reopen it",
                self.dir.display()
            )));
        }
        fs::create_dir_all(&self.dir).map_err(|e| Error::io(&self.dir, e))?;
        self.store.flush()?;
        self.saved = true;
        self.save()?;
        self.save_applied()
    }

    /// Whether the workspace is on disk; see [`Self::for_game`].
    pub fn is_saved(&self) -> bool {
        self.saved
    }

    /// Opens the workspace in `dir`.
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self> {
        let mut workspace = Self::read(dir.into())?;
        if workspace.reconcile() {
            workspace.save()?;
        }
        Ok(workspace)
    }

    /// Opens the workspace in `dir` for reading only: nothing is written,
    /// not even the repair of a save that was interrupted (the repaired
    /// state is kept in memory). Recording operations on it is a mistake;
    /// use [`Self::open`] for that.
    pub fn open_read_only(dir: impl Into<PathBuf>) -> Result<Self> {
        let mut workspace = Self::read(dir.into())?;
        workspace.reconcile();
        Ok(workspace)
    }

    fn read(dir: PathBuf) -> Result<Self> {
        let mut manifest: Manifest = read_json(&dir.join(MANIFEST_FILE))?;
        if manifest.format != FORMAT {
            return Err(Error::InvalidArgument(format!(
                "{} is not a workspace manifest",
                dir.join(MANIFEST_FILE).display()
            )));
        }
        if manifest.version > VERSION {
            return Err(Error::UnsupportedVersion {
                what: "workspace",
                version: manifest.version,
            });
        }
        let log_path = dir.join(LOG_FILE);
        let log: Log = if log_path.is_file() {
            read_json(&log_path)?
        } else {
            Log::default()
        };
        let applied = Deployment::load(&dir.join(APPLIED_FILE))?;
        let state = manifest
            .resources
            .iter()
            .map(|c| (c.key.clone(), c.clone()))
            .collect();
        let new_streams = manifest
            .streams
            .iter()
            .map(|s| (s.name.clone(), s.clone()))
            .collect();
        let files = manifest
            .files
            .iter()
            .map(|f| (f.key.clone(), f.clone()))
            .collect();
        // Older versions are read as they are and saved in the current version.
        manifest.version = VERSION;
        let workspace = Self {
            store: PayloadStore::new(dir.join(PAYLOAD_DIR)),
            dir,
            manifest,
            state,
            new_streams,
            files,
            log,
            applied,
            saved: true,
        };
        Ok(workspace)
    }

    /// Opens the workspace in `dir`, creating it for `game` if there is none.
    pub fn open_or_create(dir: impl Into<PathBuf>, game: &GameInstance) -> Result<Self> {
        let dir = dir.into();
        if Self::exists(&dir) {
            Self::open(dir)
        } else {
            Self::create(dir, game)
        }
    }

    /// Opens the workspace of `game` (in `dir`, or in the default location),
    /// or makes a new one there that is written to disk only by its first
    /// recorded operation ([`Self::is_saved`]): a command that fails or
    /// records nothing leaves no workspace behind.
    pub fn for_game(game: &GameInstance, dir: Option<&Path>) -> Result<Self> {
        let dir = dir.map_or_else(|| Self::default_dir(game.root()), Path::to_path_buf);
        if Self::exists(&dir) {
            Self::open(dir)
        } else {
            Self::unsaved(dir, game)
        }
    }

    /// Makes the manifest agree with the log after an interrupted save. The
    /// log is written first, so it is the newer of the two. Returns whether
    /// anything changed (and should be saved).
    fn reconcile(&mut self) -> bool {
        let head = self.log.head.min(self.log.operations.len());
        self.log.head = head;
        let seq_in_effect = head
            .checked_sub(1)
            .map_or(0, |i| self.log.operations[i].seq);
        if seq_in_effect == self.manifest.log_seq {
            return false;
        }
        // Number of operations the manifest's state includes.
        let position = if self.manifest.log_seq == 0 {
            Some(0)
        } else {
            self.log
                .operations
                .iter()
                .position(|op| op.seq == self.manifest.log_seq)
                .map(|i| i + 1)
        };
        match position {
            Some(position) if position < head => {
                for op in &self.log.operations[position..head] {
                    apply_edits(
                        &mut self.state,
                        &mut self.new_streams,
                        &mut self.files,
                        op,
                        true,
                    );
                }
            }
            Some(position) => {
                for op in self.log.operations[head..position].iter().rev() {
                    apply_edits(
                        &mut self.state,
                        &mut self.new_streams,
                        &mut self.files,
                        op,
                        false,
                    );
                }
            }
            // The log does not know the manifest's state: keep the state, drop the history.
            None => {
                self.log.operations.clear();
                self.log.head = 0;
            }
        }
        true
    }

    /// Re-reads the workspace from disk, e.g. after another program changed it.
    pub fn reload(&mut self) -> Result<()> {
        if !self.saved && !Self::exists(&self.dir) {
            return Ok(());
        }
        *self = Self::open(self.dir.clone())?;
        Ok(())
    }

    /// The workspace directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The payload store.
    pub fn store(&self) -> &PayloadStore {
        &self.store
    }

    /// Current changes, sorted by key.
    pub fn changes(&self) -> impl Iterator<Item = &Change> {
        self.state.values()
    }

    pub fn change(&self, key: &ResourceKey) -> Option<&Change> {
        self.state.get(key)
    }

    /// Streams the workspace creates, sorted by name.
    pub fn new_streams(&self) -> impl Iterator<Item = &NewStream> {
        self.new_streams.values()
    }

    pub fn new_stream(&self, name: &str) -> Option<&NewStream> {
        self.new_streams.get(&key::normalize_stream(name))
    }

    /// Stream `name` and the streams loaded before it (its ancestors in the
    /// tables of contents; for a stream the workspace creates, its parents
    /// and their ancestors), given the parents of the game's streams
    /// ([`GameInstance::stream_parents`]).
    pub fn load_scope(
        &self,
        parents: &std::collections::HashMap<String, Vec<String>>,
        name: &str,
    ) -> Vec<String> {
        let name = key::normalize_stream(name);
        let mut out = vec![name.clone()];
        let roots: Vec<String> = match self.new_stream(&name) {
            Some(s) => std::iter::once(&s.parent)
                .chain(s.toc_parents.values())
                .cloned()
                .collect(),
            None => vec![name],
        };
        for root in roots {
            if !out.contains(&root) {
                out.push(root.clone());
            }
            for a in crate::game::ancestors(parents, &root) {
                if !out.contains(&a) {
                    out.push(a);
                }
            }
        }
        out
    }

    /// The resources of the new stream `name` in the order its file holds
    /// them ([`file_order`]); their positions are the resource indices of
    /// the stream once written.
    pub fn new_stream_resources(&self, name: &str) -> Vec<&Change> {
        let name = key::normalize_stream(name);
        let mut list: Vec<&Change> = self
            .state
            .values()
            .filter(|c| c.key.stream == name && c.is_added())
            .collect();
        file_order(&mut list);
        list
    }

    /// Number of changed resources.
    pub fn len(&self) -> usize {
        self.state.len()
    }

    pub fn is_empty(&self) -> bool {
        self.state.is_empty() && self.new_streams.is_empty() && self.files.is_empty()
    }

    /// All operations, including those that were undone.
    pub fn operations(&self) -> &[Operation] {
        &self.log.operations
    }

    /// Number of operations in effect; the rest can be redone.
    pub fn head(&self) -> usize {
        self.log.head
    }

    /// Label of the operation [`Self::undo`] would undo.
    pub fn undo_label(&self) -> Option<&str> {
        let i = self.log.head.checked_sub(1)?;
        Some(&self.log.operations[i].label)
    }

    /// Label of the operation [`Self::redo`] would redo.
    pub fn redo_label(&self) -> Option<&str> {
        self.log
            .operations
            .get(self.log.head)
            .map(|op| op.label.as_str())
    }

    /// The edited data of a resource; `None` when the workspace does not change it.
    pub fn data(&self, key: &ResourceKey) -> Result<Option<Vec<u8>>> {
        self.state
            .get(key)
            .map(|c| self.store.get(&c.payload))
            .transpose()
    }

    /// The original data of a changed resource.
    pub fn original_data(&self, key: &ResourceKey) -> Result<Option<Vec<u8>>> {
        self.state
            .get(key)
            .map(|c| self.store.get(&c.original))
            .transpose()
    }

    /// Record of a stream the workspace has written into the game.
    pub fn applied_stream(&self, stream: &str) -> Option<&AppliedStream> {
        self.applied.streams.get(stream)
    }

    // ------------------------------------------------------------- recording

    /// Records new data for resources of stream `stream_index` as one
    /// undoable operation. Data equal to the original removes the change.
    /// Returns the sequence number of the operation, or `None` when nothing changed.
    pub fn set_resources(
        &mut self,
        game: &GameInstance,
        label: &str,
        stream_index: usize,
        edits: &[ResourceEdit],
    ) -> Result<Option<u64>> {
        let planned = self.plan_set(game, label, stream_index, edits)?;
        if planned.is_empty() {
            return Ok(None);
        }
        self.commit(label, planned, Vec::new()).map(Some)
    }

    /// Records changed and new resources of several streams as one undoable
    /// operation: `edits` as in [`Self::set_resources`] per stream index,
    /// `additions` as in [`Self::add_resources`]. Returns the sequence number
    /// of the operation, or `None` when nothing changed.
    pub fn change_resources(
        &mut self,
        game: &GameInstance,
        label: &str,
        edits: &[(usize, Vec<ResourceEdit>)],
        additions: &[NewResourceEdit],
    ) -> Result<Option<u64>> {
        let mut planned = Vec::new();
        for (stream_index, list) in edits {
            planned.extend(self.plan_set(game, label, *stream_index, list)?);
        }
        if !additions.is_empty() {
            planned.extend(self.plan_add(game, label, additions)?);
        }
        let mut keys = HashSet::new();
        if let Some(edit) = planned.iter().find(|e| !keys.insert(e.key.clone())) {
            return Err(Error::InvalidArgument(format!(
                "{} is edited twice",
                edit.key
            )));
        }
        if planned.is_empty() {
            return Ok(None);
        }
        self.commit(label, planned, Vec::new()).map(Some)
    }

    /// As [`Self::change_resources`], and also replaces (`Some`) or removes
    /// (`None`) resources this workspace added earlier, by key: all as one
    /// undoable operation. Used to update a resource made by the workspace
    /// whether or not it was applied (a visual script, `docs/scripts.md`).
    pub fn change_resources_and_added(
        &mut self,
        game: &GameInstance,
        label: &str,
        edits: &[(usize, Vec<ResourceEdit>)],
        additions: &[NewResourceEdit],
        added: &[(ResourceKey, Option<Vec<u8>>)],
    ) -> Result<Option<u64>> {
        let mut planned = Vec::new();
        for (stream_index, list) in edits {
            planned.extend(self.plan_set(game, label, *stream_index, list)?);
        }
        if !additions.is_empty() {
            planned.extend(self.plan_add(game, label, additions)?);
        }
        for (key, data) in added {
            let before = match self.state.get(key) {
                Some(c) if c.is_added() => c.clone(),
                _ => {
                    return Err(Error::InvalidArgument(format!(
                        "{key} is not a resource added by the workspace"
                    )));
                }
            };
            let after = match data {
                Some(d) => {
                    let payload = self.store.put(d)?;
                    if payload == before.payload {
                        continue;
                    }
                    Some(Change {
                        payload,
                        label: label.to_owned(),
                        ..before.clone()
                    })
                }
                None => None,
            };
            planned.push(Edit {
                key: key.clone(),
                before: Some(before),
                after,
            });
        }
        let mut keys = HashSet::new();
        if let Some(edit) = planned.iter().find(|e| !keys.insert(e.key.clone())) {
            return Err(Error::InvalidArgument(format!(
                "{} is edited twice",
                edit.key
            )));
        }
        if planned.is_empty() {
            return Ok(None);
        }
        self.commit(label, planned, Vec::new()).map(Some)
    }

    fn plan_set(
        &mut self,
        game: &GameInstance,
        label: &str,
        stream_index: usize,
        edits: &[ResourceEdit],
    ) -> Result<Vec<Edit>> {
        let file = stream_file(game, stream_index)?;
        let name = file.stream_name();
        let current = StreamFile::open(&file.path)?;
        let original_path = game.original_path(file);
        let original = if original_path == file.path {
            None
        } else {
            Some(StreamFile::open(&original_path)?)
        };
        let original = original.as_ref().unwrap_or(&current);

        let mut planned = Vec::new();
        let mut seen = HashSet::new();
        for edit in edits {
            let resource = current.layout().resources.get(edit.index).ok_or_else(|| {
                Error::InvalidArgument(format!("{name} has no resource {}", edit.index))
            })?;
            let key = ResourceKey::new(&name, &resource.header);
            if !seen.insert(key.clone()) {
                return Err(Error::InvalidArgument(format!("{key} is edited twice")));
            }
            let before = self.state.get(&key).cloned();
            let original_blob = match &before {
                Some(change) => change.original.clone(),
                None => {
                    let index = key.find_in(original.layout()).ok_or_else(|| {
                        Error::InvalidArgument(format!("{key} is not in the original stream"))
                    })?;
                    self.store.put(&original.resource_data(index)?)?
                }
            };
            let payload = Blob::of(&edit.data);
            let added = before.as_ref().and_then(|c| c.added.clone());
            let after = if added.is_none()
                && payload == original_blob
                && edit.storage == StorageChoice::Keep
            {
                None
            } else {
                self.store.put(&edit.data)?;
                Some(Change {
                    key: key.clone(),
                    payload,
                    original: original_blob,
                    storage: edit.storage,
                    label: label.to_owned(),
                    added,
                })
            };
            let unchanged = match (&before, &after) {
                (None, None) => true,
                (Some(b), Some(a)) => b.payload == a.payload && b.storage == a.storage,
                _ => false,
            };
            if !unchanged {
                planned.push(Edit { key, before, after });
            }
        }
        Ok(planned)
    }

    /// Records new resources as one undoable operation. The resources are
    /// written into their streams by [`Self::apply`]. A stream is either a
    /// stream of the game or one created with [`Self::add_stream`].
    ///
    /// The GUID of each resource (its key) must not be in use: the game loads
    /// only one resource per GUID. Allocate ids with [`Self::used_ids`] and
    /// [`crate::create::UsedIds`]. Returns the sequence number of the operation.
    pub fn add_resources(
        &mut self,
        game: &GameInstance,
        label: &str,
        resources: &[NewResourceEdit],
    ) -> Result<u64> {
        let planned = self.plan_add(game, label, resources)?;
        self.commit(label, planned, Vec::new())
    }

    fn plan_add(
        &mut self,
        game: &GameInstance,
        label: &str,
        resources: &[NewResourceEdit],
    ) -> Result<Vec<Edit>> {
        let mut planned = Vec::new();
        let mut seen = HashSet::new();
        let mut layouts: BTreeMap<String, Option<StreamLayout>> = BTreeMap::new();
        let base_order = self.log.next_seq.max(1) * 1_000_000;
        for (i, resource) in resources.iter().enumerate() {
            let stream = key::normalize_stream(&resource.stream);
            let key = ResourceKey {
                stream: stream.clone(),
                type_id: resource.header.type_id,
                ids: resource.header.ids,
            };
            if !seen.insert(key.clone()) || self.state.contains_key(&key) {
                return Err(Error::InvalidArgument(format!(
                    "{key} is already in the workspace"
                )));
            }
            if !layouts.contains_key(&stream) {
                let layout = if self.new_streams.contains_key(&stream) {
                    None
                } else {
                    let index = game.find_stream(&stream).ok_or_else(|| {
                        Error::InvalidArgument(format!(
                            "stream {stream} is neither in the game nor new"
                        ))
                    })?;
                    let path = game.original_path(&game.streams()[index]);
                    let data = fs::read(&path).map_err(|e| Error::io(&path, e))?;
                    Some(StreamLayout::parse(&data)?)
                };
                layouts.insert(stream.clone(), layout);
            }
            if let Some(layout) = &layouts[&stream] {
                if key.find_in(layout).is_some() {
                    return Err(Error::InvalidArgument(format!(
                        "{key} is already in the stream"
                    )));
                }
                if let Some(before) = &resource.before
                    && (before.stream != stream || before.find_in(layout).is_none())
                {
                    return Err(Error::InvalidArgument(format!(
                        "{before} is not a resource of the original {stream}"
                    )));
                }
            } else if resource.before.is_some() {
                return Err(Error::InvalidArgument(format!(
                    "{stream} is new: its resources are written in the order they were added"
                )));
            }
            let payload = self.store.put(&resource.data)?;
            planned.push(Edit {
                key: key.clone(),
                before: None,
                after: Some(Change {
                    key,
                    payload,
                    original: Blob::of(&[]),
                    storage: resource.storage,
                    label: label.to_owned(),
                    added: Some(Added {
                        mem_policy: resource.header.mem_policy,
                        alignment: resource.header.alignment,
                        before: resource.before.clone(),
                        order: base_order + i as u64,
                    }),
                }),
            });
        }
        if planned.is_empty() {
            return Err(Error::InvalidArgument("no resources to add".into()));
        }
        Ok(planned)
    }

    /// Records a new stream as one undoable operation; [`Self::apply`]
    /// creates the file and adds it to the tables of contents `stream.tocs`.
    pub fn add_stream(&mut self, game: &GameInstance, stream: NewStream) -> Result<u64> {
        let mut stream = stream;
        stream.name = key::normalize_stream(&stream.name);
        stream.parent = key::normalize_stream(&stream.parent);
        for parent in stream.toc_parents.values_mut() {
            *parent = key::normalize_stream(parent);
        }
        if stream.name.is_empty()
            || stream.name.contains("..")
            || stream.name.starts_with('/')
            || stream.name.contains(':')
        {
            return Err(Error::InvalidArgument(format!(
                "bad stream name {:?}",
                stream.name
            )));
        }
        if game.find_stream(&stream.name).is_some()
            || game.root().join(stream.rel_path()).exists()
            || self.new_streams.contains_key(&stream.name)
        {
            return Err(Error::InvalidArgument(format!(
                "stream {} already exists",
                stream.name
            )));
        }
        if stream.hash == 0 {
            return Err(Error::InvalidArgument(
                "a new stream needs a non-zero hash".into(),
            ));
        }
        if stream.tocs.is_empty() {
            return Err(Error::InvalidArgument(format!(
                "no table of contents lists {}",
                stream.parent
            )));
        }
        for rel in &stream.tocs {
            let index = game
                .tocs()
                .iter()
                .position(|t| t.rel_path.eq_ignore_ascii_case(rel))
                .ok_or_else(|| Error::InvalidArgument(format!("no table of contents {rel}")))?;
            let table = game.load_toc(index)?;
            let parent = stream.parent_in(rel);
            if table.find_file(parent).is_none() {
                return Err(Error::InvalidArgument(format!(
                    "{rel} does not list {parent}"
                )));
            }
            if table.names.iter().any(|n| n.hash == stream.hash) {
                return Err(Error::InvalidArgument(format!(
                    "{rel} already has a stream with hash {:08x}",
                    stream.hash
                )));
            }
        }
        if stream.label.is_empty() {
            stream.label = format!("Add stream {}", stream.name);
        }
        let label = stream.label.clone();
        let edit = StreamEdit {
            name: stream.name.clone(),
            before: None,
            after: Some(stream),
        };
        self.commit(&label, Vec::new(), vec![edit])
    }

    /// Removes a new stream and every resource added to it, as one undoable operation.
    pub fn remove_stream(&mut self, name: &str) -> Result<bool> {
        let name = key::normalize_stream(name);
        let Some(stream) = self.new_streams.get(&name).cloned() else {
            return Ok(false);
        };
        let edits = self
            .state
            .values()
            .filter(|c| c.key.stream == name)
            .map(|c| Edit {
                key: c.key.clone(),
                before: Some(c.clone()),
                after: None,
            })
            .collect();
        let label = format!("Remove stream {name}");
        self.commit(
            &label,
            edits,
            vec![StreamEdit {
                name,
                before: Some(stream),
                after: None,
            }],
        )?;
        Ok(true)
    }

    /// The identifiers in use by the game and by resources added in the
    /// workspace; new ids must avoid them.
    pub fn used_ids(&self, index: &crate::game::ResourceIndex) -> crate::create::UsedIds {
        let mut used = crate::create::UsedIds::from_index(index);
        for change in self.state.values().filter(|c| c.is_added()) {
            used.insert(&change.key.ids);
        }
        used
    }

    /// Records a texture imported from a DDS or PNG file; both resources of
    /// the texture change in one operation. Returns the new texture.
    pub fn import_texture(
        &mut self,
        game: &GameInstance,
        stream_index: usize,
        pair: TexturePair,
        image: &[u8],
        is_png: bool,
    ) -> Result<Texture> {
        let stream = game.open_stream(stream_index)?;
        let original = Texture::load(stream.data(), stream.layout(), pair)?;
        drop(stream);
        let replacement = if is_png {
            texture::from_png(&original.header, image)?
        } else {
            texture::from_dds(&original.header, image)?
        };
        let label = format!("Import texture {}", replacement.header.name);
        self.set_resources(
            game,
            &label,
            stream_index,
            &[
                ResourceEdit {
                    index: pair.header,
                    data: replacement.header.to_bytes()?,
                    storage: StorageChoice::Keep,
                },
                ResourceEdit {
                    index: pair.data,
                    data: replacement.data.clone(),
                    storage: StorageChoice::Keep,
                },
            ],
        )?;
        Ok(replacement)
    }

    /// Data of resource `index` of stream `stream_index` as the workspace
    /// has it: the recorded change, or the original data of the game. Edits
    /// should start from this so they build on earlier edits.
    pub fn current_data(
        &self,
        game: &GameInstance,
        stream_index: usize,
        index: usize,
    ) -> Result<Vec<u8>> {
        let file = stream_file(game, stream_index)?;
        let stream = StreamFile::open(&file.path)?;
        let resource = stream.layout().resources.get(index).ok_or_else(|| {
            Error::InvalidArgument(format!("{} has no resource {index}", file.stream_name()))
        })?;
        let key = ResourceKey::new(&file.stream_name(), &resource.header);
        if let Some(data) = self.data(&key)? {
            return Ok(data);
        }
        // Not the game file: it may still hold a change that was removed.
        let original = StreamFile::open(game.original_path(file))?;
        let index = key.find_in(original.layout()).ok_or_else(|| {
            Error::InvalidArgument(format!("{key} is not in the original stream"))
        })?;
        original.resource_data(index)
    }

    /// Records new data for one resource as one undoable operation labelled
    /// `label`, keeping its storage. Returns whether anything changed.
    pub fn set_resource(
        &mut self,
        game: &GameInstance,
        stream_index: usize,
        index: usize,
        data: Vec<u8>,
        label: &str,
    ) -> Result<bool> {
        let edit = ResourceEdit {
            index,
            data,
            storage: StorageChoice::Keep,
        };
        Ok(self
            .set_resources(game, label, stream_index, &[edit])?
            .is_some())
    }

    /// Records an edited string table. Returns whether anything changed.
    pub fn set_string_table(
        &mut self,
        game: &GameInstance,
        stream_index: usize,
        index: usize,
        table: &StringTable,
    ) -> Result<bool> {
        let file = stream_file(game, stream_index)?;
        let label = format!("Edit strings in {}", file.stream_name());
        Ok(self
            .set_resources(
                game,
                &label,
                stream_index,
                &[ResourceEdit {
                    index,
                    data: table.to_bytes()?,
                    storage: StorageChoice::Keep,
                }],
            )?
            .is_some())
    }

    /// Removes the changes of `keys` from the workspace as one undoable
    /// operation. The game is not touched until [`Self::apply`].
    /// Returns whether anything changed.
    pub fn revert(&mut self, keys: &[ResourceKey], label: &str) -> Result<bool> {
        let mut edits = Vec::new();
        for key in keys.iter().collect::<BTreeSet<_>>() {
            if let Some(change) = self.state.get(key) {
                edits.push(Edit {
                    key: key.clone(),
                    before: Some(change.clone()),
                    after: None,
                });
            }
        }
        if edits.is_empty() {
            return Ok(false);
        }
        self.commit(label, edits, Vec::new())?;
        Ok(true)
    }

    /// Removes every change and every new stream from the workspace as one
    /// undoable operation.
    pub fn revert_all(&mut self) -> Result<bool> {
        let edits: Vec<Edit> = self
            .state
            .values()
            .map(|c| Edit {
                key: c.key.clone(),
                before: Some(c.clone()),
                after: None,
            })
            .collect();
        let stream_edits: Vec<StreamEdit> = self
            .new_streams
            .values()
            .map(|s| StreamEdit {
                name: s.name.clone(),
                before: Some(s.clone()),
                after: None,
            })
            .collect();
        let file_edits: Vec<FileEdit> = self
            .files
            .values()
            .map(|c| FileEdit {
                key: c.key.clone(),
                before: Some(c.clone()),
                after: None,
            })
            .collect();
        if edits.is_empty() && stream_edits.is_empty() && file_edits.is_empty() {
            return Ok(false);
        }
        self.commit_all("Revert all changes", edits, stream_edits, file_edits)?;
        Ok(true)
    }

    /// Undoes the last operation. Returns its label, or `None` when there is nothing to undo.
    pub fn undo(&mut self) -> Result<Option<String>> {
        let Some(i) = self.log.head.checked_sub(1) else {
            return Ok(None);
        };
        self.check_not_changed_on_disk()?;
        let op = self.log.operations[i].clone();
        apply_edits(
            &mut self.state,
            &mut self.new_streams,
            &mut self.files,
            &op,
            false,
        );
        self.log.head = i;
        self.save()?;
        Ok(Some(op.label))
    }

    /// Redoes the next undone operation. Returns its label, or `None` when there is nothing to redo.
    pub fn redo(&mut self) -> Result<Option<String>> {
        let Some(op) = self.log.operations.get(self.log.head).cloned() else {
            return Ok(None);
        };
        self.check_not_changed_on_disk()?;
        apply_edits(
            &mut self.state,
            &mut self.new_streams,
            &mut self.files,
            &op,
            true,
        );
        self.log.head += 1;
        self.save()?;
        Ok(Some(op.label))
    }

    /// Joins the operations made since the history had `since` operations in
    /// effect ([`Self::head`]) into one undoable operation named `label`, as
    /// if they had been recorded together; operations undone since then are
    /// dropped. A task made of several recording steps (a wizard) ends with
    /// this. Returns the sequence number of the joined operation, or `None`
    /// when the operations cancel out (they are then removed).
    pub fn squash(&mut self, since: usize, label: &str) -> Result<Option<u64>> {
        let head = self.log.head;
        if since > head {
            return Err(Error::InvalidArgument(format!(
                "cannot join operations since {since}: only {head} are in effect"
            )));
        }
        if since == head {
            return Ok(None);
        }
        self.check_not_changed_on_disk()?;
        let ops = &self.log.operations[since..head];
        let seq = ops.last().map_or(0, |op| op.seq);
        let relabel = |change: &mut Option<Change>| {
            if let Some(c) = change {
                c.label = label.to_owned();
            }
        };
        let mut edits: Vec<Edit> = Vec::new();
        let mut stream_edits: Vec<StreamEdit> = Vec::new();
        let mut file_edits: Vec<FileEdit> = Vec::new();
        for op in ops {
            for edit in &op.edits {
                match edits.iter_mut().find(|e| e.key == edit.key) {
                    Some(e) => e.after = edit.after.clone(),
                    None => edits.push(edit.clone()),
                }
            }
            for edit in &op.stream_edits {
                match stream_edits.iter_mut().find(|e| e.name == edit.name) {
                    Some(e) => e.after = edit.after.clone(),
                    None => stream_edits.push(edit.clone()),
                }
            }
            for edit in &op.file_edits {
                match file_edits.iter_mut().find(|e| e.key == edit.key) {
                    Some(e) => e.after = edit.after.clone(),
                    None => file_edits.push(edit.clone()),
                }
            }
        }
        // What ends where it started is no edit.
        let same_change = |a: &Option<Change>, b: &Option<Change>| match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => {
                a.payload == b.payload && a.storage == b.storage && a.added == b.added
            }
            _ => false,
        };
        for edit in edits.iter().filter(|e| same_change(&e.before, &e.after)) {
            // Back to how it was, with the label it had.
            if let Some(before) = &edit.before {
                self.state.insert(edit.key.clone(), before.clone());
            }
        }
        edits.retain(|e| !same_change(&e.before, &e.after));
        stream_edits.retain(|e| {
            let strip = |s: &Option<NewStream>| {
                s.clone().map(|mut s| {
                    s.label.clear();
                    s
                })
            };
            strip(&e.before) != strip(&e.after)
        });
        file_edits.retain(|e| match (&e.before, &e.after) {
            (None, None) => false,
            (Some(a), Some(b)) => a.payload != b.payload,
            _ => true,
        });
        for edit in &mut edits {
            relabel(&mut edit.after);
            if let Some(after) = &edit.after {
                self.state.insert(edit.key.clone(), after.clone());
            }
        }
        for edit in &mut stream_edits {
            if let Some(after) = &mut edit.after {
                after.label = label.to_owned();
                self.new_streams.insert(edit.name.clone(), after.clone());
            }
        }
        for edit in &mut file_edits {
            if let Some(after) = &mut edit.after {
                after.label = label.to_owned();
                self.files.insert(edit.key.clone(), after.clone());
            }
        }
        let dropped_redo = self.log.operations.len() > head;
        self.log.operations.truncate(since);
        let result = if edits.is_empty() && stream_edits.is_empty() && file_edits.is_empty() {
            None
        } else {
            self.log.operations.push(Operation {
                seq,
                time: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs()),
                label: label.to_owned(),
                edits,
                stream_edits,
                file_edits,
            });
            Some(seq)
        };
        self.log.head = self.log.operations.len();
        self.save()?;
        if dropped_redo || result.is_none() {
            self.collect_garbage()?;
        }
        Ok(result)
    }

    /// Undoes and forgets every operation made since the history had `since`
    /// operations in effect: a task that was given up leaves nothing to redo.
    /// Returns whether anything was undone or dropped.
    pub fn rollback(&mut self, since: usize) -> Result<bool> {
        if since > self.log.head {
            return Err(Error::InvalidArgument(format!(
                "cannot roll back to {since}: only {} operations are in effect",
                self.log.head
            )));
        }
        if since == self.log.operations.len() {
            return Ok(false);
        }
        while self.log.head > since {
            self.undo()?;
        }
        self.check_not_changed_on_disk()?;
        self.log.operations.truncate(since);
        self.save()?;
        self.collect_garbage()?;
        Ok(true)
    }

    fn commit(
        &mut self,
        label: &str,
        edits: Vec<Edit>,
        stream_edits: Vec<StreamEdit>,
    ) -> Result<u64> {
        self.commit_all(label, edits, stream_edits, Vec::new())
    }

    fn commit_all(
        &mut self,
        label: &str,
        edits: Vec<Edit>,
        stream_edits: Vec<StreamEdit>,
        file_edits: Vec<FileEdit>,
    ) -> Result<u64> {
        if self.saved {
            self.check_not_changed_on_disk()?;
        } else {
            self.write_new()?;
        }
        let dropped_redo = self.log.operations.len() > self.log.head;
        self.log.operations.truncate(self.log.head);
        let seq = self.log.next_seq.max(1);
        self.log.next_seq = seq + 1;
        let op = Operation {
            seq,
            time: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
            label: label.to_owned(),
            edits,
            stream_edits,
            file_edits,
        };
        apply_edits(
            &mut self.state,
            &mut self.new_streams,
            &mut self.files,
            &op,
            true,
        );
        self.log.operations.push(op);
        self.log.head = self.log.operations.len();
        self.save()?;
        if dropped_redo {
            self.collect_garbage()?;
        }
        Ok(seq)
    }

    /// Removes payloads that nothing refers to any more. Returns how many were removed.
    pub fn collect_garbage(&self) -> Result<usize> {
        let mut keep: HashSet<&str> = HashSet::new();
        let edits = self.log.operations.iter().flat_map(|op| &op.edits);
        let logged = edits.flat_map(|edit| edit.before.iter().chain(&edit.after));
        for change in self.state.values().chain(logged) {
            keep.insert(&change.payload.sha256);
            keep.insert(&change.original.sha256);
        }
        let file_edits = self.log.operations.iter().flat_map(|op| &op.file_edits);
        let logged_files = file_edits.flat_map(|edit| edit.before.iter().chain(&edit.after));
        for change in self.files.values().chain(logged_files) {
            keep.insert(&change.payload.sha256);
        }
        self.store.retain(&keep)
    }

    fn check_not_changed_on_disk(&self) -> Result<()> {
        let path = self.dir.join(MANIFEST_FILE);
        let on_disk: Manifest = read_json(&path)?;
        if on_disk.revision != self.manifest.revision {
            return Err(Error::InvalidArgument(format!(
                "the workspace in {} was changed by another program; reopen it",
                self.dir.display()
            )));
        }
        Ok(())
    }

    /// Writes the log, then the manifest.
    fn save(&mut self) -> Result<()> {
        write_json(&self.dir.join(LOG_FILE), &self.log)?;
        self.manifest.revision += 1;
        self.manifest.log_seq = self
            .log
            .head
            .checked_sub(1)
            .map_or(0, |i| self.log.operations[i].seq);
        self.manifest.resources = self.state.values().cloned().collect();
        self.manifest.streams = self.new_streams.values().cloned().collect();
        self.manifest.files = self.files.values().cloned().collect();
        write_json(&self.dir.join(MANIFEST_FILE), &self.manifest)
    }

    fn save_applied(&self) -> Result<()> {
        self.applied.save(&self.applied_path())
    }

    fn applied_path(&self) -> PathBuf {
        self.dir.join(APPLIED_FILE)
    }

    // ------------------------------------------------------------------ diff

    /// Every change with its sizes.
    pub fn diff(&self) -> Vec<DiffEntry> {
        self.state
            .values()
            .map(|c| DiffEntry {
                key: c.key.clone(),
                label: c.label.clone(),
                original_size: c.original.size,
                new_size: c.payload.size,
                added: c.is_added(),
            })
            .collect()
    }

    /// Byte-level and type-aware comparison of one change.
    pub fn detail(&self, key: &ResourceKey) -> Result<Option<ChangeDetail>> {
        let Some(change) = self.state.get(key) else {
            return Ok(None);
        };
        let old = self.store.get(&change.original)?;
        let new = self.store.get(&change.payload)?;
        Ok(Some(ChangeDetail {
            bytes: diff::byte_diff(&old, &new),
            semantic: diff::semantic_diff(key.type_id, &old, &new),
        }))
    }

    // ------------------------------------------------------------ game state

    /// Changes grouped by stream name. New streams are present even when
    /// nothing is added to them.
    fn desired(&self) -> BTreeMap<String, Vec<&Change>> {
        let mut by_stream: BTreeMap<String, Vec<&Change>> = BTreeMap::new();
        for name in self.new_streams.keys() {
            by_stream.entry(name.clone()).or_default();
        }
        for change in self.state.values() {
            by_stream
                .entry(change.key.stream.clone())
                .or_default()
                .push(change);
        }
        by_stream
    }

    /// Compares the workspace with the game files.
    pub fn status(&self, game: &GameInstance) -> Result<Status> {
        let deploy = self
            .applied
            .status_with(game, &self.desired(), &self.new_streams)?;
        let files = files::status(game, &self.applied, &self.files)?;
        let wanted: BTreeSet<String> = self
            .files
            .keys()
            .map(|k| match k {
                FileKey::Loose(path) => path.clone(),
                FileKey::Archive(_) => String::new(),
            })
            .collect();
        let pending_file_reverts = self
            .applied
            .files
            .iter()
            .filter(|(path, record)| {
                if record.original.is_none() {
                    !self.files.keys().any(|k| matches!(k, FileKey::Archive(_)))
                } else {
                    !wanted.contains(*path)
                }
            })
            .map(|(path, _)| path.clone())
            .collect();
        Ok(Status {
            fingerprint_matches: GameFingerprint::of(game)? == self.manifest.game,
            streams: deploy.streams,
            changes: deploy.changes,
            pending_reverts: deploy.pending_reverts,
            files,
            pending_file_reverts,
        })
    }

    // ----------------------------------------------------------------- files

    /// Changed files outside the streams.
    pub fn file_changes(&self) -> impl Iterator<Item = &FileChange> {
        self.files.values()
    }

    /// The change of one file, if any.
    pub fn file_change(&self, key: &FileKey) -> Option<&FileChange> {
        self.files.get(key)
    }

    /// The workspace version of a file: the changed data, or the original.
    pub fn current_file(&self, game: &GameInstance, key: &FileKey) -> Result<Vec<u8>> {
        match self.files.get(key) {
            Some(change) => self.store.get(&change.payload),
            None => files::original_data(game.root(), key),
        }
    }

    /// Replaces a file outside the streams (one undoable operation). Data
    /// equal to the original removes the change.
    pub fn set_file(
        &mut self,
        game: &GameInstance,
        key: &FileKey,
        data: Vec<u8>,
        label: &str,
    ) -> Result<u64> {
        let original = match self.files.get(key) {
            Some(change) => change.original.clone(),
            None => Blob::of(&files::original_data(game.root(), key)?),
        };
        let after = if Blob::of(&data) == original {
            None
        } else {
            Some(FileChange {
                key: key.clone(),
                payload: self.store.put(&data)?,
                original,
                label: label.to_owned(),
            })
        };
        let edit = FileEdit {
            key: key.clone(),
            before: self.files.get(key).cloned(),
            after,
        };
        self.commit_all(label, Vec::new(), Vec::new(), vec![edit])
    }

    /// Removes the changes of these files (one undoable operation). Returns
    /// whether anything changed.
    pub fn revert_files(&mut self, keys: &[FileKey], label: &str) -> Result<bool> {
        let edits: Vec<FileEdit> = keys
            .iter()
            .filter_map(|k| {
                self.files.get(k).map(|c| FileEdit {
                    key: k.clone(),
                    before: Some(c.clone()),
                    after: None,
                })
            })
            .collect();
        if edits.is_empty() {
            return Ok(false);
        }
        self.commit_all(label, Vec::new(), Vec::new(), edits)?;
        Ok(true)
    }

    fn sync_files(
        &mut self,
        game: &GameInstance,
        force: bool,
        wanted: bool,
    ) -> Result<ApplyReport> {
        let path = self.applied_path();
        let empty = BTreeMap::new();
        files::sync(
            game,
            &mut self.applied,
            &path,
            if wanted { &self.files } else { &empty },
            &self.store,
            "workspace",
            force,
        )
    }

    /// Streams whose game files hold changes this workspace wrote.
    pub fn held_streams(&self, game: &GameInstance) -> Result<Vec<String>> {
        self.applied.held_streams(game)
    }

    /// Writes the changes into the game: every stream that differs from the
    /// workspace is rebuilt from its original with all of its changes, and its
    /// size is updated in the tables of contents. A stream without changes is
    /// put back to its original. Files are backed up before the first write
    /// and replaced atomically.
    ///
    /// Streams changed outside the workspace are refused unless `options.force`.
    /// While the mod manager has mods in the game, the workspace does not write
    /// into it at all (see [`crate::mods`]).
    pub fn apply(&mut self, game: &GameInstance, options: ApplyOptions) -> Result<ApplyReport> {
        if !self.saved {
            // Nothing recorded and nothing in the game.
            return Ok(ApplyReport::default());
        }
        let desired: BTreeMap<String, Vec<Change>> = self
            .desired()
            .into_iter()
            .map(|(name, changes)| (name, changes.into_iter().cloned().collect()))
            .collect();
        let mut names: BTreeSet<String> = desired
            .keys()
            .chain(self.applied.streams.keys())
            .cloned()
            .collect();
        if let Some(only) = &options.streams {
            let only: HashSet<String> = only.iter().map(|s| key::normalize_stream(s)).collect();
            names.retain(|n| only.contains(n));
        }
        let wants_changes = names.iter().any(|n| {
            desired.get(n).is_some_and(|c| !c.is_empty()) || self.new_streams.contains_key(n)
        });
        if wants_changes {
            crate::mods::ensure_mods_not_in_game(game)?;
        }
        let with_files = options
            .streams
            .as_ref()
            .is_none_or(|only| only.iter().any(|s| s == FILES));
        let path = self.applied_path();
        let store = &self.store;
        let mut report = self.applied.sync_with(
            &path,
            game,
            &names,
            options.force,
            &self.new_streams,
            |name| desired.get(name).cloned().unwrap_or_default(),
            |change| store.get(&change.payload),
        )?;
        if with_files {
            report.merge(self.sync_files(game, options.force, true)?);
        }
        Ok(report)
    }

    /// Puts the original files back into the game. The changes stay in the
    /// workspace, pending.
    pub fn unapply(&mut self, game: &GameInstance, force: bool) -> Result<ApplyReport> {
        if !self.saved {
            return Ok(ApplyReport::default());
        }
        let names: BTreeSet<String> = self.applied.streams.keys().cloned().collect();
        let path = self.applied_path();
        let store = &self.store;
        let mut report = self.applied.sync_with(
            &path,
            game,
            &names,
            force,
            &BTreeMap::new(),
            |_| Vec::new(),
            |change| store.get(&change.payload),
        )?;
        report.merge(self.sync_files(game, force, false)?);
        Ok(report)
    }
}

/// Applies the edits of `op` to `state`: forward (redo) or backward (undo).
fn apply_edits(
    state: &mut BTreeMap<ResourceKey, Change>,
    streams: &mut BTreeMap<String, NewStream>,
    files: &mut BTreeMap<FileKey, FileChange>,
    op: &Operation,
    forward: bool,
) {
    for edit in &op.file_edits {
        let value = if forward { &edit.after } else { &edit.before };
        match value {
            Some(change) => files.insert(edit.key.clone(), change.clone()),
            None => files.remove(&edit.key),
        };
    }
    for edit in &op.stream_edits {
        let value = if forward { &edit.after } else { &edit.before };
        match value {
            Some(stream) => streams.insert(edit.name.clone(), stream.clone()),
            None => streams.remove(&edit.name),
        };
    }
    let edits: Box<dyn Iterator<Item = &Edit>> = if forward {
        Box::new(op.edits.iter())
    } else {
        Box::new(op.edits.iter().rev())
    };
    for edit in edits {
        let value = if forward { &edit.after } else { &edit.before };
        match value {
            Some(change) => state.insert(edit.key.clone(), change.clone()),
            None => state.remove(&edit.key),
        };
    }
}

fn stream_file(game: &GameInstance, index: usize) -> Result<&GameFile> {
    game.streams()
        .get(index)
        .ok_or_else(|| Error::InvalidArgument(format!("stream index {index} out of range")))
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let text = fs::read(path).map_err(|e| Error::io(path, e))?;
    serde_json::from_slice(&text)
        .map_err(|e| Error::InvalidArgument(format!("{}: {e}", path.display())))
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut text = serde_json::to_vec_pretty(value)
        .map_err(|e| Error::InvalidArgument(format!("{}: {e}", path.display())))?;
    text.push(b'\n');
    crate::atomic::write(path, &text)
}

#[cfg(test)]
pub(crate) mod tests;

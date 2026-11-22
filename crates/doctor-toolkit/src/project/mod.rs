//! A mod as a project: the resources it changes kept as files in common
//! formats, built into the workspace when they change.
//!
//! ```text
//! my_mod/
//! ├── project.toml                    name, mod id, game, edition, language, dependencies
//! ├── src/
//! │   └── gf2/frontend.str/           one folder per stream
//! │       ├── stream.toml             its resources: files, type, ids, policy, keys
//! │       ├── tg4h/buttons.png        texture
//! │       ├── lh2/9e583c54.json       string table
//! │       ├── mesh/car.glb            model
//! │       └── simgroup/props.json     game logic
//! ├── shared/                         resources equal in several streams, once
//! │   └── shared.toml
//! ├── patches/                        the author's object edits and new objects
//! │   └── gf2/frontend.str/simgroup/props.toml
//! └── .doctor/
//!     ├── base/                       data strings and logic are applied to
//!     ├── build.json                  build cache (may be deleted)
//!     └── editor.json                 what the editor showed last (`document`)
//! ```
//!
//! The sources also hold what the mod makes that the game does not have
//! (stage 112): resources it **adds** to streams of the game and streams it
//! **creates**, with their ids and places in the manifests ([`manifest`]).
//! So a workspace can be made again from the sources alone:
//! [`Project::export_workspace`] writes sources for everything the
//! project's own workspace holds, and a build into an empty workspace
//! records the same changes.
//!
//! [`Project::build`] builds only what changed since the last build: the
//! SHA-256 of every source file (read again only when its size or time
//! changed) against the cache, and the state of the workspace against what
//! the last build left there. The result goes **into the workspace**, as
//! one undoable operation, never straight into the game; `apply`, "Play"
//! and mods take it from there. A file that fails is reported with its path;
//! the others are built. See `docs/project.md`.

pub mod base;
pub mod cache;
pub mod config;
pub mod cook;
pub mod create;
pub mod document;
pub mod guid;
pub mod manifest;
pub mod patch;
pub mod rebase;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::game::GameInstance;
use crate::workspace::{ResourceKey, Workspace};
use crate::{Error, Result};

pub use base::{Base, BaseDifference, BaseMod, Brought, Recorded};
pub use cache::BuildCache;
pub use config::{PROJECT_FILE, ProjectConfig};
pub use create::{
    Additions, CreateOptions, CreateReport, NewProject, create, create_from, create_with,
    workspace_additions, workspace_selection,
};
pub use document::{
    Adopted, EditorCamera, EditorState, LocalSettings, NewDocument, adopt_workspace,
    create_document, project_of_workspace,
};
pub use manifest::{AddedSource, Entry, Kind, NewStreamSource, SourceManifest};
pub use patch::SimGroupPatch;
pub use rebase::stale_warning;

/// Folder of the stream folders.
pub const SOURCE_DIR: &str = "src";
/// Folder of resources shared by several streams.
pub const SHARED_DIR: &str = "shared";
/// Folder of the author's patches.
pub const PATCH_DIR: &str = "patches";

/// A folder of sources and its manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    /// Path from the project root (`src/gf2/frontend.str`, `shared`).
    pub dir: String,
    pub manifest: SourceManifest,
}

/// An open project.
#[derive(Debug, Clone)]
pub struct Project {
    root: PathBuf,
    pub config: ProjectConfig,
    pub folders: Vec<Folder>,
}

/// Where an entry stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryState {
    /// The files are as the project made them, and so is the workspace.
    Unchanged,
    /// The files changed and were built; the workspace has the result.
    Built,
    /// The files (or the patch) changed since the last build, or were never
    /// built into this workspace: a checkout of the sources whose workspace
    /// does not hold what they describe yet.
    Modified,
    /// The files are built, but the workspace has other data for the
    /// resources now (undone, reverted or edited elsewhere). Building
    /// writes the project's version again; [`Project::pull`] takes the
    /// workspace's.
    OutOfSync,
    /// A file is missing.
    Missing(String),
    /// A view (for reading only) was changed; it is not built.
    ViewChanged,
    /// The file of a resource the project adds is gone: a build takes the
    /// resource out of the workspace and the entry out of its manifest.
    /// (The game cannot lose a resource of its own: there a missing file is
    /// [`Self::Missing`], an error.)
    Removed,
}

impl EntryState {
    /// Whether a build would write the entry.
    pub fn needs_build(&self) -> bool {
        matches!(self, Self::Modified | Self::OutOfSync | Self::Removed)
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Unchanged => "unchanged",
            Self::Built => "built",
            Self::Modified => "modified",
            Self::OutOfSync => "out of sync",
            Self::Missing(_) => "missing",
            Self::ViewChanged => "view changed",
            Self::Removed => "removed",
        }
    }
}

/// The state of one entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryStatus {
    /// Path of the main file from the project root; the entry's id.
    pub id: String,
    /// Folder of the entry.
    pub dir: String,
    pub kind: Kind,
    pub name: String,
    pub type_tag: String,
    pub state: EntryState,
    /// Path of its patch from the root, when there is one.
    pub patch: Option<String>,
    /// Digest of its files and patch now.
    pub inputs: Option<String>,
}

/// A stream the project creates, against the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewStreamStatus {
    /// Its folder from the project root (`src/gf2/doctor/yard.str`).
    pub dir: String,
    pub stream: String,
    /// The workspace creates the stream as `stream.toml` describes it; a
    /// build records it otherwise.
    pub built: bool,
}

/// [`Project::status`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectStatus {
    pub entries: Vec<EntryStatus>,
    /// The streams the project creates.
    pub new_streams: Vec<NewStreamStatus>,
    /// Streams of folders that are gone since the last build (folder,
    /// stream) which the workspace still creates: a build removes them with
    /// their resources.
    pub removed_streams: Vec<(String, String)>,
    /// Files in the source folders that no entry has (they are not built).
    pub unknown_files: Vec<String>,
    /// Another game version, missing dependencies, patches without a source.
    pub warnings: Vec<String>,
}

impl ProjectStatus {
    pub fn count(&self, state: &EntryState) -> usize {
        self.entries
            .iter()
            .filter(|e| std::mem::discriminant(&e.state) == std::mem::discriminant(state))
            .count()
    }

    /// What a build would write: entries, and streams to create or remove.
    pub fn to_build(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.state.needs_build())
            .count()
            + self.new_streams.iter().filter(|s| !s.built).count()
            + self.removed_streams.len()
    }
}

/// Options of [`Project::build`].
#[derive(Debug, Clone, Default)]
pub struct BuildOptions {
    /// Only tell what would be built.
    pub dry_run: bool,
    /// Build every entry, changed or not. Textures and models of the game
    /// are encoded again, so their data may differ from the game's even
    /// when the file did not change (those the workspace made build the
    /// data they were written from, [`Entry::template`]).
    pub force: bool,
    /// Only entries whose id (path from the root) starts with one of these.
    pub only: Vec<String>,
}

/// One built entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltEntry {
    pub id: String,
    /// Resources whose data in the workspace changed.
    pub changed: Vec<ResourceKey>,
    /// Warnings of the importer, new objects of patches.
    pub notes: Vec<String>,
}

/// What [`Project::export_workspace`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourcesReport {
    /// Entries written for changes that had no sources (ids).
    pub added: Vec<String>,
    /// Entries whose files now hold the workspace's version.
    pub pulled: Vec<String>,
    /// Entries and folders of streams removed: the workspace no longer adds
    /// the resource or creates the stream.
    pub removed: Vec<String>,
    /// Streams the workspace creates whose description was written.
    pub streams: Vec<String>,
    pub files: usize,
    pub bytes: u64,
    /// What stays in the workspace only or was kept as raw data, and why.
    pub notes: Vec<String>,
}

/// What [`Project::build`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuildReport {
    pub built: Vec<BuiltEntry>,
    /// Entries that were up to date.
    pub up_to_date: usize,
    /// `path: error` of each entry that failed.
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    /// Workspace operation of the build; `None` when nothing changed.
    pub seq: Option<u64>,
    pub workspace: Option<PathBuf>,
}

impl Project {
    /// Opens the project in `root`.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        let config = ProjectConfig::read(&root)?;
        let mut folders = Vec::new();
        let src = root.join(SOURCE_DIR);
        if src.is_dir() {
            find_stream_folders(&src, SOURCE_DIR, &mut folders)?;
        }
        let shared = root.join(SHARED_DIR).join(manifest::SHARED_MANIFEST);
        if shared.is_file() {
            folders.push(Folder {
                dir: SHARED_DIR.into(),
                manifest: SourceManifest::read(&shared)?,
            });
        }
        folders.sort_by(|a, b| a.dir.cmp(&b.dir));
        Ok(Self {
            root,
            config,
            folders,
        })
    }

    /// Whether `dir` holds a project.
    pub fn exists(dir: &Path) -> bool {
        dir.join(PROJECT_FILE).is_file()
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Every entry: id (main file from the root), entry, folder.
    pub fn entries(&self) -> impl Iterator<Item = (String, &Entry, &str)> {
        self.folders.iter().flat_map(|f| {
            f.manifest
                .resources
                .iter()
                .map(move |e| (format!("{}/{}", f.dir, e.files[0]), e, f.dir.as_str()))
        })
    }

    /// The entry with id `id`.
    pub fn entry(&self, id: &str) -> Option<(&Entry, &str)> {
        self.entries()
            .find(|(i, _, _)| i == id)
            .map(|(_, e, d)| (e, d))
    }

    /// The game of the project: `path`, or the one in `project.toml`.
    pub fn game(&self, path: Option<&Path>) -> Result<GameInstance> {
        let path = path
            .map(Path::to_path_buf)
            .or_else(|| self.game_path())
            .ok_or_else(|| {
                Error::InvalidArgument(
                    "the project does not say where the game is; give the game folder".into(),
                )
            })?;
        GameInstance::open(path)
    }

    /// The workspace builds go to: the project's own (never written into
    /// the game files by itself, [`crate::workspace::Owner::Project`]), or
    /// the game's.
    pub fn workspace(&self, game: &GameInstance) -> Result<Workspace> {
        match self.config.workspace_dir(&self.root) {
            Some(dir) => Workspace::for_project(game, &dir),
            None => Workspace::for_game(game, None),
        }
    }

    /// Path of the patch of an entry from the root:
    /// `patches/<folder below src>/<main file>.toml`.
    pub fn patch_path(dir: &str, entry: &Entry) -> Option<String> {
        if entry.kind != Kind::Logic || entry.type_tag != "simgroup" {
            return None;
        }
        let below = dir.strip_prefix(&format!("{SOURCE_DIR}/")).unwrap_or(dir);
        let file = &entry.files[0];
        let stem = file.strip_suffix(".json").unwrap_or(file);
        Some(format!("{PATCH_DIR}/{below}/{stem}.toml"))
    }

    /// The patch of an entry when its file exists.
    fn existing_patch(&self, dir: &str, entry: &Entry) -> Option<String> {
        Self::patch_path(dir, entry).filter(|p| self.root.join(p).is_file())
    }

    /// Where each entry stands, against the build cache and `ws`.
    pub fn status(&self, ws: &Workspace) -> Result<ProjectStatus> {
        let mut cache = BuildCache::read(&self.root);
        let status = self.status_with(ws, &mut cache)?;
        // The hashes of unchanged files spare reading them next time.
        let _ = cache.write(&self.root);
        Ok(status)
    }

    /// [`Self::status`] without writing anything: the build cache that
    /// spares reading unchanged files next time is read but not updated.
    pub fn status_read_only(&self, ws: &Workspace) -> Result<ProjectStatus> {
        let mut cache = BuildCache::read(&self.root);
        self.status_with(ws, &mut cache)
    }

    fn status_with(&self, ws: &Workspace, cache: &mut BuildCache) -> Result<ProjectStatus> {
        let mut status = ProjectStatus::default();
        let mut known: BTreeSet<String> = BTreeSet::new();
        let mut patches: BTreeSet<String> = BTreeSet::new();
        for folder in &self.folders {
            let manifest_name = if folder.dir == SHARED_DIR {
                manifest::SHARED_MANIFEST
            } else {
                manifest::STREAM_MANIFEST
            };
            known.insert(format!("{}/{manifest_name}", folder.dir));
            for entry in &folder.manifest.resources {
                let id = format!("{}/{}", folder.dir, entry.files[0]);
                let mut shas = Vec::new();
                let mut missing = None;
                for file in &entry.files {
                    let rel = format!("{}/{file}", folder.dir);
                    known.insert(rel.clone());
                    match cache.file_sha(&self.root, &rel)? {
                        Some(sha) => shas.push(sha),
                        None => {
                            missing.get_or_insert(rel);
                        }
                    }
                }
                let patch = self.existing_patch(&folder.dir, entry);
                if let Some(p) = Self::patch_path(&folder.dir, entry) {
                    patches.insert(p);
                }
                let patch_sha = match &patch {
                    Some(p) => cache.file_sha(&self.root, p)?,
                    None => None,
                };
                let inputs = missing.is_none().then(|| {
                    let pairs: Vec<(&str, &str)> = entry
                        .files
                        .iter()
                        .zip(&shas)
                        .map(|(f, s)| (f.as_str(), s.as_str()))
                        .collect();
                    cache::digest(&pairs, patch_sha.as_deref())
                });
                // Removed, not lost: the file of a resource the project
                // adds is gone as a whole (not one picture of a model).
                let removed = missing.is_some()
                    && entry.is_added()
                    && !self.root.join(&folder.dir).join(&entry.files[0]).exists();
                let state = match (&missing, &inputs) {
                    (Some(_), _) if removed => EntryState::Removed,
                    (Some(file), _) => EntryState::Missing(file.clone()),
                    (None, Some(inputs)) if entry.kind == Kind::View => {
                        if *inputs == entry.exported {
                            EntryState::Unchanged
                        } else {
                            EntryState::ViewChanged
                        }
                    }
                    (None, Some(inputs)) => match cache.entries.get(&id) {
                        Some(built) if built.inputs == *inputs => {
                            if outputs_match(ws, &built.outputs) {
                                if *inputs == entry.exported {
                                    EntryState::Unchanged
                                } else {
                                    EntryState::Built
                                }
                            } else {
                                EntryState::OutOfSync
                            }
                        }
                        Some(_) => EntryState::Modified,
                        // No record of a build: files of the unmodified
                        // game are what the game has; files written from a
                        // changed workspace are built unless this workspace
                        // holds the same.
                        None if *inputs == entry.exported
                            && (entry.outputs.is_empty() && entry.added.is_empty()
                                || recorded_outputs_match(ws, entry)) =>
                        {
                            EntryState::Unchanged
                        }
                        None => EntryState::Modified,
                    },
                    (None, None) => unreachable!("inputs exist when no file is missing"),
                };
                status.entries.push(EntryStatus {
                    id,
                    dir: folder.dir.clone(),
                    kind: entry.kind,
                    name: entry.name.clone(),
                    type_tag: entry.type_tag.clone(),
                    state,
                    patch,
                    inputs,
                });
            }
        }
        // The streams the project creates, and those whose folder is gone.
        for folder in &self.folders {
            if let (Some(stream), Some(new)) = (&folder.manifest.stream, &folder.manifest.new) {
                let built = new.to_stream(stream).is_ok_and(|wanted| {
                    ws.new_stream(stream)
                        .is_some_and(|s| same_stream(s, &wanted))
                });
                status.new_streams.push(NewStreamStatus {
                    dir: folder.dir.clone(),
                    stream: stream.clone(),
                    built,
                });
            }
        }
        for (dir, built) in &cache.streams {
            let stream = &built.stream;
            if !status
                .new_streams
                .iter()
                .any(|s| s.dir == *dir || s.stream == *stream)
                && ws.new_stream(stream).is_some()
            {
                status.removed_streams.push((dir.clone(), stream.clone()));
            }
        }
        // Files nobody builds.
        for top in [SOURCE_DIR, SHARED_DIR] {
            let dir = self.root.join(top);
            if dir.is_dir() {
                let mut files = Vec::new();
                collect_files(&dir, top, &mut files)?;
                status
                    .unknown_files
                    .extend(files.into_iter().filter(|f| !known.contains(f)));
            }
        }
        let patch_dir = self.root.join(PATCH_DIR);
        if patch_dir.is_dir() {
            let mut files = Vec::new();
            collect_files(&patch_dir, PATCH_DIR, &mut files)?;
            for file in files {
                if !patches.contains(&file) {
                    status.warnings.push(format!(
                        "{file}: no SimGroup source of this name; the patch is not used"
                    ));
                }
            }
        }
        Ok(status)
    }

    /// Warnings about the game: another executable, and what differs
    /// between the mods the project is made on and the mods of the game
    /// ([`ProjectConfig::check_base`]).
    pub fn game_warnings(&self, game: &GameInstance) -> Vec<String> {
        let mut warnings = Vec::new();
        let edition = crate::edition::Edition::identify(game.root());
        if let Some(exe) = &edition.executable
            && !self.config.game.executable_sha256.is_empty()
            && exe.sha256 != self.config.game.executable_sha256
        {
            warnings.push(format!(
                "the game is {}, the project was made for {}",
                edition.name(),
                if self.config.game.edition.is_empty() {
                    "another executable"
                } else {
                    &self.config.game.edition
                }
            ));
        }
        match self.config.check_base(game) {
            Ok(differences) => {
                warnings.extend(differences.iter().map(BaseDifference::describe));
            }
            Err(error) => warnings.push(format!(
                "the mods of the game cannot be compared with the base of the project: {}",
                plain(&error)
            )),
        }
        warnings
    }

    /// Builds the entries whose files changed (or whose resources the
    /// workspace no longer has as built) into `ws`, as one operation.
    pub fn build(
        &self,
        game: &GameInstance,
        ws: &mut Workspace,
        options: &BuildOptions,
    ) -> Result<BuildReport> {
        if options.dry_run {
            return self.build_entries(game, ws, options);
        }
        // One write of the workspace's log and manifest for the whole build,
        // not one per entry: a project of the whole game has a hundred
        // thousand entries (`docs/project.md` §7).
        ws.begin_batch()?;
        let built = self.build_entries(game, ws, options);
        let ended = ws.end_batch();
        let report = built?;
        ended?;
        Ok(report)
    }

    fn build_entries(
        &self,
        game: &GameInstance,
        ws: &mut Workspace,
        options: &BuildOptions,
    ) -> Result<BuildReport> {
        let mut cache = BuildCache::read(&self.root);
        let status = self.status_with(ws, &mut cache)?;
        let mut report = BuildReport {
            warnings: status.warnings.clone(),
            ..BuildReport::default()
        };
        report.warnings.extend(self.game_warnings(game));
        let wanted = |id: &str| {
            options.only.is_empty()
                || options
                    .only
                    .iter()
                    .any(|o| id.starts_with(o.trim_end_matches('/')))
        };
        let streams: Vec<&NewStreamStatus> = status
            .new_streams
            .iter()
            .filter(|s| !s.built && wanted(&format!("{}/{}", s.dir, manifest::STREAM_MANIFEST)))
            .collect();
        let removed_streams: Vec<&(String, String)> = status
            .removed_streams
            .iter()
            .filter(|(dir, _)| wanted(dir))
            .collect();
        let mut work = Vec::new();
        for entry in &status.entries {
            if !wanted(&entry.id) {
                continue;
            }
            match &entry.state {
                EntryState::Missing(file) => report.errors.push(format!(
                    "{}: {file} is missing; restore it (removing resources is not supported)",
                    entry.id
                )),
                EntryState::ViewChanged => report.warnings.push(format!(
                    "{}: changed, but it is a view for reading only; edit the raw file of the resource",
                    entry.id
                )),
                state if state.needs_build() => work.push(entry),
                _ if options.force && entry.kind.is_built() => work.push(entry),
                _ => report.up_to_date += 1,
            }
        }
        // A model is built after the materials it names: an added set has
        // them as entries of their own.
        work.sort_by_key(|e| e.kind == Kind::Model);
        if options.dry_run {
            let stream = |id: String, note: &str| BuiltEntry {
                id,
                changed: Vec::new(),
                notes: vec![note.to_owned()],
            };
            report.built = streams
                .iter()
                .map(|s| {
                    stream(
                        format!("{}/{}", s.dir, manifest::STREAM_MANIFEST),
                        "new stream",
                    )
                })
                .chain(
                    removed_streams
                        .iter()
                        .map(|(dir, _)| stream(dir.clone(), "removed stream")),
                )
                .chain(work.iter().map(|e| BuiltEntry {
                    id: e.id.clone(),
                    changed: Vec::new(),
                    notes: vec![e.state.label().to_owned()],
                }))
                .collect();
            let _ = cache.write(&self.root);
            return Ok(report);
        }
        let since = ws.head();
        let mut touched: BTreeSet<ResourceKey> = BTreeSet::new();
        // Streams first: their resources are added to them.
        for (dir, stream) in removed_streams {
            let gone: Vec<ResourceKey> = ws
                .new_stream_resources(stream)
                .iter()
                .map(|c| c.key.clone())
                .collect();
            match ws.remove_stream(stream) {
                Ok(_) => {
                    cache.streams.remove(dir);
                    cache
                        .entries
                        .retain(|id, _| !id.starts_with(&format!("{dir}/")));
                    touched.extend(gone.iter().cloned());
                    report.built.push(BuiltEntry {
                        id: dir.clone(),
                        changed: gone,
                        notes: vec![format!(
                            "the folder is gone: stream {stream} is removed with what was added to it"
                        )],
                    });
                }
                Err(error) => report.errors.push(format!("{dir}: {}", plain(&error))),
            }
        }
        for status_stream in &status.new_streams {
            let (dir, stream) = (&status_stream.dir, &status_stream.stream);
            let new = self
                .folders
                .iter()
                .find(|f| f.dir == *dir)
                .and_then(|f| f.manifest.new.as_ref())
                .expect("the status lists folders of the project");
            let recorded = cache::BuiltStream {
                stream: stream.clone(),
                described: new.digest(),
            };
            if status_stream.built {
                cache.streams.insert(dir.clone(), recorded.clone());
            }
            if !streams.iter().any(|s| s.dir == *dir) {
                continue;
            }
            let id = format!("{dir}/{}", manifest::STREAM_MANIFEST);
            match new.to_stream(stream).and_then(|s| ws.put_stream(game, s)) {
                Ok(_) => {
                    cache.streams.insert(dir.clone(), recorded);
                    report.built.push(BuiltEntry {
                        id,
                        changed: Vec::new(),
                        notes: vec![format!("stream {stream} is created by the project")],
                    });
                }
                Err(error) => report.errors.push(format!("{id}: {}", plain(&error))),
            }
        }
        // Folders that are gone and whose stream the workspace does not
        // create (any more) are forgotten.
        cache.streams.retain(|dir, _| {
            self.folders
                .iter()
                .any(|f| f.dir == *dir && f.manifest.new.is_some())
                || status.removed_streams.iter().any(|(d, _)| d == dir)
        });
        let mut cook = cook::Cook::new(
            game,
            &self.root,
            &self.config.project.id,
            self.entries().flat_map(|(_, e, _)| e.keys.iter()),
        );
        let mut removed: Vec<String> = Vec::new();
        for status_entry in work {
            let (entry, dir) = self
                .entry(&status_entry.id)
                .expect("the status lists entries of the project");
            let before = ws.head();
            if status_entry.state == EntryState::Removed {
                let gone: Vec<ResourceKey> = entry
                    .keys
                    .iter()
                    .filter(|k| ws.change(k).is_some_and(|c| c.is_added()))
                    .cloned()
                    .collect();
                match ws.revert(&gone, &format!("Build {}", status_entry.id)) {
                    Ok(_) => {
                        cache.entries.remove(&status_entry.id);
                        removed.push(status_entry.id.clone());
                        touched.extend(gone.iter().cloned());
                        report.built.push(BuiltEntry {
                            id: status_entry.id.clone(),
                            changed: gone,
                            notes: vec![
                                "its file is gone: the resource is no longer added".to_owned(),
                            ],
                        });
                    }
                    Err(error) => {
                        report
                            .errors
                            .push(format!("{}: {}", status_entry.id, plain(&error)))
                    }
                }
                continue;
            }
            let inputs = status_entry.inputs.clone().expect("not missing");
            // Sources as they were last built, and the workspace without
            // them: the original data comes back exactly (an image is not
            // encoded again).
            let restore = cache.entries.get(&status_entry.id).is_some_and(|b| {
                !options.force && b.inputs == inputs && b.outputs.values().all(Option::is_none)
            });
            let result = if restore {
                ws.revert(&entry.keys, &format!("Build {}", status_entry.id))
                    .map(|_| Vec::new())
            } else {
                cook.entry(
                    ws,
                    dir,
                    entry,
                    status_entry.patch.as_deref(),
                    inputs == entry.exported,
                )
            };
            match result {
                Ok(notes) => {
                    let mut keys: BTreeSet<ResourceKey> = entry.keys.iter().cloned().collect();
                    let mut changed = Vec::new();
                    for op in &ws.operations()[before..ws.head()] {
                        for edit in &op.edits {
                            keys.insert(edit.key.clone());
                            changed.push(edit.key.clone());
                        }
                    }
                    changed.sort();
                    changed.dedup();
                    touched.extend(changed.iter().cloned());
                    cache.entries.insert(
                        status_entry.id.clone(),
                        cache::Built {
                            inputs,
                            outputs: state_of(ws, keys.iter()),
                        },
                    );
                    report.built.push(BuiltEntry {
                        id: status_entry.id.clone(),
                        changed,
                        notes,
                    });
                }
                Err(error) => report
                    .errors
                    .push(format!("{}: {}", status_entry.id, plain(&error))),
            }
        }
        if ws.head() > since {
            let built = report
                .built
                .iter()
                .filter(|b| !b.changed.is_empty())
                .count();
            let label = format!(
                "Build project {} ({built} file(s))",
                self.config.project.name
            );
            report.seq = ws.squash(since, &label)?;
            report.workspace = Some(ws.dir().to_owned());
        }
        // What this build wrote is what other entries with the same
        // resources have now, too.
        if !touched.is_empty() {
            for built in cache.entries.values_mut() {
                for (key, state) in built.outputs.iter_mut() {
                    if let Ok(k) = ResourceKey::from_str(key)
                        && touched.contains(&k)
                    {
                        *state = ws.change(&k).map(|c| c.payload.sha256.clone());
                    }
                }
            }
        }
        // The entries whose files are gone leave their manifests.
        for folder in &self.folders {
            let id_of = |e: &Entry| format!("{}/{}", folder.dir, e.files[0]);
            if folder
                .manifest
                .resources
                .iter()
                .any(|e| removed.contains(&id_of(e)))
            {
                let mut manifest = folder.manifest.clone();
                manifest.resources.retain(|e| !removed.contains(&id_of(e)));
                manifest.write(&self.root.join(&folder.dir).join(manifest_name(&folder.dir)))?;
            }
        }
        cache.write(&self.root)?;
        Ok(report)
    }

    /// Resources the project's builds changed that the workspace still
    /// changes: what a mod of the project holds. Of a project with its own
    /// workspace — every change of that workspace: all of it is the
    /// project's, built from files or edited in the editor.
    pub fn changed_keys(&self, ws: &Workspace) -> Vec<ResourceKey> {
        if self.has_own_workspace() {
            return ws.changes().map(|c| c.key.clone()).collect();
        }
        let cache = BuildCache::read(&self.root);
        let mut keys: BTreeSet<ResourceKey> = BTreeSet::new();
        for (_, entry, _) in self.entries() {
            keys.extend(entry.keys.iter().cloned());
        }
        for built in cache.entries.values() {
            keys.extend(
                built
                    .outputs
                    .keys()
                    .filter_map(|k| ResourceKey::from_str(k).ok()),
            );
        }
        keys.into_iter()
            .filter(|k| ws.change(k).is_some())
            .collect()
    }

    /// The mod metadata of the project. The mods of its base become
    /// dependencies with the conditions the base has for them — whether
    /// they are written into the game now or not; the mods of
    /// `[dependencies] mods`, dependencies of any version
    /// ([`ProjectConfig::mod_dependencies`]).
    pub fn mod_info(&self) -> crate::mods::ModInfo {
        let p = &self.config.project;
        crate::mods::ModInfo {
            id: p.id.clone(),
            name: p.name.clone(),
            version: p.version.clone(),
            author: p.author.clone(),
            description: p.description.clone(),
            url: (!p.url.is_empty()).then(|| p.url.clone()),
            relations: crate::mods::Relations {
                depends: self.config.mod_dependencies(),
                ..Default::default()
            },
            whole_resources: !self.config.build.deltas,
        }
    }

    /// Writes a mod of the project's changes in `ws` to `out`, as it is
    /// installed ([`Self::install_into`]): with the dependencies of the
    /// project and, for the resources of those mods it replaces, their data
    /// as the original, so that the mod is checked against them as a patch
    /// (`docs/mods.md` §3.3).
    pub fn build_mod_on(
        &self,
        game: &GameInstance,
        ws: &Workspace,
        out: &Path,
        preview: Option<&crate::mods::Preview>,
    ) -> Result<crate::mods::BuildReport> {
        let (info, keys) = self.mod_of(game, ws)?;
        crate::mods::build_from_workspace(ws, &info, keys.as_deref(), preview, out)
    }

    /// [`Self::build_mod_on`] without looking at the mods of the game: the
    /// dependencies are those `project.toml` names, and every original is
    /// the game's.
    /// The mod is written in the lowest format version that holds it.
    /// Build first: only what the workspace has is in the mod.
    pub fn build_mod(
        &self,
        ws: &Workspace,
        out: &Path,
        preview: Option<&crate::mods::Preview>,
    ) -> Result<crate::mods::BuildReport> {
        let keys = self.changed_keys(ws);
        if keys.is_empty() {
            return Err(Error::mod_error(
                "the workspace has no changes of this project; build it first",
            ));
        }
        crate::mods::build_from_workspace(ws, &self.mod_info(), Some(&keys), preview, out)
    }

    /// Writes the workspace's version of out-of-sync entries (`ids`, or all
    /// of them) into their files, so that the project follows edits made
    /// elsewhere. Returns the ids written. Strings and logic also take the
    /// workspace's data as their new base. An entry of a resource the
    /// project adds that the workspace no longer has is removed with its
    /// files (its id is returned too): that is the workspace's version.
    pub fn pull(
        &mut self,
        game: &GameInstance,
        ws: &Workspace,
        ids: &[String],
    ) -> Result<Vec<String>> {
        let mut cache = BuildCache::read(&self.root);
        let status = self.status_with(ws, &mut cache)?;
        let wanted: Vec<EntryStatus> = status
            .entries
            .into_iter()
            .filter(|e| e.state == EntryState::OutOfSync)
            .filter(|e| ids.is_empty() || ids.iter().any(|i| e.id.starts_with(i.as_str())))
            .collect();
        let (pulled, removed) = self.pull_entries(game, ws, &mut cache, &wanted, None, None)?;
        cache.write(&self.root)?;
        let mut all = pulled;
        all.extend(removed);
        Ok(all)
    }

    /// [`Self::pull`] of the entries `wanted`: the ids written and the ids
    /// removed. Models are written again only with `assets`. With `notes`
    /// an entry that fails is told there and the others go on; without, the
    /// first failure is the error.
    fn pull_entries(
        &mut self,
        game: &GameInstance,
        ws: &Workspace,
        cache: &mut BuildCache,
        wanted: &[EntryStatus],
        mut assets: Option<&mut (dyn crate::export::gltf::AssetSource + '_)>,
        mut notes: Option<&mut Vec<String>>,
    ) -> Result<(Vec<String>, Vec<String>)> {
        let mut pulled = Vec::new();
        let mut removed = Vec::new();
        for status_entry in wanted {
            let (folder_index, entry_index) = self
                .locate_entry(&status_entry.id)
                .expect("the status lists entries of the project");
            let dir = self.folders[folder_index].dir.clone();
            let entry = self.folders[folder_index].manifest.resources[entry_index].clone();
            // What the project adds and the workspace no longer has.
            if entry.is_added() && entry.keys.iter().all(|k| ws.change(k).is_none()) {
                for file in &entry.files {
                    let path = self.root.join(&dir).join(file);
                    let _ = std::fs::remove_file(&path);
                    // The folder of a model's textures, when it is empty now.
                    if let Some(parent) = path.parent() {
                        let _ = std::fs::remove_dir(parent);
                    }
                    cache.files.remove(&format!("{dir}/{file}"));
                }
                cache.entries.remove(&status_entry.id);
                removed.push(status_entry.id.clone());
                continue;
            }
            let lent: Option<&mut dyn crate::export::gltf::AssetSource> = match &mut assets {
                Some(assets) => Some(&mut **assets),
                None => None,
            };
            let fresh = match create::export_entry(
                game,
                ws,
                &self.root,
                &dir,
                &entry,
                &self.config,
                lent,
            ) {
                Ok(fresh) => fresh,
                Err(error) => match notes.as_deref_mut() {
                    Some(notes) => {
                        let message = plain(&error);
                        notes.push(if message.starts_with(&status_entry.id) {
                            message
                        } else {
                            format!("{}: {message}", status_entry.id)
                        });
                        continue;
                    }
                    None => return Err(error),
                },
            };
            let id = format!("{dir}/{}", fresh.files[0]);
            let mut outputs: BTreeMap<String, Option<String>> = BTreeMap::new();
            for key in &fresh.keys {
                outputs.insert(
                    key.to_string(),
                    ws.change(key).map(|c| c.payload.sha256.clone()),
                );
            }
            let mut shas = Vec::new();
            for f in &fresh.files {
                shas.push(
                    cache
                        .file_sha(&self.root, &format!("{dir}/{f}"))?
                        .unwrap_or_default(),
                );
            }
            let pairs: Vec<(&str, &str)> = fresh
                .files
                .iter()
                .zip(&shas)
                .map(|(f, s)| (f.as_str(), s.as_str()))
                .collect();
            let patch_sha = match &status_entry.patch {
                Some(p) => cache.file_sha(&self.root, p)?,
                None => None,
            };
            cache.entries.insert(
                id.clone(),
                cache::Built {
                    inputs: cache::digest(&pairs, patch_sha.as_deref()),
                    outputs,
                },
            );
            self.folders[folder_index].manifest.resources[entry_index] = fresh;
            pulled.push(id);
        }
        for folder in &mut self.folders {
            let dir = folder.dir.clone();
            let touched = |ids: &[String]| ids.iter().any(|p| p.starts_with(&format!("{dir}/")));
            if !touched(&pulled) && !touched(&removed) {
                continue;
            }
            folder
                .manifest
                .resources
                .retain(|e| !removed.contains(&format!("{dir}/{}", e.files[0])));
            folder
                .manifest
                .write(&self.root.join(&dir).join(manifest_name(&dir)))?;
        }
        Ok((pulled, removed))
    }

    /// Writes sources for everything the project's own workspace holds, so
    /// that `src/` describes all of it: new entries for the changes no
    /// entry has yet (resources replaced and added, with the description of
    /// the streams the workspace creates), the workspace's version of the
    /// entries that are out of sync ([`Self::pull`]; models too, with
    /// `index`), and nothing left of what the workspace no longer adds or
    /// creates. Files changed and not built yet are left as they are.
    /// `index` names the files and gives models their materials and
    /// textures.
    ///
    /// Only for a project with its own workspace: the workspace of the game
    /// may hold other work, which a project made of streams does not take.
    pub fn export_workspace(
        &mut self,
        game: &GameInstance,
        ws: &Workspace,
        index: Option<&crate::navigation::GameIndex>,
    ) -> Result<SourcesReport> {
        use crate::navigation::unpack::{AssetMaps, IndexAssets};
        if !self.has_own_workspace() {
            return Err(Error::InvalidArgument(
                "the project builds into the workspace of the game, which may hold other work; \
                 sources of a workspace are written for a project with its own workspace (make \
                 one from the workspace: `project new GAME DIR --move-workspace`)"
                    .into(),
            ));
        }
        let mut report = SourcesReport::default();
        let mut cache = BuildCache::read(&self.root);

        // 1. Entries that are out of sync follow the workspace.
        let status = self.status_with(ws, &mut cache)?;
        let wanted: Vec<EntryStatus> = status
            .entries
            .into_iter()
            .filter(|e| e.state == EntryState::OutOfSync)
            .collect();
        let maps = index
            .filter(|_| wanted.iter().any(|e| e.kind == Kind::Model))
            .map(AssetMaps::new);
        let mut assets = index
            .zip(maps.as_ref())
            .map(|(index, maps)| IndexAssets::new(game, index, maps));
        let (pulled, removed) = self.pull_entries(
            game,
            ws,
            &mut cache,
            &wanted,
            assets
                .as_mut()
                .map(|a| a as &mut dyn crate::export::gltf::AssetSource),
            Some(&mut report.notes),
        )?;
        report.pulled = pulled;
        report.removed = removed;

        // 2. What no entry has yet.
        let covered: BTreeSet<ResourceKey> = self
            .entries()
            .flat_map(|(_, e, _)| e.keys.clone())
            .collect();
        let mut selection: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        let mut additions = Additions::default();
        for change in ws.changes().filter(|c| !covered.contains(&c.key)) {
            if change.is_added() {
                additions.resources.push(change.key.clone());
                continue;
            }
            let place = game.find_stream(&change.key.stream).and_then(|s| {
                let file = game.open_stream(s).ok()?;
                Some((s, change.key.find_in(file.layout())?))
            });
            match place {
                Some((s, r)) => selection.entry(s).or_default().push(r),
                None => report
                    .notes
                    .push(format!("{} is not in the game; no sources", change.key)),
            }
        }
        for stream in ws.new_streams() {
            let dir = create::stream_dir(&stream.name);
            let described = NewStreamSource::of(stream);
            let recorded = cache::BuiltStream {
                stream: stream.name.clone(),
                described: described.digest(),
            };
            match self.folders.iter_mut().find(|f| f.dir == dir) {
                Some(folder) if folder.manifest.new.as_ref() == Some(&described) => {
                    cache.streams.insert(dir, recorded);
                }
                // A description changed in `stream.toml` and not built yet
                // is the author's, like a changed file.
                Some(folder)
                    if folder.manifest.new.as_ref().is_some_and(|new| {
                        cache
                            .streams
                            .get(&dir)
                            .is_none_or(|built| built.described != new.digest())
                    }) =>
                {
                    report.notes.push(format!(
                        "{dir}/{}: the description of stream {} was changed and is not built; \
                         left as it is",
                        manifest::STREAM_MANIFEST,
                        stream.name
                    ));
                }
                // Described anew: the workspace changed how it is loaded.
                Some(folder) => {
                    folder.manifest.new = Some(described);
                    folder.manifest.stream = Some(stream.name.clone());
                    folder
                        .manifest
                        .write(&self.root.join(&dir).join(manifest::STREAM_MANIFEST))?;
                    report.streams.push(stream.name.clone());
                    cache.streams.insert(dir, recorded);
                }
                None => additions.streams.push(stream.name.clone()),
            }
        }
        let selection: Vec<(usize, Vec<usize>)> = selection.into_iter().collect();
        if !selection.is_empty() || !additions.is_empty() {
            let exported = create::export_more(game, ws, index, self, &selection, &additions)?;
            report.files = exported.report.files;
            report.bytes = exported.report.bytes;
            report.notes.extend(exported.report.notes);
            for (dir, fresh) in exported.manifests {
                create::record(&mut cache, ws, &self.root, &dir, &fresh)?;
                report.added.extend(
                    fresh
                        .resources
                        .iter()
                        .map(|e| format!("{dir}/{}", e.files[0])),
                );
                if fresh.new.is_some() {
                    report.streams.extend(fresh.stream.clone());
                }
                let folder = match self.folders.iter().position(|f| f.dir == dir) {
                    Some(at) => {
                        let folder = &mut self.folders[at];
                        folder.manifest.resources.extend(fresh.resources);
                        if folder.manifest.new.is_none() {
                            folder.manifest.new = fresh.new;
                        }
                        folder
                    }
                    None => {
                        self.folders.push(Folder {
                            dir: dir.clone(),
                            manifest: fresh,
                        });
                        self.folders.last_mut().expect("pushed above")
                    }
                };
                folder
                    .manifest
                    .resources
                    .sort_by(|a, b| a.files[0].cmp(&b.files[0]));
                folder
                    .manifest
                    .write(&self.root.join(&dir).join(manifest_name(&dir)))?;
            }
        }

        // 3. Streams the workspace no longer creates.
        let mut gone = Vec::new();
        for folder in &self.folders {
            let (Some(stream), Some(_)) = (&folder.manifest.stream, &folder.manifest.new) else {
                continue;
            };
            // Only a folder that was built or written from the workspace:
            // one written by hand and not built yet is the author's.
            if ws.new_stream(stream).is_some() || !cache.streams.contains_key(&folder.dir) {
                continue;
            }
            if folder.manifest.resources.is_empty() {
                let path = self.root.join(&folder.dir);
                let _ = std::fs::remove_file(path.join(manifest::STREAM_MANIFEST));
                let _ = std::fs::remove_dir(&path);
                cache.streams.remove(&folder.dir);
                gone.push(folder.dir.clone());
            } else {
                report.notes.push(format!(
                    "{}: the workspace does not create {stream}, but the folder has files that \
                     were changed and not built; a build creates the stream again",
                    folder.dir
                ));
            }
        }
        self.folders.retain(|f| !gone.contains(&f.dir));
        report.removed.extend(gone);
        let outside = ws.file_changes().count();
        if outside > 0 {
            report.notes.push(format!(
                "{outside} file(s) outside the streams (sound archives, movies) have no sources: \
                 they stay in the workspace"
            ));
        }
        report.notes.sort();
        self.folders.sort_by(|a, b| a.dir.cmp(&b.dir));
        cache.write(&self.root)?;
        Ok(report)
    }

    fn locate_entry(&self, id: &str) -> Option<(usize, usize)> {
        self.folders.iter().enumerate().find_map(|(f, folder)| {
            folder
                .manifest
                .resources
                .iter()
                .position(|e| format!("{}/{}", folder.dir, e.files[0]) == id)
                .map(|e| (f, e))
        })
    }
}

/// The message of an error without the `invalid argument:` prefix, for
/// errors that are put after a path.
pub(crate) fn plain(error: &Error) -> String {
    match error {
        Error::InvalidArgument(message) => message.clone(),
        other => other.to_string(),
    }
}

/// `error` with `at: ` before its message.
pub(crate) fn at(at: &str, error: Error) -> Error {
    Error::InvalidArgument(format!("{at}: {}", plain(&error)))
}

/// `stream.toml`, or `shared.toml` in the shared folder.
fn manifest_name(dir: &str) -> &'static str {
    if dir == SHARED_DIR {
        manifest::SHARED_MANIFEST
    } else {
        manifest::STREAM_MANIFEST
    }
}

/// Whether the workspace creates stream `current` as `wanted` describes it
/// (the label aside, the names as tables of contents spell them).
fn same_stream(
    current: &crate::workspace::NewStream,
    wanted: &crate::workspace::NewStream,
) -> bool {
    use crate::workspace::key::normalize_stream;
    let normal = |s: &crate::workspace::NewStream| crate::workspace::NewStream {
        name: normalize_stream(&s.name),
        parent: normalize_stream(&s.parent),
        toc_parents: s
            .toc_parents
            .iter()
            .map(|(toc, parent)| (toc.clone(), normalize_stream(parent)))
            .collect(),
        guid: s.guid.to_ascii_lowercase(),
        label: String::new(),
        ..s.clone()
    };
    normal(current) == normal(wanted)
}

/// Whether the workspace has for each key of `entry` what its manifest
/// recorded when its files were written ([`Entry::outputs`]).
fn recorded_outputs_match(ws: &Workspace, entry: &Entry) -> bool {
    entry.outputs.len() == entry.keys.len()
        && entry.keys.iter().zip(&entry.outputs).all(|(key, sha)| {
            ws.change(key).map(|c| c.payload.sha256.as_str())
                == (!sha.is_empty()).then_some(sha.as_str())
        })
}

/// Whether the workspace has for each key what `outputs` recorded.
fn outputs_match(ws: &Workspace, outputs: &cache::Outputs) -> bool {
    outputs.iter().all(|(key, sha)| {
        let Ok(key) = ResourceKey::from_str(key) else {
            return false;
        };
        ws.change(&key).map(|c| &c.payload.sha256) == sha.as_ref()
    })
}

/// The workspace's state of `keys`.
fn state_of<'k>(ws: &Workspace, keys: impl Iterator<Item = &'k ResourceKey>) -> cache::Outputs {
    keys.map(|k| {
        (
            k.to_string(),
            ws.change(k).map(|c| c.payload.sha256.clone()),
        )
    })
    .collect()
}

/// Stream folders below `dir` (`rel` from the root): every folder with a
/// `stream.toml`.
fn find_stream_folders(dir: &Path, rel: &str, out: &mut Vec<Folder>) -> Result<()> {
    let manifest = dir.join(manifest::STREAM_MANIFEST);
    if manifest.is_file() {
        out.push(Folder {
            dir: rel.to_owned(),
            manifest: SourceManifest::read(&manifest)?,
        });
        // Its subfolders are types; streams below it are next to it (`x.str`).
        return Ok(());
    }
    for entry in std::fs::read_dir(dir).map_err(|e| Error::io(dir, e))? {
        let entry = entry.map_err(|e| Error::io(dir, e))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        if entry.file_type().map_err(|e| Error::io(dir, e))?.is_dir() {
            find_stream_folders(&entry.path(), &format!("{rel}/{name}"), out)?;
        }
    }
    Ok(())
}

/// Relative paths of every file below `dir`, with forward slashes; hidden
/// files and editor backups (`name~`) are left out.
fn collect_files(dir: &Path, rel: &str, out: &mut Vec<String>) -> Result<()> {
    for entry in std::fs::read_dir(dir).map_err(|e| Error::io(dir, e))? {
        let entry = entry.map_err(|e| Error::io(dir, e))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name.ends_with('~') {
            continue;
        }
        let path = format!("{rel}/{name}");
        if entry.path().is_dir() {
            collect_files(&entry.path(), &path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod added_tests;
#[cfg(test)]
mod tests;

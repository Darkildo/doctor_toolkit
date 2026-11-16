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
//!     └── build.json                  build cache (may be deleted)
//! ```
//!
//! [`Project::build`] builds only what changed since the last build: the
//! SHA-256 of every source file (read again only when its size or time
//! changed) against the cache, and the state of the workspace against what
//! the last build left there. The result goes **into the workspace**, as
//! one undoable operation, never straight into the game; `apply`, "Play"
//! and mods take it from there. A file that fails is reported with its path;
//! the others are built. See `docs/project.md`.

pub mod cache;
pub mod config;
pub mod cook;
pub mod create;
pub mod guid;
pub mod manifest;
pub mod patch;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::game::GameInstance;
use crate::workspace::{ResourceKey, Workspace};
use crate::{Error, Result};

pub use cache::BuildCache;
pub use config::{PROJECT_FILE, ProjectConfig};
pub use create::{CreateOptions, CreateReport, create, workspace_selection};
pub use manifest::{Entry, Kind, SourceManifest};
pub use patch::SimGroupPatch;

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
    /// The files (or the patch) changed since the last build.
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
}

impl EntryState {
    /// Whether a build would write the entry.
    pub fn needs_build(&self) -> bool {
        matches!(self, Self::Modified | Self::OutOfSync)
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Unchanged => "unchanged",
            Self::Built => "built",
            Self::Modified => "modified",
            Self::OutOfSync => "out of sync",
            Self::Missing(_) => "missing",
            Self::ViewChanged => "view changed",
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

/// [`Project::status`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectStatus {
    pub entries: Vec<EntryStatus>,
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

    /// Entries a build would write.
    pub fn to_build(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.state.needs_build())
            .count()
    }
}

/// Options of [`Project::build`].
#[derive(Debug, Clone, Default)]
pub struct BuildOptions {
    /// Only tell what would be built.
    pub dry_run: bool,
    /// Build every entry, changed or not. Textures and models are encoded
    /// again, so their data may differ from the game's even when the file
    /// did not change.
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
            .or_else(|| self.config.game.path.clone())
            .ok_or_else(|| {
                Error::InvalidArgument(
                    "the project does not say where the game is; give the game folder".into(),
                )
            })?;
        GameInstance::open(path)
    }

    /// The workspace builds go to.
    pub fn workspace(&self, game: &GameInstance) -> Result<Workspace> {
        Workspace::for_game(game, self.config.workspace_dir(&self.root).as_deref())
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
                let state = match (&missing, &inputs) {
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
                        None if *inputs == entry.exported => EntryState::Unchanged,
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

    /// Warnings about the game: another executable, mods the project needs
    /// that are not installed and enabled.
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
        if !self.config.dependencies.mods.is_empty() {
            let installed: Vec<(String, bool)> = crate::mods::manager::ModManager::open(game)
                .map(|m| m.mods().iter().map(|m| (m.id.clone(), m.enabled)).collect())
                .unwrap_or_default();
            for id in &self.config.dependencies.mods {
                match installed.iter().find(|(i, _)| i == id) {
                    None => warnings.push(format!(
                        "the project needs the mod `{id}`; it is not installed"
                    )),
                    Some((_, false)) => {
                        warnings.push(format!("the project needs the mod `{id}`; it is disabled"))
                    }
                    Some(_) => {}
                }
            }
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
        if options.dry_run {
            report.built = work
                .iter()
                .map(|e| BuiltEntry {
                    id: e.id.clone(),
                    changed: Vec::new(),
                    notes: vec![e.state.label().to_owned()],
                })
                .collect();
            let _ = cache.write(&self.root);
            return Ok(report);
        }
        let since = ws.head();
        let mut cook = cook::Cook::new(game, &self.root, &self.config.project.id);
        let mut touched: BTreeSet<ResourceKey> = BTreeSet::new();
        for status_entry in work {
            let (entry, dir) = self
                .entry(&status_entry.id)
                .expect("the status lists entries of the project");
            let before = ws.head();
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
                cook.entry(ws, dir, entry, status_entry.patch.as_deref())
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
        cache.write(&self.root)?;
        Ok(report)
    }

    /// Resources the project's builds changed that the workspace still
    /// changes: what a mod of the project holds.
    pub fn changed_keys(&self, ws: &Workspace) -> Vec<ResourceKey> {
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

    /// The mod metadata of the project.
    pub fn mod_info(&self) -> crate::mods::ModInfo {
        let p = &self.config.project;
        crate::mods::ModInfo {
            id: p.id.clone(),
            name: p.name.clone(),
            version: p.version.clone(),
            author: p.author.clone(),
            description: p.description.clone(),
            url: (!p.url.is_empty()).then(|| p.url.clone()),
            // The mods the project needs become dependencies of any version.
            relations: crate::mods::Relations {
                depends: self
                    .config
                    .dependencies
                    .mods
                    .iter()
                    .map(|id| (id.clone(), "*".to_owned()))
                    .collect(),
                ..Default::default()
            },
        }
    }

    /// Writes a mod (format 2) of the project's changes in `ws` to `out`.
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
    /// workspace's data as their new base.
    pub fn pull(
        &mut self,
        game: &GameInstance,
        ws: &Workspace,
        ids: &[String],
    ) -> Result<Vec<String>> {
        let mut cache = BuildCache::read(&self.root);
        let status = self.status_with(ws, &mut cache)?;
        let wanted: Vec<&EntryStatus> = status
            .entries
            .iter()
            .filter(|e| e.state == EntryState::OutOfSync)
            .filter(|e| ids.is_empty() || ids.iter().any(|i| e.id.starts_with(i.as_str())))
            .collect();
        let mut pulled = Vec::new();
        for status_entry in wanted {
            let (folder_index, entry_index) = self
                .locate_entry(&status_entry.id)
                .expect("the status lists entries of the project");
            let dir = self.folders[folder_index].dir.clone();
            let entry = self.folders[folder_index].manifest.resources[entry_index].clone();
            let fresh = create::export_entry(game, ws, &self.root, &dir, &entry, &self.config)?;
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
        for folder in &self.folders {
            if pulled
                .iter()
                .any(|p| p.starts_with(&format!("{}/", folder.dir)))
            {
                let name = if folder.dir == SHARED_DIR {
                    manifest::SHARED_MANIFEST
                } else {
                    manifest::STREAM_MANIFEST
                };
                folder
                    .manifest
                    .write(&self.root.join(&folder.dir).join(name))?;
            }
        }
        cache.write(&self.root)?;
        Ok(pulled)
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
mod tests;

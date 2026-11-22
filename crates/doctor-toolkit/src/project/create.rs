//! Making a project: resources of the game (or the changes of a workspace)
//! written as source files with their manifests.
//!
//! A resource is taken from where it is ([`Res`]): the game's stream, by its
//! position there, or the workspace, which **added** it (stage 112). A
//! stream the workspace creates has only added resources and no file of the
//! game; its folder carries its description. So the sources of a workspace
//! hold all of it, and [`super::Project::build`] makes the same changes
//! again in another workspace.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use rayon::prelude::*;
use serde_json::{Value, json};

use crate::container::ustream::{ResourceHeader, StreamLayout};
use crate::create::TYPE_MATERIAL;
use crate::export::gltf::{AssetSource, NoAssets};
use crate::game::{GameInstance, StreamFile};
use crate::navigation::GameIndex;
use crate::navigation::unpack::{AssetMaps, IndexAssets, Selection};
use crate::resource::anim::{AnimBank, TYPE_BNK};
use crate::resource::chv::{ChatterSet, TYPE_CHV};
use crate::resource::dlg::{Dialogue, TYPE_DLG};
use crate::resource::graph::{Graph, TYPE_GRAPH};
use crate::resource::lua::{self, Chunk, Comparison, TYPE_LUA};
use crate::resource::mesh::{Model, TYPE_MESH, TYPE_MESH_VOLATILE, volatile_for};
use crate::resource::mobface::TYPE_MOBFACE;
use crate::resource::names::{mem_policies, type_tags};
use crate::resource::rcb::{Rig, TYPE_RCB};
use crate::resource::texture::{TYPE_TG4D, TYPE_TG4H, Texture, pair_of};
use crate::resource::tg4::Tg4Header;
use crate::resource::trinity::{SeqMaster, TYPE_TRINITY_SEQ_MASTER};
use crate::resource::{logic, logic_import};
use crate::scene::{self, ModelFormat};
use crate::workspace::store::sha256_hex;
use crate::workspace::{Change, EditedAssets, ResourceKey, Workspace};
use crate::{Error, Result};

use super::cache::{self, BuildCache, Built};
use super::config::{
    BuildSettings, Dependencies, FORMAT, GameInfo, PROJECT_FILE, ProjectConfig, ProjectInfo,
};
use super::manifest::{
    AddedSource, Entry, Kind, NewStreamSource, SHARED_MANIFEST, STREAM_MANIFEST, SourceManifest,
};
use super::{Project, SHARED_DIR, SOURCE_DIR};

const TYPE_LH2: u32 = crate::hash::sdbm(b"lh2");
const TYPE_APX: u32 = crate::hash::sdbm(b"apx");

/// What to put into a new project.
#[derive(Debug, Clone)]
pub struct CreateOptions {
    /// Name of the project (and of its mod).
    pub name: String,
    /// Mod id; default: made from the name.
    pub id: Option<String>,
    pub author: String,
    /// Resources to take: per stream, resource indices. Textures and models
    /// take the other resource of their pair along.
    pub selection: Selection,
    pub settings: BuildSettings,
}

/// What [`create`] wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CreateReport {
    /// Source entries (one per resource, texture or model, and views).
    pub entries: usize,
    /// Entries in `shared/`.
    pub shared: usize,
    pub files: usize,
    pub resources: usize,
    pub bytes: u64,
    /// Resources kept as raw data because their conversion failed, with the reason.
    pub notes: Vec<String>,
    pub cancelled: bool,
    /// Streams the workspace creates whose description was written.
    pub new_streams: usize,
}

/// What a project takes that is not in the game: resources the workspace
/// adds, by key, and streams it creates, by name. A [`Selection`] names
/// resources by their place in a stream of the game, which these do not have.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Additions {
    pub resources: Vec<ResourceKey>,
    pub streams: Vec<String>,
}

impl Additions {
    pub fn is_empty(&self) -> bool {
        self.resources.is_empty() && self.streams.is_empty()
    }
}

/// Every resource a workspace adds and every stream it creates: with
/// [`workspace_selection`], all of its changes of streams.
pub fn workspace_additions(ws: &Workspace) -> Additions {
    Additions {
        resources: ws
            .changes()
            .filter(|c| c.is_added())
            .map(|c| c.key.clone())
            .collect(),
        streams: ws.new_streams().map(|s| s.name.clone()).collect(),
    }
}

/// The resources a workspace replaces, as a selection. The resources it
/// adds and the streams it creates are not in the game:
/// [`workspace_additions`] names them.
pub fn workspace_selection(game: &GameInstance, ws: &Workspace) -> Result<Selection> {
    let mut by_stream: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let mut open: HashMap<usize, StreamFile> = HashMap::new();
    for change in ws.changes().filter(|c| !c.is_added()) {
        let Some(s) = game.find_stream(&change.key.stream) else {
            continue;
        };
        if let std::collections::hash_map::Entry::Vacant(e) = open.entry(s) {
            e.insert(game.open_stream(s)?);
        }
        if let Some(r) = change.key.find_in(open[&s].layout()) {
            by_stream.entry(s).or_default().push(r);
        }
    }
    Ok(by_stream
        .into_iter()
        .map(|(s, mut list)| {
            list.sort_unstable();
            list.dedup();
            (s, list)
        })
        .collect())
}

/// A resource of a stream as a project takes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Res {
    /// A resource of the game's stream, by its position in the file.
    Game(usize),
    /// A resource the workspace adds to the stream: its position in
    /// [`StreamSource::added`].
    Added(usize),
}

/// The resources of one stream as the workspace has it: those of the game's
/// file with the workspace's data, and those the workspace adds.
pub(super) struct StreamSource {
    pub name: String,
    /// The game's file; `None` for a stream the workspace creates.
    pub file: Option<StreamFile>,
    /// The original file when the game file is a written one (backup).
    original: Option<StreamFile>,
    /// What the workspace adds to the stream, in the order of its file.
    pub added: Vec<Change>,
}

impl StreamSource {
    pub fn open(game: &GameInstance, ws: &Workspace, name: &str) -> Result<Self> {
        let name = crate::workspace::key::normalize_stream(name);
        let mut added: Vec<Change> = ws
            .changes()
            .filter(|c| c.key.stream == name && c.is_added())
            .cloned()
            .collect();
        added.sort_by_key(|c| c.added.as_ref().map_or(0, |a| a.order));
        // A stream the workspace creates is checked first: once it is
        // applied or staged its file is among the game's as well, and the
        // workspace still holds all of it.
        let stream = match ws.new_stream(&name) {
            Some(_) => None,
            None => Some(game.find_stream(&name).ok_or_else(|| {
                Error::InvalidArgument(format!(
                    "stream {name} is neither in the game nor new in the workspace"
                ))
            })?),
        };
        let (file, original) = match stream {
            Some(stream) => {
                let info = &game.streams()[stream];
                let file = game.open_stream(stream)?;
                let original_path = game.original_path(info);
                let original = if original_path != info.path {
                    Some(StreamFile::open(&original_path)?)
                } else {
                    None
                };
                (Some(file), original)
            }
            None => (None, None),
        };
        Ok(Self {
            name,
            file,
            original,
            added,
        })
    }

    fn layout(&self) -> Option<&StreamLayout> {
        self.file.as_ref().map(StreamFile::layout)
    }

    pub fn header(&self, res: Res) -> ResourceHeader {
        match res {
            Res::Game(r) => {
                self.layout()
                    .expect("a resource of the game has a file")
                    .resources[r]
                    .header
            }
            Res::Added(i) => {
                let change = &self.added[i];
                change
                    .added
                    .as_ref()
                    .expect("only added resources are listed")
                    .header(&change.key, change.payload.size)
            }
        }
    }

    pub fn key(&self, res: Res) -> ResourceKey {
        match res {
            Res::Game(_) => ResourceKey::new(&self.name, &self.header(res)),
            Res::Added(i) => self.added[i].key.clone(),
        }
    }

    /// The resource with key `key`: one the workspace adds, else one of the
    /// game's file.
    pub fn resource(&self, key: &ResourceKey) -> Option<Res> {
        self.added
            .iter()
            .position(|c| c.key == *key)
            .map(Res::Added)
            .or_else(|| key.find_in(self.layout()?).map(Res::Game))
    }

    /// The record of `res` for a manifest when the workspace adds it.
    fn added_source(&self, res: Res) -> Option<AddedSource> {
        let Res::Added(i) = res else { return None };
        let change = &self.added[i];
        Some(AddedSource::of(
            &change.key,
            change.added.as_ref()?,
            change.storage,
        ))
    }

    /// Data of `res`: the workspace's change, or the original data.
    pub fn data(&self, ws: &Workspace, res: Res) -> Result<Vec<u8>> {
        let key = self.key(res);
        if let Some(data) = ws.data(&key)? {
            return Ok(data);
        }
        let Res::Game(r) = res else {
            return Err(Error::InvalidArgument(format!(
                "{key} is not in the workspace"
            )));
        };
        match &self.original {
            Some(original) => {
                let index = key.find_in(original.layout()).ok_or_else(|| {
                    Error::InvalidArgument(format!("{key} is not in the original stream"))
                })?;
                original.resource_data(index)
            }
            None => self
                .file
                .as_ref()
                .expect("a resource of the game has a file")
                .resource_data(r),
        }
    }
}

/// One source entry to write: the resources it stands for in one stream.
#[derive(Debug, Clone)]
struct Unit {
    kind: Kind,
    resources: Vec<Res>,
    /// Name from the index, when there is one.
    name: Option<String>,
    /// SHA-256 of the data of each resource; with the headers, what makes
    /// two units of different streams the same.
    shas: Vec<String>,
    /// Type and ids of the main resource.
    type_id: u32,
    ids: [u32; 4],
}

struct StreamPlan {
    /// The stream of the game; `None` for one the workspace creates.
    stream: Option<usize>,
    name: String,
    units: Vec<Unit>,
    /// The description of a stream the workspace creates.
    new: Option<NewStreamSource>,
}

/// What of one stream goes into a project.
#[derive(Debug, Clone, Default)]
struct PlanInput {
    name: String,
    /// Resources of the game's file.
    indices: Vec<usize>,
    /// Resources the workspace adds.
    added: Vec<ResourceKey>,
}

/// The streams of a selection and of additions, the selected ones first in
/// their order, then the others by name.
fn inputs_of(game: &GameInstance, selection: &Selection, additions: &Additions) -> Vec<PlanInput> {
    let mut inputs: Vec<PlanInput> = selection
        .iter()
        .map(|(s, resources)| PlanInput {
            name: game.streams()[*s].stream_name(),
            indices: resources.clone(),
            added: Vec::new(),
        })
        .collect();
    let mut rest: BTreeMap<String, PlanInput> = BTreeMap::new();
    let names = additions
        .resources
        .iter()
        .map(|k| (k.stream.clone(), Some(k)))
        .chain(
            additions
                .streams
                .iter()
                .map(|s| (crate::workspace::key::normalize_stream(s), None)),
        );
    for (name, key) in names {
        let input = match inputs.iter_mut().find(|i| i.name == name) {
            Some(input) => input,
            None => rest.entry(name.clone()).or_insert_with(|| PlanInput {
                name,
                ..PlanInput::default()
            }),
        };
        if let Some(key) = key
            && !input.added.contains(key)
        {
            input.added.push(key.clone());
        }
    }
    inputs.extend(rest.into_values());
    inputs
}

/// Where each unit goes.
#[derive(Debug, Clone)]
enum Place {
    /// Its stream folder, with this stem (`tg4h/buttons`).
    Stream(String),
    /// `shared/`, written by this stream; `copies` are the same resource in
    /// other streams (plan, unit).
    SharedOwner {
        stem: String,
        copies: Vec<(usize, usize)>,
    },
    /// `shared/`, written by another stream.
    SharedCopy,
}

/// How the folder of a workspace is written into `project.toml` of the
/// project in `root`: from the project folder when it is inside it, else
/// whole. A path relative to where the command ran would be read from the
/// project folder later (`ProjectConfig::workspace_dir`) and name another
/// place.
pub fn stored_workspace(root: &Path, dir: &Path) -> std::path::PathBuf {
    let whole = std::path::absolute(dir).unwrap_or_else(|_| dir.to_owned());
    let root = std::path::absolute(root).unwrap_or_else(|_| root.to_owned());
    match whole.strip_prefix(&root) {
        Ok(inside) => inside.to_owned(),
        Err(_) => whole,
    }
}

/// What a project records about `game`: where it is, its profile and
/// edition, the language shown first.
pub(super) fn game_info(game: &GameInstance) -> GameInfo {
    let edition = crate::edition::Edition::identify(game.root());
    GameInfo {
        path: Some(std::fs::canonicalize(game.root()).unwrap_or_else(|_| game.root().to_owned())),
        profile: game.profile().name.to_owned(),
        edition: edition.name(),
        executable_sha256: edition
            .executable
            .as_ref()
            .map(|e| e.sha256.clone())
            .unwrap_or_default(),
        language: edition
            .languages
            .first()
            .cloned()
            .unwrap_or_else(|| "en".into()),
    }
}

/// Makes a project in `root` (a new or empty directory) from the selected
/// resources, as the workspace has them (its change, else the original).
/// `index` gives file names and the materials and textures of models.
pub fn create(
    game: &GameInstance,
    ws: &Workspace,
    index: Option<&GameIndex>,
    root: &Path,
    options: &CreateOptions,
    progress: &(dyn Fn(usize, usize) + Sync),
    cancel: &AtomicBool,
) -> Result<(Project, CreateReport)> {
    create_with(
        game,
        ws,
        index,
        root,
        options,
        &Additions::default(),
        progress,
        cancel,
    )
}

/// [`create`], with the resources the workspace adds and the streams it
/// creates that `additions` names.
#[allow(clippy::too_many_arguments)]
pub fn create_with(
    game: &GameInstance,
    ws: &Workspace,
    index: Option<&GameIndex>,
    root: &Path,
    options: &CreateOptions,
    additions: &Additions,
    progress: &(dyn Fn(usize, usize) + Sync),
    cancel: &AtomicBool,
) -> Result<(Project, CreateReport)> {
    if root.join(PROJECT_FILE).exists() {
        return Err(Error::InvalidArgument(format!(
            "{} already holds a project",
            root.display()
        )));
    }
    if root.is_dir()
        && std::fs::read_dir(root)
            .map_err(|e| Error::io(root, e))?
            .next()
            .is_some()
    {
        return Err(Error::InvalidArgument(format!(
            "{} is not empty; a project goes into a new or empty folder",
            root.display()
        )));
    }
    let id = match &options.id {
        Some(id) => id.clone(),
        None => crate::mods::id_from_name(&options.name),
    };
    crate::mods::validate_id(&id)?;
    model_format_of(&options.settings)?;
    std::fs::create_dir_all(root).map_err(|e| Error::io(root, e))?;

    let inputs = inputs_of(game, &options.selection, additions);
    let exported = export(
        game,
        ws,
        index,
        root,
        &options.settings,
        &inputs,
        options.settings.shared,
        &|_| Vec::new(),
        progress,
        cancel,
    )?;

    // Manifests, project file and the state of the workspace.
    let mut report = exported.report;
    let mut manifests = exported.manifests;
    let mut cache = BuildCache::default();
    for (folder, manifest) in &mut manifests {
        manifest
            .resources
            .sort_by(|a, b| a.files[0].cmp(&b.files[0]));
        let file = if folder == SHARED_DIR {
            SHARED_MANIFEST
        } else {
            STREAM_MANIFEST
        };
        manifest.write(&root.join(folder).join(file))?;
        record(&mut cache, ws, root, folder, manifest)?;
    }
    report.notes.sort();
    let config = ProjectConfig {
        format: FORMAT,
        project: ProjectInfo {
            name: options.name.clone(),
            id,
            version: "1.0".into(),
            author: options.author.clone(),
            description: String::new(),
            url: String::new(),
        },
        game: game_info(game),
        dependencies: Dependencies::default(),
        base: None,
        build: options.settings.clone(),
        zones: Vec::new(),
    };
    config.write(root)?;
    crate::atomic::write(
        &root.join(".gitignore"),
        format!(
            "# Build cache of doctor_toolkit (`{dir}/{base}` is needed to build).\n/{dir}/{cache}\n",
            dir = cache::TOOL_DIR,
            base = cache::BASE_DIR,
            cache = cache::CACHE_FILE
        )
        .as_bytes(),
    )?;
    cache.write(root)?;
    Ok((Project::open(root)?, report))
}

/// Records in `cache` that the entries of `manifest` (of folder `folder`)
/// are what the workspace has: the state a project starts from, in which
/// nothing is to be built.
pub(super) fn record(
    cache: &mut BuildCache,
    ws: &Workspace,
    root: &Path,
    folder: &str,
    manifest: &SourceManifest,
) -> Result<()> {
    if let (Some(stream), Some(new)) = (&manifest.stream, &manifest.new) {
        cache.streams.insert(
            folder.to_owned(),
            cache::BuiltStream {
                stream: stream.clone(),
                described: new.digest(),
            },
        );
    }
    for entry in &manifest.resources {
        let outputs = entry
            .keys
            .iter()
            .map(|k| {
                (
                    k.to_string(),
                    ws.change(k).map(|c| c.payload.sha256.clone()),
                )
            })
            .collect();
        cache.entries.insert(
            format!("{folder}/{}", entry.files[0]),
            Built {
                inputs: entry.exported.clone(),
                outputs,
            },
        );
        for f in &entry.files {
            cache.file_sha(root, &format!("{folder}/{f}"))?;
        }
    }
    Ok(())
}

/// [`export`]: the entries written, by folder, with the manifests of the
/// streams (also of those without entries), and the numbers.
pub(super) struct Exported {
    pub manifests: BTreeMap<String, SourceManifest>,
    pub report: CreateReport,
}

/// Writes the source files of `inputs` under `root` and returns their
/// entries in new manifests (nothing but the files of entries and the data
/// in `.doctor/base/` is written). `taken(folder)` lists the paths of the
/// files a folder already has, from that folder: new files get other names.
#[allow(clippy::too_many_arguments)]
fn export(
    game: &GameInstance,
    ws: &Workspace,
    index: Option<&GameIndex>,
    root: &Path,
    settings: &BuildSettings,
    inputs: &[PlanInput],
    shared: bool,
    taken: &(dyn Fn(&str) -> Vec<String> + Sync),
    progress: &(dyn Fn(usize, usize) + Sync),
    cancel: &AtomicBool,
) -> Result<Exported> {
    let model_format = model_format_of(settings)?;
    // 1. What each stream holds, with data hashes to find shared resources.
    let total: usize = inputs.iter().map(|i| i.indices.len() + i.added.len()).sum();
    let plans: Vec<StreamPlan> = inputs
        .par_iter()
        .map(|input| plan_stream(game, ws, index, input))
        .collect::<Result<_>>()?;

    // 2. Shared resources and file names.
    let mut places: Vec<Vec<Option<Place>>> =
        plans.iter().map(|p| vec![None; p.units.len()]).collect();
    let mut shared_count = 0;
    if shared && plans.len() > 1 {
        let mut groups: BTreeMap<String, Vec<(usize, usize)>> = BTreeMap::new();
        for (p, plan) in plans.iter().enumerate() {
            let source = StreamSource::open(game, ws, &plan.name)?;
            for (u, unit) in plan.units.iter().enumerate() {
                // What the workspace adds is one stream's own.
                if unit.resources.iter().all(|r| matches!(r, Res::Game(_))) {
                    groups
                        .entry(fingerprint(&source, unit))
                        .or_default()
                        .push((p, u));
                }
            }
        }
        let mut shared_stems = stems_of(&taken(SHARED_DIR));
        for list in groups.values() {
            let streams: HashSet<usize> = list.iter().map(|(p, _)| *p).collect();
            if streams.len() < 2 {
                continue;
            }
            let (p, u) = list[0];
            let stem = free_stem(&mut shared_stems, &plans[p].units[u]);
            places[p][u] = Some(Place::SharedOwner {
                stem,
                copies: list[1..].to_vec(),
            });
            for &(cp, cu) in &list[1..] {
                places[cp][cu] = Some(Place::SharedCopy);
            }
            shared_count += 1;
        }
    }
    for (p, plan) in plans.iter().enumerate() {
        let mut stems = stems_of(&taken(&stream_dir(&plan.name)));
        for (u, unit) in plan.units.iter().enumerate() {
            if places[p][u].is_none() {
                places[p][u] = Some(Place::Stream(free_stem(&mut stems, unit)));
            }
        }
    }

    // 3. The files.
    let maps = index
        .filter(|_| {
            plans
                .iter()
                .any(|p| p.units.iter().any(|u| u.kind == Kind::Model))
        })
        .map(AssetMaps::new);
    let done = AtomicUsize::new(0);
    let writer = Writer {
        ws,
        root,
        model_format,
        dds: settings.textures.eq_ignore_ascii_case("dds"),
    };
    let outputs: Vec<Result<StreamOutput>> = plans
        .par_iter()
        .enumerate()
        .map(|(p, plan)| {
            let mut output = StreamOutput::default();
            if cancel.load(Ordering::Relaxed) {
                output.cancelled = true;
                return Ok(output);
            }
            let source = StreamSource::open(game, ws, &plan.name)?;
            let mut assets = index
                .zip(maps.as_ref())
                .map(|(index, maps)| IndexAssets::new(game, index, maps));
            for (u, unit) in plan.units.iter().enumerate() {
                let count = unit.resources.len();
                let (folder, stem, copies) = match places[p][u].as_ref().expect("placed") {
                    Place::Stream(stem) => (stream_dir(&plan.name), stem.clone(), Vec::new()),
                    Place::SharedOwner { stem, copies } => {
                        (SHARED_DIR.to_owned(), stem.clone(), copies.clone())
                    }
                    Place::SharedCopy => {
                        progress(done.fetch_add(count, Ordering::Relaxed) + count, total);
                        continue;
                    }
                };
                let mut keys: Vec<ResourceKey> =
                    unit.resources.iter().map(|&r| source.key(r)).collect();
                for &(cp, cu) in &copies {
                    let other = &plans[cp];
                    let file =
                        game.open_stream(other.stream.expect("shared units are the game's"))?;
                    for r in &other.units[cu].resources {
                        if let Res::Game(r) = r {
                            keys.push(ResourceKey::new(
                                &other.name,
                                &file.layout().resources[*r].header,
                            ));
                        }
                    }
                }
                let entry = writer.unit(
                    &source,
                    assets.as_mut().map(|a| a as &mut dyn AssetSource),
                    unit,
                    &folder,
                    &stem,
                    keys,
                    &mut output.notes,
                )?;
                progress(done.fetch_add(count, Ordering::Relaxed) + count, total);
                let Some(entry) = entry else {
                    continue;
                };
                output.files += entry.files.len();
                output.bytes += entry
                    .files
                    .iter()
                    .map(|f| std::fs::metadata(root.join(&folder).join(f)).map_or(0, |m| m.len()))
                    .sum::<u64>();
                if unit.kind != Kind::View {
                    output.resources += count * (1 + copies.len());
                }
                output.entries.push((folder, entry));
            }
            Ok(output)
        })
        .collect();

    // 4. The entries by folder.
    let mut report = CreateReport {
        shared: shared_count,
        ..CreateReport::default()
    };
    let mut manifests: BTreeMap<String, SourceManifest> = BTreeMap::new();
    for plan in &plans {
        manifests.entry(stream_dir(&plan.name)).or_insert_with(|| {
            let mut manifest = SourceManifest::new(Some(plan.name.clone()));
            manifest.new = plan.new.clone();
            manifest
        });
        report.new_streams += usize::from(plan.new.is_some());
    }
    for output in outputs {
        let output = output?;
        report.files += output.files;
        report.bytes += output.bytes;
        report.resources += output.resources;
        report.notes.extend(output.notes);
        report.cancelled |= output.cancelled;
        for (folder, entry) in output.entries {
            report.entries += 1;
            manifests
                .entry(folder.clone())
                .or_insert_with(|| SourceManifest::new(None))
                .resources
                .push(entry);
        }
    }
    Ok(Exported { manifests, report })
}

/// Writes sources for what the workspace holds that no entry of `project`
/// has yet (`selection`, `additions`) into the folders of the project,
/// under names its files do not use. Returns the new entries by folder;
/// the caller merges them into its manifests. Nothing is shared between
/// streams here: `shared/` is made when a project is.
pub(super) fn export_more(
    game: &GameInstance,
    ws: &Workspace,
    index: Option<&GameIndex>,
    project: &Project,
    selection: &Selection,
    additions: &Additions,
) -> Result<Exported> {
    let root = project.root();
    let inputs = inputs_of(game, selection, additions);
    export(
        game,
        ws,
        index,
        root,
        &project.config.build,
        &inputs,
        false,
        &|folder| {
            let mut files = Vec::new();
            let dir = root.join(folder);
            if dir.is_dir() {
                let _ = super::collect_files(&dir, "", &mut files);
            }
            files
                .into_iter()
                .map(|f| f.trim_start_matches('/').to_owned())
                .collect()
        },
        &|_, _| {},
        &AtomicBool::new(false),
    )
}

/// Writes the files of `entry` (of folder `dir`) again from the workspace's
/// data of its first stream, under the same names, and returns the entry
/// with its new digest and base. A model is written again only with
/// `assets` (its materials and textures come from there); its other files
/// may change then, and those it no longer has are removed.
pub(super) fn export_entry(
    game: &GameInstance,
    ws: &Workspace,
    root: &Path,
    dir: &str,
    entry: &Entry,
    config: &ProjectConfig,
    assets: Option<&mut dyn AssetSource>,
) -> Result<Entry> {
    if entry.kind == Kind::Model && assets.is_none() {
        return Err(Error::InvalidArgument(format!(
            "{dir}/{}: models are not taken back from the workspace; export the model again",
            entry.files[0]
        )));
    }
    let (stream, keys) = entry
        .targets()
        .into_iter()
        .next()
        .ok_or_else(|| Error::InvalidArgument("an entry without resources".into()))?;
    let source = StreamSource::open(game, ws, &stream)?;
    let resources: Vec<Res> = keys
        .iter()
        .map(|k| {
            source.resource(k).ok_or_else(|| {
                Error::InvalidArgument(format!("{k} is not in the game or the workspace"))
            })
        })
        .collect::<Result<_>>()?;
    let shas = resources
        .iter()
        .map(|&r| source.data(ws, r).map(|d| sha256_hex(&d)))
        .collect::<Result<Vec<_>>>()?;
    let main = source.header(resources[0]);
    let unit = Unit {
        kind: entry.kind,
        resources,
        name: Some(entry.name.clone()),
        shas,
        type_id: main.type_id,
        ids: main.ids,
    };
    let file = &entry.files[0];
    let stem = file
        .rsplit_once('.')
        .map_or_else(|| file.clone(), |(stem, _)| stem.to_owned());
    let writer = Writer {
        ws,
        root,
        model_format: model_format_of(&config.build)?,
        dds: file.to_ascii_lowercase().ends_with(".dds"),
    };
    let mut notes = Vec::new();
    let fresh = writer
        .unit(
            &source,
            assets,
            &unit,
            dir,
            &stem,
            entry.keys.clone(),
            &mut notes,
        )?
        .ok_or_else(|| Error::InvalidArgument(notes.join("; ")))?;
    if fresh.kind != entry.kind || fresh.files[0] != entry.files[0] {
        return Err(Error::InvalidArgument(format!(
            "{dir}/{file}: the workspace's data gives other files ({}); {}",
            fresh.files.join(", "),
            notes.join("; ")
        )));
    }
    if fresh.files != entry.files {
        if entry.kind != Kind::Model {
            return Err(Error::InvalidArgument(format!(
                "{dir}/{file}: the workspace's data gives other files ({}); {}",
                fresh.files.join(", "),
                notes.join("; ")
            )));
        }
        for stale in entry.files.iter().filter(|f| !fresh.files.contains(f)) {
            let _ = std::fs::remove_file(root.join(dir).join(stale));
        }
    }
    Ok(Entry {
        name: entry.name.clone(),
        ..fresh
    })
}

/// The model format of `settings`.
fn model_format_of(settings: &BuildSettings) -> Result<ModelFormat> {
    ModelFormat::parse_name(&settings.models)
        .filter(|f| ModelFormat::WRITABLE.contains(f))
        .ok_or_else(|| {
            Error::InvalidArgument(format!(
                "unknown model format `{}`; use glb, fbx, obj, usd, usda, usdc or usdz",
                settings.models
            ))
        })
}

/// Folder of a stream below the project root.
pub(super) fn stream_dir(stream: &str) -> String {
    format!("{SOURCE_DIR}/{stream}.str")
}

#[derive(Default)]
struct StreamOutput {
    entries: Vec<(String, Entry)>,
    files: usize,
    resources: usize,
    bytes: u64,
    notes: Vec<String>,
    cancelled: bool,
}

/// The units of one stream: textures and models with their second
/// resource, a view next to the raw data of `rcb` and `mobface`.
fn plan_stream(
    game: &GameInstance,
    ws: &Workspace,
    index: Option<&GameIndex>,
    input: &PlanInput,
) -> Result<StreamPlan> {
    let source = StreamSource::open(game, ws, &input.name)?;
    let stream = source
        .file
        .as_ref()
        .and_then(|_| game.find_stream(&source.name));
    let layout = source.layout();
    // A resource the workspace added is taken as that, also when the game
    // file has it by now (the workspace was applied).
    let is_added = |r: usize| {
        let key = ResourceKey::new(
            &source.name,
            &layout.expect("of a file").resources[r].header,
        );
        source.added.iter().any(|c| c.key == key)
    };
    // A stream the workspace creates has no resources of the game: what a
    // file of it (staged, applied) lists is what the workspace adds.
    let mut resources: Vec<Res> = input
        .indices
        .iter()
        .filter(|&&r| layout.is_some() && !is_added(r))
        .map(|&r| Res::Game(r))
        .collect();
    for key in &input.added {
        if let Some(i) = source.added.iter().position(|c| c.key == *key) {
            resources.push(Res::Added(i));
        }
    }
    let added_with = |type_id: u32, id: u32| {
        source
            .added
            .iter()
            .position(|c| c.key.type_id == type_id && c.key.ids[3] == id)
            .map(Res::Added)
    };
    // The header and the data of the texture `res` is one half of.
    let texture_of = |res: Res| -> Option<(Res, Res)> {
        match res {
            Res::Game(r) => pair_of(layout?, r).map(|p| (Res::Game(p.header), Res::Game(p.data))),
            Res::Added(_) => {
                let id = source.header(res).ids[3];
                Some((added_with(TYPE_TG4H, id)?, added_with(TYPE_TG4D, id)?))
            }
        }
    };
    // The volatile data of the mesh `res`: of the same origin, else the one
    // the workspace added to a mesh of the game that had none.
    let volatile_of = |res: Res| -> Option<Res> {
        let own = match res {
            Res::Game(r) => volatile_for(layout?, r).map(Res::Game),
            Res::Added(_) => None,
        };
        own.or_else(|| added_with(TYPE_MESH_VOLATILE, !source.header(res).ids[3]))
    };
    // The name of the unit made of `list`.
    let name_of = |list: &[Res]| -> Option<String> {
        match list[0] {
            Res::Game(r) => {
                let index = index?;
                let id = index.locate(stream?, r)?;
                index.record(id).name().map(str::to_owned)
            }
            // The index does not know it; some types carry their name.
            res @ Res::Added(_) => {
                let data = source.data(ws, res).ok()?;
                let name = match source.header(res).type_id {
                    TYPE_TG4H => Tg4Header::parse(&data).ok()?.name,
                    TYPE_MESH => {
                        let volatile = match list.get(1) {
                            Some(&v) => source.data(ws, v).ok()?,
                            None => Vec::new(),
                        };
                        Model::parse(&data, &volatile).ok()?.name
                    }
                    TYPE_MATERIAL => crate::resource::material::Material::parse(&data).ok()?.name,
                    _ => return None,
                };
                (!name.is_empty()).then_some(name)
            }
        }
    };
    // A mesh takes its volatile data along, selected or not.
    let mut claimed: HashSet<Res> = HashSet::new();
    for &res in &resources {
        if source.header(res).type_id == TYPE_MESH
            && let Some(v) = volatile_of(res)
        {
            claimed.insert(v);
        }
    }
    let mut units = Vec::new();
    let mut done: HashSet<Res> = HashSet::new();
    for &res in &resources {
        if done.contains(&res) || claimed.contains(&res) {
            continue;
        }
        let type_id = source.header(res).type_id;
        let (kind, list) = match type_id {
            TYPE_TG4H | TYPE_TG4D => match texture_of(res) {
                Some((header, data)) => (Kind::Texture, vec![header, data]),
                None => (Kind::Raw, vec![res]),
            },
            TYPE_MESH => match volatile_of(res) {
                Some(v) => (Kind::Model, vec![res, v]),
                None => (Kind::Model, vec![res]),
            },
            TYPE_LH2 => (Kind::Strings, vec![res]),
            t if logic_import::is_supported(t) => (Kind::Logic, vec![res]),
            TYPE_DLG | TYPE_CHV | TYPE_TRINITY_SEQ_MASTER | TYPE_GRAPH => {
                (Kind::Document, vec![res])
            }
            TYPE_LUA => (Kind::Script, vec![res]),
            TYPE_BNK => (Kind::Animations, vec![res]),
            _ => (Kind::Raw, vec![res]),
        };
        done.extend(list.iter().copied());
        let shas = list
            .iter()
            .map(|&x| source.data(ws, x).map(|d| sha256_hex(&d)))
            .collect::<Result<Vec<_>>>()?;
        let main = source.header(list[0]);
        let name = name_of(&list);
        if matches!(type_id, TYPE_RCB | TYPE_MOBFACE) {
            units.push(Unit {
                kind: Kind::View,
                resources: list.clone(),
                name: name.clone(),
                shas: shas.clone(),
                type_id: main.type_id,
                ids: main.ids,
            });
        }
        units.push(Unit {
            kind,
            resources: list,
            name,
            shas,
            type_id: main.type_id,
            ids: main.ids,
        });
    }
    Ok(StreamPlan {
        stream,
        new: ws.new_stream(&source.name).map(NewStreamSource::of),
        name: source.name.clone(),
        units,
    })
}

/// What makes two units of different streams the same resource with the
/// same data.
fn fingerprint(source: &StreamSource, unit: &Unit) -> String {
    let mut text = format!("{:?}", unit.kind);
    for (k, &r) in unit.resources.iter().enumerate() {
        let h = source.header(r);
        text += &format!(
            "|{:08x}:{}:{:08x}:{}",
            h.type_id,
            ids_text(&h.ids),
            h.mem_policy,
            unit.shas[k]
        );
    }
    text
}

/// The stems (lower case) the files `files` of a folder take: each path
/// without its last extension, and without its last two (`x.view.json`).
fn stems_of(files: &[String]) -> HashSet<String> {
    let mut used = HashSet::new();
    for file in files {
        let lower = file.to_ascii_lowercase();
        let (dir, name) = lower.rsplit_once('/').unwrap_or(("", &lower));
        let mut name = name.to_owned();
        for _ in 0..2 {
            let Some((stem, _)) = name.rsplit_once('.') else {
                break;
            };
            name = stem.to_owned();
            used.insert(if dir.is_empty() {
                name.clone()
            } else {
                format!("{dir}/{name}")
            });
        }
    }
    used
}

/// A stem `<tag>/<name>` not yet in `used` (compared without case).
fn free_stem(used: &mut HashSet<String>, unit: &Unit) -> String {
    let tag = sanitize(&type_tags().display(unit.type_id)).unwrap_or_else(|| "type".into());
    let ids = ids_text(&unit.ids);
    let name = unit.name.as_deref().and_then(sanitize);
    let view = if unit.kind == Kind::View { ".view" } else { "" };
    let place = match unit.resources[0] {
        Res::Game(r) => format!("{r:05}_{ids}"),
        Res::Added(_) => format!("added_{ids}"),
    };
    let candidates = [
        name.clone().unwrap_or_else(|| ids.clone()),
        format!("{}_{ids}", name.unwrap_or_default()),
        place.clone(),
    ];
    for base in candidates {
        let stem = format!("{tag}/{base}{view}");
        if used.insert(stem.to_ascii_lowercase()) {
            return stem;
        }
    }
    // Only files of the folder that are not the project's can take the
    // name the place of the resource makes unique.
    (2..)
        .map(|n| format!("{tag}/{place}_{n}{view}"))
        .find(|stem| used.insert(stem.to_ascii_lowercase()))
        .expect("a number that is free")
}

/// `ids[3]`, or all four ids joined by dots when they differ.
pub(super) fn ids_text(ids: &[u32; 4]) -> String {
    if ids.iter().all(|&i| i == ids[0]) {
        format!("{:08x}", ids[0])
    } else {
        ids.map(|i| format!("{i:08x}")).join(".")
    }
}

/// A file name made of a resource name: letters, digits, `.`, `-` and `_`.
pub(super) fn sanitize(name: &str) -> Option<String> {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .take(96)
        .collect();
    let cleaned = cleaned.trim_matches(['.', '_']).to_owned();
    (!cleaned.is_empty()).then_some(cleaned)
}

/// The files of a unit: `(name, bytes)` with names relative to the folder,
/// the base data to keep, and the data an added texture or model is made
/// over ([`Entry::template`]).
struct Converted {
    kind: Kind,
    files: Vec<(String, Vec<u8>)>,
    base: Option<Vec<u8>>,
    template: Vec<Vec<u8>>,
}

impl Converted {
    fn new(kind: Kind, files: Vec<(String, Vec<u8>)>) -> Self {
        Self {
            kind,
            files,
            base: None,
            template: Vec::new(),
        }
    }

    fn with_base(mut self, base: Vec<u8>) -> Self {
        self.base = Some(base);
        self
    }
}

/// Writes the files of units.
struct Writer<'a> {
    ws: &'a Workspace,
    root: &'a Path,
    model_format: ModelFormat,
    dds: bool,
}

impl Writer<'_> {
    /// Writes the files of `unit` into `folder` (relative to the root) as
    /// `stem.<ext>` and returns its entry. A conversion that fails gives
    /// raw files and a note; a view that fails gives nothing.
    #[allow(clippy::too_many_arguments)]
    fn unit(
        &self,
        source: &StreamSource,
        assets: Option<&mut dyn AssetSource>,
        unit: &Unit,
        folder: &str,
        stem: &str,
        keys: Vec<ResourceKey>,
        notes: &mut Vec<String>,
    ) -> Result<Option<Entry>> {
        let main = unit.resources[0];
        let header = source.header(main);
        let data: Vec<Vec<u8>> = unit
            .resources
            .iter()
            .map(|&r| source.data(self.ws, r))
            .collect::<Result<_>>()?;
        let converted = match self.convert(source, assets, unit, stem, &data) {
            Ok(result) => result,
            Err(error) if unit.kind == Kind::View => {
                // The raw entry next to it has the data.
                notes.push(format!("{}: no view: {error}", source.key(main)));
                return Ok(None);
            }
            Err(error) => {
                notes.push(format!("{}: {error}; kept as raw data", source.key(main)));
                let mut files = Vec::new();
                for (k, &r) in unit.resources.iter().enumerate() {
                    let h = source.header(r);
                    let name = if k == 0 {
                        format!("{stem}.{}", raw_ext(h.type_id))
                    } else {
                        format!(
                            "{stem}.{}.{}",
                            type_tags().display(h.type_id),
                            raw_ext(h.type_id)
                        )
                    };
                    files.push((name, data[k].clone()));
                }
                Converted::new(Kind::Raw, files)
            }
        };
        let base = match &converted.base {
            Some(b) => Some(cache::put_base(self.root, b)?),
            None => None,
        };
        let template = converted
            .template
            .iter()
            .map(|t| cache::put_base(self.root, t))
            .collect::<Result<Vec<_>>>()?;
        let mut names = Vec::new();
        let mut shas = Vec::new();
        for (name, bytes) in &converted.files {
            let path = self.root.join(folder).join(name);
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
            }
            std::fs::write(&path, bytes).map_err(|e| Error::io(&path, e))?;
            names.push(name.clone());
            shas.push(sha256_hex(bytes));
        }
        let pairs: Vec<(&str, &str)> = names
            .iter()
            .zip(&shas)
            .map(|(n, s)| (n.as_str(), s.as_str()))
            .collect();
        // What the workspace holds for the keys: a checkout of the sources
        // without a build cache builds what its workspace lacks.
        let mut outputs: Vec<String> = keys
            .iter()
            .map(|k| {
                self.ws
                    .change(k)
                    .map_or_else(String::new, |c| c.payload.sha256.clone())
            })
            .collect();
        if outputs.iter().all(String::is_empty) {
            outputs.clear();
        }
        // A storage the workspace asked for a resource of the game.
        let mut storage: Vec<crate::workspace::StorageChoice> = keys
            .iter()
            .map(|k| {
                self.ws
                    .change(k)
                    .filter(|c| !c.is_added())
                    .map_or_else(Default::default, |c| c.storage)
            })
            .collect();
        if storage.iter().all(crate::workspace::StorageChoice::is_keep) {
            storage.clear();
        }
        Ok(Some(Entry {
            name: unit.name.clone().unwrap_or_else(|| ids_text(&header.ids)),
            kind: converted.kind,
            type_tag: type_tags().display(header.type_id).to_string(),
            ids: ids_text(&header.ids),
            policy: mem_policies().display(header.mem_policy).to_string(),
            exported: cache::digest(&pairs, None),
            files: names,
            keys,
            base,
            template,
            outputs,
            storage,
            // A view only shows the resource; its raw entry adds it.
            added: if converted.kind == Kind::View {
                Vec::new()
            } else {
                unit.resources
                    .iter()
                    .filter_map(|&r| source.added_source(r))
                    .collect()
            },
        }))
    }

    fn convert(
        &self,
        source: &StreamSource,
        assets: Option<&mut dyn AssetSource>,
        unit: &Unit,
        stem: &str,
        data: &[Vec<u8>],
    ) -> Result<Converted> {
        let main = unit.resources[0];
        let header = source.header(main);
        // A resource the project adds is made over its own data at build:
        // the game has none for it.
        let is_added = matches!(main, Res::Added(_));
        // What the workspace made, of a kind that is encoded anew by every
        // build (a picture, a model): its data is kept, so that files left
        // as they are build exactly what the workspace has.
        let made = is_added
            || unit
                .resources
                .iter()
                .any(|&r| self.ws.change(&source.key(r)).is_some());
        let position = match main {
            Res::Game(r) => Some(r),
            Res::Added(_) => None,
        };
        let pretty = |value: &Value| -> Result<Vec<u8>> {
            let text = serde_json::to_string_pretty(value)
                .map_err(|e| Error::InvalidArgument(e.to_string()))?;
            Ok((text + "\n").into_bytes())
        };
        let value_of = |v: std::result::Result<Value, serde_json::Error>| {
            v.map_err(|e| Error::InvalidArgument(e.to_string()))
        };
        let one = |ext: &str, bytes: Vec<u8>| vec![(format!("{stem}.{ext}"), bytes)];
        Ok(match unit.kind {
            Kind::Texture => {
                let texture = Texture {
                    header: Tg4Header::parse(&data[0])?,
                    data: data[1].clone(),
                };
                let files = if self.dds || texture.header.layer_count() > 1 {
                    one("dds", texture.to_dds()?)
                } else {
                    one("png", texture.to_png(0)?)
                };
                let mut converted = Converted::new(Kind::Texture, files);
                if made {
                    converted.template = data.to_vec();
                }
                converted
            }
            Kind::Strings => {
                let json = crate::navigation::unpack::strings_json(
                    &source.name,
                    position.unwrap_or(0),
                    &header.ids,
                    &data[0],
                )?;
                Converted::new(Kind::Strings, one("json", json)).with_base(data[0].clone())
            }
            Kind::Logic => {
                let json = logic::to_json(header.type_id, &data[0])
                    .ok_or_else(|| Error::InvalidArgument("no JSON for this type".into()))??;
                // Where the resource is, as `logic-export` writes it.
                let mut located = serde_json::Map::new();
                located.insert("stream".into(), source.name.clone().into());
                if let Some(position) = position {
                    located.insert("resource".into(), position.into());
                }
                located.insert(
                    "type".into(),
                    type_tags().display(header.type_id).to_string().into(),
                );
                located.insert("id".into(), format!("{:08x}", header.ids[3]).into());
                if let Value::Object(map) = json {
                    located.extend(map);
                }
                let bytes = pretty(&Value::Object(located))?;
                Converted::new(Kind::Logic, one("json", bytes)).with_base(data[0].clone())
            }
            Kind::Document => {
                let value = match header.type_id {
                    TYPE_DLG => value_of(serde_json::to_value(Dialogue::parse(&data[0])?))?,
                    TYPE_CHV => value_of(serde_json::to_value(ChatterSet::parse(&data[0])?))?,
                    TYPE_GRAPH => value_of(serde_json::to_value(Graph::parse(&data[0])?))?,
                    _ => value_of(serde_json::to_value(SeqMaster::parse(&data[0])?))?,
                };
                // Only a document that writes back the same bytes is editable.
                if document_bytes(header.type_id, &value)? != data[0] {
                    return Err(Error::InvalidArgument(
                        "the document does not write back byte for byte".into(),
                    ));
                }
                Converted::new(Kind::Document, one("json", pretty(&value)?))
            }
            Kind::Script => {
                let chunk = Chunk::parse(&data[0])?;
                let round = lua::round_trip(&chunk);
                // Only a script whose source compiles back byte for byte
                // (line information included) becomes source: a build of
                // the unchanged project gives the game's data.
                let converted = match round.comparison {
                    Comparison::Identical => Converted::new(
                        Kind::Script,
                        one("lua", round.decompiled.source.into_bytes()),
                    ),
                    Comparison::SameCode => {
                        return Err(Error::InvalidArgument(
                            "the decompiled script compiles to the same code \
                             with other debug information"
                                .into(),
                        ));
                    }
                    Comparison::Different(why) => {
                        return Err(Error::InvalidArgument(format!(
                            "the decompiled script does not compile to the same code ({why})"
                        )));
                    }
                };
                // The name of the chunk and the size of its numbers are
                // taken from the compiled script.
                if is_added {
                    converted.with_base(data[0].clone())
                } else {
                    converted
                }
            }
            Kind::Model => {
                let volatile = data.get(1).cloned().unwrap_or_default();
                let model = Model::parse(&data[0], &volatile)?;
                let mut none = NoAssets;
                let game: &mut dyn AssetSource = match assets {
                    Some(assets) => assets,
                    None => &mut none,
                };
                // The materials and textures the workspace adds or changes
                // (those of an imported model set) are shown as it has them.
                let edited = self
                    .ws
                    .changes()
                    .any(|c| matches!(c.key.type_id, TYPE_MATERIAL | TYPE_TG4H | TYPE_TG4D));
                let built = if edited {
                    scene::game::model_scene(&model, &mut EditedAssets::new(self.ws, game))?
                } else {
                    scene::game::model_scene(&model, game)?
                };
                let (dir, file_stem) = stem.rsplit_once('/').unwrap_or(("", stem));
                let written = scene::write_with_textures(
                    &built,
                    self.model_format,
                    file_stem,
                    &format!("{file_stem}_textures/"),
                )?;
                let files = written
                    .into_iter()
                    .map(|f| {
                        let name = if dir.is_empty() {
                            f.name
                        } else {
                            format!("{dir}/{}", f.name)
                        };
                        (name, f.bytes)
                    })
                    .collect();
                let mut converted = Converted::new(Kind::Model, files);
                // A model the workspace made — added, or imported over a
                // mesh of the game — is built again over its own data: its
                // materials are the ones that import made, which have
                // entries of their own.
                if made {
                    converted.template = data.to_vec();
                }
                converted
            }
            Kind::Animations => {
                let bank = AnimBank::parse(&data[0])?;
                let clips: Vec<Value> = bank
                    .clips
                    .iter()
                    .enumerate()
                    .map(|(k, clip)| {
                        json!({
                            "index": k,
                            "clip": clip.display_name(),
                            "events": clip.events,
                        })
                    })
                    .collect();
                let events = pretty(&json!({ "clips": clips }))?;
                Converted::new(
                    Kind::Animations,
                    vec![
                        (format!("{stem}.bin"), data[0].clone()),
                        (format!("{stem}.events.json"), events),
                    ],
                )
            }
            Kind::View => {
                let value = match header.type_id {
                    TYPE_RCB => value_of(serde_json::to_value(Rig::parse(&data[0])?))?,
                    t => logic::to_json(t, &data[0])
                        .ok_or_else(|| Error::InvalidArgument("no view for this type".into()))??,
                };
                Converted::new(Kind::View, one("json", pretty(&value)?))
            }
            Kind::Raw => Converted::new(Kind::Raw, one(raw_ext(header.type_id), data[0].clone())),
        })
    }
}

/// Extension of the raw data of a type.
pub(super) fn raw_ext(type_id: u32) -> &'static str {
    match type_id {
        TYPE_LUA => "luac",
        TYPE_APX => "apx",
        _ => "bin",
    }
}

/// The resource data of a document (`dlg`, `chv`, `trinity_seq_master`,
/// `graph`).
pub(super) fn document_bytes(type_id: u32, value: &Value) -> Result<Vec<u8>> {
    let bad = |e: serde_json::Error| Error::InvalidArgument(format!("not a valid document: {e}"));
    match type_id {
        TYPE_DLG => serde_json::from_value::<Dialogue>(value.clone())
            .map_err(bad)?
            .to_bytes(),
        TYPE_CHV => serde_json::from_value::<ChatterSet>(value.clone())
            .map_err(bad)?
            .to_bytes(),
        TYPE_TRINITY_SEQ_MASTER => serde_json::from_value::<SeqMaster>(value.clone())
            .map_err(bad)?
            .to_bytes(),
        // Lengths, R-tree and paths follow edited positions and links.
        TYPE_GRAPH => {
            let mut graph = serde_json::from_value::<Graph>(value.clone()).map_err(bad)?;
            graph.repair_derived();
            graph.to_bytes()
        }
        _ => Err(Error::InvalidArgument(format!(
            "type {} is not a document",
            type_tags().display(type_id)
        ))),
    }
}

/// A new project as a surface describes it: stream and type names.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct NewProject {
    /// Name of the project and its mod [default: the folder name].
    #[serde(default)]
    pub name: Option<String>,
    /// Mod id [default: made from the name].
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub author: String,
    /// Streams to take, e.g. `gf2/frontend`.
    #[serde(default)]
    pub streams: Vec<String>,
    /// Only resources of these types: tags such as `tg4h`.
    #[serde(default)]
    pub types: Vec<String>,
    /// Take what the workspace changes, as it has it, instead of whole
    /// streams: the resources it replaces and adds, the streams it creates.
    /// Without this and without streams: an empty project with a workspace
    /// of its own.
    #[serde(default)]
    pub from_workspace: bool,
    /// Textures as DDS instead of PNG.
    #[serde(default)]
    pub dds: bool,
    /// Model format: glb (default), fbx, obj, usd, usda, usdc or usdz.
    #[serde(default)]
    pub model_format: Option<String>,
    /// Keep resources equal in several streams in each stream's folder.
    #[serde(default)]
    pub no_shared: bool,
}

/// Makes a project in `root` (a new or empty directory) of what `new`
/// names; `workspace_dir` is written into the project when it is not the
/// game's default workspace.
///
/// Streams come with what the workspace adds to them; the changes of a
/// workspace come whole, with the resources it adds and the streams it
/// creates. Neither streams nor the workspace: an empty project with a
/// workspace of its own ([`super::create_document`]), the document of the
/// editor.
pub fn create_from(
    game: &GameInstance,
    ws: &Workspace,
    index: Option<&GameIndex>,
    root: &Path,
    new: &NewProject,
    workspace_dir: Option<std::path::PathBuf>,
) -> Result<(Project, CreateReport)> {
    if new.from_workspace && !new.streams.is_empty() {
        return Err(Error::InvalidArgument(
            "give the streams to take, or take the changes of the workspace (not both)".into(),
        ));
    }
    let name = new.name.clone().unwrap_or_else(|| {
        root.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Project".into())
    });
    if !new.from_workspace && new.streams.is_empty() {
        if !new.types.is_empty() {
            return Err(Error::InvalidArgument(
                "types choose among the resources of streams or of the workspace; an empty \
                 project takes none"
                    .into(),
            ));
        }
        let document = super::NewDocument {
            name,
            id: new.id.clone(),
            author: new.author.clone(),
        };
        let project = super::create_document(game, root, &document)?;
        return Ok((project, CreateReport::default()));
    }
    let workspace_dir = workspace_dir.map(|dir| stored_workspace(root, &dir));
    let type_ids: Vec<u32> = new
        .types
        .iter()
        .map(|t| {
            type_tags()
                .resolve(t)
                .ok_or_else(|| Error::InvalidArgument(format!("unknown type `{t}`")))
        })
        .collect::<Result<_>>()?;
    let mut selection = if new.from_workspace {
        workspace_selection(game, ws)?
    } else {
        crate::navigation::unpack::select(game, &new.streams, &type_ids)?
    };
    if new.from_workspace && !type_ids.is_empty() {
        for (s, list) in &mut selection {
            let file = game.open_stream(*s)?;
            list.retain(|&r| type_ids.contains(&file.layout().resources[r].header.type_id));
        }
        selection.retain(|(_, l)| !l.is_empty());
    }
    let mut additions = workspace_additions(ws);
    if !new.from_workspace {
        // A stream comes as the workspace has it: with what it adds there.
        let chosen: HashSet<String> = selection
            .iter()
            .map(|(s, _)| game.streams()[*s].stream_name())
            .collect();
        additions.resources.retain(|k| chosen.contains(&k.stream));
        additions.streams.clear();
    }
    if !type_ids.is_empty() {
        additions
            .resources
            .retain(|k| type_ids.contains(&k.type_id));
    }
    if selection.is_empty() && additions.is_empty() {
        return Err(Error::InvalidArgument(
            "nothing to put into the project".into(),
        ));
    }
    create_with(
        game,
        ws,
        index,
        root,
        &CreateOptions {
            name,
            id: new.id.clone(),
            author: new.author.clone(),
            selection,
            settings: BuildSettings {
                textures: if new.dds { "dds" } else { "png" }.into(),
                models: new.model_format.clone().unwrap_or_else(|| "glb".into()),
                shared: !new.no_shared,
                workspace: workspace_dir,
                deltas: true,
            },
        },
        &additions,
        &|_, _| {},
        &AtomicBool::new(false),
    )
}

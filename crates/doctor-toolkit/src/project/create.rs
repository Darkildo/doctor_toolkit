//! Making a project: resources of the game (or the changes of a workspace)
//! written as source files with their manifests.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use rayon::prelude::*;
use serde_json::{Value, json};

use crate::container::ustream::ResourceHeader;
use crate::export::gltf::{AssetSource, NoAssets};
use crate::game::{GameInstance, StreamFile};
use crate::navigation::GameIndex;
use crate::navigation::unpack::{AssetMaps, IndexAssets, Selection};
use crate::resource::anim::{AnimBank, TYPE_BNK};
use crate::resource::chv::{ChatterSet, TYPE_CHV};
use crate::resource::dlg::{Dialogue, TYPE_DLG};
use crate::resource::logic::TYPE_GRAPH;
use crate::resource::lua::{self, Chunk, Comparison, TYPE_LUA};
use crate::resource::mesh::{Model, TYPE_MESH, volatile_for};
use crate::resource::mobface::TYPE_MOBFACE;
use crate::resource::names::{mem_policies, type_tags};
use crate::resource::rcb::{Rig, TYPE_RCB};
use crate::resource::texture::{TYPE_TG4D, TYPE_TG4H, Texture, pair_of};
use crate::resource::tg4::Tg4Header;
use crate::resource::trinity::{SeqMaster, TYPE_TRINITY_SEQ_MASTER};
use crate::resource::{logic, logic_import};
use crate::scene::{self, ModelFormat};
use crate::workspace::store::sha256_hex;
use crate::workspace::{ResourceKey, Workspace};
use crate::{Error, Result};

use super::cache::{self, BuildCache, Built};
use super::config::{
    BuildSettings, Dependencies, FORMAT, GameInfo, PROJECT_FILE, ProjectConfig, ProjectInfo,
};
use super::manifest::{Entry, Kind, SHARED_MANIFEST, STREAM_MANIFEST, SourceManifest};
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
}

/// The resources a workspace replaces, as a selection (added resources and
/// new streams are not in the game and are left out).
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

/// The data of resources of one stream as the workspace has it.
pub(super) struct StreamSource {
    pub name: String,
    pub file: StreamFile,
    /// The original file when the game file is a written one (backup).
    original: Option<StreamFile>,
}

impl StreamSource {
    pub fn open(game: &GameInstance, stream: usize) -> Result<Self> {
        let info = &game.streams()[stream];
        let file = game.open_stream(stream)?;
        let original_path = game.original_path(info);
        let original = if original_path != info.path {
            Some(StreamFile::open(&original_path)?)
        } else {
            None
        };
        Ok(Self {
            name: info.stream_name(),
            file,
            original,
        })
    }

    pub fn header(&self, r: usize) -> ResourceHeader {
        self.file.layout().resources[r].header
    }

    pub fn key(&self, r: usize) -> ResourceKey {
        ResourceKey::new(&self.name, &self.header(r))
    }

    /// Data of resource `r`: the workspace's change, or the original data.
    pub fn data(&self, ws: &Workspace, r: usize) -> Result<Vec<u8>> {
        let key = self.key(r);
        if let Some(data) = ws.data(&key)? {
            return Ok(data);
        }
        match &self.original {
            Some(original) => {
                let index = key.find_in(original.layout()).ok_or_else(|| {
                    Error::InvalidArgument(format!("{key} is not in the original stream"))
                })?;
                original.resource_data(index)
            }
            None => self.file.resource_data(r),
        }
    }
}

/// One source entry to write: the resources it stands for in one stream.
#[derive(Debug, Clone)]
struct Unit {
    kind: Kind,
    resources: Vec<usize>,
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
    stream: usize,
    name: String,
    units: Vec<Unit>,
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
    let model_format = model_format_of(&options.settings)?;
    std::fs::create_dir_all(root).map_err(|e| Error::io(root, e))?;

    // 1. What each stream holds, with data hashes to find shared resources.
    let total: usize = options.selection.iter().map(|(_, l)| l.len()).sum();
    let plans: Vec<StreamPlan> = options
        .selection
        .par_iter()
        .map(|(s, resources)| plan_stream(game, ws, index, *s, resources))
        .collect::<Result<_>>()?;

    // 2. Shared resources and file names.
    let mut places: Vec<Vec<Option<Place>>> =
        plans.iter().map(|p| vec![None; p.units.len()]).collect();
    let mut shared_count = 0;
    if options.settings.shared && plans.len() > 1 {
        let mut groups: BTreeMap<String, Vec<(usize, usize)>> = BTreeMap::new();
        for (p, plan) in plans.iter().enumerate() {
            let source = StreamSource::open(game, plan.stream)?;
            for (u, unit) in plan.units.iter().enumerate() {
                groups
                    .entry(fingerprint(&source, unit))
                    .or_default()
                    .push((p, u));
            }
        }
        let mut shared_stems = HashSet::new();
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
        let mut stems = HashSet::new();
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
        dds: options.settings.textures.eq_ignore_ascii_case("dds"),
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
            let source = StreamSource::open(game, plan.stream)?;
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
                    let file = game.open_stream(other.stream)?;
                    for &r in &other.units[cu].resources {
                        keys.push(ResourceKey::new(
                            &other.name,
                            &file.layout().resources[r].header,
                        ));
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

    // 4. Manifests, project file and the state of the workspace.
    let mut report = CreateReport {
        shared: shared_count,
        ..CreateReport::default()
    };
    let mut manifests: BTreeMap<String, SourceManifest> = BTreeMap::new();
    for plan in &plans {
        manifests
            .entry(stream_dir(&plan.name))
            .or_insert_with(|| SourceManifest::new(Some(plan.name.clone())));
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
    report.notes.sort();
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
    }
    let edition = crate::edition::Edition::identify(game.root());
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
        game: GameInfo {
            path: Some(
                std::fs::canonicalize(game.root()).unwrap_or_else(|_| game.root().to_owned()),
            ),
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
        },
        dependencies: Dependencies::default(),
        build: options.settings.clone(),
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

/// Writes the files of `entry` (of folder `dir`) again from the workspace's
/// data of its first stream, under the same names, and returns the entry
/// with its new digest and base. Models are not written again.
pub(super) fn export_entry(
    game: &GameInstance,
    ws: &Workspace,
    root: &Path,
    dir: &str,
    entry: &Entry,
    config: &ProjectConfig,
) -> Result<Entry> {
    if entry.kind == Kind::Model {
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
    let s = game
        .find_stream(&stream)
        .ok_or_else(|| Error::InvalidArgument(format!("stream {stream} is not in the game")))?;
    let source = StreamSource::open(game, s)?;
    let resources: Vec<usize> = keys
        .iter()
        .map(|k| {
            k.find_in(source.file.layout())
                .ok_or_else(|| Error::InvalidArgument(format!("{k} is not in the game")))
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
            None,
            &unit,
            dir,
            &stem,
            entry.keys.clone(),
            &mut notes,
        )?
        .ok_or_else(|| Error::InvalidArgument(notes.join("; ")))?;
    if fresh.files != entry.files {
        return Err(Error::InvalidArgument(format!(
            "{dir}/{file}: the workspace's data gives other files ({}); {}",
            fresh.files.join(", "),
            notes.join("; ")
        )));
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
/// resource, a view next to the raw data of `graph`, `rcb` and `mobface`.
fn plan_stream(
    game: &GameInstance,
    ws: &Workspace,
    index: Option<&GameIndex>,
    stream: usize,
    resources: &[usize],
) -> Result<StreamPlan> {
    let source = StreamSource::open(game, stream)?;
    let layout = source.file.layout();
    let name_of = |r: usize| -> Option<String> {
        let index = index?;
        let id = index.locate(stream, r)?;
        index.record(id).name().map(str::to_owned)
    };
    // A mesh takes its volatile data along, selected or not.
    let mut claimed: HashSet<usize> = HashSet::new();
    for &r in resources {
        if source.header(r).type_id == TYPE_MESH
            && let Some(v) = volatile_for(layout, r)
        {
            claimed.insert(v);
        }
    }
    let mut units = Vec::new();
    let mut done: HashSet<usize> = HashSet::new();
    for &r in resources {
        if done.contains(&r) || claimed.contains(&r) {
            continue;
        }
        let type_id = source.header(r).type_id;
        let (kind, list) = match type_id {
            TYPE_TG4H | TYPE_TG4D => match pair_of(layout, r) {
                Some(pair) => (Kind::Texture, vec![pair.header, pair.data]),
                None => (Kind::Raw, vec![r]),
            },
            TYPE_MESH => match volatile_for(layout, r) {
                Some(v) => (Kind::Model, vec![r, v]),
                None => (Kind::Model, vec![r]),
            },
            TYPE_LH2 => (Kind::Strings, vec![r]),
            t if logic_import::is_supported(t) => (Kind::Logic, vec![r]),
            TYPE_DLG | TYPE_CHV | TYPE_TRINITY_SEQ_MASTER => (Kind::Document, vec![r]),
            TYPE_LUA => (Kind::Script, vec![r]),
            TYPE_BNK => (Kind::Animations, vec![r]),
            _ => (Kind::Raw, vec![r]),
        };
        done.extend(list.iter().copied());
        let shas = list
            .iter()
            .map(|&x| source.data(ws, x).map(|d| sha256_hex(&d)))
            .collect::<Result<Vec<_>>>()?;
        let main = source.header(list[0]);
        let name = name_of(list[0]);
        if matches!(type_id, TYPE_GRAPH | TYPE_RCB | TYPE_MOBFACE) {
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

/// A stem `<tag>/<name>` not yet in `used` (compared without case).
fn free_stem(used: &mut HashSet<String>, unit: &Unit) -> String {
    let r = unit.resources[0];
    let tag = sanitize(&type_tags().display(unit.type_id)).unwrap_or_else(|| "type".into());
    let ids = ids_text(&unit.ids);
    let name = unit.name.as_deref().and_then(sanitize);
    let view = if unit.kind == Kind::View { ".view" } else { "" };
    let candidates = [
        name.clone().unwrap_or_else(|| ids.clone()),
        format!("{}_{ids}", name.unwrap_or_default()),
        format!("{r:05}_{ids}"),
    ];
    for base in candidates {
        let stem = format!("{tag}/{base}{view}");
        if used.insert(stem.to_ascii_lowercase()) {
            return stem;
        }
    }
    unreachable!("the resource index makes the last candidate unique")
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

/// Files of a unit: kind, `(name, bytes)` with names relative to the
/// folder, and the base data to keep.
type Converted = (Kind, Vec<(String, Vec<u8>)>, Option<Vec<u8>>);

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
        let (kind, files, base) = match self.convert(source, assets, unit, stem, &data) {
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
                (Kind::Raw, files, None)
            }
        };
        let base = match base {
            Some(b) => Some(cache::put_base(self.root, &b)?),
            None => None,
        };
        let mut names = Vec::new();
        let mut shas = Vec::new();
        for (name, bytes) in &files {
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
        Ok(Some(Entry {
            name: unit.name.clone().unwrap_or_else(|| ids_text(&header.ids)),
            kind,
            type_tag: type_tags().display(header.type_id).to_string(),
            ids: ids_text(&header.ids),
            policy: mem_policies().display(header.mem_policy).to_string(),
            exported: cache::digest(&pairs, None),
            files: names,
            keys,
            base,
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
                if self.dds || texture.header.layer_count() > 1 {
                    (Kind::Texture, one("dds", texture.to_dds()?), None)
                } else {
                    (Kind::Texture, one("png", texture.to_png(0)?), None)
                }
            }
            Kind::Strings => {
                let json = crate::navigation::unpack::strings_json(
                    &source.name,
                    main,
                    &header.ids,
                    &data[0],
                )?;
                (Kind::Strings, one("json", json), Some(data[0].clone()))
            }
            Kind::Logic => {
                let json = logic::to_json(header.type_id, &data[0])
                    .ok_or_else(|| Error::InvalidArgument("no JSON for this type".into()))??;
                // Where the resource is, as `logic-export` writes it.
                let mut located = serde_json::Map::new();
                located.insert("stream".into(), source.name.clone().into());
                located.insert("resource".into(), main.into());
                located.insert(
                    "type".into(),
                    type_tags().display(header.type_id).to_string().into(),
                );
                located.insert("id".into(), format!("{:08x}", header.ids[3]).into());
                if let Value::Object(map) = json {
                    located.extend(map);
                }
                let bytes = pretty(&Value::Object(located))?;
                (Kind::Logic, one("json", bytes), Some(data[0].clone()))
            }
            Kind::Document => {
                let value = match header.type_id {
                    TYPE_DLG => value_of(serde_json::to_value(Dialogue::parse(&data[0])?))?,
                    TYPE_CHV => value_of(serde_json::to_value(ChatterSet::parse(&data[0])?))?,
                    _ => value_of(serde_json::to_value(SeqMaster::parse(&data[0])?))?,
                };
                // Only a document that writes back the same bytes is editable.
                if document_bytes(header.type_id, &value)? != data[0] {
                    return Err(Error::InvalidArgument(
                        "the document does not write back byte for byte".into(),
                    ));
                }
                (Kind::Document, one("json", pretty(&value)?), None)
            }
            Kind::Script => {
                let chunk = Chunk::parse(&data[0])?;
                let round = lua::round_trip(&chunk);
                match round.comparison {
                    Comparison::Identical | Comparison::SameCode => (
                        Kind::Script,
                        one("lua", round.decompiled.source.into_bytes()),
                        None,
                    ),
                    Comparison::Different(why) => {
                        return Err(Error::InvalidArgument(format!(
                            "the decompiled script does not compile to the same code ({why})"
                        )));
                    }
                }
            }
            Kind::Model => {
                let volatile = data.get(1).cloned().unwrap_or_default();
                let model = Model::parse(&data[0], &volatile)?;
                let built = match assets {
                    Some(assets) => scene::game::model_scene(&model, assets)?,
                    None => scene::game::model_scene(&model, &mut NoAssets)?,
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
                (Kind::Model, files, None)
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
                (
                    Kind::Animations,
                    vec![
                        (format!("{stem}.bin"), data[0].clone()),
                        (format!("{stem}.events.json"), events),
                    ],
                    None,
                )
            }
            Kind::View => {
                let value = match header.type_id {
                    TYPE_RCB => value_of(serde_json::to_value(Rig::parse(&data[0])?))?,
                    t => logic::to_json(t, &data[0])
                        .ok_or_else(|| Error::InvalidArgument("no view for this type".into()))??,
                };
                (Kind::View, one("json", pretty(&value)?), None)
            }
            Kind::Raw => (
                Kind::Raw,
                one(raw_ext(header.type_id), data[0].clone()),
                None,
            ),
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

/// The resource data of a document (`dlg`, `chv`, `trinity_seq_master`).
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
        _ => Err(Error::InvalidArgument(format!(
            "type {} is not a document",
            type_tags().display(type_id)
        ))),
    }
}

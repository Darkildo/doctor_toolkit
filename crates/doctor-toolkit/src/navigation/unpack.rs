//! `unpack` and `pack`: resources as a tree of files in common formats.
//!
//! [`unpack`] writes the chosen resources to `<dir>/<stream>/<type>/<name>.<ext>`
//! and a `manifest.json` that maps every file to its resource keys with the
//! SHA-256 of the file and of the resource data. [`pack`] finds the files
//! whose contents changed since and records them **in the workspace**, never
//! in the game files, with the importers of the library:
//!
//! | Resource | Files | Editable |
//! |---|---|---|
//! | texture (`tg4h` + `tg4d`) | PNG (first surface), DDS for cube and volume textures or with `dds` | yes |
//! | string table (`lh2`) | JSON as written by `text-export` | yes |
//! | model (`mesh`) | glTF (`.glb`, default), FBX, OBJ or USD with side files in `<name>_textures/` | yes: imported again over the mesh |
//! | game logic, rigs | JSON | no, `.bin` |
//! | sound bank (`sbk`) | `.snu` or WAV per sound | no, `.bin` |
//! | script (`lua`) | `.luac` | yes (raw) |
//! | everything else | `.bin`, the decompressed data | yes (raw) |
//!
//! Files that are only views (glTF, JSON of game logic, sounds) are marked
//! as such in the manifest; changing one is an error at `pack`, as is a file
//! that is not in the manifest or a missing file. Nothing is recorded when
//! any file is in error.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::export::gltf::AssetSource;
use crate::game::{GameInstance, StreamFile};
use crate::resource::lh2::StringTable;
use crate::resource::logic;
use crate::resource::material::Material;
use crate::resource::mesh::{self, TYPE_MESH};
use crate::resource::names::type_tags;
use crate::resource::rcb::{self, Skeleton};
use crate::resource::texture::{self, TYPE_TG4D, TYPE_TG4H, Texture, TexturePair};
use crate::resource::{audio, lua};
use crate::scene::{self, ModelFormat};
use crate::workspace::store::sha256_hex;
use crate::workspace::{ResourceEdit, ResourceKey, StorageChoice, Workspace};
use crate::{Error, Result};

use super::index::{Anchor, GameIndex, RecordId, Target};

/// Name of the manifest file in an unpacked directory.
pub const MANIFEST: &str = "manifest.json";
/// Value of the manifest's `format` field.
pub const FORMAT: &str = "doctor-toolkit-unpack";
/// Current version of the manifest format.
pub const VERSION: u32 = 1;

const TYPE_LH2: u32 = crate::hash::sdbm(b"lh2");
const TYPE_APX: u32 = crate::hash::sdbm(b"apx");

/// What a file of an unpacked directory holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FileKind {
    /// A texture as PNG; `keys` are its `tg4h` and `tg4d`.
    TexturePng,
    /// A texture as DDS; `keys` are its `tg4h` and `tg4d`.
    TextureDds,
    /// A string table as JSON.
    Strings,
    /// The decompressed data of one resource.
    Raw,
    /// A conversion for reading only (JSON, sounds); not imported.
    View,
    /// A model (glTF, FBX, OBJ, USD) of a `mesh`; imported over it again.
    Model,
    /// A file next to a model (material library, texture); a change
    /// imports the model of the same resource again.
    ModelFile,
}

impl FileKind {
    pub fn is_editable(self) -> bool {
        self != Self::View
    }
}

/// One file of an unpacked directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// Path relative to the directory, with forward slashes.
    pub path: String,
    pub kind: FileKind,
    /// Resources the file stands for.
    pub keys: Vec<ResourceKey>,
    /// SHA-256 of the file as written.
    pub sha256: String,
    /// SHA-256 of the data of each resource of `keys` when it was unpacked.
    pub sources: Vec<String>,
}

/// `manifest.json` of an unpacked directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: String,
    pub version: u32,
    /// Profile of the game the files come from.
    pub game: String,
    pub files: Vec<FileEntry>,
}

impl Manifest {
    /// Reads `<dir>/manifest.json`.
    pub fn read(dir: &Path) -> Result<Self> {
        let path = dir.join(MANIFEST);
        let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
        let manifest: Self = serde_json::from_str(&text)
            .map_err(|e| Error::InvalidArgument(format!("{}: {e}", path.display())))?;
        if manifest.format != FORMAT {
            return Err(Error::InvalidArgument(format!(
                "{} is not an unpack manifest",
                path.display()
            )));
        }
        if manifest.version > VERSION {
            return Err(Error::InvalidArgument(format!(
                "{} has version {}; this tool reads up to {VERSION}",
                path.display(),
                manifest.version
            )));
        }
        Ok(manifest)
    }

    fn write(&self, dir: &Path) -> Result<()> {
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| Error::InvalidArgument(e.to_string()))?;
        crate::atomic::write(&dir.join(MANIFEST), (text + "\n").as_bytes())
    }
}

/// Options of [`unpack`].
#[derive(Debug, Clone, Copy)]
pub struct UnpackOptions {
    /// Textures as DDS instead of PNG.
    pub dds: bool,
    /// Sounds of banks as WAV instead of `.snu`.
    pub wav: bool,
    /// Also write the raw data of resources that get a view (models, game
    /// logic, rigs, sound banks), so that they can be edited and packed.
    pub raw: bool,
    /// Write views at all (glTF, JSON, sounds).
    pub views: bool,
    /// Embed materials, textures and skeletons in models (needs the index).
    pub model_assets: bool,
    /// File format of models.
    pub model_format: ModelFormat,
}

impl Default for UnpackOptions {
    fn default() -> Self {
        Self {
            dds: false,
            wav: false,
            raw: true,
            views: true,
            model_assets: true,
            model_format: ModelFormat::Glb,
        }
    }
}

/// What [`unpack`] wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnpackReport {
    pub files: usize,
    pub resources: usize,
    pub bytes: u64,
    /// Resources that could not be converted, with the reason; their raw data
    /// is written instead when possible.
    pub failed: Vec<String>,
    pub cancelled: bool,
}

/// Resources to unpack: per stream (index into [`GameInstance::streams`]),
/// resource indices in stream order.
pub type Selection = Vec<(usize, Vec<usize>)>;

/// Every resource of the named streams (all streams when empty) whose type is
/// in `types` (all types when empty).
pub fn select(game: &GameInstance, streams: &[String], types: &[u32]) -> Result<Selection> {
    let chosen: Vec<usize> = if streams.is_empty() {
        (0..game.streams().len()).collect()
    } else {
        streams
            .iter()
            .map(|name| {
                game.find_stream(name)
                    .ok_or_else(|| Error::InvalidArgument(format!("stream `{name}` not found")))
            })
            .collect::<Result<_>>()?
    };
    let mut selection = Vec::new();
    for s in chosen {
        let stream = game.open_stream(s)?;
        let resources: Vec<usize> = (0..stream.layout().resources.len())
            .filter(|&i| {
                types.is_empty() || types.contains(&stream.layout().resources[i].header.type_id)
            })
            .collect();
        if !resources.is_empty() {
            selection.push((s, resources));
        }
    }
    Ok(selection)
}

/// The selection of a list of index records.
pub fn select_records(index: &GameIndex, records: &[RecordId]) -> Selection {
    let mut by_stream: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for &id in records {
        by_stream
            .entry(index.stream_of(id))
            .or_default()
            .push(index.record(id).resource as usize);
    }
    by_stream
        .into_iter()
        .map(|(s, mut list)| {
            list.sort_unstable();
            list.dedup();
            (s, list)
        })
        .collect()
}

/// Writes the selected resources into `out` with a manifest. An existing
/// manifest in `out` is kept and updated, so several exports can share a
/// directory. `index` gives file names and the materials, textures and
/// skeletons of models; without it files are named by id and models carry
/// geometry only.
///
/// `progress(done, total)` counts resources; `cancel` stops between streams.
pub fn unpack(
    game: &GameInstance,
    index: Option<&GameIndex>,
    selection: &Selection,
    out: &Path,
    options: UnpackOptions,
    progress: &(dyn Fn(usize, usize) + Sync),
    cancel: &AtomicBool,
) -> Result<UnpackReport> {
    std::fs::create_dir_all(out).map_err(|e| Error::io(out, e))?;
    let total: usize = selection.iter().map(|(_, list)| list.len()).sum();
    let done = AtomicUsize::new(0);
    let has_models = index.is_some_and(|index| {
        selection.iter().any(|(s, list)| {
            list.iter().any(|&r| {
                index
                    .locate(*s, r)
                    .is_some_and(|id| index.record(id).header.type_id == TYPE_MESH)
            })
        })
    });
    let maps = index
        .filter(|_| options.model_assets && options.views && has_models)
        .map(AssetMaps::new);
    let results: Vec<Result<StreamOutput>> = selection
        .par_iter()
        .map(|(s, resources)| {
            if cancel.load(Ordering::Relaxed) {
                return Ok(StreamOutput {
                    cancelled: true,
                    ..StreamOutput::default()
                });
            }
            let mut writer = StreamWriter::new(game, index, maps.as_ref(), *s, out, options)?;
            for &r in resources {
                writer.resource(r);
                progress(done.fetch_add(1, Ordering::Relaxed) + 1, total);
            }
            Ok(writer.output)
        })
        .collect();

    let mut manifest = match Manifest::read(out) {
        Ok(m) => m,
        Err(_) => Manifest {
            format: FORMAT.into(),
            version: VERSION,
            game: game.profile().name.to_owned(),
            files: Vec::new(),
        },
    };
    let mut report = UnpackReport::default();
    let mut entries: BTreeMap<String, FileEntry> = manifest
        .files
        .drain(..)
        .map(|e| (e.path.clone(), e))
        .collect();
    for result in results {
        let output = result?;
        report.resources += output.resources;
        report.bytes += output.bytes;
        report.failed.extend(output.failed);
        report.cancelled |= output.cancelled;
        for entry in output.entries {
            report.files += 1;
            entries.insert(entry.path.clone(), entry);
        }
    }
    manifest.files = entries.into_values().collect();
    manifest.write(out)?;
    Ok(report)
}

#[derive(Default)]
struct StreamOutput {
    entries: Vec<FileEntry>,
    resources: usize,
    bytes: u64,
    failed: Vec<String>,
    cancelled: bool,
}

/// Writes the files of one stream.
struct StreamWriter<'a> {
    stream: StreamFile,
    stream_index: usize,
    name: String,
    index: Option<&'a GameIndex>,
    assets: Option<IndexAssets<'a>>,
    out: &'a Path,
    options: UnpackOptions,
    used: HashSet<String>,
    done: HashSet<usize>,
    output: StreamOutput,
}

impl<'a> StreamWriter<'a> {
    fn new(
        game: &'a GameInstance,
        index: Option<&'a GameIndex>,
        maps: Option<&'a AssetMaps>,
        stream_index: usize,
        out: &'a Path,
        options: UnpackOptions,
    ) -> Result<Self> {
        let stream = game.open_stream(stream_index)?;
        let assets = index
            .zip(maps)
            .map(|(index, maps)| IndexAssets::new(game, index, maps));
        Ok(Self {
            stream,
            stream_index,
            name: game.streams()[stream_index].stream_name(),
            index,
            assets,
            out,
            options,
            used: HashSet::new(),
            done: HashSet::new(),
            output: StreamOutput::default(),
        })
    }

    fn key(&self, resource: usize) -> ResourceKey {
        ResourceKey::new(&self.name, &self.stream.layout().resources[resource].header)
    }

    /// A free path `<stream>/<tag>/<name>.<ext>` for resource `resource`.
    fn path_for(&mut self, resource: usize, ext: &str) -> String {
        let header = self.stream.layout().resources[resource].header;
        let tag = sanitize(&type_tags().display(header.type_id)).unwrap_or_else(|| "type".into());
        let name = self
            .index
            .and_then(|index| index.locate(self.stream_index, resource))
            .and_then(|id| self.index.and_then(|index| index.record(id).name()))
            .and_then(sanitize);
        let id = ids_text(&header.ids);
        let candidates = [
            name.clone().unwrap_or_else(|| id.clone()),
            format!("{}_{id}", name.unwrap_or_default()),
            format!("{resource:05}_{id}"),
        ];
        for base in candidates {
            let path = format!("{}/{tag}/{base}.{ext}", self.name);
            if self.used.insert(path.to_ascii_lowercase()) {
                return path;
            }
        }
        unreachable!("the resource index makes the last candidate unique")
    }

    fn write(
        &mut self,
        path: String,
        kind: FileKind,
        resources: &[usize],
        sources: &[&[u8]],
        bytes: &[u8],
    ) -> Result<()> {
        let full = self.out.join(&path);
        if let Some(dir) = full.parent() {
            std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
        }
        std::fs::write(&full, bytes).map_err(|e| Error::io(&full, e))?;
        self.output.bytes += bytes.len() as u64;
        self.output.entries.push(FileEntry {
            path,
            kind,
            keys: resources.iter().map(|&r| self.key(r)).collect(),
            sha256: sha256_hex(bytes),
            sources: sources.iter().map(|d| sha256_hex(d)).collect(),
        });
        Ok(())
    }

    fn resource(&mut self, r: usize) {
        if self.done.contains(&r) {
            return;
        }
        self.output.resources += 1;
        if let Err(error) = self.try_resource(r) {
            let message = format!("{}: {error}", self.key(r));
            self.output.failed.push(message);
        }
    }

    fn try_resource(&mut self, r: usize) -> Result<()> {
        let header = self.stream.layout().resources[r].header;
        let data = self.stream.resource_data(r)?;
        let ext = match header.type_id {
            lua::TYPE_LUA => "luac",
            TYPE_APX => "apx",
            _ => "bin",
        };
        match header.type_id {
            TYPE_TG4H | TYPE_TG4D => {
                if let Some(pair) = texture::pair_of(self.stream.layout(), r) {
                    match self.texture(pair) {
                        Ok(()) => return Ok(()),
                        Err(error) => self
                            .output
                            .failed
                            .push(format!("{}: {error}; written raw", self.key(pair.header))),
                    }
                }
            }
            TYPE_LH2 => match strings_json(&self.name, r, &header.ids, &data) {
                Ok(json) => {
                    let path = self.path_for(r, "json");
                    return self.write(path, FileKind::Strings, &[r], &[&data], &json);
                }
                Err(error) => self
                    .output
                    .failed
                    .push(format!("{}: {error}; written raw", self.key(r))),
            },
            _ => {
                if self.options.views {
                    match self.views(r, header.type_id, &data) {
                        Ok(true) if !self.options.raw => return Ok(()),
                        Ok(_) => {}
                        Err(error) => self.output.failed.push(format!("{}: {error}", self.key(r))),
                    }
                }
            }
        }
        let path = self.path_for(r, ext);
        self.write(path, FileKind::Raw, &[r], &[&data], &data)
    }

    fn texture(&mut self, pair: TexturePair) -> Result<()> {
        let layout = self.stream.layout();
        let tex = Texture::load(self.stream.data(), layout, pair)?;
        let header_data = self.stream.resource_data(pair.header)?;
        let use_png = !self.options.dds && tex.header.layer_count() == 1;
        let (kind, ext, bytes) = match use_png.then(|| tex.to_png(0)) {
            Some(Ok(png)) => (FileKind::TexturePng, "png", png),
            _ => (FileKind::TextureDds, "dds", tex.to_dds()?),
        };
        let path = self.path_for(pair.header, ext);
        self.write(
            path,
            kind,
            &[pair.header, pair.data],
            &[&header_data, &tex.data],
            &bytes,
        )?;
        self.done.insert(pair.header);
        // The other resource of the pair counts as unpacked too.
        self.output.resources += 1;
        self.done.insert(pair.data);
        Ok(())
    }

    /// Writes the views of a resource; returns whether it has any.
    fn views(&mut self, r: usize, type_id: u32, data: &[u8]) -> Result<bool> {
        if let Some(json) = logic::to_json(type_id, data) {
            let text = serde_json::to_string_pretty(&json?)
                .map_err(|e| Error::InvalidArgument(e.to_string()))?;
            let path = self.path_for(r, "json");
            self.write(
                path,
                FileKind::View,
                &[r],
                &[data],
                (text + "\n").as_bytes(),
            )?;
            return Ok(true);
        }
        match type_id {
            TYPE_MESH => {
                let model = mesh::load(&self.stream, r)?;
                let built = match &mut self.assets {
                    Some(assets) => scene::game::model_scene(&model, assets)?,
                    None => scene::game::model_scene(&model, &mut crate::export::gltf::NoAssets)?,
                };
                let format = self.options.model_format;
                let path = self.path_for(r, format.extension());
                let (dir, file) = path.rsplit_once('/').unwrap_or(("", path.as_str()));
                let stem = file
                    .strip_suffix(&format!(".{}", format.extension()))
                    .unwrap_or(file)
                    .to_owned();
                let dir = dir.to_owned();
                let files = scene::write_with_textures(
                    &built,
                    format,
                    &stem,
                    &format!("{stem}_textures/"),
                )?;
                for (k, f) in files.into_iter().enumerate() {
                    let (kind, rel) = if k == 0 {
                        (FileKind::Model, path.clone())
                    } else {
                        (FileKind::ModelFile, format!("{dir}/{}", f.name))
                    };
                    self.used.insert(rel.to_ascii_lowercase());
                    self.write(rel, kind, &[r], &[data], &f.bytes)?;
                }
                Ok(true)
            }
            rcb::TYPE_RCB => {
                let rig = rcb::Rig::parse(data)?;
                let text = serde_json::to_string_pretty(&rig)
                    .map_err(|e| Error::InvalidArgument(e.to_string()))?;
                let path = self.path_for(r, "json");
                self.write(
                    path,
                    FileKind::View,
                    &[r],
                    &[data],
                    (text + "\n").as_bytes(),
                )?;
                Ok(true)
            }
            audio::TYPE_SBK => {
                let sounds = audio::parse_sbk(data)?;
                let dir = self.path_for(r, "sounds");
                let dir = dir.trim_end_matches(".sounds").to_owned();
                for sound in &sounds {
                    let (ext, bytes) = if self.options.wav {
                        ("wav", sound.sample.to_wav()?)
                    } else {
                        ("snu", audio::to_snu(data, &sound.sample))
                    };
                    let path = format!("{dir}/{:04}.{ext}", sound.index);
                    self.used.insert(path.to_ascii_lowercase());
                    self.write(path, FileKind::View, &[r], &[data], &bytes)?;
                }
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

/// `ids[3]`, or all four ids joined by dots when they differ.
fn ids_text(ids: &[u32; 4]) -> String {
    if ids.iter().all(|&i| i == ids[0]) {
        format!("{:08x}", ids[0])
    } else {
        ids.map(|i| format!("{i:08x}")).join(".")
    }
}

/// A file name made of a resource name: letters, digits, `.`, `-` and `_`.
fn sanitize(name: &str) -> Option<String> {
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

/// A string table as the JSON document of `text-export`.
pub(crate) fn strings_json(
    stream: &str,
    resource: usize,
    ids: &[u32; 4],
    data: &[u8],
) -> Result<Vec<u8>> {
    let table = StringTable::parse(data)?;
    let columns = string_columns(&table);
    let entries: Vec<Value> = table
        .hashes
        .iter()
        .enumerate()
        .map(|(entry, hash)| {
            let mut object = Map::new();
            object.insert("hash".into(), json!(format!("{hash:08x}")));
            for (lang, column) in columns.iter().enumerate() {
                object.insert(column.clone(), json!(table.strings[lang][entry]));
            }
            Value::Object(object)
        })
        .collect();
    let document = json!({
        "stream": format!("{stream}.str"),
        "resource": resource,
        "id": format!("{:08x}", ids[3]),
        "languages": columns,
        "entries": entries,
    });
    let text = serde_json::to_string_pretty(&document)
        .map_err(|e| Error::InvalidArgument(e.to_string()))?;
    Ok((text + "\n").into_bytes())
}

/// Column names of a string table: its language codes, or `text`, `text1`, ….
fn string_columns(table: &StringTable) -> Vec<String> {
    if !table.languages.is_empty() {
        table.languages.clone()
    } else if table.strings.len() == 1 {
        vec!["text".to_owned()]
    } else {
        (0..table.strings.len())
            .map(|i| format!("text{i}"))
            .collect()
    }
}

/// Applies the entries of a JSON document to `table`. Returns the number of
/// changed strings.
pub(crate) fn apply_strings(table: &mut StringTable, document: &Value) -> Result<usize> {
    let bad = |why: String| Error::InvalidArgument(why);
    let columns: Vec<&str> = document
        .get("languages")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("`languages` must be a list".into()))?
        .iter()
        .map(|v| {
            v.as_str()
                .ok_or_else(|| bad("language codes must be strings".into()))
        })
        .collect::<Result<_>>()?;
    if columns.len() != table.strings.len() {
        return Err(bad(format!(
            "{} language column(s), the table has {}",
            columns.len(),
            table.strings.len()
        )));
    }
    let entries = document
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("`entries` must be a list".into()))?;
    let mut changed = 0;
    for entry in entries {
        let hash = entry
            .get("hash")
            .and_then(Value::as_str)
            .and_then(|h| u32::from_str_radix(h, 16).ok())
            .ok_or_else(|| bad("entry without a valid `hash`".into()))?;
        let row = table.find(hash).ok_or_else(|| {
            bad(format!(
                "hash {hash:08x} is not in the table; adding entries is not supported"
            ))
        })?;
        for (lang, column) in columns.iter().enumerate() {
            let value = entry
                .get(*column)
                .and_then(Value::as_str)
                .ok_or_else(|| bad(format!("entry {hash:08x} has no `{column}`")))?;
            if table.strings[lang][row] != value {
                table.strings[lang][row] = value.to_owned();
                changed += 1;
            }
        }
    }
    Ok(changed)
}

// ------------------------------------------------------------------- pack

/// Options of [`pack`].
#[derive(Debug, Clone, Default)]
pub struct PackOptions {
    /// Workspace directory; default `<game>/_doctor_toolkit/workspace`.
    pub workspace_dir: Option<PathBuf>,
    /// Only report what would be recorded.
    pub dry_run: bool,
}

/// One changed file and the resources it changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackedFile {
    pub path: String,
    pub keys: Vec<ResourceKey>,
    /// Whether the workspace changed (an edit that gives the original data back
    /// removes a change instead).
    pub recorded: bool,
}

/// What [`pack`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackReport {
    pub changed: Vec<PackedFile>,
    pub unchanged: usize,
    /// Changed files whose resources changed in the game since the unpack.
    pub warnings: Vec<String>,
    /// The workspace the edits went to.
    pub workspace: Option<PathBuf>,
}

/// The entries of the manifest whose files changed, or every problem found.
pub fn changed_files(dir: &Path) -> Result<(Manifest, Vec<FileEntry>, usize)> {
    let manifest = Manifest::read(dir)?;
    let mut problems = Vec::new();
    let mut changed = Vec::new();
    let mut unchanged = 0;
    let mut known: HashSet<String> = HashSet::from([MANIFEST.to_owned()]);
    for entry in &manifest.files {
        known.insert(entry.path.clone());
        let path = dir.join(&entry.path);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(_) => {
                problems.push(format!(
                    "{}: missing; restore it, removing resources is not supported",
                    entry.path
                ));
                continue;
            }
        };
        if sha256_hex(&bytes) == entry.sha256 {
            unchanged += 1;
        } else if !entry.kind.is_editable() {
            problems.push(format!(
                "{}: changed, but it is a view for reading only; edit the raw `.bin` file \
                 of the resource instead",
                entry.path
            ));
        } else {
            changed.push(entry.clone());
        }
    }
    let mut files = Vec::new();
    collect_files(dir, "", &mut files)?;
    for file in files {
        if !known.contains(&file) {
            problems.push(format!(
                "{file}: not in the manifest; adding new resources is not supported"
            ));
        }
    }
    if !problems.is_empty() {
        problems.sort();
        return Err(Error::InvalidArgument(format!(
            "cannot pack {}:\n  {}",
            dir.display(),
            problems.join("\n  ")
        )));
    }
    Ok((manifest, changed, unchanged))
}

/// Records the files of an unpacked directory that changed since [`unpack`]
/// in the workspace of `game`. Unchanged files are skipped; see the module
/// documentation for what is an error.
pub fn pack(game: &GameInstance, dir: &Path, options: &PackOptions) -> Result<PackReport> {
    let (manifest, mut changed, unchanged) = changed_files(dir)?;
    let mut report = PackReport {
        unchanged,
        ..PackReport::default()
    };
    // A changed side file of a model imports its model again.
    let side: Vec<FileEntry> = changed
        .iter()
        .filter(|e| e.kind == FileKind::ModelFile)
        .cloned()
        .collect();
    changed.retain(|e| e.kind != FileKind::ModelFile);
    for entry in side {
        let main = manifest
            .files
            .iter()
            .find(|m| m.kind == FileKind::Model && m.keys == entry.keys)
            .ok_or_else(|| {
                Error::InvalidArgument(format!(
                    "{}: no model file for it in the manifest",
                    entry.path
                ))
            })?;
        if !changed.iter().any(|c| c.path == main.path) {
            changed.push(main.clone());
        }
    }
    // Model files must read before anything is recorded.
    let mut scenes: HashMap<String, scene::Scene> = HashMap::new();
    for entry in changed.iter().filter(|e| e.kind == FileKind::Model) {
        let read = scene::read_file(&dir.join(&entry.path))
            .map_err(|e| Error::InvalidArgument(format!("{}: {e}", entry.path)))?;
        scenes.insert(entry.path.clone(), read);
    }
    // Resolve every file before recording anything.
    let mut plans = Vec::new();
    for entry in changed {
        let at = |e: Error| Error::InvalidArgument(format!("{}: {e}", entry.path));
        let mut located = Vec::new();
        for (k, key) in entry.keys.iter().enumerate() {
            let stream = game.find_stream(&key.stream).ok_or_else(|| {
                at(Error::InvalidArgument(format!(
                    "stream {} not in the game",
                    key.stream
                )))
            })?;
            let file = game.open_stream(stream).map_err(at)?;
            let resource = key
                .find_in(file.layout())
                .ok_or_else(|| at(Error::InvalidArgument(format!("{key} not in the game"))))?;
            let current = file.resource_data(resource).map_err(at)?;
            if entry.sources.get(k) != Some(&sha256_hex(&current)) {
                report.warnings.push(format!(
                    "{}: {key} changed in the game since it was unpacked",
                    entry.path
                ));
            }
            located.push((stream, resource));
        }
        let bytes = std::fs::read(dir.join(&entry.path)).map_err(|e| Error::io(&entry.path, e))?;
        plans.push((entry, located, bytes));
    }
    if options.dry_run {
        report.changed = plans
            .into_iter()
            .map(|(entry, _, _)| PackedFile {
                path: entry.path,
                keys: entry.keys,
                recorded: false,
            })
            .collect();
        return Ok(report);
    }
    if plans.is_empty() {
        return Ok(report);
    }
    let mut ws = Workspace::for_game(game, options.workspace_dir.as_deref())?;
    report.workspace = Some(ws.dir().to_owned());
    // Model imports are planned before anything is recorded.
    let mut model_plans: HashMap<String, crate::import::model::ImportPlan> = HashMap::new();
    if !scenes.is_empty() {
        let (index, _) = game.build_index(|_| {});
        let mut assets = crate::game::GameAssets::new(std::sync::Arc::new(game.clone()), &index);
        for (entry, located, _) in &plans {
            let Some(model) = scenes.get(&entry.path) else {
                continue;
            };
            let (stream, first) = located[0];
            let plan = crate::import::model::plan(
                game,
                &ws,
                &index,
                &mut assets,
                stream,
                first,
                model,
                &crate::import::model::ImportOptions::default(),
            )
            .map_err(|e| Error::InvalidArgument(format!("{}: {e}", entry.path)))?;
            model_plans.insert(entry.path.clone(), plan);
        }
    }
    for (entry, located, bytes) in plans {
        let at = |e: Error| Error::InvalidArgument(format!("{}: {e}", entry.path));
        let (stream, first) = located[0];
        let before = ws.head();
        match entry.kind {
            FileKind::TexturePng | FileKind::TextureDds => {
                let &[(_, header), (_, data)] = located.as_slice() else {
                    return Err(at(Error::InvalidArgument(
                        "a texture needs two resources".into(),
                    )));
                };
                let pair = TexturePair { header, data };
                let is_png = entry.kind == FileKind::TexturePng;
                ws.import_texture(game, stream, pair, &bytes, is_png)
                    .map_err(at)?;
            }
            FileKind::Strings => {
                let document: Value = serde_json::from_slice(&bytes)
                    .map_err(|e| at(Error::InvalidArgument(format!("not valid JSON: {e}"))))?;
                let key = &entry.keys[0];
                let data = match ws.data(key)? {
                    Some(data) => data,
                    None => game.open_stream(stream)?.resource_data(first)?,
                };
                let mut table = StringTable::parse(&data)?;
                apply_strings(&mut table, &document).map_err(at)?;
                ws.set_string_table(game, stream, first, &table)
                    .map_err(at)?;
            }
            FileKind::Raw => {
                let label = format!("Pack {}", entry.path);
                ws.set_resources(
                    game,
                    &label,
                    stream,
                    &[ResourceEdit {
                        index: first,
                        data: bytes,
                        storage: StorageChoice::Keep,
                    }],
                )
                .map_err(at)?;
            }
            FileKind::Model => {
                let plan = &model_plans[&entry.path];
                for warning in &plan.summary.warnings {
                    report.warnings.push(format!("{}: {warning}", entry.path));
                }
                plan.commit(&mut ws, game).map_err(at)?;
            }
            FileKind::View | FileKind::ModelFile => {
                unreachable!("views are rejected by changed_files, side files resolved above")
            }
        }
        report.changed.push(PackedFile {
            path: entry.path,
            keys: entry.keys,
            recorded: ws.head() != before,
        });
    }
    Ok(report)
}

/// Relative paths of every file below `dir`, with forward slashes.
fn collect_files(dir: &Path, rel: &str, out: &mut Vec<String>) -> Result<()> {
    for entry in std::fs::read_dir(dir).map_err(|e| Error::io(dir, e))? {
        let entry = entry.map_err(|e| Error::io(dir, e))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        // Hidden files and editor backups are not resources.
        if name.starts_with('.') || name.ends_with('~') {
            continue;
        }
        let rel_path = if rel.is_empty() {
            name
        } else {
            format!("{rel}/{name}")
        };
        if entry.path().is_dir() {
            collect_files(&entry.path(), &rel_path, out)?;
        } else {
            out.push(rel_path);
        }
    }
    Ok(())
}

// ----------------------------------------------------------- model assets

/// Where the materials, textures and rigs of the game are, for glTF export.
pub struct AssetMaps {
    textures: HashMap<u32, RecordId>,
    materials: HashMap<u32, RecordId>,
    /// Bone table of a model → the rig its skeleton reference resolves to.
    rigs: HashMap<Box<[(u32, u32)]>, RecordId>,
}

impl AssetMaps {
    pub fn new(index: &GameIndex) -> Self {
        let mut maps = Self {
            textures: HashMap::new(),
            materials: HashMap::new(),
            rigs: HashMap::new(),
        };
        for r in index.ids() {
            let record = index.record(r);
            if record.header.type_id == TYPE_TG4H {
                maps.textures.entry(record.header.ids[3]).or_insert(r);
            }
            for anchor in &record.anchors {
                if let Anchor::Material(h) = anchor {
                    maps.materials.entry(*h).or_insert(r);
                }
            }
            for (reference, targets) in index.uses(r) {
                if let (Target::Bones(bones), Some(&rig)) = (&reference.target, targets.first()) {
                    maps.rigs.entry(bones.clone()).or_insert(rig);
                }
            }
        }
        maps
    }
}

/// Materials, textures and skeletons for glTF export, found through the index.
pub struct IndexAssets<'a> {
    game: &'a GameInstance,
    index: &'a GameIndex,
    maps: &'a AssetMaps,
    open: HashMap<usize, StreamFile>,
}

/// Streams kept open by [`IndexAssets`]; all are closed beyond this.
const OPEN_STREAMS: usize = 32;

impl<'a> IndexAssets<'a> {
    pub fn new(game: &'a GameInstance, index: &'a GameIndex, maps: &'a AssetMaps) -> Self {
        Self {
            game,
            index,
            maps,
            open: HashMap::new(),
        }
    }

    /// The open stream and resource index of a record.
    fn locate(&mut self, id: RecordId) -> Option<(&StreamFile, usize)> {
        let stream = self.index.stream_of(id);
        if !self.open.contains_key(&stream) {
            if self.open.len() >= OPEN_STREAMS {
                self.open.clear();
            }
            self.open
                .insert(stream, self.game.open_stream(stream).ok()?);
        }
        Some((&self.open[&stream], self.index.record(id).resource as usize))
    }
}

impl AssetSource for IndexAssets<'_> {
    fn material(&mut self, hash: u32) -> Option<Material> {
        let id = *self.maps.materials.get(&hash)?;
        let (stream, r) = self.locate(id)?;
        Material::parse(&stream.resource_data(r).ok()?).ok()
    }

    fn texture(&mut self, hash: u32) -> Option<Texture> {
        let id = *self.maps.textures.get(&hash)?;
        let (stream, r) = self.locate(id)?;
        let pair = texture::pair_of(stream.layout(), r)?;
        Texture::load(stream.data(), stream.layout(), pair).ok()
    }

    fn skeleton(&mut self, bones: &[(u32, u32)]) -> Option<Skeleton> {
        let mut rigs = match self.maps.rigs.get(bones) {
            Some(&rig) => vec![rig],
            None => Vec::new(),
        };
        // A bone table no game model has (an imported model binds only the
        // bones it uses): any rig whose skeleton has these bones.
        if rigs.is_empty() {
            let mut all: Vec<RecordId> = self.maps.rigs.values().copied().collect();
            all.sort_unstable();
            all.dedup();
            rigs = all;
        }
        rigs.into_iter().find_map(|rig| {
            let (stream, r) = self.locate(rig)?;
            let data = stream.resource_data(r).ok()?;
            rcb::Rig::parse(&data).ok()?.skeleton_for(bones).cloned()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_sanitized() {
        assert_eq!(sanitize("Car Body/LOD1").as_deref(), Some("Car_Body_LOD1"));
        assert_eq!(sanitize("..").as_deref(), None);
        assert_eq!(sanitize("").as_deref(), None);
        assert_eq!(ids_text(&[1; 4]), "00000001");
        assert_eq!(
            ids_text(&[0, 1, 2, 3]),
            "00000000.00000001.00000002.00000003"
        );
    }

    #[test]
    fn string_documents_round_trip() {
        let table = StringTable {
            version: 1,
            flags: 0x4000,
            languages: vec![],
            hashes: vec![1, 2],
            strings: vec![vec!["one".into(), "two".into()]],
        };
        let json = strings_json("gf2/text/x", 0, &[2; 4], &table.to_bytes().unwrap()).unwrap();
        let mut document: Value = serde_json::from_slice(&json).unwrap();
        let mut copy = table.clone();
        assert_eq!(apply_strings(&mut copy, &document).unwrap(), 0);
        document["entries"][1]["text"] = json!("deux");
        assert_eq!(apply_strings(&mut copy, &document).unwrap(), 1);
        assert_eq!(copy.strings[0][1], "deux");
        document["entries"][1]["hash"] = json!("00000009");
        assert!(apply_strings(&mut copy, &document).is_err());
    }
}

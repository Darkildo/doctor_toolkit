//! `Game`, `Stream` and `Resource`: reading a game directory.

use std::path::PathBuf;
use std::sync::Arc;

use doctor_toolkit::container::ustream::ResourceHeader;
use doctor_toolkit::game::{GameInstance, StreamFile};
use doctor_toolkit::workspace::ResourceKey;
use pyo3::exceptions::{PyIndexError, PyKeyError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};

use doctor_toolkit::container::big::{BigArchive, find_archives};
use doctor_toolkit::mods::integrity;
use doctor_toolkit::navigation::unpack;
use doctor_toolkit::workspace as ws;

use crate::error::{ResultExt, invalid};
use crate::formats::{self, Sound};
use crate::mods::ModManager;
use crate::navigation::Index;
use crate::util::{self, mem_policy_name, resolve_type, type_name};
use crate::workspace::Workspace;

/// An open stream of a game: the game, its position in the game and the mapped file.
#[derive(Clone)]
pub struct Loc {
    pub game: Arc<GameInstance>,
    pub stream_index: usize,
    /// Normalised stream name (`gf2/frontend`).
    pub name: String,
    pub file: Arc<StreamFile>,
}

impl Loc {
    pub fn open(game: &Arc<GameInstance>, stream_index: usize) -> PyResult<Self> {
        let file = game.open_stream(stream_index).py()?;
        Ok(Self {
            game: game.clone(),
            stream_index,
            name: game.streams()[stream_index].stream_name(),
            file: Arc::new(file),
        })
    }

    /// Opens the stream called `name` (`gf2/frontend`, `gf2\frontend`, `gf2/frontend.str`).
    pub fn find(game: &Arc<GameInstance>, name: &str) -> PyResult<Self> {
        let index = game
            .find_stream(name)
            .ok_or_else(|| PyKeyError::new_err(format!("stream `{name}` not found")))?;
        Self::open(game, index)
    }

    /// The stream of `key` and the index of its resource.
    pub fn locate(game: &Arc<GameInstance>, key: &ResourceKey) -> PyResult<(Self, usize)> {
        let loc = Self::find(game, &key.stream)?;
        let index = key
            .find_in(loc.file.layout())
            .ok_or_else(|| PyKeyError::new_err(format!("resource {key} not found")))?;
        Ok((loc, index))
    }

    pub fn len(&self) -> usize {
        self.file.layout().resources.len()
    }

    /// Checks a resource index; negative indices count from the end.
    pub fn check_index(&self, index: isize) -> PyResult<usize> {
        let len = self.len() as isize;
        let i = if index < 0 { index + len } else { index };
        if (0..len).contains(&i) {
            Ok(i as usize)
        } else {
            Err(PyIndexError::new_err(format!(
                "{} has no resource {index} ({len} resource(s))",
                self.name
            )))
        }
    }

    pub fn header(&self, index: usize) -> &ResourceHeader {
        &self.file.layout().resources[index].header
    }

    pub fn key(&self, index: usize) -> ResourceKey {
        ResourceKey::new(&self.name, self.header(index))
    }

    pub fn resource(&self, index: usize) -> Resource {
        let layout = self.file.layout();
        Resource {
            loc: self.clone(),
            index,
            header: layout.resources[index].header,
            stored_size: layout.stored_size(index),
            compressed: layout.is_compressed(index),
        }
    }

    /// Decompressed data of resource `index`, read with the GIL released.
    pub fn read(&self, py: Python<'_>, index: usize) -> PyResult<Vec<u8>> {
        let file = &self.file;
        py.detach(|| file.resource_data(index)).py()
    }
}

/// Accepts a resource key as text or a `Resource`.
pub fn key_of(value: &Bound<'_, PyAny>) -> PyResult<ResourceKey> {
    if let Ok(resource) = value.cast::<Resource>() {
        let r = resource.get();
        return Ok(r.loc.key(r.index));
    }
    let text: String = value.extract().map_err(|_| {
        pyo3::exceptions::PyTypeError::new_err("expected a resource key (str) or a Resource")
    })?;
    util::parse_key(&text)
}

/// An installed game: its streams (`.str`) and tables of contents (`.toc`).
///
/// Opening scans the directory; streams are opened (memory-mapped) on demand.
#[pyclass(module = "doctor_toolkit", frozen)]
pub struct Game {
    pub game: Arc<GameInstance>,
}

#[pymethods]
impl Game {
    #[new]
    fn new(py: Python<'_>, root: PathBuf) -> PyResult<Self> {
        let game = py.detach(|| GameInstance::open(root)).py()?;
        Ok(Self {
            game: Arc::new(game),
        })
    }

    /// The game directory.
    #[getter]
    fn root(&self) -> PathBuf {
        self.game.root().to_owned()
    }

    /// Name of the game profile, e.g. `The Godfather II (PC)`.
    #[getter]
    fn profile(&self) -> &'static str {
        self.game.profile().name
    }

    /// Names of all streams (`gf2/frontend`), sorted by path.
    #[getter]
    fn streams(&self) -> Vec<String> {
        self.game
            .streams()
            .iter()
            .map(|s| s.stream_name())
            .collect()
    }

    /// Paths of all tables of contents relative to the game directory.
    #[getter]
    fn tocs(&self) -> Vec<String> {
        self.game
            .tocs()
            .iter()
            .map(|t| t.rel_path.clone())
            .collect()
    }

    /// Every stream as a dict: `name`, `path` (relative), `size` (bytes).
    fn stream_files<'py>(&self, py: Python<'py>) -> PyResult<Vec<Bound<'py, PyDict>>> {
        self.game
            .streams()
            .iter()
            .map(|s| {
                let dict = PyDict::new(py);
                dict.set_item("name", s.stream_name())?;
                dict.set_item("path", &s.rel_path)?;
                dict.set_item("size", s.len)?;
                Ok(dict)
            })
            .collect()
    }

    /// Whether the game has a stream called `name`.
    fn has_stream(&self, name: &str) -> bool {
        self.game.find_stream(name).is_some()
    }

    /// Opens a stream by name: `gf2/frontend`, `gf2\frontend` or `gf2/frontend.str`.
    /// Raises `KeyError` when there is none.
    fn stream(&self, py: Python<'_>, name: &str) -> PyResult<Stream> {
        let game = &self.game;
        let index = game
            .find_stream(name)
            .ok_or_else(|| PyKeyError::new_err(format!("stream `{name}` not found")))?;
        let loc = py.detach(|| Loc::open(game, index))?;
        Ok(Stream { loc })
    }

    /// Iterates over the resources of the game (or of the named streams),
    /// optionally only those of one type (`"tg4h"`, `"simgroup"`, a type id).
    /// Streams are opened one after another.
    #[pyo3(signature = (r#type = None, streams = None))]
    fn resources(
        &self,
        r#type: Option<&Bound<'_, PyAny>>,
        streams: Option<Vec<String>>,
    ) -> PyResult<ResourceIter> {
        let type_id = r#type.map(resolve_type).transpose()?;
        let indices = match streams {
            None => (0..self.game.streams().len()).collect(),
            Some(names) => names
                .iter()
                .map(|name| {
                    self.game
                        .find_stream(name)
                        .ok_or_else(|| PyKeyError::new_err(format!("stream `{name}` not found")))
                })
                .collect::<PyResult<Vec<_>>>()?,
        };
        Ok(ResourceIter::new(self.game.clone(), indices, type_id))
    }

    /// The resource with key `key` (`gf2/frontend:tg4h:00000000.4802e0a7.f3d257db.9cf57f36`).
    /// Raises `KeyError` when the game has no such resource.
    fn resource(&self, key: &Bound<'_, PyAny>) -> PyResult<Resource> {
        let key = key_of(key)?;
        let (loc, index) = Loc::locate(&self.game, &key)?;
        Ok(loc.resource(index))
    }

    /// The decompressed data of the resource with key `key`.
    fn read<'py>(&self, py: Python<'py>, key: &Bound<'_, PyAny>) -> PyResult<Bound<'py, PyBytes>> {
        let key = key_of(key)?;
        let (loc, index) = Loc::locate(&self.game, &key)?;
        let data = loc.read(py, index)?;
        Ok(util::bytes(py, &data))
    }

    /// A table of contents as a dict: `version` and `files` (`name`, `data_size`,
    /// `hash`, `deps`). `path` is relative to the game directory.
    fn toc<'py>(&self, py: Python<'py>, path: &str) -> PyResult<Bound<'py, PyDict>> {
        let wanted = path.replace('\\', "/").to_ascii_lowercase();
        let index = self
            .game
            .tocs()
            .iter()
            .position(|t| t.rel_path.to_ascii_lowercase() == wanted)
            .ok_or_else(|| PyKeyError::new_err(format!("table of contents `{path}` not found")))?;
        let toc = self.game.load_toc(index).py()?;
        let files = toc
            .files
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let file = PyDict::new(py);
                let name = toc.file_name(i);
                file.set_item("name", name.name_lossy())?;
                file.set_item("hash", name.hash)?;
                file.set_item("data_size", f.data_size)?;
                let deps: Vec<String> = toc.file_deps(i).map(|d| d.name_lossy()).collect();
                file.set_item("deps", deps)?;
                Ok(file)
            })
            .collect::<PyResult<Vec<_>>>()?;
        let dict = PyDict::new(py);
        dict.set_item("version", toc.version)?;
        dict.set_item("files", files)?;
        Ok(dict)
    }

    /// The game index with names and references of every resource. With
    /// `cache` the index is kept in `~/.cache/doctor-toolkit/` (or
    /// `$DOCTOR_TOOLKIT_CACHE_DIR`), never in the game directory, and only
    /// changed streams are read again; `cache` may also be a file path.
    /// Building from scratch reads every stream (about a minute for the whole game).
    #[pyo3(signature = (cache = None))]
    fn index(&self, py: Python<'_>, cache: Option<&Bound<'_, PyAny>>) -> PyResult<Index> {
        let cache = match cache {
            None => Index::default_cache(&self.game),
            Some(value) => {
                if let Ok(flag) = value.cast::<pyo3::types::PyBool>() {
                    if flag.is_true() {
                        Index::default_cache(&self.game)
                    } else {
                        None
                    }
                } else {
                    Some(value.extract::<PathBuf>()?)
                }
            }
        };
        Ok(Index::open(py, self.game.clone(), cache))
    }

    /// Opens the workspace of the game: in `dir`, or in
    /// `<game>/_doctor_toolkit/workspace`. With `create` (the default) a
    /// missing workspace is created; without it, `InvalidArgumentError`.
    #[pyo3(signature = (dir = None, create = true))]
    fn workspace(&self, py: Python<'_>, dir: Option<PathBuf>, create: bool) -> PyResult<Workspace> {
        let dir = dir.unwrap_or_else(|| ws::Workspace::default_dir(self.game.root()));
        let game = &self.game;
        let opened = py.detach(|| {
            if ws::Workspace::exists(&dir) {
                ws::Workspace::open(&dir)
            } else if create {
                ws::Workspace::create(&dir, game)
            } else {
                Err(doctor_toolkit::Error::InvalidArgument(format!(
                    "no workspace in {}",
                    dir.display()
                )))
            }
        });
        Ok(Workspace::new(self.game.clone(), opened.py()?))
    }

    /// Creates an empty workspace in `dir` (default `<game>/_doctor_toolkit/workspace`);
    /// refused when one exists there.
    #[pyo3(signature = (dir = None))]
    fn init_workspace(&self, py: Python<'_>, dir: Option<PathBuf>) -> PyResult<Workspace> {
        let dir = dir.unwrap_or_else(|| ws::Workspace::default_dir(self.game.root()));
        let game = &self.game;
        let created = py.detach(|| ws::Workspace::create(&dir, game)).py()?;
        Ok(Workspace::new(self.game.clone(), created))
    }

    /// The mod manager of the game.
    fn mods(&self, py: Python<'_>) -> PyResult<ModManager> {
        ModManager::open(py, self.game.clone())
    }

    /// Writes resources into `out` in common formats (PNG or DDS, JSON, models
    /// as `model_format`: glb, fbx, obj, usd, usda, usdc or usdz, WAV or
    /// `.snu`, raw `.bin`) with a `manifest.json`, like
    /// `doctor-toolkit unpack`. `streams` and `types` select (all when `None`);
    /// `keys` selects single resources instead (needs `index`). `index` gives
    /// names and model assets. Returns `files`, `resources`, `bytes`, `failed`.
    #[pyo3(signature = (out, streams = None, types = None, *, keys = None, index = None, dds = false, wav = false, raw = true, views = true, model_assets = true, model_format = "glb"))]
    #[allow(clippy::too_many_arguments)]
    fn unpack<'py>(
        &self,
        py: Python<'py>,
        out: PathBuf,
        streams: Option<Vec<String>>,
        types: Option<Vec<Bound<'py, PyAny>>>,
        keys: Option<Vec<Bound<'py, PyAny>>>,
        index: Option<PyRef<'py, Index>>,
        dds: bool,
        wav: bool,
        raw: bool,
        views: bool,
        model_assets: bool,
        model_format: &str,
    ) -> PyResult<Bound<'py, PyDict>> {
        let model_format: doctor_toolkit::scene::ModelFormat = model_format.parse().py()?;
        let types = types
            .unwrap_or_default()
            .iter()
            .map(resolve_type)
            .collect::<PyResult<Vec<_>>>()?;
        let streams = streams.unwrap_or_default();
        let game = &self.game;
        let idx = index.as_ref().map(|i| i.game_index());
        let selection = match keys {
            Some(keys) => {
                let idx = idx.ok_or_else(|| invalid("selecting by keys needs the index"))?;
                let records = keys
                    .iter()
                    .map(|k| {
                        let key = key_of(k)?;
                        idx.find_key(&key)
                            .ok_or_else(|| PyKeyError::new_err(format!("resource {key} not found")))
                    })
                    .collect::<PyResult<Vec<_>>>()?;
                unpack::select_records(idx, &records)
            }
            None => py.detach(|| unpack::select(game, &streams, &types)).py()?,
        };
        let options = unpack::UnpackOptions {
            dds,
            wav,
            raw,
            views,
            model_assets,
            model_format,
        };
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let report = py
            .detach(|| unpack::unpack(game, idx, &selection, &out, options, &|_, _| {}, &cancel))
            .py()?;
        let dict = PyDict::new(py);
        dict.set_item("files", report.files)?;
        dict.set_item("resources", report.resources)?;
        dict.set_item("bytes", report.bytes)?;
        dict.set_item("failed", report.failed)?;
        Ok(dict)
    }

    /// Records the files of an unpacked directory that changed since
    /// `unpack` in the workspace (`workspace_dir`, default the game's), like
    /// `doctor-toolkit pack`. Nothing is recorded when any file cannot be
    /// imported. Returns `changed` (dicts `path`, `keys`, `recorded`),
    /// `unchanged`, `warnings`, `workspace`.
    #[pyo3(signature = (dir, workspace_dir = None, dry_run = false))]
    fn pack<'py>(
        &self,
        py: Python<'py>,
        dir: PathBuf,
        workspace_dir: Option<PathBuf>,
        dry_run: bool,
    ) -> PyResult<Bound<'py, PyDict>> {
        let game = &self.game;
        let options = unpack::PackOptions {
            workspace_dir,
            dry_run,
        };
        let report = py.detach(|| unpack::pack(game, &dir, &options)).py()?;
        let changed = report
            .changed
            .iter()
            .map(|f| {
                let item = PyDict::new(py);
                item.set_item("path", &f.path)?;
                let keys: Vec<String> = f.keys.iter().map(|k| k.to_string()).collect();
                item.set_item("keys", keys)?;
                item.set_item("recorded", f.recorded)?;
                Ok(item)
            })
            .collect::<PyResult<Vec<_>>>()?;
        let dict = PyDict::new(py);
        dict.set_item("changed", changed)?;
        dict.set_item("unchanged", report.unchanged)?;
        dict.set_item("warnings", report.warnings)?;
        dict.set_item("workspace", report.workspace)?;
        Ok(dict)
    }

    /// Which game files differ from their originals and who changed them,
    /// like `doctor-toolkit integrity`: dicts `path`, `condition`, `changed`.
    /// `full` hashes the files without a backup (reads the whole game).
    #[pyo3(signature = (full = false))]
    fn integrity<'py>(&self, py: Python<'py>, full: bool) -> PyResult<Vec<Bound<'py, PyDict>>> {
        let game = &self.game;
        let options = integrity::IntegrityOptions { full };
        let report = py
            .detach(|| integrity::check(game, options, |_, _| {}))
            .py()?;
        report
            .files
            .iter()
            .map(|f| {
                let item = PyDict::new(py);
                item.set_item("path", &f.rel_path)?;
                item.set_item("condition", f.condition.describe())?;
                item.set_item("changed", f.condition.is_changed())?;
                Ok(item)
            })
            .collect()
    }

    /// Entries of the `.viv` sound archives: dicts `archive` (file name),
    /// `name`, `size`.
    fn archive_entries<'py>(&self, py: Python<'py>) -> PyResult<Vec<Bound<'py, PyDict>>> {
        let root = self.game.root();
        let archives = py
            .detach(|| {
                find_archives(root)?
                    .into_iter()
                    .map(BigArchive::open)
                    .collect::<doctor_toolkit::Result<Vec<_>>>()
            })
            .py()?;
        let mut out = Vec::new();
        for archive in &archives {
            let file = archive
                .path
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            for entry in &archive.entries {
                if doctor_toolkit::container::big::is_tool_entry(&entry.name) {
                    continue;
                }
                let item = PyDict::new(py);
                item.set_item("archive", &file)?;
                item.set_item("name", &entry.name)?;
                item.set_item("size", entry.size)?;
                out.push(item);
            }
        }
        Ok(out)
    }

    /// A streamed sound of the `.viv` archives by entry name (see `archive_entries`).
    fn archive_sound(&self, py: Python<'_>, name: &str) -> PyResult<Sound> {
        let root = self.game.root();
        let found = py
            .detach(|| {
                for path in find_archives(root)? {
                    let archive = BigArchive::open(path)?;
                    if let Some(entry) = archive.entries.iter().find(|e| e.name == name) {
                        return archive.read(entry).map(Some);
                    }
                }
                Ok(None)
            })
            .py()?;
        let data =
            found.ok_or_else(|| PyKeyError::new_err(format!("no archive entry `{name}`")))?;
        Sound::from_snu(name.to_owned(), data)
    }

    fn __repr__(&self) -> String {
        format!(
            "Game({:?}, {} stream(s))",
            self.game.root().display().to_string(),
            self.game.streams().len()
        )
    }
}

/// One stream file (`.str`), memory-mapped. Iterating yields its resources.
///
/// The file must not be changed in place while it is open; writes of this
/// library replace files atomically, so an open stream keeps showing the old data.
#[pyclass(module = "doctor_toolkit", frozen)]
pub struct Stream {
    pub loc: Loc,
}

#[pymethods]
impl Stream {
    /// Normalised stream name, e.g. `gf2/frontend`.
    #[getter]
    fn name(&self) -> &str {
        &self.loc.name
    }

    /// Path relative to the game directory, e.g. `gf2/frontend.str`.
    #[getter]
    fn rel_path(&self) -> String {
        self.loc.game.streams()[self.loc.stream_index]
            .rel_path
            .clone()
    }

    /// Absolute path of the file.
    #[getter]
    fn path(&self) -> PathBuf {
        self.loc.file.path().to_owned()
    }

    /// Size of the file in bytes.
    #[getter]
    fn size(&self) -> usize {
        self.loc.file.data().len()
    }

    /// Sum of the decompressed sizes of the resources (the size tables of contents record).
    #[getter]
    fn data_size(&self) -> u64 {
        self.loc.file.layout().total_data_size()
    }

    fn __len__(&self) -> usize {
        self.loc.len()
    }

    fn __getitem__(&self, index: isize) -> PyResult<Resource> {
        let index = self.loc.check_index(index)?;
        Ok(self.loc.resource(index))
    }

    fn __iter__(&self) -> ResourceIter {
        ResourceIter::of_stream(self.loc.clone())
    }

    /// The resources, optionally only those of one type.
    #[pyo3(signature = (r#type = None))]
    fn resources(&self, r#type: Option<&Bound<'_, PyAny>>) -> PyResult<Vec<Resource>> {
        let type_id = r#type.map(resolve_type).transpose()?;
        Ok((0..self.loc.len())
            .filter(|&i| type_id.is_none_or(|t| self.loc.header(i).type_id == t))
            .map(|i| self.loc.resource(i))
            .collect())
    }

    /// The first resource of type `type` (any type when `None`) whose last id is
    /// `id` (any id when `None`), or `None`.
    #[pyo3(signature = (r#type = None, id = None))]
    fn find(
        &self,
        r#type: Option<&Bound<'_, PyAny>>,
        id: Option<u32>,
    ) -> PyResult<Option<Resource>> {
        let type_id = r#type.map(resolve_type).transpose()?;
        Ok((0..self.loc.len())
            .find(|&i| {
                let h = self.loc.header(i);
                type_id.is_none_or(|t| h.type_id == t) && id.is_none_or(|id| h.ids[3] == id)
            })
            .map(|i| self.loc.resource(i)))
    }

    /// The resource with key `key`, or `None`.
    fn get(&self, key: &Bound<'_, PyAny>) -> PyResult<Option<Resource>> {
        let key = key_of(key)?;
        if key.stream != self.loc.name {
            return Ok(None);
        }
        Ok(key
            .find_in(self.loc.file.layout())
            .map(|i| self.loc.resource(i)))
    }

    /// Decompressed data of resource `index`.
    fn read<'py>(&self, py: Python<'py>, index: isize) -> PyResult<Bound<'py, PyBytes>> {
        let index = self.loc.check_index(index)?;
        let data = self.loc.read(py, index)?;
        Ok(util::bytes(py, &data))
    }

    /// Every texture of the stream (`tg4h` with its `tg4d`).
    fn textures(&self, py: Python<'_>) -> PyResult<Vec<formats::Texture>> {
        formats::textures(py, &self.loc)
    }

    fn __repr__(&self) -> String {
        format!(
            "Stream({:?}, {} resource(s))",
            self.loc.name,
            self.loc.len()
        )
    }
}

/// One resource of a stream: its header and access to its data and views.
#[pyclass(module = "doctor_toolkit", frozen)]
pub struct Resource {
    pub loc: Loc,
    pub index: usize,
    pub header: ResourceHeader,
    pub stored_size: usize,
    pub compressed: bool,
}

#[pymethods]
impl Resource {
    /// Name of the stream, e.g. `gf2/frontend`.
    #[getter]
    fn stream(&self) -> &str {
        &self.loc.name
    }

    /// Position of the resource in its stream.
    #[getter]
    fn index(&self) -> usize {
        self.index
    }

    /// Resource key `stream:type:ids`, stable across rewrites of the stream.
    #[getter]
    fn key(&self) -> String {
        self.loc.key(self.index).to_string()
    }

    /// Type tag, e.g. `tg4h`, or the type id in hexadecimal when it is not known.
    #[getter]
    fn r#type(&self) -> String {
        type_name(self.header.type_id)
    }

    /// Type id: SDBM hash of the type tag.
    #[getter]
    fn type_id(&self) -> u32 {
        self.header.type_id
    }

    /// The four identifiers of the header.
    #[getter]
    fn ids(&self) -> [u32; 4] {
        self.header.ids
    }

    /// The last identifier: the name hash for most types.
    #[getter]
    fn id(&self) -> u32 {
        self.header.ids[3]
    }

    /// Size of the decompressed data in bytes.
    #[getter]
    fn data_size(&self) -> u32 {
        self.header.data_size
    }

    /// Bytes the data occupies in the stream file.
    #[getter]
    fn stored_size(&self) -> usize {
        self.stored_size
    }

    /// Whether the data is stored compressed.
    #[getter]
    fn compressed(&self) -> bool {
        self.compressed
    }

    /// Memory policy name (`default`, `volatile`, ...).
    #[getter]
    fn mem_policy(&self) -> String {
        mem_policy_name(self.header.mem_policy)
    }

    /// Alignment of the loaded data.
    #[getter]
    fn alignment(&self) -> u32 {
        self.header.alignment
    }

    /// The decompressed data.
    fn read<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let data = self.loc.read(py, self.index)?;
        Ok(util::bytes(py, &data))
    }

    /// The parsed resource: a `Texture` (`tg4h`, `tg4d`), a `StringTable`
    /// (`lh2`), a `Model` (`mesh`), a `Font` (`ffn`), a list of `Sound` (`sbk`)
    /// or a dict (game logic, `rcb`, `material`, `lua`, `hkx`, `bnk`).
    /// Raises `InvalidArgumentError` for types without a parser.
    fn parse<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        formats::parse(py, &self.loc, self.index)
    }

    /// The texture this `tg4h` or `tg4d` belongs to.
    fn texture(&self, py: Python<'_>) -> PyResult<formats::Texture> {
        formats::texture(py, &self.loc, self.index)
    }

    /// The string table of an `lh2` resource.
    fn string_table(&self, py: Python<'_>) -> PyResult<formats::StringTable> {
        formats::string_table(py, &self.loc, self.index)
    }

    /// The model of a `mesh` resource (with its `meshvolatile`).
    fn model(&self, py: Python<'_>) -> PyResult<formats::Model> {
        formats::model(py, &self.loc, self.index)
    }

    /// The header fields as a dict.
    fn to_dict<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let dict = PyDict::new(py);
        dict.set_item("key", self.key())?;
        dict.set_item("stream", &self.loc.name)?;
        dict.set_item("index", self.index)?;
        dict.set_item("type", self.r#type())?;
        dict.set_item("type_id", self.header.type_id)?;
        dict.set_item("ids", self.header.ids)?;
        dict.set_item("data_size", self.header.data_size)?;
        dict.set_item("stored_size", self.stored_size)?;
        dict.set_item("compressed", self.compressed)?;
        dict.set_item("mem_policy", self.mem_policy())?;
        dict.set_item("alignment", self.header.alignment)?;
        Ok(dict)
    }

    fn __repr__(&self) -> String {
        format!(
            "Resource({:?}, index={}, {} byte(s))",
            self.key(),
            self.index,
            self.header.data_size
        )
    }
}

/// Iterator over resources of one or more streams.
#[pyclass(module = "doctor_toolkit")]
pub struct ResourceIter {
    game: Arc<GameInstance>,
    streams: std::vec::IntoIter<usize>,
    type_id: Option<u32>,
    current: Option<(Loc, usize)>,
}

impl ResourceIter {
    fn new(game: Arc<GameInstance>, streams: Vec<usize>, type_id: Option<u32>) -> Self {
        Self {
            game,
            streams: streams.into_iter(),
            type_id,
            current: None,
        }
    }

    fn of_stream(loc: Loc) -> Self {
        Self {
            game: loc.game.clone(),
            streams: Vec::new().into_iter(),
            type_id: None,
            current: Some((loc, 0)),
        }
    }
}

#[pymethods]
impl ResourceIter {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&mut self, py: Python<'_>) -> PyResult<Option<Resource>> {
        loop {
            if let Some((loc, position)) = &mut self.current {
                while *position < loc.len() {
                    let i = *position;
                    *position += 1;
                    if self.type_id.is_none_or(|t| loc.header(i).type_id == t) {
                        return Ok(Some(loc.resource(i)));
                    }
                }
                self.current = None;
            }
            let Some(next) = self.streams.next() else {
                return Ok(None);
            };
            let game = &self.game;
            let loc = py.detach(|| Loc::open(game, next))?;
            self.current = Some((loc, 0));
        }
    }
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Game>()?;
    m.add_class::<Stream>()?;
    m.add_class::<Resource>()?;
    m.add_class::<ResourceIter>()?;
    Ok(())
}

//! `Workspace`: edits kept on disk with history, applied to the game on demand.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::mods::{self, ModInfo, Preview};
use doctor_toolkit::resource::lh2;
use doctor_toolkit::workspace::{
    self as ws, ApplyOptions, ApplyReport, ChangeState, ResourceEdit, ResourceKey, SemanticDiff,
    StorageChoice, StreamAction, StreamState,
};
use pyo3::exceptions::{PyKeyError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};

use crate::error::{ResultExt, invalid};
use crate::formats::{self, StringTable, Texture};
use crate::game::{Loc, key_of};
use crate::util;

/// The changes of a game kept on disk, with undo, redo and a diff against
/// the original data. The game files are only written by `apply` and `unapply`.
///
/// Opened by `Game.workspace()` or `Game.init_workspace()`.
#[pyclass(module = "doctor_toolkit")]
pub struct Workspace {
    game: Arc<GameInstance>,
    ws: ws::Workspace,
}

impl Workspace {
    pub fn new(game: Arc<GameInstance>, ws: ws::Workspace) -> Self {
        Self { game, ws }
    }

    pub fn inner(&self) -> &ws::Workspace {
        &self.ws
    }

    pub fn game(&self) -> &Arc<GameInstance> {
        &self.game
    }

    /// The current data of a resource: the edited data, or the game's.
    fn current(&self, py: Python<'_>, key: &ResourceKey) -> PyResult<Vec<u8>> {
        if let Some(data) = self.ws.data(key).py()? {
            return Ok(data);
        }
        let (loc, index) = Loc::locate(&self.game, key)?;
        loc.read(py, index)
    }

    /// Records `edits` of the resources of one stream as one operation.
    fn record(
        &mut self,
        py: Python<'_>,
        label: &str,
        stream: &str,
        edits: Vec<(ResourceKey, Vec<u8>, StorageChoice)>,
    ) -> PyResult<bool> {
        let loc = Loc::find(&self.game, stream)?;
        let edits = edits
            .into_iter()
            .map(|(key, data, storage)| {
                let index = key
                    .find_in(loc.file.layout())
                    .ok_or_else(|| PyKeyError::new_err(format!("resource {key} not found")))?;
                Ok(ResourceEdit {
                    index,
                    data,
                    storage,
                })
            })
            .collect::<PyResult<Vec<_>>>()?;
        drop(loc);
        let game = &self.game;
        let workspace = &mut self.ws;
        let index = game.find_stream(stream).expect("stream was found above");
        let seq = py
            .detach(|| workspace.set_resources(game, label, index, &edits))
            .py()?;
        Ok(seq.is_some())
    }
}

fn storage_choice(text: &str) -> PyResult<StorageChoice> {
    match text {
        "keep" => Ok(StorageChoice::Keep),
        "raw" => Ok(StorageChoice::Raw),
        "refpack" => Ok(StorageChoice::Refpack),
        _ => Err(PyValueError::new_err(format!(
            "storage must be `keep`, `raw` or `refpack`, not `{text}`"
        ))),
    }
}

fn storage_name(storage: StorageChoice) -> &'static str {
    match storage {
        StorageChoice::Keep => "keep",
        StorageChoice::Raw => "raw",
        StorageChoice::Refpack => "refpack",
    }
}

pub fn state_name(state: ChangeState) -> &'static str {
    match state {
        ChangeState::Pending => "pending",
        ChangeState::Applied => "applied",
        ChangeState::Conflict => "conflict",
    }
}

pub fn stream_state_name(state: StreamState) -> &'static str {
    match state {
        StreamState::Original => "original",
        StreamState::Applied => "applied",
        StreamState::ChangedExternally => "changed_externally",
        StreamState::Missing => "missing",
    }
}

/// An apply report as a dict: `streams` (`(name, action, replaced)`),
/// `written`, `backed_up`, `warnings`.
pub fn report_dict<'py>(py: Python<'py>, report: &ApplyReport) -> PyResult<Bound<'py, PyDict>> {
    let streams: Vec<(String, &str, usize)> = report
        .streams
        .iter()
        .map(|(name, action)| match action {
            StreamAction::Rebuilt(n) => (name.clone(), "rebuilt", *n),
            StreamAction::Restored => (name.clone(), "restored", 0),
        })
        .collect();
    let dict = PyDict::new(py);
    dict.set_item("streams", streams)?;
    dict.set_item("written", &report.written)?;
    dict.set_item("backed_up", &report.backed_up)?;
    dict.set_item("warnings", &report.warnings)?;
    Ok(dict)
}

/// Mod metadata from keyword arguments, with the CLI's defaults. Without
/// `deltas` every resource stays whole (`ModInfo::whole_resources`).
pub fn mod_info(
    name: &str,
    id: Option<String>,
    version: &str,
    author: &str,
    description: &str,
    url: Option<String>,
    deltas: bool,
) -> ModInfo {
    ModInfo {
        id: id.unwrap_or_else(|| mods::id_from_name(name)),
        name: name.to_owned(),
        version: version.to_owned(),
        author: author.to_owned(),
        description: description.to_owned(),
        url,
        relations: Default::default(),
        whole_resources: !deltas,
    }
}

/// Keys of the changes matching `keys` (exact keys or parts of keys); all when `None`.
/// The project whose own workspace `workspace` is: its changes are
/// compared with the game without the project's own mod.
fn rebase_project(workspace: &ws::Workspace) -> Option<doctor_toolkit::project::Project> {
    let root = doctor_toolkit::project::project_of_workspace(workspace.dir())?;
    doctor_toolkit::project::Project::open(root).ok()
}

pub fn match_keys(
    workspace: &ws::Workspace,
    keys: Option<Vec<String>>,
) -> PyResult<Option<Vec<ResourceKey>>> {
    let Some(args) = keys else {
        return Ok(None);
    };
    let mut found = Vec::new();
    for arg in &args {
        if let Ok(key) = arg.parse::<ResourceKey>()
            && workspace.change(&key).is_some()
        {
            found.push(key);
            continue;
        }
        let matching: Vec<_> = workspace
            .changes()
            .filter(|c| c.key.to_string().contains(arg.as_str()))
            .map(|c| c.key.clone())
            .collect();
        if matching.is_empty() {
            return Err(PyKeyError::new_err(format!("no change matches `{arg}`")));
        }
        found.extend(matching);
    }
    found.sort();
    found.dedup();
    Ok(Some(found))
}

#[pymethods]
impl Workspace {
    /// The workspace directory.
    #[getter]
    fn dir(&self) -> PathBuf {
        self.ws.dir().to_owned()
    }

    /// Number of changed resources.
    fn __len__(&self) -> usize {
        self.ws.len()
    }

    /// Whether the workspace changes the resource (a key or a `Resource`).
    fn __contains__(&self, key: &Bound<'_, PyAny>) -> PyResult<bool> {
        Ok(self.ws.change(&key_of(key)?).is_some())
    }

    /// The changes as dicts: `key`, `label`, `original_size`, `new_size`,
    /// `original_sha256`, `new_sha256`, `storage`.
    #[getter]
    fn changes<'py>(&self, py: Python<'py>) -> PyResult<Vec<Bound<'py, PyDict>>> {
        self.ws
            .changes()
            .map(|c| {
                let dict = PyDict::new(py);
                dict.set_item("key", c.key.to_string())?;
                dict.set_item("label", &c.label)?;
                dict.set_item("original_size", c.original.size)?;
                dict.set_item("new_size", c.payload.size)?;
                dict.set_item("original_sha256", &c.original.sha256)?;
                dict.set_item("new_sha256", &c.payload.sha256)?;
                dict.set_item("storage", storage_name(c.storage))?;
                Ok(dict)
            })
            .collect()
    }

    /// Keys of the changed resources.
    #[getter]
    fn keys(&self) -> Vec<String> {
        self.ws.changes().map(|c| c.key.to_string()).collect()
    }

    /// The operations of the history, oldest first, as dicts: `seq`, `time`
    /// (Unix seconds), `label`, `keys`, `in_effect` (`False` for undone ones).
    #[getter]
    fn history<'py>(&self, py: Python<'py>) -> PyResult<Vec<Bound<'py, PyDict>>> {
        self.ws
            .operations()
            .iter()
            .enumerate()
            .map(|(i, op)| {
                let dict = PyDict::new(py);
                dict.set_item("seq", op.seq)?;
                dict.set_item("time", op.time)?;
                dict.set_item("label", &op.label)?;
                let keys: Vec<String> = op.edits.iter().map(|e| e.key.to_string()).collect();
                dict.set_item("keys", keys)?;
                dict.set_item("in_effect", i < self.ws.head())?;
                Ok(dict)
            })
            .collect()
    }

    /// Label of the operation `undo` would undo, or `None`.
    #[getter]
    fn undo_label(&self) -> Option<String> {
        self.ws.undo_label().map(str::to_owned)
    }

    /// Label of the operation `redo` would redo, or `None`.
    #[getter]
    fn redo_label(&self) -> Option<String> {
        self.ws.redo_label().map(str::to_owned)
    }

    /// The edited data of a resource, or `None` when the workspace does not change it.
    fn data<'py>(
        &self,
        py: Python<'py>,
        key: &Bound<'_, PyAny>,
    ) -> PyResult<Option<Bound<'py, PyBytes>>> {
        let key = key_of(key)?;
        Ok(self.ws.data(&key).py()?.map(|d| util::bytes(py, &d)))
    }

    /// The original data of a changed resource, or `None` when it is not changed.
    fn original_data<'py>(
        &self,
        py: Python<'py>,
        key: &Bound<'_, PyAny>,
    ) -> PyResult<Option<Bound<'py, PyBytes>>> {
        let key = key_of(key)?;
        Ok(self
            .ws
            .original_data(&key)
            .py()?
            .map(|d| util::bytes(py, &d)))
    }

    /// The current data of a resource: the edited data, or the game's when it is not changed.
    fn read<'py>(&self, py: Python<'py>, key: &Bound<'_, PyAny>) -> PyResult<Bound<'py, PyBytes>> {
        let key = key_of(key)?;
        Ok(util::bytes(py, &self.current(py, &key)?))
    }

    /// Records new data for a resource as one undoable operation. Data equal
    /// to the original removes the change. `storage` is `keep` (like the
    /// original), `raw` or `refpack`. Returns whether anything changed.
    #[pyo3(signature = (key, data, label = None, storage = "keep"))]
    fn set_data(
        &mut self,
        py: Python<'_>,
        key: &Bound<'_, PyAny>,
        data: Vec<u8>,
        label: Option<&str>,
        storage: &str,
    ) -> PyResult<bool> {
        let key = key_of(key)?;
        let storage = storage_choice(storage)?;
        let label = label.map_or_else(|| format!("Replace {key}"), str::to_owned);
        let stream = key.stream.clone();
        self.record(py, &label, &stream, vec![(key, data, storage)])
    }

    /// Records new data for several resources: `{key: bytes}`. The resources
    /// of each stream change in one undoable operation. Returns the number of
    /// operations recorded.
    #[pyo3(signature = (edits, label = None, storage = "keep"))]
    fn set_many(
        &mut self,
        py: Python<'_>,
        edits: BTreeMap<String, Vec<u8>>,
        label: Option<&str>,
        storage: &str,
    ) -> PyResult<usize> {
        let storage = storage_choice(storage)?;
        let mut by_stream: BTreeMap<String, Vec<(ResourceKey, Vec<u8>, StorageChoice)>> =
            BTreeMap::new();
        for (key, data) in edits {
            let key = util::parse_key(&key)?;
            by_stream
                .entry(key.stream.clone())
                .or_default()
                .push((key, data, storage));
        }
        let mut recorded = 0;
        for (stream, edits) in by_stream {
            let label =
                label.map_or_else(|| format!("Replace resources in {stream}"), str::to_owned);
            if self.record(py, &label, &stream, edits)? {
                recorded += 1;
            }
        }
        Ok(recorded)
    }

    /// Records a texture imported from a PNG or DDS file (recognised by its
    /// signature): it is encoded to the original format with a full mip chain;
    /// the `tg4h` and `tg4d` change in one operation. `key` is either resource
    /// of the texture. Returns the new texture.
    fn import_texture(
        &mut self,
        py: Python<'_>,
        key: &Bound<'_, PyAny>,
        image: Vec<u8>,
    ) -> PyResult<Texture> {
        let key = key_of(key)?;
        let (loc, index) = Loc::locate(&self.game, &key)?;
        let pair = formats::pair(&loc, index)?;
        let keys = (
            loc.key(pair.header).to_string(),
            loc.key(pair.data).to_string(),
        );
        let stream_index = loc.stream_index;
        drop(loc);
        let is_png = formats::is_png(&image).py()?;
        let game = &self.game;
        let workspace = &mut self.ws;
        let texture = py
            .detach(|| workspace.import_texture(game, stream_index, pair, &image, is_png))
            .py()?;
        Ok(texture_with_keys(texture, keys))
    }

    /// Imports a model file (`.glb`, `.gltf`, `.fbx`, `.obj`, `.usd`, `.usda`,
    /// `.usdc` or `.usdz`) over the mesh or
    /// metamodel `key`, or with `new_name` as a new model set (clones under
    /// new ids); `retarget` lists resource indices of the same stream
    /// (SimGroup, metamodel) to point at the new set. Records one undoable
    /// operation unless `dry_run`. Returns the summary: parts, records,
    /// skeleton and bones mapped, materials, added resources, warnings.
    #[pyo3(signature = (key, path, new_name = None, retarget = Vec::new(), dry_run = false))]
    fn import_model<'py>(
        &mut self,
        py: Python<'py>,
        key: &Bound<'_, PyAny>,
        path: PathBuf,
        new_name: Option<String>,
        retarget: Vec<usize>,
        dry_run: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        use doctor_toolkit::import::model;
        let key = key_of(key)?;
        let (loc, index) = Loc::locate(&self.game, &key)?;
        let stream_index = loc.stream_index;
        drop(loc);
        let game = &self.game;
        let workspace = &mut self.ws;
        let summary = py
            .detach(|| -> doctor_toolkit::Result<model::ImportSummary> {
                let gltf = doctor_toolkit::scene::read_file(&path)?;
                let (resource_index, _) = game.build_index(|_| {});
                let mut assets =
                    doctor_toolkit::game::GameAssets::new(game.clone(), &resource_index);
                let plan = model::plan(
                    game,
                    workspace,
                    &resource_index,
                    &mut assets,
                    stream_index,
                    index,
                    &gltf,
                    &model::ImportOptions {
                        new_name,
                        retarget,
                        format: None,
                        collision: None,
                        this_stream_only: false,
                    },
                )?;
                if !dry_run {
                    plan.commit(workspace, game)?;
                }
                Ok(plan.summary)
            })
            .py()?;
        util::to_python(py, &summary)
    }

    /// The model of a `mesh` or `metamodel` resource (its first mesh) as the
    /// workspace has it, as a binary glTF (`.glb`) with materials, textures
    /// and skeleton — the file `import_model` takes back. Bone and material
    /// names are the ones the import matches by. `index` (`Game.index()`)
    /// finds materials and textures quickly; without it the game is indexed.
    #[pyo3(signature = (key, index = None))]
    fn export_model<'py>(
        &self,
        py: Python<'py>,
        key: &Bound<'_, PyAny>,
        index: Option<PyRef<'py, crate::navigation::Index>>,
    ) -> PyResult<Bound<'py, PyBytes>> {
        use doctor_toolkit::external::{self, Format, Target};
        let key = key_of(key)?;
        let game = &self.game;
        let ws = &self.ws;
        let glb = match &index {
            Some(index) => {
                let (_, idx, maps) = index.parts();
                py.detach(|| {
                    let target = Target::find(game, &key, Some(Format::Glb))?;
                    let mut assets =
                        doctor_toolkit::navigation::unpack::IndexAssets::new(game, idx, maps);
                    external::export(game, Some(ws), &target, Some(&mut assets))
                })
            }
            None => py.detach(|| {
                let target = Target::find(game, &key, Some(Format::Glb))?;
                let (resources, _) = game.build_index(|_| {});
                let mut assets = doctor_toolkit::game::GameAssets::new(game.clone(), &resources);
                external::export(game, Some(ws), &target, Some(&mut assets))
            }),
        }
        .py()?;
        Ok(util::bytes(py, &glb))
    }

    /// Imports an animation (`.glb` / `.gltf`) into the animation bank `key`:
    /// over the clip `clip` (name or hexadecimal hash), or with `add=True` as
    /// a new clip named `clip` with a named entry that plays it. `animation`
    /// picks an animation of the file (name or index), `looping` overrides
    /// the clip's flag, `max_angle` (radians) and `max_shift` (metres) bound
    /// the encoding error. Records one undoable operation unless `dry_run`.
    /// Returns the summary: frames, bones mapped, error, sizes, warnings.
    #[pyo3(signature = (key, path, clip, add = false, animation = None, looping = None,
        scheme = 4, max_angle = 0.002, max_shift = 0.0005, dry_run = false))]
    #[allow(clippy::too_many_arguments)]
    fn import_animation<'py>(
        &mut self,
        py: Python<'py>,
        key: &Bound<'_, PyAny>,
        path: PathBuf,
        clip: String,
        add: bool,
        animation: Option<String>,
        looping: Option<bool>,
        scheme: u32,
        max_angle: f32,
        max_shift: f32,
        dry_run: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        use doctor_toolkit::import::anim::{
            ClipTarget, ImportOptions, import_clip, read_animations,
        };
        use doctor_toolkit::resource::anim_encode::EncodeOptions;
        let key = key_of(key)?;
        let data = self.current(py, &key)?;
        let (loc, index) = Loc::locate(&self.game, &key)?;
        let stream_index = loc.stream_index;
        drop(loc);
        let game = &self.game;
        let workspace = &mut self.ws;
        let summary = py
            .detach(|| -> doctor_toolkit::Result<_> {
                let animations = read_animations(&path)?;
                let source = match &animation {
                    None => animations.first(),
                    Some(a) => match a.parse::<usize>() {
                        Ok(i) => animations.get(i),
                        Err(_) => animations.iter().find(|x| x.name.eq_ignore_ascii_case(a)),
                    },
                }
                .ok_or_else(|| {
                    doctor_toolkit::Error::InvalidArgument("no such animation in the file".into())
                })?;
                let bank = doctor_toolkit::resource::anim::AnimBank::parse(&data)?;
                let target = if add {
                    ClipTarget::Add {
                        name: clip.clone(),
                        entry: true,
                        priority: 0,
                        like: None,
                    }
                } else {
                    let hash = doctor_toolkit::hash::sdbm(clip.to_ascii_lowercase().as_bytes());
                    let found = bank.clips.iter().position(|c| {
                        c.hash == hash
                            || c.name
                                .as_deref()
                                .is_some_and(|n| n.eq_ignore_ascii_case(&clip))
                            || u32::from_str_radix(clip.trim_start_matches("0x"), 16) == Ok(c.hash)
                    });
                    ClipTarget::Replace(found.ok_or_else(|| {
                        doctor_toolkit::Error::InvalidArgument(format!("no clip `{clip}`"))
                    })?)
                };
                let (resource_index, _) = game.build_index(|_| {});
                let mut assets =
                    doctor_toolkit::game::GameAssets::new(game.clone(), &resource_index);
                let rigs = doctor_toolkit::export::gltf::AssetSource::rigs(&mut assets).to_vec();
                let options = ImportOptions {
                    encode: EncodeOptions {
                        scheme,
                        max_angle,
                        max_translation: max_shift,
                    },
                    looping,
                    ..Default::default()
                };
                let (out, summary) = import_clip(&data, &rigs, source, &target, &options)?;
                if !dry_run {
                    let label = format!("Import animation {} into {key}", summary.clip);
                    workspace.set_resource(game, stream_index, index, out, &label)?;
                }
                Ok(summary)
            })
            .py()?;
        util::to_python(py, &summary)
    }

    /// Replaces the events of clip `clip` (name or hexadecimal hash) of the
    /// animation bank `key` with `events`: dicts `{frame, name, signature,
    /// args}` as `animation_bank` returns them (time in ticks, 2 per frame).
    /// Records one undoable operation.
    fn set_clip_events(
        &mut self,
        py: Python<'_>,
        key: &Bound<'_, PyAny>,
        clip: String,
        events: &Bound<'_, PyAny>,
    ) -> PyResult<bool> {
        use doctor_toolkit::resource::anim::{AnimBank, ClipEvent};
        use doctor_toolkit::resource::anim_image::BankImage;
        let key = key_of(key)?;
        let json = py
            .import("json")?
            .call_method1("dumps", (events,))?
            .extract::<String>()?;
        let events: Vec<ClipEvent> = serde_json::from_str(&json)
            .map_err(|e| crate::error::invalid(format!("events: {e}")))?;
        let data = self.current(py, &key)?;
        let bank = AnimBank::parse(&data).py()?;
        let hash = doctor_toolkit::hash::sdbm(clip.to_ascii_lowercase().as_bytes());
        let index = bank
            .clips
            .iter()
            .position(|c| {
                c.hash == hash
                    || u32::from_str_radix(clip.trim_start_matches("0x"), 16) == Ok(c.hash)
            })
            .ok_or_else(|| crate::error::invalid(format!("no clip `{clip}`")))?;
        let mut image = BankImage::parse(&data).py()?;
        image.set_events(index, &events).py()?;
        let out = image.encode().py()?;
        let label = format!("Edit events of {clip} in {key}");
        self.record(
            py,
            &label,
            &key.stream.clone(),
            vec![(key, out, StorageChoice::Keep)],
        )
    }

    /// Replaces the node graph of the animation bank `key` with `graph`: a
    /// dict with `entries` as `animation_graph` returns them. Refuses a graph
    /// the checks find errors in. Records one undoable operation.
    fn set_animation_graph(
        &mut self,
        py: Python<'_>,
        key: &Bound<'_, PyAny>,
        graph: &Bound<'_, PyAny>,
    ) -> PyResult<bool> {
        use doctor_toolkit::resource::anim_graph::{AnimGraph, GraphJson, Severity};
        use doctor_toolkit::resource::anim_image::BankImage;
        let key = key_of(key)?;
        let json = py
            .import("json")?
            .call_method1("dumps", (graph,))?
            .extract::<String>()?;
        let json: GraphJson = serde_json::from_str(&json)
            .map_err(|e| crate::error::invalid(format!("graph: {e}")))?;
        let graph = AnimGraph::from_json(&json).py()?;
        let data = self.current(py, &key)?;
        let mut image = BankImage::parse(&data).py()?;
        let errors: Vec<String> = graph
            .check(&image)
            .into_iter()
            .filter(|i| i.severity == Severity::Error)
            .map(|i| match i.node {
                Some(n) => format!("node {n}: {}", i.text),
                None => i.text,
            })
            .collect();
        if !errors.is_empty() {
            return Err(crate::error::invalid(format!(
                "the graph has errors: {}",
                errors.join("; ")
            )));
        }
        graph.store(&mut image).py()?;
        let out = image.encode().py()?;
        let label = format!("Edit the animation graph of {key}");
        self.record(
            py,
            &label,
            &key.stream.clone(),
            vec![(key, out, StorageChoice::Keep)],
        )
    }

    /// The string table of an `lh2` resource as the workspace has it (edited
    /// or original).
    fn string_table(&self, py: Python<'_>, key: &Bound<'_, PyAny>) -> PyResult<StringTable> {
        let key = key_of(key)?;
        let data = self.current(py, &key)?;
        Ok(StringTable {
            table: lh2::StringTable::parse(&data).py()?,
            key: Some(key.to_string()),
        })
    }

    /// Records an edited string table. `key` defaults to the resource the
    /// table was read from. Returns whether anything changed.
    #[pyo3(signature = (table, key = None))]
    fn set_string_table(
        &mut self,
        py: Python<'_>,
        table: PyRef<'_, StringTable>,
        key: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<bool> {
        let key = match (key, &table.key) {
            (Some(key), _) => key_of(key)?,
            (None, Some(key)) => util::parse_key(key)?,
            (None, None) => {
                return Err(invalid(
                    "the table was not read from a resource; pass `key`",
                ));
            }
        };
        let data = table.table.to_bytes().py()?;
        let label = format!("Edit strings in {}", key.stream);
        let stream = key.stream.clone();
        self.record(py, &label, &stream, vec![(key, data, StorageChoice::Keep)])
    }

    /// Sets strings of an `lh2` resource: `{hash: text}`, in column
    /// `language` (every column when `None`). Unknown hashes raise `KeyError`.
    /// Returns whether anything changed.
    #[pyo3(signature = (key, strings, language = None))]
    fn edit_strings(
        &mut self,
        py: Python<'_>,
        key: &Bound<'_, PyAny>,
        strings: BTreeMap<u32, String>,
        language: Option<usize>,
    ) -> PyResult<bool> {
        let key = key_of(key)?;
        let mut table = lh2::StringTable::parse(&self.current(py, &key)?).py()?;
        let columns = table.strings.len();
        if let Some(language) = language
            && language >= columns
        {
            return Err(PyValueError::new_err(format!(
                "the table has {columns} language column(s)"
            )));
        }
        for (hash, text) in strings {
            let entry = table
                .find(hash)
                .ok_or_else(|| PyKeyError::new_err(format!("{key} has no string {hash:08x}")))?;
            for (i, column) in table.strings.iter_mut().enumerate() {
                if language.is_none_or(|l| l == i) {
                    column[entry] = text.clone();
                }
            }
        }
        let data = table.to_bytes().py()?;
        let label = format!("Edit strings in {}", key.stream);
        let stream = key.stream.clone();
        self.record(py, &label, &stream, vec![(key, data, StorageChoice::Keep)])
    }

    /// Removes the changes of `keys` (exact keys or parts of keys) as one
    /// undoable operation; the game is not touched until `apply`. Returns
    /// whether anything changed.
    fn revert(&mut self, keys: Vec<String>) -> PyResult<bool> {
        let keys = match_keys(&self.ws, Some(keys))?.unwrap_or_default();
        let label = format!("Revert {} change(s)", keys.len());
        self.ws.revert(&keys, &label).py()
    }

    /// Keys of the changed files outside the streams: `viv:<archive entry>`
    /// (streamed sounds, written to an overlay archive) and `file:<path>`
    /// (movies).
    fn file_keys(&self) -> Vec<String> {
        self.ws.file_changes().map(|c| c.key.to_string()).collect()
    }

    /// The current contents of a file outside the streams (`viv:…` or
    /// `file:…`): the recorded change, or the game's original.
    fn file_data<'py>(&self, py: Python<'py>, key: &str) -> PyResult<Bound<'py, PyBytes>> {
        let key: ws::FileKey = key.parse().py()?;
        let data = py.detach(|| self.ws.current_file(&self.game, &key)).py()?;
        Ok(util::bytes(py, &data))
    }

    /// Records new contents of a file outside the streams as one undoable
    /// operation; data equal to the original removes the change.
    #[pyo3(signature = (key, data, label = None))]
    fn set_file(
        &mut self,
        py: Python<'_>,
        key: &str,
        data: Vec<u8>,
        label: Option<&str>,
    ) -> PyResult<()> {
        let key: ws::FileKey = key.parse().py()?;
        let label = label.map_or_else(|| format!("Replace {key}"), str::to_owned);
        let (game, workspace) = (&self.game, &mut self.ws);
        py.detach(|| workspace.set_file(game, &key, data, &label))
            .py()?;
        Ok(())
    }

    /// Replaces a sound with an audio file (WAV, or any format ffmpeg reads),
    /// converted to the channels, rate and loop of the old sound. `target` is
    /// a bank sound `<sbk key>#<sound>` or a streamed sound of the `.viv`
    /// archives (`audiostreams\...exa.snu`). Returns the new duration in seconds.
    fn import_sound(&mut self, py: Python<'_>, target: &str, path: PathBuf) -> PyResult<f64> {
        use doctor_toolkit::resource::audio::{Sample, parse_sbk};
        use doctor_toolkit::resource::audio_encode::{
            encode_like, encode_snu_like, replace_sbk_sound,
        };
        let pcm = py
            .detach(|| doctor_toolkit::resource::video::import::read_audio_file(&path))
            .py()?;
        if let Some((key, sound)) = target.rsplit_once('#') {
            let key = util::parse_key(key)?;
            let sound: u32 = sound
                .parse()
                .map_err(|_| PyValueError::new_err("the sound number after `#`"))?;
            let bank = self.current(py, &key)?;
            let sounds = parse_sbk(&bank).py()?;
            let position = sounds
                .iter()
                .position(|s| s.index == sound)
                .ok_or_else(|| PyKeyError::new_err(format!("{key} has no sound {sound}")))?;
            let old = sounds[position].sample.header;
            let (fitted, sample) = py.detach(|| encode_like(&pcm, &old)).py()?;
            let new_bank = replace_sbk_sound(&bank, position, &sample).py()?;
            let seconds = fitted.frames() as f64 / old.sample_rate as f64;
            let label = format!("Import sound {sound} of {} ({seconds:.2} s)", key.stream);
            let stream = key.stream.clone();
            self.record(
                py,
                &label,
                &stream,
                vec![(key, new_bank, StorageChoice::Keep)],
            )?;
            Ok(seconds)
        } else {
            let key = ws::FileKey::archive_entry(target);
            let (game, workspace) = (&self.game, &mut self.ws);
            py.detach(|| {
                let original = workspace.current_file(game, &key)?;
                let data = encode_snu_like(&pcm, &original)?;
                let header = Sample::parse_snu(&data)?.header;
                workspace.set_file(game, &key, data, &format!("Import sound {}", key.name()))?;
                Ok(header.sample_count as f64 / header.sample_rate as f64)
            })
            .py()
        }
    }

    /// Removes the changes of files outside the streams (`viv:…`, `file:…`).
    fn revert_files(&mut self, keys: Vec<String>) -> PyResult<bool> {
        let keys = keys
            .iter()
            .map(|k| k.parse::<ws::FileKey>())
            .collect::<Result<Vec<_>, _>>()
            .py()?;
        let label = format!("Revert {} file(s)", keys.len());
        self.ws.revert_files(&keys, &label).py()
    }

    /// Removes every change as one undoable operation.
    fn revert_all(&mut self) -> PyResult<bool> {
        self.ws.revert_all().py()
    }

    /// The changes made on other data than the game gives now: a mod under
    /// them was updated, or the mods of the game are others since the edits
    /// (a mod built of such changes takes back what changed). Dicts `key`,
    /// `kind` (`simgroup`, `lh2`, `partsdb`, `wgi`: rebased by its
    /// operations; `None`: kept or given up), `made_on_sha256`, `now_sha256`.
    fn stale<'py>(&self, py: Python<'py>) -> PyResult<Vec<Bound<'py, PyDict>>> {
        let (game, workspace) = (&self.game, &self.ws);
        let stale = py
            .detach(|| match rebase_project(workspace) {
                Some(project) => project.stale(game, workspace),
                None => workspace.stale(game),
            })
            .py()?;
        stale
            .iter()
            .map(|s| {
                let dict = PyDict::new(py);
                dict.set_item("key", s.key.to_string())?;
                dict.set_item("kind", s.kind.map(|kind| kind.name()))?;
                dict.set_item("made_on_sha256", &s.made_on.sha256)?;
                dict.set_item("now_sha256", &s.now.sha256)?;
                Ok(dict)
            })
            .collect()
    }

    /// Moves the stale changes (`stale()`) onto the data the game gives
    /// now, as one undoable operation: those of `only` (exact keys or parts
    /// of keys; all when `None`) are merged by their operations, those of
    /// `keep` stay as they are, those of `take_base` are removed. Returns a
    /// dict: `recorded` and `changes` — dicts `key`, `outcome` (`rebased`,
    /// `kept`, `taken`, `undecided`, `not_stale`), and for a rebased one
    /// `applied`, `skipped` and `conflicts` (lists of texts that name the
    /// object or string), `removed`; for an undecided one `reason`.
    #[pyo3(signature = (only = None, keep = None, take_base = None))]
    fn rebase<'py>(
        &mut self,
        py: Python<'py>,
        only: Option<Vec<String>>,
        keep: Option<Vec<String>>,
        take_base: Option<Vec<String>>,
    ) -> PyResult<Bound<'py, PyDict>> {
        let only = match_keys(&self.ws, only)?;
        let keep = match_keys(&self.ws, keep)?.unwrap_or_default();
        let take_base = match_keys(&self.ws, take_base)?.unwrap_or_default();
        let (game, workspace) = (&self.game, &mut self.ws);
        let report = py
            .detach(|| {
                let request = ws::RebaseRequest {
                    keys: only.as_deref(),
                    keep: &keep,
                    take_base: &take_base,
                };
                match rebase_project(workspace) {
                    Some(project) => project.rebase(game, workspace, &request),
                    None => workspace.rebase_with(game, &request),
                }
            })
            .py()?;
        let changes = report
            .changes
            .iter()
            .map(|c| {
                let dict = PyDict::new(py);
                dict.set_item("key", c.key.to_string())?;
                let outcome = match &c.outcome {
                    ws::RebaseOutcome::Merged {
                        applied,
                        skipped,
                        conflicts,
                        removed,
                    } => {
                        let texts = |notes: &[doctor_toolkit::resource::delta::Note]| {
                            notes.iter().map(ToString::to_string).collect::<Vec<_>>()
                        };
                        dict.set_item("applied", applied)?;
                        dict.set_item("skipped", texts(skipped))?;
                        dict.set_item("conflicts", texts(conflicts))?;
                        dict.set_item("removed", removed)?;
                        "rebased"
                    }
                    ws::RebaseOutcome::Kept => "kept",
                    ws::RebaseOutcome::Taken => "taken",
                    ws::RebaseOutcome::Undecided { reason } => {
                        dict.set_item("reason", reason)?;
                        "undecided"
                    }
                    ws::RebaseOutcome::NotStale => "not_stale",
                };
                dict.set_item("outcome", outcome)?;
                Ok(dict)
            })
            .collect::<PyResult<Vec<_>>>()?;
        let dict = PyDict::new(py);
        dict.set_item("recorded", report.seq.is_some())?;
        dict.set_item("changes", changes)?;
        Ok(dict)
    }

    /// Undoes the last operation; returns its label, or `None` when there is nothing to undo.
    fn undo(&mut self) -> PyResult<Option<String>> {
        self.ws.undo().py()
    }

    /// Redoes the last undone operation; returns its label, or `None`.
    fn redo(&mut self) -> PyResult<Option<String>> {
        self.ws.redo().py()
    }

    /// Re-reads the workspace from disk (after another program changed it).
    fn reload(&mut self) -> PyResult<()> {
        self.ws.reload().py()
    }

    /// Compares the workspace with the game files: `fingerprint_matches`,
    /// `up_to_date`, `changes` (`(key, state)` with state `pending`, `applied`
    /// or `conflict`), `streams` (dicts `stream`, `state`, `up_to_date`),
    /// `pending_reverts` (keys), `conflicts` (streams changed by another program).
    fn status<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let game = &self.game;
        let workspace = &self.ws;
        let status = py.detach(|| workspace.status(game)).py()?;
        let dict = PyDict::new(py);
        dict.set_item("fingerprint_matches", status.fingerprint_matches)?;
        dict.set_item("up_to_date", status.is_up_to_date())?;
        let changes: Vec<(String, &str)> = status
            .changes
            .iter()
            .map(|(k, s)| (k.to_string(), state_name(*s)))
            .collect();
        dict.set_item("changes", changes)?;
        let streams = status
            .streams
            .iter()
            .map(|s| {
                let item = PyDict::new(py);
                item.set_item("stream", &s.stream)?;
                item.set_item("state", stream_state_name(s.state))?;
                item.set_item("up_to_date", s.up_to_date)?;
                Ok(item)
            })
            .collect::<PyResult<Vec<_>>>()?;
        dict.set_item("streams", streams)?;
        let reverts: Vec<String> = status
            .pending_reverts
            .iter()
            .map(|k| k.to_string())
            .collect();
        dict.set_item("pending_reverts", reverts)?;
        dict.set_item("conflicts", status.conflicts())?;
        Ok(dict)
    }

    /// Every change compared with its original: dicts `key`, `label`,
    /// `original_size`, `new_size`, `changed_bytes`, `ranges` (changed byte
    /// ranges, at most 64), and for string tables `strings` (`changed`:
    /// `(hash, language, old, new)`, `added`, `removed`), for textures
    /// `texture` (`name`, `old`, `new`). `filter` keeps keys containing the text.
    #[pyo3(signature = (filter = None))]
    fn diff<'py>(
        &self,
        py: Python<'py>,
        filter: Option<&str>,
    ) -> PyResult<Vec<Bound<'py, PyDict>>> {
        let mut out = Vec::new();
        for entry in self.ws.diff() {
            let key = entry.key.to_string();
            if filter.is_some_and(|f| !key.contains(f)) {
                continue;
            }
            let dict = PyDict::new(py);
            dict.set_item("key", &key)?;
            dict.set_item("label", &entry.label)?;
            dict.set_item("original_size", entry.original_size)?;
            dict.set_item("new_size", entry.new_size)?;
            let workspace = &self.ws;
            let detail = py.detach(|| workspace.detail(&entry.key)).py()?;
            if let Some(detail) = detail {
                dict.set_item("changed_bytes", detail.bytes.changed_bytes)?;
                let ranges: Vec<(usize, usize)> = detail
                    .bytes
                    .ranges
                    .iter()
                    .map(|r| (r.start, r.end))
                    .collect();
                dict.set_item("ranges", ranges)?;
                dict.set_item("ranges_truncated", detail.bytes.truncated)?;
                match detail.semantic {
                    Some(SemanticDiff::Strings(strings)) => {
                        let item = PyDict::new(py);
                        let changed: Vec<(u32, String, String, String)> = strings
                            .changed
                            .into_iter()
                            .map(|c| (c.hash, c.language_name, c.old, c.new))
                            .collect();
                        item.set_item("changed", changed)?;
                        item.set_item("added", strings.added)?;
                        item.set_item("removed", strings.removed)?;
                        dict.set_item("strings", item)?;
                    }
                    Some(SemanticDiff::Texture(texture)) => {
                        let item = PyDict::new(py);
                        item.set_item("name", texture.name)?;
                        item.set_item("old", texture.old.to_string())?;
                        item.set_item("new", texture.new.to_string())?;
                        dict.set_item("texture", item)?;
                    }
                    Some(SemanticDiff::Logic(logic)) => {
                        let item = PyDict::new(py);
                        let values: Vec<(String, String, String)> = logic
                            .values
                            .into_iter()
                            .map(|v| (v.place, v.old, v.new))
                            .collect();
                        item.set_item("changed", logic.changed)?;
                        item.set_item("values", values)?;
                        item.set_item("truncated", logic.truncated)?;
                        item.set_item("other", logic.other)?;
                        dict.set_item("logic", item)?;
                    }
                    Some(SemanticDiff::Apx(apx)) => {
                        let item = PyDict::new(py);
                        let blocks: Vec<(String, usize, Option<usize>, Option<usize>)> = apx
                            .blocks
                            .into_iter()
                            .map(|b| (b.tag, b.ordinal, b.old_size, b.new_size))
                            .collect();
                        let textures: Vec<(String, String, String)> = apx
                            .textures
                            .into_iter()
                            .map(|t| (t.name, t.old.to_string(), t.new.to_string()))
                            .collect();
                        item.set_item("blocks", blocks)?;
                        item.set_item("textures", textures)?;
                        dict.set_item("apx", item)?;
                    }
                    None => {}
                }
            }
            out.push(dict);
        }
        Ok(out)
    }

    /// Writes the changes into the game: every stream that differs from the
    /// workspace is rebuilt from its original with all its changes and its
    /// size updated in the tables of contents; files are backed up first and
    /// replaced atomically. `streams` limits it to some streams.
    ///
    /// Refused (`InvalidArgumentError`) for streams changed by another
    /// program unless `force`, and (`ModError`) while the mod manager has
    /// mods in the game — the same checks as `doctor-toolkit workspace apply`.
    /// Returns a report: `streams` (`(name, action, replaced)`), `written`,
    /// `backed_up`, `warnings`.
    #[pyo3(signature = (streams = None, force = false))]
    fn apply<'py>(
        &mut self,
        py: Python<'py>,
        streams: Option<Vec<String>>,
        force: bool,
    ) -> PyResult<Bound<'py, PyDict>> {
        let game = &self.game;
        let workspace = &mut self.ws;
        let options = ApplyOptions { force, streams };
        let report = py.detach(|| workspace.apply(game, options)).py()?;
        report_dict(py, &report)
    }

    /// Puts the original files back into the game; the changes stay in the
    /// workspace, pending. Files changed by another program are only
    /// restored with `force`.
    #[pyo3(signature = (force = false))]
    fn unapply<'py>(&mut self, py: Python<'py>, force: bool) -> PyResult<Bound<'py, PyDict>> {
        let game = &self.game;
        let workspace = &mut self.ws;
        let report = py.detach(|| workspace.unapply(game, force)).py()?;
        report_dict(py, &report)
    }

    /// Builds a mod (`.zip`) from the changes (all, or those matching `keys`)
    /// and writes it to `out`. `id` defaults to one made from `name`;
    /// `preview` is a PNG or JPEG picture. Added resources and new streams
    /// go into the mod too (format version 2). A changed SimGroup, string
    /// table, parts database or world grid goes as a delta — the operations
    /// that make the change, which the mod manager merges with other mods
    /// (format version 6) — unless `deltas` is false, which keeps every
    /// resource whole for older mod managers. Returns `resources`, `added`,
    /// `new_streams`, `deltas` (resources stored as deltas),
    /// `format_version`, `payload_bytes`, `file_size`, `notes`.
    #[pyo3(signature = (out, name, *, id = None, version = "1.0", author = "", description = "", url = None, keys = None, preview = None, deltas = true))]
    #[allow(clippy::too_many_arguments)]
    fn build_mod<'py>(
        &self,
        py: Python<'py>,
        out: PathBuf,
        name: &str,
        id: Option<String>,
        version: &str,
        author: &str,
        description: &str,
        url: Option<String>,
        keys: Option<Vec<String>>,
        preview: Option<Vec<u8>>,
        deltas: bool,
    ) -> PyResult<Bound<'py, PyDict>> {
        let info = mod_info(name, id, version, author, description, url, deltas);
        let keys = match_keys(&self.ws, keys)?;
        let preview = preview.map(Preview::from_image).transpose().py()?;
        let workspace = &self.ws;
        let report = py
            .detach(|| {
                mods::build_from_workspace(
                    workspace,
                    &info,
                    keys.as_deref(),
                    preview.as_ref(),
                    &out,
                )
            })
            .py()?;
        let dict = PyDict::new(py);
        dict.set_item("id", &info.id)?;
        dict.set_item("resources", report.resources)?;
        dict.set_item("added", report.added)?;
        dict.set_item("new_streams", report.new_streams)?;
        dict.set_item("deltas", report.deltas)?;
        dict.set_item("format_version", report.format_version)?;
        dict.set_item("payload_bytes", report.payload_bytes)?;
        dict.set_item("file_size", report.file_size)?;
        dict.set_item("notes", &report.notes)?;
        Ok(dict)
    }

    fn __repr__(&self) -> String {
        format!(
            "Workspace({:?}, {} change(s))",
            self.ws.dir().display().to_string(),
            self.ws.len()
        )
    }
}

fn texture_with_keys(
    texture: doctor_toolkit::resource::texture::Texture,
    keys: (String, String),
) -> Texture {
    Texture {
        texture,
        keys: Some(keys),
    }
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Workspace>()?;
    Ok(())
}

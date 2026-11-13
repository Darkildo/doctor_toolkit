//! `Index`: the game index with names and references, search and the name dictionary.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::navigation::index::default_cache_file;
use doctor_toolkit::navigation::unpack::AssetMaps;
use doctor_toolkit::navigation::{Dictionary, GameIndex, OpenReport, Query, RecordId, search};
use pyo3::exceptions::PyKeyError;
use pyo3::prelude::*;
use pyo3::types::PyDict;

use crate::error::{ResultExt, invalid};
use crate::game::key_of;
use crate::util::{lookup_in, type_name};

/// Every resource of a game with the names read from its data and its
/// references to other resources. Built by `Game.index()`.
#[pyclass(module = "doctor_toolkit")]
pub struct Index {
    game: Arc<GameInstance>,
    index: GameIndex,
    cache: Option<PathBuf>,
    report: OpenReport,
    maps: OnceLock<AssetMaps>,
    dictionary: OnceLock<Dictionary>,
}

impl Index {
    /// Builds or loads the index of `game`, with the GIL released.
    pub fn open(py: Python<'_>, game: Arc<GameInstance>, cache: Option<PathBuf>) -> Self {
        let (index, report) = py.detach(|| GameIndex::open(&game, cache.as_deref(), &|_, _| {}));
        Self {
            game,
            index,
            cache,
            report,
            maps: OnceLock::new(),
            dictionary: OnceLock::new(),
        }
    }

    /// The default cache file of a game's index.
    pub fn default_cache(game: &GameInstance) -> Option<PathBuf> {
        default_cache_file(game.root())
    }

    /// The game, the index and the asset maps for model export.
    pub fn parts(&self) -> (&GameInstance, &GameIndex, &AssetMaps) {
        let maps = self.maps.get_or_init(|| AssetMaps::new(&self.index));
        (&self.game, &self.index, maps)
    }

    pub fn game_index(&self) -> &GameIndex {
        &self.index
    }

    fn dictionary(&self) -> &Dictionary {
        self.dictionary
            .get_or_init(|| Dictionary::for_game(&self.index))
    }

    fn find(&self, key: &Bound<'_, PyAny>) -> PyResult<RecordId> {
        let key = key_of(key)?;
        self.index
            .find_key(&key)
            .ok_or_else(|| PyKeyError::new_err(format!("resource {key} not found")))
    }

    fn brief<'py>(&self, py: Python<'py>, id: RecordId) -> PyResult<Bound<'py, PyDict>> {
        let record = self.index.record(id);
        let dict = PyDict::new(py);
        dict.set_item("key", self.index.key(id).to_string())?;
        dict.set_item("label", self.index.label(id))?;
        dict.set_item("type", type_name(record.header.type_id))?;
        dict.set_item("stream", self.index.stream_name(self.index.stream_of(id)))?;
        dict.set_item("index", record.resource)?;
        Ok(dict)
    }
}

#[pymethods]
impl Index {
    /// Number of resources.
    fn __len__(&self) -> usize {
        self.index.len()
    }

    /// Number of streams.
    #[getter]
    fn stream_count(&self) -> usize {
        self.index.stream_count()
    }

    /// The cache file, or `None` when the index is not cached.
    #[getter]
    fn cache_file(&self) -> Option<PathBuf> {
        self.cache.clone()
    }

    /// What opening or the last refresh did: `reused`, `rebuilt`, `removed`
    /// streams, `saved` (cache path or `None`), `cache_error`.
    #[getter]
    fn report<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let dict = PyDict::new(py);
        dict.set_item("reused", self.report.reused)?;
        dict.set_item("rebuilt", self.report.rebuilt)?;
        dict.set_item("removed", self.report.removed)?;
        dict.set_item("saved", self.report.saved.clone())?;
        dict.set_item("cache_error", self.report.cache_error.clone())?;
        Ok(dict)
    }

    /// Number of resources whose data could not be read or parsed.
    #[getter]
    fn record_errors(&self) -> usize {
        self.index.record_errors()
    }

    /// Re-reads the streams changed since the index was built (after
    /// `Workspace.apply`, for example) and updates the cache.
    fn refresh(&mut self, py: Python<'_>) {
        let game = &self.game;
        let index = &mut self.index;
        let cache = self.cache.as_deref();
        self.report = py.detach(|| index.refresh(game, cache, &|_, _| {}));
        self.maps = OnceLock::new();
        self.dictionary = OnceLock::new();
    }

    /// Searches the game. `query` is a line of words as in the command line
    /// and the editor: names (`wheel`, `"rear wheel"`), globs (`car_*`),
    /// hashes (`0x9cf57f36`), keys, and filters `type:mesh,material`,
    /// `stream:mp_mall`, `kind:object`, `text:Don Corleone`.
    ///
    /// Returns at most `limit` results as dicts: `key`, `label`, `type`,
    /// `stream`, `index`, `hits` (why it matched).
    #[pyo3(signature = (query, limit = 1000))]
    fn search<'py>(
        &self,
        py: Python<'py>,
        query: &str,
        limit: usize,
    ) -> PyResult<Vec<Bound<'py, PyDict>>> {
        let query = Query::parse(query).py()?;
        if query.is_empty() {
            return Err(invalid("empty search"));
        }
        let index = &self.index;
        let (results, _) = py.detach(|| search::search(index, &query, limit));
        results
            .iter()
            .map(|result| {
                let dict = self.brief(py, result.record)?;
                let hits: Vec<String> = result.hits.iter().map(|h| h.describe()).collect();
                dict.set_item("hits", hits)?;
                Ok(dict)
            })
            .collect()
    }

    /// Number of resources matching `query` (all of them, not limited).
    fn count(&self, py: Python<'_>, query: &str) -> PyResult<usize> {
        let query = Query::parse(query).py()?;
        if query.is_empty() {
            return Err(invalid("empty search"));
        }
        let index = &self.index;
        Ok(py.detach(|| search::search(index, &query, 0)).1)
    }

    /// Everything the index knows about one resource (a key or a `Resource`):
    /// `key`, `label`, `type`, `stream`, `index`, `data_size`, `names`
    /// (`(kind, name)`), `hashes` (`(kind, hash)`), `texts` of string tables,
    /// `error` (why the data could not be read, or `None`).
    fn record<'py>(&self, py: Python<'py>, key: &Bound<'_, PyAny>) -> PyResult<Bound<'py, PyDict>> {
        let id = self.find(key)?;
        let record = self.index.record(id);
        let dict = self.brief(py, id)?;
        dict.set_item("data_size", record.header.data_size)?;
        let names: Vec<(&str, &str)> = record
            .names
            .iter()
            .map(|(kind, name)| (kind.label(), name.as_str()))
            .collect();
        dict.set_item("names", names)?;
        let hashes: Vec<(&str, u32)> = record
            .hashes
            .iter()
            .map(|(kind, hash)| (kind.label(), *hash))
            .collect();
        dict.set_item("hashes", hashes)?;
        let texts: Vec<(u32, &str, &str)> = record
            .texts
            .iter()
            .map(|t| {
                let language = record
                    .languages
                    .get(t.language as usize)
                    .map_or("", String::as_str);
                (t.hash, language, t.text.as_str())
            })
            .collect();
        dict.set_item("texts", texts)?;
        dict.set_item("error", record.error.clone())?;
        Ok(dict)
    }

    /// References of one resource (a key or a `Resource`):
    /// `uses` — outgoing references as dicts `label`, `target` (description),
    /// `resolved` (keys of the resources it points at);
    /// `used_by` — resources that reference it, as dicts `key`, `label`, `type`,
    /// `stream`, `index`, `ref` (the label of the reference);
    /// `copies` — keys of the same resource in other streams.
    fn refs<'py>(&self, py: Python<'py>, key: &Bound<'_, PyAny>) -> PyResult<Bound<'py, PyDict>> {
        let id = self.find(key)?;
        let dict = self.brief(py, id)?;
        let uses = self
            .index
            .uses(id)
            .map(|(reference, targets)| {
                let item = PyDict::new(py);
                item.set_item("label", &reference.label)?;
                item.set_item("target", reference.target.describe())?;
                let resolved: Vec<String> = targets
                    .iter()
                    .map(|&t| self.index.key(t).to_string())
                    .collect();
                item.set_item("resolved", resolved)?;
                Ok(item)
            })
            .collect::<PyResult<Vec<_>>>()?;
        dict.set_item("uses", uses)?;
        let used_by = self
            .index
            .used_by(id)
            .iter()
            .map(|&(source, k)| {
                let item = self.brief(py, source)?;
                item.set_item("ref", &self.index.record(source).refs[k as usize].label)?;
                Ok(item)
            })
            .collect::<PyResult<Vec<_>>>()?;
        dict.set_item("used_by", used_by)?;
        let copies: Vec<String> = self
            .index
            .copies(id)
            .into_iter()
            .map(|c| self.index.key(c).to_string())
            .collect();
        dict.set_item("copies", copies)?;
        Ok(dict)
    }

    /// Keys of every resource, optionally of one type or one stream.
    #[pyo3(signature = (r#type = None, stream = None))]
    fn keys(
        &self,
        r#type: Option<&Bound<'_, PyAny>>,
        stream: Option<&str>,
    ) -> PyResult<Vec<String>> {
        let type_id = r#type.map(crate::util::resolve_type).transpose()?;
        let stream = stream.map(doctor_toolkit::workspace::key::normalize_stream);
        Ok(self
            .index
            .ids()
            .filter(|&id| {
                type_id.is_none_or(|t| self.index.record(id).header.type_id == t)
                    && stream
                        .as_deref()
                        .is_none_or(|s| self.index.stream_name(self.index.stream_of(id)) == s)
            })
            .map(|id| self.index.key(id).to_string())
            .collect())
    }

    /// Names known for a 32-bit hash: the builtin lists plus every name and
    /// stored hash/name pair of the game, as `(name, origin)`.
    fn lookup(&self, py: Python<'_>, hash: u32) -> Vec<(String, String)> {
        let dict = py.detach(|| self.dictionary());
        lookup_in(dict, hash)
    }

    fn __repr__(&self) -> String {
        format!(
            "Index({:?}, {} resource(s))",
            self.game.root().display().to_string(),
            self.index.len()
        )
    }
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Index>()?;
    Ok(())
}

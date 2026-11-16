//! `ModManager` and `mod_info`: thin wrappers of the library's mod manager.

use std::path::PathBuf;
use std::sync::Arc;

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::mods::manager::{Clash, InstallReport, ModApplyOptions};
use doctor_toolkit::mods::vpat::{self, VpatPatch};
use doctor_toolkit::mods::{ModArchive, ModManager as LibModManager};
use doctor_toolkit::workspace::ResourceKey;
use pyo3::prelude::*;
use pyo3::types::PyDict;

use crate::error::{ResultExt, to_py};
use crate::util;
use crate::workspace::{
    Workspace, match_keys, mod_info, report_dict, state_name, stream_state_name,
};

/// The installed mods of a game (`<game>/_doctor_toolkit/mods`), in load
/// order; later mods win. Nothing is written into the game until `apply`.
///
/// Opened by `Game.mods()`.
#[pyclass(module = "doctor_toolkit")]
pub struct ModManager {
    game: Arc<GameInstance>,
    manager: LibModManager,
}

impl ModManager {
    pub fn open(py: Python<'_>, game: Arc<GameInstance>) -> PyResult<Self> {
        let manager = py.detach(|| LibModManager::open(&game)).py()?;
        Ok(Self { game, manager })
    }
}

fn install_dict<'py>(py: Python<'py>, report: &InstallReport) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    dict.set_item("id", &report.id)?;
    dict.set_item("name", &report.name)?;
    dict.set_item("replaced_version", &report.replaced_version)?;
    dict.set_item("resources", report.resources)?;
    dict.set_item("added", report.added)?;
    dict.set_item("new_streams", report.new_streams)?;
    dict.set_item("warnings", &report.warnings)?;
    let issues: Vec<String> = report.issues.iter().map(|i| i.describe()).collect();
    dict.set_item("issues", issues)?;
    Ok(dict)
}

#[pymethods]
impl ModManager {
    /// The mod manager directory.
    #[getter]
    fn dir(&self) -> PathBuf {
        self.manager.dir().to_owned()
    }

    /// Installed mods in load order, as dicts: `id`, `name`, `version`,
    /// `author`, `description`, `enabled`, `resources` (replaced and added),
    /// `added`, `new_streams` (names), `source`, `installed` (Unix seconds),
    /// `problem` (why a damaged mod cannot be used, or `None`).
    #[getter]
    fn mods<'py>(&self, py: Python<'py>) -> PyResult<Vec<Bound<'py, PyDict>>> {
        self.manager
            .mods()
            .iter()
            .map(|m| {
                let dict = PyDict::new(py);
                dict.set_item("id", &m.id)?;
                dict.set_item("name", &m.manifest.name)?;
                dict.set_item("version", &m.manifest.version)?;
                dict.set_item("author", &m.manifest.author)?;
                dict.set_item("description", &m.manifest.description)?;
                dict.set_item("enabled", m.enabled)?;
                dict.set_item("resources", m.manifest.resources.len())?;
                dict.set_item("added", m.manifest.added().count())?;
                let streams: Vec<&str> =
                    m.manifest.streams.iter().map(|s| s.name.as_str()).collect();
                dict.set_item("new_streams", streams)?;
                dict.set_item("source", &m.source)?;
                dict.set_item("installed", m.installed)?;
                dict.set_item("problem", &m.problem)?;
                Ok(dict)
            })
            .collect()
    }

    /// The `mod.json` of an installed mod as a dict.
    fn manifest<'py>(&self, py: Python<'py>, id: &str) -> PyResult<Bound<'py, PyAny>> {
        let m = self.manager.get(id).ok_or_else(|| {
            pyo3::exceptions::PyKeyError::new_err(format!("no mod `{id}` is installed"))
        })?;
        util::to_python(py, &m.manifest)
    }

    /// Installs a mod (`.zip`) or an old toolkit patch (`.vpat`). A mod with
    /// the id of an installed one replaces it. Returns `id`, `name`,
    /// `replaced_version`, `resources`, `warnings`, `issues` (how it fits the game).
    #[pyo3(signature = (path, enable = true))]
    fn install<'py>(
        &mut self,
        py: Python<'py>,
        path: PathBuf,
        enable: bool,
    ) -> PyResult<Bound<'py, PyDict>> {
        let game = &self.game;
        let manager = &mut self.manager;
        let report = py
            .detach(|| manager.install_file(game, &path, enable))
            .py()?;
        install_dict(py, &report)
    }

    /// Installs the changes of a workspace (all, or those matching `keys`) as
    /// a mod, enabled and last in the load order.
    #[pyo3(signature = (workspace, name, *, id = None, version = "1.0", author = "", description = "", url = None, keys = None))]
    #[allow(clippy::too_many_arguments)]
    fn install_workspace<'py>(
        &mut self,
        py: Python<'py>,
        workspace: PyRef<'_, Workspace>,
        name: &str,
        id: Option<String>,
        version: &str,
        author: &str,
        description: &str,
        url: Option<String>,
        keys: Option<Vec<String>>,
    ) -> PyResult<Bound<'py, PyDict>> {
        let info = mod_info(name, id, version, author, description, url);
        let keys: Option<Vec<ResourceKey>> = match_keys(workspace.inner(), keys)?;
        let game = &self.game;
        let manager = &mut self.manager;
        let ws = workspace.inner();
        let report = py
            .detach(|| manager.install_workspace(game, ws, &info, keys.as_deref()))
            .py()?;
        install_dict(py, &report)
    }

    /// Removes a mod; the game keeps its data until `apply`.
    fn uninstall(&mut self, id: &str) -> PyResult<()> {
        self.manager.uninstall(id).py()
    }

    /// Enables or disables a mod.
    #[pyo3(signature = (id, enabled = true))]
    fn enable(&mut self, id: &str, enabled: bool) -> PyResult<()> {
        self.manager.set_enabled(id, enabled).py()
    }

    /// Moves a mod to `position` in the load order (0 is applied first).
    #[pyo3(name = "move")]
    fn move_mod(&mut self, id: &str, position: usize) -> PyResult<()> {
        self.manager.move_mod(id, position).map(|_| ()).py()
    }

    /// Sorts the load order by the mods' rules (dependencies first, then
    /// `load_after`/`load_before`), keeping the present order where they
    /// allow. Returns the ids in the sorted order; `dry_run` only tells it.
    #[pyo3(signature = (dry_run = false))]
    fn sort(&mut self, dry_run: bool) -> PyResult<Vec<String>> {
        self.manager.sort(dry_run).map(|r| r.order).py()
    }

    /// The enabled mods that need mod `id`, directly or through others.
    fn dependents(&self, id: &str) -> Vec<String> {
        self.manager.dependents(id)
    }

    /// Resources replaced by more than one mod, as dicts: `key`, `mods` (load
    /// order, the last one wins), `same_data`.
    #[pyo3(signature = (enabled_only = true))]
    fn conflicts<'py>(
        &self,
        py: Python<'py>,
        enabled_only: bool,
    ) -> PyResult<Vec<Bound<'py, PyDict>>> {
        self.manager
            .conflicts(enabled_only)
            .into_iter()
            .map(|c| {
                let dict = PyDict::new(py);
                dict.set_item("key", c.key.to_string())?;
                dict.set_item("mods", &c.mods)?;
                dict.set_item("same_data", c.same_data)?;
                Ok(dict)
            })
            .collect()
    }

    /// Clashes between mods that add resources or create streams, as dicts:
    /// `kind` (`stream`, `stream_hash`, `guid`, `shared_stream`, `incompatible`), `mods`
    /// (load order), `fatal` (`apply` refuses while such mods are enabled),
    /// `description`.
    #[pyo3(signature = (enabled_only = true))]
    fn clashes<'py>(
        &self,
        py: Python<'py>,
        enabled_only: bool,
    ) -> PyResult<Vec<Bound<'py, PyDict>>> {
        self.manager
            .clashes(enabled_only)
            .into_iter()
            .map(|c| {
                let dict = PyDict::new(py);
                let kind = match c {
                    Clash::Stream { .. } => "stream",
                    Clash::StreamHash { .. } => "stream_hash",
                    Clash::Guid { .. } => "guid",
                    Clash::SharedStream { .. } => "shared_stream",
                    Clash::Incompatible { .. } => "incompatible",
                };
                dict.set_item("kind", kind)?;
                dict.set_item("mods", c.mods())?;
                dict.set_item("fatal", c.is_fatal())?;
                dict.set_item("description", c.describe())?;
                Ok(dict)
            })
            .collect()
    }

    /// Checks the enabled mods against the game: descriptions of the
    /// resources that are missing or whose original differs, and of added
    /// resources and new streams that do not fit.
    fn check(&self, py: Python<'_>) -> PyResult<Vec<String>> {
        let game = &self.game;
        let manager = &self.manager;
        let issues = py.detach(|| manager.check(game)).py()?;
        Ok(issues.iter().map(|i| i.describe()).collect())
    }

    /// Compares the enabled mods with the game: `changes` (`(key, state)`),
    /// `streams` (`(stream, state, up_to_date)`), `pending_reverts`.
    fn status<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let game = &self.game;
        let manager = &self.manager;
        let status = py.detach(|| manager.status(game)).py()?;
        let dict = PyDict::new(py);
        let changes: Vec<(String, &str)> = status
            .changes
            .iter()
            .map(|(k, s)| (k.to_string(), state_name(*s)))
            .collect();
        dict.set_item("changes", changes)?;
        let streams: Vec<(String, &str, bool)> = status
            .streams
            .iter()
            .map(|s| (s.stream.clone(), stream_state_name(s.state), s.up_to_date))
            .collect();
        dict.set_item("streams", streams)?;
        let reverts: Vec<String> = status
            .pending_reverts
            .iter()
            .map(|k| k.to_string())
            .collect();
        dict.set_item("pending_reverts", reverts)?;
        Ok(dict)
    }

    /// Writes the enabled mods into the game. Refused (`ModError`) while the
    /// editor's workspace has changes in the game, when an enabled mod does
    /// not fit the game (unless `ignore_compatibility` for different
    /// originals), and for streams changed by another program (unless `force`)
    /// — the same checks as `doctor-toolkit mods apply`.
    #[pyo3(signature = (force = false, ignore_compatibility = false))]
    fn apply<'py>(
        &mut self,
        py: Python<'py>,
        force: bool,
        ignore_compatibility: bool,
    ) -> PyResult<Bound<'py, PyDict>> {
        let game = &self.game;
        let manager = &mut self.manager;
        let options = ModApplyOptions {
            force,
            ignore_compatibility,
        };
        let report = py.detach(|| manager.apply(game, options)).py()?;
        report_dict(py, &report)
    }

    /// Puts back the original of every stream the mod manager has written;
    /// the mods stay installed.
    #[pyo3(signature = (force = false))]
    fn unapply<'py>(&mut self, py: Python<'py>, force: bool) -> PyResult<Bound<'py, PyDict>> {
        let game = &self.game;
        let manager = &mut self.manager;
        let report = py.detach(|| manager.unapply(game, force)).py()?;
        report_dict(py, &report)
    }

    /// Writes an installed mod back into a package file.
    fn export(&self, py: Python<'_>, id: &str, out: PathBuf) -> PyResult<u64> {
        let manager = &self.manager;
        py.detach(|| manager.export(id, &out)).py()
    }

    fn __repr__(&self) -> String {
        format!("ModManager({} mod(s))", self.manager.mods().len())
    }
}

/// What a mod package (`.zip`) or an old toolkit patch (`.vpat`) contains.
/// A package is verified (sizes, entry names, payload hashes); a damaged one
/// raises `ModError`. Returns the `mod.json` as a dict plus `kind` (`mod`)
/// and `warnings`, or for a patch `kind` (`vpat`), `name`, `author`,
/// `version`, `files` (`path`, `stream`, `resources` keys).
#[pyfunction(name = "mod_info")]
fn mod_info_py<'py>(py: Python<'py>, path: PathBuf) -> PyResult<Bound<'py, PyAny>> {
    let head = {
        use std::io::Read;
        let mut head = [0u8; 4];
        let mut file =
            std::fs::File::open(&path).map_err(|e| to_py(doctor_toolkit::Error::io(&path, e)))?;
        let n = file
            .read(&mut head)
            .map_err(|e| to_py(doctor_toolkit::Error::io(&path, e)))?;
        head[..n].to_vec()
    };
    if vpat::is_vpat(&head) {
        let patch = py.detach(|| VpatPatch::open(&path)).py()?;
        let dict = PyDict::new(py);
        dict.set_item("kind", "vpat")?;
        dict.set_item("name", &patch.name)?;
        dict.set_item("author", &patch.author)?;
        dict.set_item("version", patch.version)?;
        dict.set_item("format_version", patch.format_version)?;
        dict.set_item("patch_id", patch.patch_id)?;
        let files = patch
            .files
            .iter()
            .map(|f| {
                let item = PyDict::new(py);
                item.set_item("path", &f.path)?;
                item.set_item("stream", &f.stream)?;
                let keys: Vec<String> = f
                    .resources
                    .iter()
                    .map(|r| {
                        ResourceKey {
                            stream: f.stream.clone(),
                            type_id: r.type_id,
                            ids: r.ids,
                        }
                        .to_string()
                    })
                    .collect();
                item.set_item("resources", keys)?;
                Ok(item)
            })
            .collect::<PyResult<Vec<_>>>()?;
        dict.set_item("files", files)?;
        return Ok(dict.into_any());
    }
    let (manifest, warnings) = py
        .detach(|| {
            let mut archive = ModArchive::open(&path)?;
            archive.verify()?;
            Ok::<_, doctor_toolkit::Error>((
                archive.manifest().clone(),
                archive.warnings().to_vec(),
            ))
        })
        .py()?;
    let value = util::to_python(py, &manifest)?;
    value.set_item("kind", "mod")?;
    value.set_item("warnings", warnings)?;
    Ok(value)
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<ModManager>()?;
    m.add_function(wrap_pyfunction!(mod_info_py, m)?)?;
    Ok(())
}

//! `Project`: a mod as a folder of source files, built into the workspace
//! when they change (`docs/project.md`).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::navigation::unpack;
use doctor_toolkit::project::{self as lib, BuildOptions, CreateOptions, config::BuildSettings};
use doctor_toolkit::workspace::Workspace;
use pyo3::prelude::*;
use pyo3::types::PyDict;

use crate::error::{ResultExt, invalid};
use crate::game::Game;
use crate::navigation::Index;
use crate::util;

/// A project: `project.toml`, source files by stream under `src/`, common
/// ones under `shared/`, the author's object patches under `patches/`.
///
/// `Project(path)` opens one; `Project.create(game, path, ...)` makes one.
/// Builds go into the workspace of `project.toml` (default: the game's).
#[pyclass(module = "doctor_toolkit")]
pub struct Project {
    project: lib::Project,
    game: Arc<GameInstance>,
    workspace_dir: Option<PathBuf>,
}

impl Project {
    fn workspace(&self, py: Python<'_>) -> PyResult<Workspace> {
        let (game, dir, project) = (&self.game, &self.workspace_dir, &self.project);
        py.detach(|| match dir {
            Some(dir) => Workspace::for_game(game, Some(dir)),
            None => project.workspace(game),
        })
        .py()
    }
}

#[pymethods]
impl Project {
    /// Opens the project in `path`. `game` overrides the game of
    /// `project.toml`, `workspace_dir` its workspace.
    #[new]
    #[pyo3(signature = (path, game = None, workspace_dir = None))]
    fn new(
        py: Python<'_>,
        path: PathBuf,
        game: Option<PyRef<'_, Game>>,
        workspace_dir: Option<PathBuf>,
    ) -> PyResult<Self> {
        let project = py.detach(|| lib::Project::open(&path)).py()?;
        let game = match game {
            Some(game) => game.game.clone(),
            None => Arc::new(py.detach(|| project.game(None)).py()?),
        };
        Ok(Self {
            project,
            game,
            workspace_dir,
        })
    }

    /// Makes a project in `path` (new or empty) from the resources of
    /// `streams` (all when `None`) of types `types`, or with
    /// `from_workspace` from the resources the workspace changes. `index`
    /// names the files and gives models their materials and textures.
    #[staticmethod]
    #[pyo3(signature = (game, path, *, name = None, id = None, author = "", streams = None, types = None, from_workspace = false, workspace_dir = None, index = None, dds = false, model_format = "glb", shared = true))]
    #[allow(clippy::too_many_arguments)]
    fn create<'py>(
        py: Python<'py>,
        game: PyRef<'py, Game>,
        path: PathBuf,
        name: Option<String>,
        id: Option<String>,
        author: &str,
        streams: Option<Vec<String>>,
        types: Option<Vec<Bound<'py, PyAny>>>,
        from_workspace: bool,
        workspace_dir: Option<PathBuf>,
        index: Option<PyRef<'py, Index>>,
        dds: bool,
        model_format: &str,
        shared: bool,
    ) -> PyResult<Self> {
        let types = types
            .unwrap_or_default()
            .iter()
            .map(util::resolve_type)
            .collect::<PyResult<Vec<_>>>()?;
        let streams = streams.unwrap_or_default();
        let instance = game.game.clone();
        let ws = py
            .detach(|| Workspace::for_game(&instance, workspace_dir.as_deref()))
            .py()?;
        let mut selection = if from_workspace {
            py.detach(|| lib::workspace_selection(&instance, &ws))
                .py()?
        } else {
            py.detach(|| unpack::select(&instance, &streams, &types))
                .py()?
        };
        if from_workspace && !types.is_empty() {
            for (s, list) in &mut selection {
                let file = instance.open_stream(*s).py()?;
                list.retain(|&r| types.contains(&file.layout().resources[r].header.type_id));
            }
            selection.retain(|(_, l)| !l.is_empty());
        }
        if selection.is_empty() {
            return Err(invalid("nothing to put into the project"));
        }
        let name = name.unwrap_or_else(|| {
            path.file_name()
                .map_or_else(|| "Project".into(), |n| n.to_string_lossy().into_owned())
        });
        let options = CreateOptions {
            name,
            id,
            author: author.to_owned(),
            selection,
            settings: BuildSettings {
                workspace: workspace_dir.clone(),
                textures: if dds { "dds" } else { "png" }.into(),
                models: model_format.to_owned(),
                shared,
            },
        };
        let idx = index.as_ref().map(|i| i.game_index());
        let cancel = AtomicBool::new(false);
        let (project, _) = py
            .detach(|| lib::create(&instance, &ws, idx, &path, &options, &|_, _| {}, &cancel))
            .py()?;
        Ok(Self {
            project,
            game: instance,
            workspace_dir,
        })
    }

    /// The project folder.
    #[getter]
    fn root(&self) -> PathBuf {
        self.project.root().to_owned()
    }

    #[getter]
    fn name(&self) -> String {
        self.project.config.project.name.clone()
    }

    /// The mod id.
    #[getter]
    fn id(&self) -> String {
        self.project.config.project.id.clone()
    }

    /// Every entry: dicts `id` (main file from the root), `kind`, `type`,
    /// `name`, `ids`, `files`, `keys`.
    fn entries<'py>(&self, py: Python<'py>) -> PyResult<Vec<Bound<'py, PyDict>>> {
        self.project
            .entries()
            .map(|(id, e, dir)| {
                let d = PyDict::new(py);
                d.set_item("id", id)?;
                d.set_item("kind", e.kind.label())?;
                d.set_item("type", &e.type_tag)?;
                d.set_item("name", &e.name)?;
                d.set_item("ids", &e.ids)?;
                d.set_item(
                    "files",
                    e.files
                        .iter()
                        .map(|f| format!("{dir}/{f}"))
                        .collect::<Vec<_>>(),
                )?;
                d.set_item(
                    "keys",
                    e.keys.iter().map(ToString::to_string).collect::<Vec<_>>(),
                )?;
                Ok(d)
            })
            .collect()
    }

    /// The state of each entry against the last build and the workspace:
    /// dicts `id`, `state` (`unchanged`, `built`, `modified`, `out of
    /// sync`, `missing`, `view changed`), `kind`, `type`, `name`, `patch`.
    fn status<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let ws = self.workspace(py)?;
        let project = &self.project;
        let status = py.detach(|| project.status(&ws)).py()?;
        let entries = status
            .entries
            .iter()
            .map(|e| {
                let d = PyDict::new(py);
                d.set_item("id", &e.id)?;
                d.set_item("state", e.state.label())?;
                d.set_item("kind", e.kind.label())?;
                d.set_item("type", &e.type_tag)?;
                d.set_item("name", &e.name)?;
                d.set_item("patch", &e.patch)?;
                Ok(d)
            })
            .collect::<PyResult<Vec<_>>>()?;
        let dict = PyDict::new(py);
        dict.set_item("entries", entries)?;
        dict.set_item("unknown_files", &status.unknown_files)?;
        dict.set_item("warnings", &status.warnings)?;
        dict.set_item("to_build", status.to_build())?;
        Ok(dict)
    }

    /// Builds the changed entries (all with `force`; those starting with
    /// `only`) into the workspace as one operation. Returns `built` (dicts
    /// `id`, `changed`, `notes`), `up_to_date`, `errors`, `warnings`, `seq`.
    #[pyo3(signature = (*, only = None, force = false, dry_run = false))]
    fn build<'py>(
        &self,
        py: Python<'py>,
        only: Option<Vec<String>>,
        force: bool,
        dry_run: bool,
    ) -> PyResult<Bound<'py, PyDict>> {
        let mut ws = self.workspace(py)?;
        let options = BuildOptions {
            dry_run,
            force,
            only: only.unwrap_or_default(),
        };
        let (project, game) = (&self.project, &self.game);
        let report = py.detach(|| project.build(game, &mut ws, &options)).py()?;
        let built = report
            .built
            .iter()
            .map(|b| {
                let d = PyDict::new(py);
                d.set_item("id", &b.id)?;
                d.set_item(
                    "changed",
                    b.changed
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>(),
                )?;
                d.set_item("notes", &b.notes)?;
                Ok(d)
            })
            .collect::<PyResult<Vec<_>>>()?;
        let dict = PyDict::new(py);
        dict.set_item("built", built)?;
        dict.set_item("up_to_date", report.up_to_date)?;
        dict.set_item("errors", &report.errors)?;
        dict.set_item("warnings", &report.warnings)?;
        dict.set_item("seq", report.seq)?;
        Ok(dict)
    }

    /// Writes the workspace's version of out-of-sync entries (whose id
    /// starts with one of `only`, or all) into their files. Returns their ids.
    #[pyo3(signature = (only = None))]
    fn pull(&mut self, py: Python<'_>, only: Option<Vec<String>>) -> PyResult<Vec<String>> {
        let ws = self.workspace(py)?;
        let only = only.unwrap_or_default();
        let (project, game) = (&mut self.project, &self.game);
        py.detach(|| project.pull(game, &ws, &only)).py()
    }

    /// Writes a mod of the project's changes in the workspace to `out`.
    /// Returns `resources`, `added`, `file_size`.
    #[pyo3(signature = (out, preview = None))]
    fn build_mod<'py>(
        &self,
        py: Python<'py>,
        out: PathBuf,
        preview: Option<PathBuf>,
    ) -> PyResult<Bound<'py, PyDict>> {
        let ws = self.workspace(py)?;
        let preview = match preview {
            Some(path) => Some(
                doctor_toolkit::mods::Preview::from_image(
                    std::fs::read(&path)
                        .map_err(|e| invalid(format!("{}: {e}", path.display())))?,
                )
                .py()?,
            ),
            None => None,
        };
        let project = &self.project;
        let report = py
            .detach(|| project.build_mod(&ws, &out, preview.as_ref()))
            .py()?;
        let dict = PyDict::new(py);
        dict.set_item("resources", report.resources)?;
        dict.set_item("added", report.added)?;
        dict.set_item("file_size", report.file_size)?;
        Ok(dict)
    }

    fn __repr__(&self) -> String {
        format!(
            "Project({:?}, {} entries)",
            self.project.root(),
            self.project.entries().count()
        )
    }
}

/// The GUID a project gives the new object `name` in `scope`
/// (`<mod id>:simgroup:<ids>`), as 32 hexadecimal digits.
#[pyfunction]
fn project_object_guid(scope: &str, name: &str) -> String {
    lib::guid::derived(scope, name, |_| false).to_string()
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Project>()?;
    m.add_function(wrap_pyfunction!(project_object_guid, m)?)?;
    Ok(())
}

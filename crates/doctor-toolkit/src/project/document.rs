//! A project as the document of the editor (stage 106): a folder with its
//! **own workspace**, so that the work on one mod — its edits and their
//! history — is apart from the work on another and from the game folder.
//!
//! ```text
//! my_mod/
//! ├── project.toml        [build] workspace = "workspace"
//! ├── workspace/          manifest.json, log.json, payloads/ — the edits
//! ├── src/ shared/ patches/   sources (stage 36), when the author wants them
//! └── .doctor/
//!     ├── local.toml      where the game is on this computer
//!     └── editor.json     what the editor showed last: the zones of the level
//! ```
//!
//! The path of the game differs from one computer to another, so a project
//! with its own workspace keeps it in `.doctor/local.toml` (not for version
//! control) and not in `project.toml`. A project of stage 36 (sources built
//! into the game's workspace, the game in `project.toml`) opens as before.
//!
//! The sets of zones the author saved are the project's (`[[zones]]` of
//! `project.toml`, stage 113); what the level editor had loaded last is one
//! computer's and one author's, so it is in `.doctor/editor.json`
//! ([`EditorState`]), which is not for version control either.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::game::GameInstance;
use crate::workspace::{Deployment, Workspace};
use crate::{Error, Result};

use super::config::{BuildSettings, Dependencies, FORMAT, ProjectConfig, ProjectInfo};
use super::{PROJECT_FILE, Project, cache};
use crate::mods::manager::{InstallReport, Payloads};
use crate::mods::{ModInfo, ModManager};

/// Folder of a project's own workspace, from the project folder.
pub const WORKSPACE_DIR: &str = "workspace";
/// What is true on this computer only, from the project folder.
pub const LOCAL_FILE: &str = ".doctor/local.toml";

/// What the editor showed last, from the project folder.
pub const EDITOR_FILE: &str = ".doctor/editor.json";

/// A camera of the level view.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EditorCamera {
    pub eye: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
}

/// `.doctor/editor.json`: the state of the editor with this project open, on
/// this computer (stage 113). Keys this version does not know are kept.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EditorState {
    /// The zones the level editor loaded last: stream names, folders of
    /// zones, `preset:<id>` (`level::presets::resolve`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub zones: Vec<String>,
    /// The camera over those zones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<EditorCamera>,
    #[serde(flatten)]
    pub other: serde_json::Map<String, serde_json::Value>,
}

impl EditorState {
    /// Reads the file of the project in `root`; nothing when there is none
    /// or it cannot be read (it is rewritten by the next [`Self::write`]).
    pub fn read(root: &Path) -> Self {
        std::fs::read(root.join(EDITOR_FILE))
            .ok()
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default()
    }

    /// Writes the file and makes sure the project's `.gitignore` (when it
    /// is the one [`create_document`] wrote) keeps it out of version
    /// control: a project made before stage 113 does not list it.
    pub fn write(&self, root: &Path) -> Result<()> {
        let path = root.join(EDITOR_FILE);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| Error::InvalidArgument(e.to_string()))?;
        crate::atomic::write(&path, text.as_bytes())?;
        let ignore = root.join(".gitignore");
        if let Ok(text) = std::fs::read_to_string(&ignore) {
            let (local, editor) = (format!("/{LOCAL_FILE}"), format!("/{EDITOR_FILE}"));
            let has = |wanted: &str| text.lines().any(|line| line.trim() == wanted);
            if has(&local) && !has(&editor) {
                let lines: Vec<String> = text
                    .lines()
                    .flat_map(|line| {
                        let after = (line.trim() == local).then(|| editor.clone());
                        std::iter::once(line.to_owned()).chain(after)
                    })
                    .collect();
                crate::atomic::write(&ignore, (lines.join("\n") + "\n").as_bytes())?;
            }
        }
        Ok(())
    }
}

/// `.doctor/local.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalSettings {
    /// The game folder on this computer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game: Option<PathBuf>,
}

impl LocalSettings {
    /// Reads the file of the project in `root`; nothing when there is none
    /// or it cannot be read (it is rewritten by the next [`Self::write`]).
    pub fn read(root: &Path) -> Self {
        std::fs::read_to_string(root.join(LOCAL_FILE))
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn write(&self, root: &Path) -> Result<()> {
        let path = root.join(LOCAL_FILE);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
        }
        let text =
            toml::to_string_pretty(self).map_err(|e| Error::InvalidArgument(e.to_string()))?;
        crate::atomic::write(&path, text.as_bytes())
    }
}

impl Project {
    /// Whether the project has a workspace of its own (`[build] workspace`)
    /// instead of building into the game's.
    pub fn has_own_workspace(&self) -> bool {
        self.config.build.workspace.is_some()
    }

    /// The folder of the project's own workspace, if it has one.
    pub fn workspace_dir(&self) -> Option<PathBuf> {
        self.config.workspace_dir(self.root())
    }

    /// The game folder on this computer: of `.doctor/local.toml`, else of
    /// `project.toml`.
    pub fn game_path(&self) -> Option<PathBuf> {
        LocalSettings::read(self.root())
            .game
            .or_else(|| self.config.game.path.clone())
    }

    /// Installs the project into `manager` as a mod: its changes in `ws`
    /// ([`Self::changed_keys`]; of a project with its own workspace, all of
    /// that workspace) under the project's id, name and version, enabled
    /// and last in the load order. A mod of that id — an earlier state of
    /// the project — is replaced. The mods of the project's base — without
    /// a base, the mods the manager has written into `game` that the
    /// changes were made on — become dependencies, so that the project is
    /// checked against their data and wins over them as a patch
    /// ([`Self::mod_of`]).
    ///
    /// This is the only way a project with its own workspace reaches the
    /// game files: the manager of the game for Install into Mods
    /// ([`Payloads::Copied`]), the manager of a staged copy for one launch
    /// ([`crate::play::overlay`], [`Payloads::Linked`]).
    pub fn install_into(
        &self,
        game: &GameInstance,
        manager: &mut ModManager,
        ws: &Workspace,
        payloads: Payloads,
    ) -> Result<InstallReport> {
        let (info, keys) = self.mod_of(game, ws)?;
        manager.install_workspace_with(game, ws, &info, keys.as_deref(), payloads)
    }

    /// The mod of the project's changes in `ws`: its metadata with its
    /// dependencies, and the changes it holds (`None`: all of the
    /// workspace).
    ///
    /// A project with a base (stage 108) depends on the mods of the base
    /// and on no others, with the conditions written there — also on one
    /// that is enabled but not written into `game`, which the guess by what
    /// is written does not find. Without a base the dependencies are that
    /// guess ([`crate::mods::add_suggested_dependencies`]). Either way the
    /// data of a dependency the project replaces becomes the original of
    /// that resource in the mod.
    pub(crate) fn mod_of(
        &self,
        game: &GameInstance,
        ws: &Workspace,
    ) -> Result<(ModInfo, Option<Vec<crate::workspace::ResourceKey>>)> {
        // All of its own workspace: the new streams and the files of the
        // archives too, which no list of resources names.
        let keys = (!self.has_own_workspace()).then(|| self.changed_keys(ws));
        if keys.as_ref().is_some_and(Vec::is_empty) {
            return Err(Error::mod_error(
                "the workspace has no changes of this project; build it first",
            ));
        }
        let mut info = self.mod_info();
        if self.config.base.is_some() {
            let manager = ModManager::open(game)?;
            crate::mods::add_made_on(&mut info, game, &manager, ws, keys.as_deref())?;
        } else {
            crate::mods::add_suggested_dependencies(&mut info, game, ws, keys.as_deref())?;
        }
        Ok((info, keys))
    }

    /// Saves the sets of zones of the project (`[[zones]]` of
    /// `project.toml`, [`ProjectConfig::write_zones`]).
    pub fn set_zone_sets(&mut self, zones: Vec<crate::level::ZoneSet>) -> Result<()> {
        ProjectConfig::write_zones(self.root(), &zones)?;
        self.config.zones = zones;
        Ok(())
    }

    /// Remembers where the game is on this computer (`.doctor/local.toml`).
    pub fn set_game_path(&self, game: &Path) -> Result<()> {
        let mut local = LocalSettings::read(self.root());
        local.game = Some(std::fs::canonicalize(game).unwrap_or_else(|_| game.to_owned()));
        local.write(self.root())
    }
}

/// The folder of the project whose own workspace the folder `dir` is: the
/// folder above `dir` holds a `project.toml` that names `dir`
/// (`[build] workspace`). `None` for any other folder — the game's own
/// workspace, a folder given to a command.
///
/// This is how a workspace is known to be a project's when it is opened by
/// its folder alone (`--workspace-dir my_mod/workspace`), also one written
/// before the mark of its manifest existed ([`crate::workspace::Owner`]).
pub fn project_of_workspace(dir: &Path) -> Option<PathBuf> {
    let dir = std::path::absolute(dir).ok()?;
    let root = dir.parent()?;
    if !Project::exists(root) {
        return None;
    }
    let named = ProjectConfig::read(root).ok()?.workspace_dir(root)?;
    let same = named == dir
        || matches!(
            (std::fs::canonicalize(&named), std::fs::canonicalize(&dir)),
            (Ok(a), Ok(b)) if a == b
        );
    same.then(|| root.to_owned())
}

/// [`create_document`]: what the new project is called.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewDocument {
    pub name: String,
    /// Id of the mod; from the name when `None`.
    pub id: Option<String>,
    pub author: String,
}

/// Makes a project with its own, empty workspace in `root` (a new or empty
/// folder) for `game`. Nothing of the game is exported: the project is
/// filled by editing. The workspace is written by its first operation, as
/// a project's own ([`crate::workspace::Owner::Project`]) whichever program
/// records it: the folder is the one `project.toml` names
/// ([`project_of_workspace`]).
pub fn create_document(game: &GameInstance, root: &Path, new: &NewDocument) -> Result<Project> {
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
    let name = new.name.trim();
    if name.is_empty() {
        return Err(Error::InvalidArgument("the project needs a name".into()));
    }
    let id = match &new.id {
        Some(id) => id.clone(),
        None => crate::mods::id_from_name(name),
    };
    crate::mods::validate_id(&id)?;
    std::fs::create_dir_all(root).map_err(|e| Error::io(root, e))?;
    let mut game_info = super::create::game_info(game);
    let game_path = game_info.path.take();
    let config = ProjectConfig {
        format: FORMAT,
        project: ProjectInfo {
            name: name.to_owned(),
            id,
            version: "1.0".into(),
            author: new.author.clone(),
            description: String::new(),
            url: String::new(),
        },
        game: game_info,
        dependencies: Dependencies::default(),
        base: None,
        build: BuildSettings {
            workspace: Some(PathBuf::from(WORKSPACE_DIR)),
            ..Default::default()
        },
        zones: Vec::new(),
    };
    config.write(root)?;
    LocalSettings { game: game_path }.write(root)?;
    crate::atomic::write(&root.join(".gitignore"), gitignore().as_bytes())?;
    Project::open(root)
}

/// `.gitignore` of a project with its own workspace: what is rebuilt or is
/// true on one computer only.
fn gitignore() -> String {
    format!(
        "# doctor_toolkit: the build cache (`{dir}/{base}` is needed to build), what is true on\n\
         # this computer only, and what the workspace rebuilds.\n\
         /{dir}/{cache}\n\
         /{local}\n\
         /{editor}\n\
         /{ws}/applied.json\n\
         /{ws}/lock\n\
         /{ws}/lock.holder\n\
         /{ws}/staged/\n",
        dir = cache::TOOL_DIR,
        base = cache::BASE_DIR,
        cache = cache::CACHE_FILE,
        local = LOCAL_FILE,
        editor = EDITOR_FILE,
        ws = WORKSPACE_DIR,
    )
}

/// Files of a workspace folder that describe one game folder or one
/// running program, not the edits: they do not move with the workspace.
const NOT_MOVED: &[&str] = &["applied.json", "lock", "lock.holder", "staged"];

/// What [`adopt_workspace`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adopted {
    /// Changes the project's workspace has now.
    pub changes: usize,
    /// Operations of its history.
    pub operations: usize,
    pub files: usize,
}

/// Moves the workspace in `from` (the game's own, as a rule) into the own
/// workspace of `project`, which must be empty: its edits and their history
/// become the project's, and `from` is removed. Refused while `from` has
/// its changes in the files of `game` — put the originals back first
/// (`workspace revert --game`): a project's workspace does not own files of
/// the game.
///
/// The caller holds no lock of either workspace.
pub fn adopt_workspace(game: &GameInstance, from: &Path, project: &Project) -> Result<Adopted> {
    let to = project.workspace_dir().ok_or_else(|| {
        Error::InvalidArgument("the project has no workspace of its own to move into".into())
    })?;
    if !Workspace::exists(from) {
        return Err(Error::InvalidArgument(format!(
            "there is no workspace in {}",
            from.display()
        )));
    }
    if Workspace::exists(&to) {
        return Err(Error::InvalidArgument(format!(
            "the project already has edits in {}",
            to.display()
        )));
    }
    let held = Deployment::load(&from.join("applied.json"))?.held_streams(game)?;
    if !held.is_empty() {
        return Err(Error::InvalidArgument(format!(
            "the workspace has its changes in {} stream(s) of the game ({}{}); put the \
             originals back first (Revert game, `workspace revert --game`)",
            held.len(),
            held.iter().take(3).cloned().collect::<Vec<_>>().join(", "),
            if held.len() > 3 { ", …" } else { "" }
        )));
    }
    // Copied first and checked, removed after: a failure leaves `from` whole.
    let mut files = 0;
    // The project's from now on: it does not write into the game by itself.
    let copied = copy_tree(from, &to, true, &mut files)
        .and_then(|()| Workspace::open(&to))
        .and_then(|mut ws| ws.make_project_owned().map(|()| ws));
    let ws = match copied {
        Ok(ws) => ws,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&to);
            return Err(error);
        }
    };
    std::fs::remove_dir_all(from).map_err(|e| Error::io(from, e))?;
    Ok(Adopted {
        changes: ws.changes().count(),
        operations: ws.head(),
        files,
    })
}

fn copy_tree(from: &Path, to: &Path, top: bool, files: &mut usize) -> Result<()> {
    std::fs::create_dir_all(to).map_err(|e| Error::io(to, e))?;
    for entry in std::fs::read_dir(from).map_err(|e| Error::io(from, e))? {
        let entry = entry.map_err(|e| Error::io(from, e))?;
        let name = entry.file_name();
        if top && NOT_MOVED.iter().any(|n| name == std::ffi::OsStr::new(n)) {
            continue;
        }
        let (source, target) = (entry.path(), to.join(&name));
        let kind = entry.file_type().map_err(|e| Error::io(&source, e))?;
        if kind.is_dir() {
            copy_tree(&source, &target, false, files)?;
        } else if kind.is_file() {
            std::fs::copy(&source, &target).map_err(|e| Error::io(&source, e))?;
            *files += 1;
        }
    }
    Ok(())
}

//! `project.toml`: what the project is and how it is built.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// Name of the project file in the project directory.
pub const PROJECT_FILE: &str = "project.toml";
/// Current version of the project format (`format` in `project.toml` and in
/// the source manifests).
pub const FORMAT: u32 = 1;

/// `project.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectConfig {
    /// Version of the project format.
    pub format: u32,
    pub project: ProjectInfo,
    pub game: GameInfo,
    #[serde(default)]
    pub dependencies: Dependencies,
    #[serde(default)]
    pub build: BuildSettings,
}

/// Name and authorship; they become the metadata of the mod.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectInfo {
    pub name: String,
    /// Id of the mod built from the project (`a-z`, `0-9`, `-`, `_`). New
    /// objects take their GUIDs from it, so it should not change.
    pub id: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub author: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
}

fn default_version() -> String {
    "1.0".into()
}

/// The game the project was made from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameInfo {
    /// Game directory on this computer; commands take another with `--game`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// Game profile (`The Godfather II (PC)`).
    pub profile: String,
    /// Edition of the executable (`docs/shell.md`), for information.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub edition: String,
    /// SHA-256 of the executable; a different one gives a warning.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub executable_sha256: String,
    /// Language of the text the project shows first (`en`, `ru`, ...).
    #[serde(default = "default_language")]
    pub language: String,
}

fn default_language() -> String {
    "en".into()
}

/// What the project needs besides the game.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dependencies {
    /// Ids of mods that must be installed (and enabled) with this one;
    /// `project status` warns when one is missing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mods: Vec<String>,
}

/// File formats of new sources and where builds go.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildSettings {
    /// Workspace the build records into; default: the game's
    /// (`<game>/_doctor_toolkit/workspace`). Relative paths are relative to
    /// the project directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<PathBuf>,
    /// Format of textures: `png` (cube and volume textures are always DDS) or `dds`.
    #[serde(default = "default_textures")]
    pub textures: String,
    /// Format of models: `glb`, `fbx`, `obj`, `usd`, `usda`, `usdc` or `usdz`.
    #[serde(default = "default_models")]
    pub models: String,
    /// Resources equal in several streams go to `shared/` once.
    #[serde(default = "default_true")]
    pub shared: bool,
}

fn default_textures() -> String {
    "png".into()
}

fn default_models() -> String {
    "glb".into()
}

fn default_true() -> bool {
    true
}

impl Default for BuildSettings {
    fn default() -> Self {
        Self {
            workspace: None,
            textures: default_textures(),
            models: default_models(),
            shared: true,
        }
    }
}

impl ProjectConfig {
    /// Reads `<root>/project.toml`.
    pub fn read(root: &Path) -> Result<Self> {
        let path = root.join(PROJECT_FILE);
        let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
        let config: Self = toml::from_str(&text)
            .map_err(|e| Error::InvalidArgument(format!("{}: {e}", path.display())))?;
        if config.format > FORMAT {
            return Err(Error::InvalidArgument(format!(
                "{} has format {}; this tool reads up to {FORMAT}",
                path.display(),
                config.format
            )));
        }
        crate::mods::validate_id(&config.project.id)
            .map_err(|e| Error::InvalidArgument(format!("{}: {e}", path.display())))?;
        Ok(config)
    }

    /// Writes `<root>/project.toml` atomically.
    pub fn write(&self, root: &Path) -> Result<()> {
        let text =
            toml::to_string_pretty(self).map_err(|e| Error::InvalidArgument(e.to_string()))?;
        crate::atomic::write(&root.join(PROJECT_FILE), text.as_bytes())
    }

    /// The workspace directory of builds: [`BuildSettings::workspace`]
    /// against `root`, or `None` for the game's default.
    pub fn workspace_dir(&self, root: &Path) -> Option<PathBuf> {
        self.build.workspace.as_ref().map(|w| root.join(w))
    }
}

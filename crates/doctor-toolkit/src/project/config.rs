//! `project.toml`: what the project is and how it is built.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::base::Base;
use crate::level::ZoneSet;
use crate::{Error, Result};

/// Name of the project file in the project directory.
pub const PROJECT_FILE: &str = "project.toml";
/// Version of the project format of a project without a base (`format` in
/// `project.toml`), and of the source manifests and the build cache.
pub const FORMAT: u32 = 1;
/// The first version of `project.toml` with a base (`[base]`, stage 108). A
/// program that reads up to version 1 skips tables it does not know: it
/// would open such a project as one made on the unmodded game and build its
/// mod without the dependencies, without a word — so the base needs a
/// version that program refuses.
pub const BASE_FORMAT: u32 = 2;
/// The newest version of `project.toml` this program reads.
pub const NEWEST_FORMAT: u32 = BASE_FORMAT;

/// `project.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectConfig {
    /// Version of the project format.
    pub format: u32,
    pub project: ProjectInfo,
    pub game: GameInfo,
    #[serde(default)]
    pub dependencies: Dependencies,
    /// The mods the project is made on (stage 108), `[base]`: what the
    /// editor compares the game with and what the project's mod depends on.
    /// `None`: no base was recorded. With one, the file is written as
    /// format [`BASE_FORMAT`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<Base>,
    #[serde(default)]
    pub build: BuildSettings,
    /// Sets of zones saved for the level editor (stage 113), `[[zones]]`:
    /// what to load to work on this mod. They are the project's, so they
    /// travel with it; what was loaded last on one computer is in
    /// `.doctor/editor.json`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub zones: Vec<ZoneSet>,
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
    /// `project status` warns when one is missing. The form of stage 36: a
    /// base without versions and hashes (`[base]` names them, stage 108).
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
    /// The mod of the project stores a changed SimGroup, string table,
    /// parts database or world grid as a delta (mod format 6), which merges
    /// with other mods; `false` keeps every resource whole, for the mod
    /// managers before that format. Written only when `false`.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub deltas: bool,
}

fn is_true(value: &bool) -> bool {
    *value
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
            deltas: true,
        }
    }
}

impl ProjectConfig {
    /// Reads `<root>/project.toml`.
    pub fn read(root: &Path) -> Result<Self> {
        Self::read_up_to(root, NEWEST_FORMAT)
    }

    /// [`Self::read`] by a program that reads formats up to `newest`: what
    /// an older program says about a newer project.
    pub fn read_up_to(root: &Path, newest: u32) -> Result<Self> {
        let path = root.join(PROJECT_FILE);
        let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
        let config: Self = toml::from_str(&text)
            .map_err(|e| Error::InvalidArgument(format!("{}: {e}", path.display())))?;
        if config.format > newest {
            return Err(Error::InvalidArgument(format!(
                "{} has format {}; this tool reads up to {newest}",
                path.display(),
                config.format
            )));
        }
        let at =
            |e: Error| Error::InvalidArgument(format!("{}: {}", path.display(), super::plain(&e)));
        crate::mods::validate_id(&config.project.id).map_err(at)?;
        if let Some(base) = &config.base {
            base.validate(&config.project.id).map_err(at)?;
        }
        Ok(config)
    }

    /// The version of the format the file needs: [`BASE_FORMAT`] with a
    /// base, else [`FORMAT`], which older programs read.
    pub fn required_format(&self) -> u32 {
        if self.base.is_some() {
            BASE_FORMAT
        } else {
            FORMAT
        }
    }

    /// Writes `<root>/project.toml` atomically, in the oldest format that
    /// holds it ([`Self::required_format`]): a project without a base is
    /// written as before stage 108, byte for byte.
    pub fn write(&self, root: &Path) -> Result<()> {
        let config = Self {
            format: self.required_format(),
            ..self.clone()
        };
        let text =
            toml::to_string_pretty(&config).map_err(|e| Error::InvalidArgument(e.to_string()))?;
        crate::atomic::write(&root.join(PROJECT_FILE), text.as_bytes())
    }

    /// Sets (or, with `None`, takes away) the base of `<root>/project.toml`
    /// — the tables `[base]` and `[[base.mod]]`, and the `format` the file
    /// needs with them — and leaves the rest of the file as its author wrote
    /// it, like [`Self::write_zones`]. The file is rewritten whole only when
    /// it cannot be changed in place.
    pub fn write_base(root: &Path, base: Option<&Base>) -> Result<()> {
        let path = root.join(PROJECT_FILE);
        let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
        let mut config = Self::read(root)?;
        if let Some(base) = base {
            base.validate(&config.project.id)?;
        }
        config.base = base.cloned();
        config.format = config.required_format();
        // Every line of the tables of the base, up to the next table; the
        // format, which is a key before the first table.
        let mut kept = String::new();
        let (mut inside, mut top, mut format_set) = (false, true, false);
        for line in text.lines() {
            let line_start = line.trim_start();
            if line_start.starts_with('[') {
                top = false;
                let name = line_start
                    .trim_start_matches('[')
                    .split(']')
                    .next()
                    .unwrap_or_default()
                    .trim();
                inside = name == "base" || name.starts_with("base.");
            }
            if inside {
                continue;
            }
            let is_format = top
                && line_start
                    .strip_prefix("format")
                    .is_some_and(|rest| rest.trim_start().starts_with('='));
            if is_format {
                kept += &format!("format = {}", config.format);
                format_set = true;
            } else {
                kept += line;
            }
            kept.push('\n');
        }
        let mut new = kept.trim_end().to_owned();
        new.push('\n');
        if let Some(base) = base {
            #[derive(Serialize)]
            struct Tables<'a> {
                base: &'a Base,
            }
            new.push('\n');
            new += &toml::to_string_pretty(&Tables { base })
                .map_err(|e| Error::InvalidArgument(e.to_string()))?;
        }
        if format_set && toml::from_str::<Self>(&new).is_ok_and(|read| read == config) {
            crate::atomic::write(&path, new.as_bytes())
        } else {
            config.write(root)
        }
    }

    /// Replaces the `[[zones]]` tables of `<root>/project.toml` with `zones`
    /// and leaves the rest of the file as its author wrote it (comments,
    /// order, keys this version does not know). The file is rewritten whole
    /// only when it cannot be changed in place.
    pub fn write_zones(root: &Path, zones: &[ZoneSet]) -> Result<()> {
        let path = root.join(PROJECT_FILE);
        let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
        let mut config = Self::read(root)?;
        config.zones = zones.to_vec();
        // Every line of a `[[zones]]` table, up to the next table.
        let mut kept = String::new();
        let mut inside = false;
        for line in text.lines() {
            let line_start = line.trim_start();
            if line_start.starts_with('[') {
                inside = line_start
                    .strip_prefix("[[")
                    .and_then(|rest| rest.split_once("]]"))
                    .is_some_and(|(name, _)| name.trim() == "zones");
            }
            if !inside {
                kept += line;
                kept.push('\n');
            }
        }
        let mut new = kept.trim_end().to_owned();
        new.push('\n');
        if !zones.is_empty() {
            #[derive(Serialize)]
            struct Tables<'a> {
                zones: &'a [ZoneSet],
            }
            new.push('\n');
            new += &toml::to_string_pretty(&Tables { zones })
                .map_err(|e| Error::InvalidArgument(e.to_string()))?;
        }
        if toml::from_str::<Self>(&new).is_ok_and(|read| read == config) {
            crate::atomic::write(&path, new.as_bytes())
        } else {
            config.write(root)
        }
    }

    /// The workspace directory of builds: [`BuildSettings::workspace`]
    /// against `root`, or `None` for the game's default.
    pub fn workspace_dir(&self, root: &Path) -> Option<PathBuf> {
        self.build.workspace.as_ref().map(|w| root.join(w))
    }
}

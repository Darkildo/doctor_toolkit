//! Source manifests: `stream.toml` in each stream folder and `shared.toml`
//! in `shared/` — the resources of the folder, their files and metadata.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::workspace::ResourceKey;
use crate::{Error, Result};

use super::config::FORMAT;

/// Manifest file of a stream folder.
pub const STREAM_MANIFEST: &str = "stream.toml";
/// Manifest file of the shared folder.
pub const SHARED_MANIFEST: &str = "shared.toml";

/// How a resource is kept as files and built back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// A texture (`tg4h` + `tg4d`) as PNG or DDS; built like `texture-import`.
    Texture,
    /// A string table (`lh2`) as JSON (`text-export`); the strings are
    /// written into the table as it was when the project was made.
    Strings,
    /// Game logic (`simgroup`, `metamodel`, `variabledictionary`, `partsdb`)
    /// as JSON (`logic-export`); the values are written into the resource as
    /// it was when the project was made, then the patch of `patches/` (SimGroup).
    Logic,
    /// A whole document as JSON: dialogue (`dlg`), speech events (`chv`),
    /// cut-scene shot (`trinity_seq_master`), navigation graph (`graph`).
    Document,
    /// A Lua script as source; compiled for the game.
    Script,
    /// A model (`mesh` + `meshvolatile`) as glTF, FBX, OBJ or USD; imported
    /// like `model-import`.
    Model,
    /// An animation bank (`bnk`): the data (`.bin`) and the events of its
    /// clips as JSON.
    Animations,
    /// The data of a resource as it is in the stream.
    Raw,
    /// A conversion for reading only (JSON of `rcb`, `mobface`);
    /// the resource is built from its raw file.
    View,
}

impl Kind {
    /// Whether the files are built into resources.
    pub fn is_built(self) -> bool {
        self != Self::View
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Texture => "texture",
            Self::Strings => "strings",
            Self::Logic => "logic",
            Self::Document => "document",
            Self::Script => "script",
            Self::Model => "model",
            Self::Animations => "animations",
            Self::Raw => "raw",
            Self::View => "view",
        }
    }
}

/// One resource (or a texture or model made of two) and its files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Name of the resource from the game index, or its id.
    pub name: String,
    pub kind: Kind,
    /// Type tag of the main resource (`tg4h`, `mesh`, `simgroup`, ...).
    #[serde(rename = "type")]
    pub type_tag: String,
    /// The four ids of the main resource (its GUID in the stream), one
    /// hexadecimal number when they are equal.
    pub ids: String,
    /// Memory policy of the main resource.
    pub policy: String,
    /// Files, relative to the folder of the manifest; the first is the main one.
    pub files: Vec<String>,
    /// Resources the files are built into; in `shared.toml` the same
    /// resources in every stream that has them.
    pub keys: Vec<ResourceKey>,
    /// Digest of the files as written when the project was made
    /// ([`super::cache::digest`]).
    pub exported: String,
    /// SHA-256 of the resource data the files are applied to (strings and
    /// logic), kept in `.doctor/base/`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
}

impl Entry {
    /// The keys grouped by stream, in order: the main resource first.
    pub fn targets(&self) -> Vec<(String, Vec<ResourceKey>)> {
        let mut out: Vec<(String, Vec<ResourceKey>)> = Vec::new();
        for key in &self.keys {
            match out.iter_mut().find(|(s, _)| *s == key.stream) {
                Some((_, keys)) => keys.push(key.clone()),
                None => out.push((key.stream.clone(), vec![key.clone()])),
            }
        }
        out
    }
}

/// `stream.toml` or `shared.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceManifest {
    pub format: u32,
    /// Stream of a stream folder; absent in `shared.toml`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<String>,
    #[serde(default, rename = "resource")]
    pub resources: Vec<Entry>,
}

impl SourceManifest {
    pub fn new(stream: Option<String>) -> Self {
        Self {
            format: FORMAT,
            stream,
            resources: Vec::new(),
        }
    }

    pub fn read(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
        let manifest: Self = toml::from_str(&text)
            .map_err(|e| Error::InvalidArgument(format!("{}: {e}", path.display())))?;
        if manifest.format > FORMAT {
            return Err(Error::InvalidArgument(format!(
                "{} has format {}; this tool reads up to {FORMAT}",
                path.display(),
                manifest.format
            )));
        }
        Ok(manifest)
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
        }
        let text =
            toml::to_string_pretty(self).map_err(|e| Error::InvalidArgument(e.to_string()))?;
        crate::atomic::write(path, text.as_bytes())
    }
}

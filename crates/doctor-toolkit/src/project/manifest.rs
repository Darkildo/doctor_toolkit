//! Source manifests: `stream.toml` in each stream folder and `shared.toml`
//! in `shared/` — the resources of the folder, their files and metadata.
//!
//! A manifest also holds what the project makes that the game does not
//! have (stage 112): a resource the workspace **adds** carries where and how
//! it goes into its stream ([`AddedSource`], `[[resource.added]]`), and the
//! folder of a stream the project **creates** carries the description of
//! that stream ([`NewStreamSource`], `[new]`). Their ids are written, never
//! made again: the key of an added resource and the GUID of a new stream are
//! what the rest of the mod refers to.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::resource::names::mem_policies;
use crate::workspace::{Added, NewStream, ResourceKey, StorageChoice};
use crate::zone_ops::StreamLoad;
use crate::{Error, Result};

use super::config::FORMAT;

/// `format` of a manifest that has added resources or describes a new
/// stream. A program older than stage 112 would take such a folder for
/// resources of the game; it refuses this format instead.
pub const FORMAT_ADDED: u32 = 2;

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

/// Where and how a resource the project adds goes into its stream: what
/// [`Added`] holds in the workspace, written so that a person can read and
/// write it.
///
/// ```toml
/// [[resource.added]]
/// key = "gf2/town/yard:mesh:5a1c03f2"
/// policy = "default"
/// alignment = 16
/// before = "gf2/town/yard:simgroup:00000072"   # absent: at the end
/// order = 3000001
/// storage = "refpack"                           # absent: not compressed
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddedSource {
    /// The resource: one of the keys of its entry. Its type and ids are the
    /// resource's own, so they are never made again by a build.
    pub key: ResourceKey,
    /// Memory policy: a name (`default`, `volatile`, ...) or its hash in
    /// hexadecimal.
    #[serde(default = "default_policy")]
    pub policy: String,
    #[serde(default = "default_alignment")]
    pub alignment: u32,
    /// The resource of the game's stream it is placed before; at the end of
    /// the stream when absent. A stream the project creates has no places:
    /// its resources are written by `order`, SimGroups last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<ResourceKey>,
    /// Order among the resources added at the same place (in a new stream:
    /// among all of them): lower first, equal ones by key. The editor counts
    /// it from its history (operation × 1 000 000 + position in it).
    #[serde(default)]
    pub order: u64,
    /// `refpack` for a compressed resource; not compressed when absent.
    #[serde(default, skip_serializing_if = "StorageChoice::is_keep")]
    pub storage: StorageChoice,
}

fn default_policy() -> String {
    "default".into()
}

fn default_alignment() -> u32 {
    16
}

impl AddedSource {
    /// The record of a resource the workspace adds.
    pub fn of(key: &ResourceKey, added: &Added, storage: StorageChoice) -> Self {
        Self {
            key: key.clone(),
            policy: mem_policies().display(added.mem_policy),
            alignment: added.alignment,
            before: added.before.clone(),
            order: added.order,
            storage,
        }
    }

    /// What the workspace records for the resource.
    pub fn to_added(&self) -> Result<Added> {
        let mem_policy = mem_policies().resolve(&self.policy).ok_or_else(|| {
            Error::InvalidArgument(format!(
                "{}: unknown memory policy `{}`",
                self.key, self.policy
            ))
        })?;
        Ok(Added {
            mem_policy,
            alignment: self.alignment,
            before: self.before.clone(),
            order: self.order,
        })
    }
}

/// The description of a stream the project creates: `[new]` of the
/// `stream.toml` in its folder. The name is `stream` of the manifest.
///
/// ```toml
/// [new]
/// guid = "58444f43544f52544b58b1d0a3c7e942"
/// parent = "gf2/town"
/// load = "on_demand"            # or with_parent, auto_load
/// tocs = ["gf2/level.toc"]
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewStreamSource {
    /// 128-bit GUID as 32 hexadecimal digits; the tables of contents and
    /// the world grid know the stream by it.
    pub guid: String,
    /// The hash of the stream in the tables of contents, in hexadecimal,
    /// when it is not the fold of the GUID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    /// The stream it depends on, loaded before it.
    pub parent: String,
    /// How the game comes to load it.
    #[serde(default)]
    pub load: StreamLoad,
    /// Tables of contents that list it, relative paths.
    pub tocs: Vec<String>,
    /// Another parent in some tables of contents (path → stream).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub toc_parents: BTreeMap<String, String>,
}

impl NewStreamSource {
    /// The description of a stream the workspace creates.
    pub fn of(stream: &NewStream) -> Self {
        let fold = crate::create::parse_guid(&stream.guid)
            .ok()
            .map(|guid| crate::toc::guid_hash(&guid));
        Self {
            guid: stream.guid.clone(),
            hash: (fold != Some(stream.hash)).then(|| format!("{:08x}", stream.hash)),
            parent: stream.parent.clone(),
            load: match (stream.with_parent, stream.auto_load) {
                (false, _) => StreamLoad::OnDemand,
                (true, false) => StreamLoad::WithParent,
                (true, true) => StreamLoad::AutoLoad,
            },
            tocs: stream.tocs.clone(),
            toc_parents: stream.toc_parents.clone(),
        }
    }

    /// Digest of the description, for the build cache: whether
    /// `stream.toml` still says what was built.
    pub fn digest(&self) -> String {
        let text = toml::to_string(self).unwrap_or_default();
        crate::workspace::store::sha256_hex(text.as_bytes())
    }

    /// The stream `name` for [`crate::workspace::Workspace::put_stream`].
    pub fn to_stream(&self, name: &str) -> Result<NewStream> {
        let guid = crate::create::parse_guid(&self.guid)?;
        let hash = match &self.hash {
            Some(text) => u32::from_str_radix(text.trim_start_matches("0x"), 16).map_err(|_| {
                Error::InvalidArgument(format!("`{text}`: the hash is a hexadecimal number"))
            })?,
            None => crate::toc::guid_hash(&guid),
        };
        Ok(NewStream {
            name: name.to_owned(),
            guid: self.guid.trim().to_ascii_lowercase(),
            hash,
            parent: self.parent.clone(),
            with_parent: self.load != StreamLoad::OnDemand,
            auto_load: self.load == StreamLoad::AutoLoad,
            tocs: self.tocs.clone(),
            toc_parents: self.toc_parents.clone(),
            label: String::new(),
        })
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
    /// ([`super::cache::digest`]); empty in an entry written by hand.
    #[serde(default)]
    pub exported: String,
    /// SHA-256 of the resource data the files are applied to (strings and
    /// logic; the compiled script of an added `lua`), kept in `.doctor/base/`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// SHA-256 of the data of each key when the files were written, for a
    /// texture or a model **the workspace made** (added, or imported over
    /// the game's), kept in `.doctor/base/`. A picture and a model file are
    /// not the resource: encoding them gives other bytes each time. So
    /// files that did not change since they were written build exactly this
    /// data, and changed ones are encoded over it — the header of an added
    /// texture (its format and name are not in a picture), the mesh (bones,
    /// vertex formats and what the tool does not write anew). A texture or
    /// model of the unmodified game has none: it is made over the game's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub template: Vec<String>,
    /// SHA-256 of the data each key had in the workspace the files were
    /// written from, empty for a resource that workspace did not change;
    /// left out when it changed none (files of the unmodified game). With
    /// it a project builds into a workspace that has no build cache yet (a
    /// checkout of the sources): what the workspace lacks is built.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<String>,
    /// How the data of each key is stored in its stream (`keep`, `raw`,
    /// `refpack`), when the workspace asked for another storage than the
    /// game's resource has; left out when it did not. An added key has its
    /// storage in its record of `added`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub storage: Vec<StorageChoice>,
    /// How the keys the project **adds** go into their streams; the keys
    /// without a record are resources of the game. Written last: a table.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub added: Vec<AddedSource>,
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

    /// The storage the entry asks for the game's resource `key`.
    pub fn storage_of(&self, key: &ResourceKey) -> StorageChoice {
        self.keys
            .iter()
            .position(|k| k == key)
            .and_then(|at| self.storage.get(at))
            .copied()
            .unwrap_or_default()
    }

    /// The record of `key` when the project adds it.
    pub fn added_record(&self, key: &ResourceKey) -> Option<&AddedSource> {
        self.added.iter().find(|a| a.key == *key)
    }

    /// Whether the project adds every resource of the entry: nothing of it
    /// is in the game, so removing its file removes the resources.
    pub fn is_added(&self) -> bool {
        !self.keys.is_empty() && self.keys.iter().all(|k| self.added_record(k).is_some())
    }
}

/// `stream.toml` or `shared.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceManifest {
    pub format: u32,
    /// Stream of a stream folder; absent in `shared.toml`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<String>,
    /// Set when the game does not have the stream: the project creates it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new: Option<NewStreamSource>,
    #[serde(default, rename = "resource")]
    pub resources: Vec<Entry>,
}

impl SourceManifest {
    pub fn new(stream: Option<String>) -> Self {
        Self {
            format: FORMAT,
            stream,
            new: None,
            resources: Vec::new(),
        }
    }

    /// The format a manifest with these contents is written in: the lowest
    /// that holds them, so that a project without added resources stays
    /// readable by the programs that read it before.
    fn format_needed(&self) -> u32 {
        if self.new.is_some() || self.resources.iter().any(|e| !e.added.is_empty()) {
            FORMAT_ADDED
        } else {
            FORMAT
        }
    }

    pub fn read(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
        let manifest: Self = toml::from_str(&text)
            .map_err(|e| Error::InvalidArgument(format!("{}: {e}", path.display())))?;
        if manifest.format > FORMAT_ADDED {
            return Err(Error::InvalidArgument(format!(
                "{} has format {}; this tool reads up to {FORMAT_ADDED}",
                path.display(),
                manifest.format
            )));
        }
        for entry in &manifest.resources {
            if entry.files.is_empty() || entry.keys.is_empty() {
                return Err(Error::InvalidArgument(format!(
                    "{}: the resource `{}` needs `files` and `keys`",
                    path.display(),
                    entry.name
                )));
            }
            if let Some(stray) = entry.added.iter().find(|a| !entry.keys.contains(&a.key)) {
                return Err(Error::InvalidArgument(format!(
                    "{}: `added` of `{}` names {}, which is not one of its keys",
                    path.display(),
                    entry.name,
                    stray.key
                )));
            }
        }
        Ok(manifest)
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
        }
        let mut copy = self.clone();
        copy.format = self.format_needed();
        let text =
            toml::to_string_pretty(&copy).map_err(|e| Error::InvalidArgument(e.to_string()))?;
        crate::atomic::write(path, text.as_bytes())
    }
}

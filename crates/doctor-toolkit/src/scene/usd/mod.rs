//! Pixar USD: `.usda` (text), `.usdc` (crate binary) and `.usdz` (zip
//! package), read and written by own code.
//!
//! Both encodings go through one [`Layer`] model (prims, attributes with
//! default values and time samples, relationships, metadata), mapped to and
//! from a [`Scene`] by [`convert`]:
//!
//! - `UsdGeomMesh` (`faceVertexCounts`/`faceVertexIndices`, `points`,
//!   `normals`, `primvars:st` with any interpolation and indices,
//!   `primvars:displayColor`/`displayOpacity`, `GeomSubset`s per material);
//! - `UsdShade`: `UsdPreviewSurface` with `UsdUVTexture` inputs for the
//!   diffuse colour, normal and specular colour, materials bound with
//!   `material:binding`;
//! - `UsdSkel`: `Skeleton` (`joints`, `bindTransforms`, `restTransforms`),
//!   meshes bound with `skel:skeleton`, `primvars:skel:jointIndices` /
//!   `jointWeights` of any element size (the four largest are kept) and
//!   `geomBindTransform`; `SkelAnimation` clips;
//! - `Xform` transforms (`xformOp:*` in `xformOpOrder`), `metersPerUnit`
//!   and `upAxis` (converted to Y up, metres).
//!
//! Not supported: composition (references, payloads, sublayers, variants
//! are not followed), subdivision surfaces (the cage is read), instancing.

pub mod convert;
pub mod crate_file;
pub mod usda;

use std::collections::HashMap;

use super::{ModelFormat, Scene, SideFiles, WrittenFile};
use crate::{Error, Result};

/// A value of an attribute or metadata field.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Float(f64),
    Token(String),
    String(String),
    Asset(String),
    /// A fixed-size tuple: vectors, colours, quaternions (`[i, j, k, real]`),
    /// matrices (16 values, rows of the USD matrix one after the other).
    Tuple(Vec<f64>),
    /// A numeric array; `width` values per element.
    Array {
        values: Vec<f64>,
        width: usize,
    },
    /// Token, string or asset arrays.
    Strings(Vec<String>),
    /// Paths (`targetPaths`, `connectionPaths`).
    Paths(Vec<String>),
    Dictionary(Vec<(String, Value)>),
    /// A blocked value (`None`).
    Blocked,
}

impl Value {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Bool(b) => Some(*b as u8 as f64),
            Self::Int(i) => Some(*i as f64),
            Self::Float(f) => Some(*f),
            Self::Tuple(v) if v.len() == 1 => Some(v[0]),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Token(s) | Self::String(s) | Self::Asset(s) => Some(s),
            _ => None,
        }
    }

    /// Numbers of a tuple or an array, or one number.
    pub fn numbers(&self) -> Option<&[f64]> {
        match self {
            Self::Tuple(v) => Some(v),
            Self::Array { values, .. } => Some(values),
            Self::Float(f) => Some(std::slice::from_ref(f)),
            _ => None,
        }
    }

    pub fn strings(&self) -> Option<&[String]> {
        match self {
            Self::Strings(v) | Self::Paths(v) => Some(v),
            _ => None,
        }
    }
}

/// An attribute.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Attribute {
    /// `point3f[]`, `token`, `matrix4d`…
    pub type_name: String,
    pub uniform: bool,
    pub custom: bool,
    pub default: Option<Value>,
    /// `(time code, value)`, ascending.
    pub samples: Vec<(f64, Value)>,
    /// Paths of `.connect`.
    pub connections: Vec<String>,
    /// `interpolation`, `elementSize`…
    pub metadata: Vec<(String, Value)>,
}

impl Attribute {
    pub fn new(type_name: &str, default: Value) -> Self {
        Self {
            type_name: type_name.to_owned(),
            default: Some(default),
            ..Default::default()
        }
    }

    pub fn meta(&self, key: &str) -> Option<&Value> {
        self.metadata.iter().find(|m| m.0 == key).map(|m| &m.1)
    }

    /// The default value, else the first time sample.
    pub fn value(&self) -> Option<&Value> {
        self.default
            .as_ref()
            .filter(|v| **v != Value::Blocked)
            .or_else(|| self.samples.first().map(|s| &s.1))
            .filter(|v| **v != Value::Blocked)
    }
}

/// A property of a prim.
#[derive(Debug, Clone, PartialEq)]
pub enum Property {
    Attribute(Attribute),
    /// Target paths.
    Relationship(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Specifier {
    #[default]
    Def,
    Over,
    Class,
}

/// A prim with its properties and children.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Prim {
    pub name: String,
    pub specifier: Specifier,
    /// `Mesh`, `Xform`, `Skeleton`…; empty for typeless prims.
    pub type_name: String,
    /// `apiSchemas` and other prim metadata.
    pub metadata: Vec<(String, Value)>,
    pub properties: Vec<(String, Property)>,
    pub children: Vec<Prim>,
}

impl Prim {
    pub fn new(name: &str, type_name: &str) -> Self {
        Self {
            name: name.to_owned(),
            type_name: type_name.to_owned(),
            ..Default::default()
        }
    }

    pub fn attribute(&self, name: &str) -> Option<&Attribute> {
        self.properties.iter().find_map(|(n, p)| match p {
            Property::Attribute(a) if n == name => Some(a),
            _ => None,
        })
    }

    pub fn relationship(&self, name: &str) -> Option<&[String]> {
        self.properties.iter().find_map(|(n, p)| match p {
            Property::Relationship(t) if n == name => Some(t.as_slice()),
            _ => None,
        })
    }

    pub fn set(&mut self, name: &str, attribute: Attribute) {
        self.properties
            .push((name.to_owned(), Property::Attribute(attribute)));
    }

    pub fn relate(&mut self, name: &str, targets: Vec<String>) {
        self.properties
            .push((name.to_owned(), Property::Relationship(targets)));
    }

    pub fn meta(&self, key: &str) -> Option<&Value> {
        self.metadata.iter().find(|m| m.0 == key).map(|m| &m.1)
    }
}

/// A layer: metadata and root prims.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Layer {
    pub metadata: Vec<(String, Value)>,
    pub prims: Vec<Prim>,
}

impl Layer {
    pub fn meta(&self, key: &str) -> Option<&Value> {
        self.metadata.iter().find(|m| m.0 == key).map(|m| &m.1)
    }

    /// The prim at an absolute path (`/a/b`).
    pub fn prim(&self, path: &str) -> Option<&Prim> {
        let mut list = &self.prims;
        let mut found = None;
        for name in path.split('/').filter(|s| !s.is_empty()) {
            let p = list.iter().find(|p| p.name == name)?;
            found = Some(p);
            list = &p.children;
        }
        found
    }
}

fn bad(message: impl Into<String>) -> Error {
    Error::InvalidArgument(format!("USD: {}", message.into()))
}

/// Parses a `.usda` or `.usdc` layer (by contents).
pub fn parse(bytes: &[u8]) -> Result<Layer> {
    if bytes.starts_with(crate_file::MAGIC) {
        crate_file::read(bytes)
    } else if bytes.starts_with(b"#usda") {
        usda::parse(bytes)
    } else {
        Err(bad(
            "neither a USD text (#usda) nor a crate (PXR-USDC) file",
        ))
    }
}

/// Reads a `.usd`, `.usda` or `.usdc` file; textures through `files`.
pub fn read(bytes: &[u8], files: &SideFiles) -> Result<Scene> {
    let layer = parse(bytes)?;
    convert::to_scene(&layer, files)
}

/// Largest package accepted (bytes of all entries).
const MAX_PACKAGE: u64 = 2 << 30;

/// Reads a `.usdz` package: the first layer in it, textures from it.
pub fn read_usdz(bytes: &[u8]) -> Result<Scene> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| bad(format!("package: {e}")))?;
    let mut files: HashMap<String, Vec<u8>> = HashMap::new();
    let mut root: Option<String> = None;
    let mut total = 0u64;
    // Deflate packs at most ~1032:1.
    let budget = (bytes.len() as u64)
        .saturating_mul(1100)
        .clamp(64 << 20, MAX_PACKAGE);
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| bad(format!("package: {e}")))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry
            .name()
            .map_err(|e| bad(format!("package: {e}")))?
            .into_owned();
        total += entry.size();
        if total > budget {
            return Err(bad("package too large"));
        }
        let mut data = Vec::with_capacity(entry.size().min(budget) as usize);
        let size = entry.size();
        std::io::Read::read_to_end(&mut (&mut entry).take(size), &mut data)
            .map_err(|e| bad(format!("package entry `{name}`: {e}")))?;
        let lower = name.to_ascii_lowercase();
        if root.is_none()
            && (lower.ends_with(".usda") || lower.ends_with(".usdc") || lower.ends_with(".usd"))
        {
            root = Some(name.clone());
        }
        files.insert(name, data);
    }
    let root = root.ok_or_else(|| bad("package has no layer"))?;
    let layer = parse(&files[&root])?;
    let base = root.rsplit_once('/').map_or("", |(dir, _)| dir).to_owned();
    // Asset paths are relative to the root layer.
    let files: HashMap<String, Vec<u8>> = files
        .into_iter()
        .map(|(k, v)| {
            let rel = match k.strip_prefix(&base) {
                Some(rest) if !base.is_empty() => rest.trim_start_matches('/').to_owned(),
                _ => k,
            };
            (rel, v)
        })
        .collect();
    convert::to_scene(&layer, &SideFiles::in_memory(files))
}

use std::io::Read as _;

/// Writes `stem.usd[a|c]` with textures under `textures` (a relative
/// folder such as `textures/`).
pub fn write_files(
    scene: &Scene,
    stem: &str,
    format: ModelFormat,
    textures: &str,
) -> Result<Vec<WrittenFile>> {
    let (layer, textures) = convert::from_scene(scene, textures)?;
    let bytes = match format {
        ModelFormat::Usda => usda::write(&layer).into_bytes(),
        _ => crate_file::write(&layer)?,
    };
    let mut out = vec![WrittenFile {
        name: format!("{stem}.{}", format.extension()),
        bytes,
    }];
    out.extend(textures);
    Ok(out)
}

/// Writes a `.usdz` package: a crate layer and its textures, stored
/// uncompressed with every file aligned to 64 bytes.
pub fn write_usdz(scene: &Scene) -> Result<Vec<u8>> {
    let (layer, textures) = convert::from_scene(scene, "textures/")?;
    let mut files = vec![("model.usdc".to_owned(), crate_file::write(&layer)?)];
    files.extend(textures.into_iter().map(|t| (t.name, t.bytes)));
    Ok(super::zipfile::write_stored(&files, 64))
}

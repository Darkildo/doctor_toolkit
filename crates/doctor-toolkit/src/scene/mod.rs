//! A format-neutral 3D scene: meshes, materials, a skeleton and animation
//! clips, with readers and writers for glTF 2.0, FBX, OBJ and USD.
//!
//! Every reader produces a [`Scene`] in the game's space: **Y up, right
//! handed, metres, UV origin at the top left** (Direct3D, like glTF). Unit
//! scale, up axis and handedness of the file are converted on reading, the
//! texture V axis is flipped for formats with the origin at the bottom left
//! (FBX, OBJ, USD). Writers do the reverse. The model importer
//! ([`crate::import::model`]) consumes a [`Scene`], so all formats share the
//! same planning: bone-name matching, four weights, materials, collision.
//!
//! - Meshes are flat [`ScenePart`]s, one per material, with their node
//!   transforms applied; skinned parts are in bind space, where
//!   [`SceneJoint::inverse_bind`] maps a point into the joint's space.
//! - The skeleton is one list of joints with parents (several skins are
//!   merged by joint name).
//! - Animation clips are sampled at a constant rate ([`SceneClip`]), one
//!   track of local (parent-space) transforms per animated joint;
//!   [`anim`] maps them to and from the game skeleton.
//!
//! | Format | Read | Write |
//! |---|---|---|
//! | glTF 2.0 | `.glb`, `.gltf` ([`crate::import::gltf`]) | `.glb` ([`crate::export::gltf`]) |
//! | FBX | binary 6.1–7.7 (zlib arrays), ASCII ([`fbx`]) | binary 7.4 |
//! | OBJ + MTL | [`obj`] | [`obj`] (static meshes only) |
//! | USD | `.usda`, `.usdc`, `.usdz` ([`usd`]) | all three |

pub mod anim;
pub mod fbx;
pub mod game;
pub mod math;
pub mod obj;
pub mod retarget;
#[cfg(test)]
mod retarget_tests;
pub mod usd;
mod zipfile;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::resource::mesh_build::PartGeometry;
use crate::resource::rcb::Mat4;
use crate::{Error, Result};

pub use math::IDENTITY;

/// An image referenced by a material, still encoded (PNG or JPEG).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneImage {
    pub name: String,
    pub mime_type: String,
    pub bytes: Vec<u8>,
}

impl SceneImage {
    /// A PNG image.
    pub fn png(name: impl Into<String>, bytes: Vec<u8>) -> Self {
        Self {
            name: name.into(),
            mime_type: "image/png".into(),
            bytes,
        }
    }

    /// File extension from the contents: `png`, `jpg` or `bin`.
    pub fn extension(&self) -> &'static str {
        if self.bytes.starts_with(b"\x89PNG") {
            "png"
        } else if self.bytes.starts_with(&[0xFF, 0xD8]) {
            "jpg"
        } else {
            "bin"
        }
    }

    /// Decodes the image to RGBA8.
    pub fn rgba(&self) -> Result<(usize, usize, Vec<u8>)> {
        if self.bytes.starts_with(b"\x89PNG") {
            return crate::resource::pixels::decode_png(&self.bytes);
        }
        if self.bytes.starts_with(&[0xFF, 0xD8]) {
            let mut decoder = jpeg_decoder::Decoder::new(&self.bytes[..]);
            let pixels = decoder
                .decode()
                .map_err(|e| Error::InvalidArgument(format!("image `{}`: {e}", self.name)))?;
            let info = decoder
                .info()
                .ok_or_else(|| Error::InvalidArgument(format!("image `{}`", self.name)))?;
            let (w, h) = (info.width as usize, info.height as usize);
            let rgba = match info.pixel_format {
                jpeg_decoder::PixelFormat::RGB24 => pixels
                    .chunks_exact(3)
                    .flat_map(|p| [p[0], p[1], p[2], 255])
                    .collect(),
                jpeg_decoder::PixelFormat::L8 => {
                    pixels.iter().flat_map(|&l| [l, l, l, 255]).collect()
                }
                other => {
                    return Err(Error::InvalidArgument(format!(
                        "image `{}`: JPEG pixel format {other:?} is not supported",
                        self.name
                    )));
                }
            };
            return Ok((w, h, rgba));
        }
        Err(Error::InvalidArgument(format!(
            "image `{}` ({}) is neither PNG nor JPEG",
            self.name, self.mime_type
        )))
    }

    /// A key identifying the contents, to write each image once.
    pub(crate) fn key(&self) -> (usize, u64) {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.bytes.hash(&mut h);
        (self.bytes.len(), h.finish())
    }
}

/// A material: its name, the images of the roles the game uses and factors.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneMaterial {
    pub name: String,
    pub base_color: Option<SceneImage>,
    pub normal: Option<SceneImage>,
    pub specular: Option<SceneImage>,
    /// RGBA, linear.
    pub base_color_factor: [f32; 4],
    /// Alpha mask or blending.
    pub alpha: bool,
}

impl Default for SceneMaterial {
    fn default() -> Self {
        Self {
            name: String::new(),
            base_color: None,
            normal: None,
            specular: None,
            base_color_factor: [1.0; 4],
            alpha: false,
        }
    }
}

/// One triangle mesh with one material.
#[derive(Debug, Clone, PartialEq)]
pub struct ScenePart {
    /// Part name: the mesh name, with the primitive or material number when
    /// a mesh has several.
    pub name: String,
    /// Name of the object (node, mesh) the part belongs to; writers group
    /// consecutive parts of one object.
    pub object: String,
    /// Index into [`Scene::materials`].
    pub material: Option<usize>,
    /// Geometry; `skin` joints index [`Scene::joints`], material hash 0.
    pub geometry: PartGeometry,
}

/// A joint of the skeleton.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneJoint {
    pub name: String,
    /// Index into [`Scene::joints`], always lower than the joint's own.
    pub parent: Option<usize>,
    /// Rest transform in the parent joint's space (model space for roots).
    pub local: Mat4,
    /// Model space → joint space in the bind pose (column-major).
    pub inverse_bind: Mat4,
}

/// Transforms of one joint over a clip, one key per frame (or one key for
/// a constant value).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct JointTrack {
    /// Index into [`Scene::joints`].
    pub joint: usize,
    /// Quaternions `[x, y, z, w]` in parent space.
    pub rotations: Vec<[f32; 4]>,
    pub translations: Vec<[f32; 3]>,
    /// Empty: unit scale.
    pub scales: Vec<[f32; 3]>,
}

/// An animation clip sampled at a constant rate.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SceneClip {
    pub name: String,
    pub frame_rate: f32,
    pub tracks: Vec<JointTrack>,
}

impl SceneClip {
    /// Number of frames (the longest track).
    pub fn frames(&self) -> usize {
        self.tracks
            .iter()
            .map(|t| {
                t.rotations
                    .len()
                    .max(t.translations.len())
                    .max(t.scales.len())
            })
            .max()
            .unwrap_or(0)
    }
}

/// A scene in the game's space (Y up, metres, UV origin top left).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scene {
    /// Name of the first mesh (or of the file).
    pub name: String,
    pub parts: Vec<ScenePart>,
    pub materials: Vec<SceneMaterial>,
    pub joints: Vec<SceneJoint>,
    pub clips: Vec<SceneClip>,
    pub warnings: Vec<String>,
    /// Joints the file gives no bind pose for (FBX without a `BindPose`
    /// or skin cluster for them); their bind pose is their rest transform.
    pub joints_without_bind: Vec<usize>,
}

impl Scene {
    /// Index of the joint called `name`.
    pub fn joint(&self, name: &str) -> Option<usize> {
        self.joints.iter().position(|j| j.name == name)
    }

    /// Joint → model space transforms in the rest pose, from the locals.
    pub fn rest_globals(&self) -> Vec<Mat4> {
        let mut out: Vec<Mat4> = Vec::with_capacity(self.joints.len());
        for joint in &self.joints {
            let world = match joint.parent.and_then(|p| out.get(p)) {
                Some(parent) => math::multiply(parent, &joint.local),
                None => joint.local,
            };
            out.push(world);
        }
        out
    }

    /// Joint → parent transforms of the bind pose (from the inverse bind matrices).
    pub fn bind_locals(&self) -> Vec<Mat4> {
        self.joints
            .iter()
            .map(|joint| {
                let global = math::invert(&joint.inverse_bind);
                match joint.parent {
                    Some(p) => math::multiply(&self.joints[p].inverse_bind, &global),
                    None => global,
                }
            })
            .collect()
    }

    /// Whether any part has bone weights.
    pub fn is_skinned(&self) -> bool {
        self.parts.iter().any(|p| p.geometry.skin.is_some())
    }

    /// Checks the internal consistency of the scene (indices in range), as
    /// readers of untrusted files must guarantee.
    pub fn validate(&self) -> Result<()> {
        let bad = |m: String| Err(Error::InvalidArgument(m));
        for (i, j) in self.joints.iter().enumerate() {
            if j.parent.is_some_and(|p| p >= i) {
                return bad(format!("joint `{}`: parent after the joint", j.name));
            }
        }
        for part in &self.parts {
            let g = &part.geometry;
            let n = g.positions.len();
            if part.material.is_some_and(|m| m >= self.materials.len()) {
                return bad(format!("part `{}`: material out of range", part.name));
            }
            if g.triangles.iter().flatten().any(|&i| i as usize >= n) {
                return bad(format!("part `{}`: index out of range", part.name));
            }
            let lengths = [
                g.normals.as_ref().map(Vec::len),
                g.tangents.as_ref().map(Vec::len),
                g.uv0.as_ref().map(Vec::len),
                g.uv1.as_ref().map(Vec::len),
                g.colors.as_ref().map(Vec::len),
                g.skin.as_ref().map(Vec::len),
            ];
            if lengths.iter().flatten().any(|&l| l != n) {
                return bad(format!("part `{}`: attribute lengths differ", part.name));
            }
            if g.skin
                .iter()
                .flatten()
                .any(|(j, w)| (0..4).any(|k| w[k] > 0.0 && j[k] as usize >= self.joints.len()))
            {
                return bad(format!("part `{}`: joint out of range", part.name));
            }
        }
        for clip in &self.clips {
            if clip.tracks.iter().any(|t| t.joint >= self.joints.len()) {
                return bad(format!("clip `{}`: joint out of range", clip.name));
            }
        }
        Ok(())
    }
}

/// A model file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelFormat {
    /// Binary glTF 2.0.
    Glb,
    /// glTF 2.0 with JSON (read only).
    Gltf,
    /// Binary FBX 7.4 (reads binary and ASCII).
    Fbx,
    /// Wavefront OBJ with an MTL file.
    Obj,
    /// USD crate (binary) in a `.usd` file.
    Usd,
    /// USD text.
    Usda,
    /// USD crate (binary).
    Usdc,
    /// USD zip package.
    Usdz,
}

impl ModelFormat {
    /// Formats that can be written, in menu order.
    pub const WRITABLE: [Self; 7] = [
        Self::Glb,
        Self::Fbx,
        Self::Obj,
        Self::Usd,
        Self::Usda,
        Self::Usdc,
        Self::Usdz,
    ];

    /// File extensions read by [`read_file`].
    pub const READ_EXTENSIONS: [&'static str; 8] =
        ["glb", "gltf", "fbx", "obj", "usd", "usda", "usdc", "usdz"];

    /// The format of a file extension (without the dot, any case).
    pub fn from_extension(ext: &str) -> Option<Self> {
        Some(match ext.to_ascii_lowercase().as_str() {
            "glb" => Self::Glb,
            "gltf" => Self::Gltf,
            "fbx" => Self::Fbx,
            "obj" => Self::Obj,
            "usd" => Self::Usd,
            "usda" => Self::Usda,
            "usdc" => Self::Usdc,
            "usdz" => Self::Usdz,
            _ => return None,
        })
    }

    /// The format of a path, by extension.
    pub fn from_path(path: &Path) -> Option<Self> {
        path.extension()
            .and_then(|e| e.to_str())
            .and_then(Self::from_extension)
    }

    /// Format names accepted by [`std::str::FromStr`].
    pub fn parse_name(name: &str) -> Option<Self> {
        Self::from_extension(name.trim_start_matches('.'))
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Glb => "glb",
            Self::Gltf => "gltf",
            Self::Fbx => "fbx",
            Self::Obj => "obj",
            Self::Usd => "usd",
            Self::Usda => "usda",
            Self::Usdc => "usdc",
            Self::Usdz => "usdz",
        }
    }

    /// Human-readable name for menus.
    pub fn label(self) -> &'static str {
        match self {
            Self::Glb => "glTF binary (.glb)",
            Self::Gltf => "glTF (.gltf)",
            Self::Fbx => "FBX 7.4 binary (.fbx)",
            Self::Obj => "Wavefront OBJ + MTL (.obj)",
            Self::Usd => "USD crate (.usd)",
            Self::Usda => "USD text (.usda)",
            Self::Usdc => "USD crate (.usdc)",
            Self::Usdz => "USD package (.usdz)",
        }
    }

    /// Whether the format keeps skeletons and skin weights.
    pub fn has_skeletons(self) -> bool {
        self != Self::Obj
    }
}

impl std::str::FromStr for ModelFormat {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        Self::parse_name(s).ok_or_else(|| {
            Error::InvalidArgument(format!(
                "unknown model format `{s}` (glb, fbx, obj, usd, usda, usdc, usdz)"
            ))
        })
    }
}

impl std::fmt::Display for ModelFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.extension())
    }
}

/// A file produced by [`write`]: a name relative to the main file's folder
/// and its contents. The first file is the main one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrittenFile {
    pub name: String,
    pub bytes: Vec<u8>,
}

/// Writes a scene in `format`. `stem` names the main file (`stem.ext`) and
/// the files next to it (OBJ material library and textures, USD textures).
pub fn write(scene: &Scene, format: ModelFormat, stem: &str) -> Result<Vec<WrittenFile>> {
    write_with_textures(scene, format, stem, "textures/")
}

/// [`write`] with the side textures (OBJ, `.usd`/`.usda`/`.usdc`) under
/// `textures` (a relative folder ending with `/`) instead of `textures/`.
pub fn write_with_textures(
    scene: &Scene,
    format: ModelFormat,
    stem: &str,
    textures: &str,
) -> Result<Vec<WrittenFile>> {
    let main = |bytes| {
        vec![WrittenFile {
            name: format!("{stem}.{}", format.extension()),
            bytes,
        }]
    };
    Ok(match format {
        ModelFormat::Glb | ModelFormat::Gltf => main(crate::export::gltf::write_glb(scene)?),
        ModelFormat::Fbx => main(fbx::write(scene)?),
        ModelFormat::Obj => obj::write(scene, stem, textures)?,
        ModelFormat::Usdz => main(usd::write_usdz(scene)?),
        ModelFormat::Usd | ModelFormat::Usda | ModelFormat::Usdc => {
            usd::write_files(scene, stem, format, textures)?
        }
    })
}

/// A scene as one file in memory: `.glb`, `.fbx` and `.usdz` hold their
/// textures; `.usd`/`.usda`/`.usdc` reference them as `textures/…` files
/// that are not included (use [`write`] for those); OBJ needs a material
/// library and is refused.
pub fn to_bytes(scene: &Scene, format: ModelFormat) -> Result<Vec<u8>> {
    if format == ModelFormat::Obj {
        return Err(Error::InvalidArgument(
            "OBJ is several files (.obj, .mtl, textures); write it to a folder".into(),
        ));
    }
    Ok(write(scene, format, "model")?.swap_remove(0).bytes)
}

/// Writes a scene to `path` (format from the extension, or `format`), with
/// the side files next to it. Returns the paths written.
pub fn write_file(scene: &Scene, path: &Path, format: Option<ModelFormat>) -> Result<Vec<PathBuf>> {
    let format = format
        .or_else(|| ModelFormat::from_path(path))
        .ok_or_else(|| {
            Error::InvalidArgument(format!(
                "{}: unknown model extension (glb, fbx, obj, usd, usda, usdc, usdz)",
                path.display()
            ))
        })?;
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("model")
        .to_owned();
    let dir = path.parent().unwrap_or(Path::new(""));
    let files = write(scene, format, &stem)?;
    let mut written = Vec::new();
    for (k, file) in files.iter().enumerate() {
        let target = if k == 0 {
            path.to_owned()
        } else {
            dir.join(&file.name)
        };
        if let Some(parent) = target.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
        std::fs::write(&target, &file.bytes).map_err(|e| Error::io(&target, e))?;
        written.push(target);
    }
    Ok(written)
}

/// Reads a model file of any supported format (by extension; `.usd` by
/// contents). Side files (buffers, material libraries, textures) are looked
/// up next to it and may not leave its folder.
pub fn read_file(path: &Path) -> Result<Scene> {
    let format = ModelFormat::from_path(path).ok_or_else(|| {
        Error::InvalidArgument(format!(
            "{}: unknown model extension (glb, gltf, fbx, obj, usd, usda, usdc, usdz)",
            path.display()
        ))
    })?;
    let bytes = std::fs::read(path).map_err(|e| Error::io(path, e))?;
    let base = path.parent();
    let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("model");
    let mut scene = read(&bytes, format, base, name)?;
    if scene.name.is_empty() {
        scene.name = name.to_owned();
    }
    Ok(scene)
}

/// Reads a model from memory. `base` resolves side files (none: only
/// embedded data); `name` names the scene when the file has no name.
pub fn read(bytes: &[u8], format: ModelFormat, base: Option<&Path>, name: &str) -> Result<Scene> {
    let files = SideFiles::new(base);
    let scene = match format {
        ModelFormat::Glb | ModelFormat::Gltf => crate::import::gltf::read(bytes, base)?,
        ModelFormat::Fbx => fbx::read(bytes, &files)?,
        ModelFormat::Obj => obj::read(bytes, &files, name)?,
        ModelFormat::Usd | ModelFormat::Usda | ModelFormat::Usdc => usd::read(bytes, &files)?,
        ModelFormat::Usdz => usd::read_usdz(bytes)?,
    };
    scene.validate()?;
    Ok(scene)
}

/// Side files of a model: textures, material libraries, buffers. Paths are
/// relative to the model's folder and may not leave it; a path that does
/// (or an absolute one) is tried by its file name in the model's folder.
#[derive(Debug, Clone, Default)]
pub struct SideFiles {
    base: Option<PathBuf>,
    /// Files in memory (USDZ packages), by normalised path.
    memory: HashMap<String, Vec<u8>>,
}

/// Largest side file read (textures, buffers).
const MAX_SIDE_FILE: u64 = 512 << 20;

impl SideFiles {
    pub fn new(base: Option<&Path>) -> Self {
        Self {
            base: base.map(Path::to_owned),
            memory: HashMap::new(),
        }
    }

    /// Files held in memory (a package).
    pub fn in_memory(files: HashMap<String, Vec<u8>>) -> Self {
        Self {
            base: None,
            memory: files.into_iter().map(|(k, v)| (normalize(&k), v)).collect(),
        }
    }

    /// Reads a side file. `uri` may use `\` or `/` and be absolute.
    pub fn read(&self, uri: &str) -> Result<Vec<u8>> {
        let uri = uri.trim();
        let wanted = normalize(uri);
        let file_name = wanted.rsplit('/').next().unwrap_or(&wanted).to_owned();
        if !self.memory.is_empty() {
            let found = self
                .memory
                .get(&wanted)
                .or_else(|| {
                    self.memory
                        .iter()
                        .find(|(k, _)| k.rsplit('/').next() == Some(file_name.as_str()))
                        .map(|(_, v)| v)
                })
                .ok_or_else(|| {
                    Error::InvalidArgument(format!("file `{uri}` not in the package"))
                })?;
            return Ok(found.clone());
        }
        let base = self
            .base
            .as_ref()
            .ok_or_else(|| Error::InvalidArgument(format!("external file `{uri}`")))?;
        let safe = !wanted.is_empty()
            && !wanted.split('/').any(|c| c == "..")
            && !uri.starts_with('/')
            && !uri.starts_with('\\')
            && !wanted.contains(':');
        let mut candidates = Vec::new();
        if safe {
            candidates.push(base.join(&wanted));
        }
        if !file_name.is_empty() && file_name != ".." {
            candidates.push(base.join(&file_name));
        }
        for path in &candidates {
            if let Ok(meta) = std::fs::metadata(path)
                && meta.is_file()
            {
                if meta.len() > MAX_SIDE_FILE {
                    return Err(Error::InvalidArgument(format!("file `{uri}` is too large")));
                }
                return std::fs::read(path).map_err(|e| Error::io(path, e));
            }
        }
        Err(Error::InvalidArgument(format!(
            "file `{uri}` not found in {}",
            base.display()
        )))
    }

    /// Reads an image file into a [`SceneImage`].
    pub fn image(&self, uri: &str) -> Result<SceneImage> {
        let bytes = self.read(uri)?;
        let name = normalize(uri)
            .rsplit('/')
            .next()
            .unwrap_or("")
            .rsplit_once('.')
            .map_or_else(|| uri.to_owned(), |(stem, _)| stem.to_owned());
        let mime_type = if bytes.starts_with(b"\x89PNG") {
            "image/png"
        } else if bytes.starts_with(&[0xFF, 0xD8]) {
            "image/jpeg"
        } else {
            ""
        };
        Ok(SceneImage {
            name,
            mime_type: mime_type.into(),
            bytes,
        })
    }
}

/// Forward slashes, no `./`, percent-decoded.
fn normalize(uri: &str) -> String {
    let decoded = percent_decode(uri).replace('\\', "/");
    decoded
        .split('/')
        .filter(|c| !c.is_empty() && *c != ".")
        .collect::<Vec<_>>()
        .join("/")
}

pub(crate) fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A name usable as a file name (letters, digits, `_`, `-`, `.`).
pub fn file_name(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() || s.starts_with('.') {
        format!("_{s}")
    } else {
        s
    }
}

/// Images of the materials of a scene, each once, with unique file names
/// `prefix/name.ext`: `(material, role) → file`, and the files.
pub(crate) struct ImageFiles {
    pub files: Vec<WrittenFile>,
    names: HashMap<(usize, u64), String>,
}

impl ImageFiles {
    pub fn new(scene: &Scene, prefix: &str) -> Self {
        let mut files: Vec<WrittenFile> = Vec::new();
        let mut names = HashMap::new();
        let mut used: HashMap<String, usize> = HashMap::new();
        for material in &scene.materials {
            for image in [&material.base_color, &material.normal, &material.specular]
                .into_iter()
                .flatten()
            {
                let key = image.key();
                if names.contains_key(&key) {
                    continue;
                }
                let base = file_name(if image.name.is_empty() {
                    "texture"
                } else {
                    &image.name
                });
                let n = used.entry(base.to_ascii_lowercase()).or_insert(0);
                *n += 1;
                let stem = if *n == 1 { base } else { format!("{base}_{n}") };
                let name = format!("{prefix}{stem}.{}", image.extension());
                names.insert(key, name.clone());
                files.push(WrittenFile {
                    name,
                    bytes: image.bytes.clone(),
                });
            }
        }
        Self { files, names }
    }

    /// The file name of an image.
    pub fn name(&self, image: &SceneImage) -> Option<&str> {
        self.names.get(&image.key()).map(String::as_str)
    }
}

/// Builds the vertices of a part from polygon corners: corners with the same
/// control point and attributes share a vertex.
#[derive(Debug, Default)]
pub(crate) struct CornerBuilder {
    pub geometry: PartGeometry,
    map: HashMap<CornerKey, u32>,
}

#[derive(Debug, Hash, PartialEq, Eq)]
struct CornerKey {
    point: u32,
    attributes: Vec<u32>,
}

/// Attributes of one polygon corner.
#[derive(Debug, Clone, Default)]
pub(crate) struct Corner {
    pub point: u32,
    pub position: [f32; 3],
    pub normal: Option<[f32; 3]>,
    pub tangent: Option<[f32; 4]>,
    pub uv0: Option<[f32; 2]>,
    pub uv1: Option<[f32; 2]>,
    pub color: Option<[u8; 4]>,
    pub skin: Option<VertexSkin>,
}

impl CornerBuilder {
    pub fn new(name: &str) -> Self {
        Self {
            geometry: PartGeometry {
                name: name.to_owned(),
                ..Default::default()
            },
            map: HashMap::new(),
        }
    }

    /// The vertex of a corner, added when new.
    pub fn vertex(&mut self, c: &Corner) -> u32 {
        let mut attributes = Vec::with_capacity(16);
        let mut put = |v: &[f32]| attributes.extend(v.iter().map(|f| f.to_bits()));
        if let Some(n) = c.normal {
            put(&n);
        }
        if let Some(t) = c.tangent {
            put(&t);
        }
        if let Some(t) = c.uv0 {
            put(&t);
        }
        if let Some(t) = c.uv1 {
            put(&t);
        }
        if let Some(col) = c.color {
            attributes.push(u32::from_le_bytes(col));
        }
        let key = CornerKey {
            point: c.point,
            attributes,
        };
        if let Some(&v) = self.map.get(&key) {
            return v;
        }
        let g = &mut self.geometry;
        let index = g.positions.len() as u32;
        let first = index == 0;
        g.positions.push(c.position);
        fn push<T: Copy + Default>(
            list: &mut Option<Vec<T>>,
            value: Option<T>,
            first: bool,
            n: usize,
        ) {
            match (list.as_mut(), value) {
                (Some(l), v) => l.push(v.unwrap_or_default()),
                (None, Some(v)) => {
                    if first {
                        *list = Some(vec![v]);
                    } else {
                        let mut l = vec![T::default(); n];
                        l.push(v);
                        *list = Some(l);
                    }
                }
                (None, None) => {}
            }
        }
        let n = index as usize;
        push(&mut g.normals, c.normal, first, n);
        push(&mut g.tangents, c.tangent, first, n);
        push(&mut g.uv0, c.uv0, first, n);
        push(&mut g.uv1, c.uv1, first, n);
        push(&mut g.colors, c.color, first, n);
        push(&mut g.skin, c.skin, first, n);
        self.map.insert(key, index);
        index
    }

    /// Adds a polygon (corners in order) as a triangle fan, or ear clipping
    /// for concave polygons.
    pub fn polygon(&mut self, corners: &[Corner]) {
        if corners.len() < 3 {
            return;
        }
        let vertices: Vec<u32> = corners.iter().map(|c| self.vertex(c)).collect();
        let positions: Vec<[f32; 3]> = corners.iter().map(|c| c.position).collect();
        for t in math::triangulate(&positions) {
            let tri = t.map(|i| vertices[i]);
            if tri[0] != tri[1] && tri[1] != tri[2] && tri[0] != tri[2] {
                self.geometry.triangles.push(tri);
            }
        }
    }

    pub fn finish(self) -> PartGeometry {
        self.geometry
    }
}

/// Normalises up to four weights; drops weights beyond the four largest.
/// Returns `None` when there is no positive weight, and whether some were dropped.
/// Joints and weights of one vertex.
pub type VertexSkin = ([u16; 4], [f32; 4]);

pub(crate) fn top_four(influences: &mut Vec<(u16, f32)>) -> (Option<VertexSkin>, bool) {
    influences.retain(|i| i.1 > 0.0 && i.1.is_finite());
    // Merge duplicate joints.
    influences.sort_by_key(|i| i.0);
    influences.dedup_by(|a, b| {
        if a.0 == b.0 {
            b.1 += a.1;
            true
        } else {
            false
        }
    });
    influences.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let dropped = influences.len() > 4;
    influences.truncate(4);
    let sum: f32 = influences.iter().map(|i| i.1).sum();
    if sum <= 0.0 {
        return (None, dropped);
    }
    let mut j = [0u16; 4];
    let mut w = [0f32; 4];
    for (k, &(joint, weight)) in influences.iter().enumerate() {
        j[k] = joint;
        w[k] = weight / sum;
    }
    (Some((j, w)), dropped)
}

#[cfg(test)]
mod tests;

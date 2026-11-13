//! Parsed views of resources: textures, string tables, models, fonts, sounds,
//! and dicts for the types the library serialises.

use std::sync::Arc;

use doctor_toolkit::export::gltf::{self, NoAssets};
use doctor_toolkit::navigation::unpack::IndexAssets;
use doctor_toolkit::resource::audio::{self, Sample, SnrHeader};
use doctor_toolkit::resource::texture::{self as tex, TYPE_TG4D, TYPE_TG4H, TexturePair};
use doctor_toolkit::resource::tg4::{self, Kind, ParamValue};
use doctor_toolkit::resource::{
    anim, apx, ffn, hkx, lh2, logic, lua, material, mesh, names::type_tags, rcb, tob,
};
use pyo3::exceptions::{PyIndexError, PyKeyError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyType};

use crate::error::{ResultExt, invalid};
use crate::game::Loc;
use crate::navigation::Index;
use crate::util::{self, to_python};

const TYPE_LH2: u32 = doctor_toolkit::hash::sdbm(b"lh2");
const TYPE_MATERIAL: u32 = doctor_toolkit::hash::sdbm(b"material");
const TYPE_APX: u32 = doctor_toolkit::hash::sdbm(b"apx");

// ---------------------------------------------------------------- textures

/// A texture: a `tg4h` header with its `tg4d` data.
#[pyclass(module = "doctor_toolkit", frozen)]
pub struct Texture {
    pub texture: tex::Texture,
    /// Keys of the `tg4h` and `tg4d` resources, when the texture comes from a stream.
    pub keys: Option<(String, String)>,
}

impl Texture {
    fn new(texture: tex::Texture, keys: Option<(String, String)>) -> Self {
        Self { texture, keys }
    }
}

#[pymethods]
impl Texture {
    /// Texture name.
    #[getter]
    fn name(&self) -> &str {
        &self.texture.header.name
    }

    /// Package name (shared by the textures of a set).
    #[getter]
    fn package(&self) -> &str {
        &self.texture.header.package
    }

    /// Name hash (the last id of the `tg4h` and `tg4d`).
    #[getter]
    fn name_hash(&self) -> u32 {
        self.texture.header.name_hash
    }

    #[getter]
    fn width(&self) -> u16 {
        self.texture.header.width
    }

    #[getter]
    fn height(&self) -> u16 {
        self.texture.header.height
    }

    /// Pixel format, e.g. `DXT1c`, `DXT5`, `A8R8G8B8`.
    #[getter]
    fn format(&self) -> String {
        self.texture.header.format().name().to_owned()
    }

    /// `flat`, `cube` or `volume`.
    #[getter]
    fn kind(&self) -> &'static str {
        match self.texture.header.kind() {
            Kind::Flat => "flat",
            Kind::Cube => "cube",
            Kind::Volume => "volume",
        }
    }

    /// Number of mip levels.
    #[getter]
    fn mip_count(&self) -> usize {
        self.texture.header.mip_count()
    }

    /// Number of layers: faces of a cube map, slices of a volume, 1 otherwise.
    #[getter]
    fn layer_count(&self) -> usize {
        self.texture.header.layer_count()
    }

    /// Whether the colour data is sRGB.
    #[getter]
    fn srgb(&self) -> bool {
        self.texture.header.is_srgb()
    }

    /// Size of the pixel data in bytes.
    #[getter]
    fn data_size(&self) -> u32 {
        self.texture.header.data_size
    }

    /// Parameters of the header by name hash (str or int values).
    #[getter]
    fn params<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let dict = PyDict::new(py);
        for p in &self.texture.header.params {
            match &p.value {
                ParamValue::Str(s) => dict.set_item(p.name_hash, s)?,
                ParamValue::Int(v) => dict.set_item(p.name_hash, v)?,
            }
        }
        Ok(dict)
    }

    /// Keys of the `tg4h` and `tg4d` resources, or `None` for a texture not read from a stream.
    #[getter]
    fn keys(&self) -> Option<(String, String)> {
        self.keys.clone()
    }

    /// The pixel data as stored (all surfaces).
    #[getter]
    fn data<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        util::bytes(py, &self.texture.data)
    }

    /// The `tg4h` resource data of this texture.
    fn header_bytes<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        Ok(util::bytes(py, &self.texture.header.to_bytes().py()?))
    }

    /// The texture as a DDS file (lossless).
    fn to_dds<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let texture = &self.texture;
        let dds = py.detach(|| texture.to_dds()).py()?;
        Ok(util::bytes(py, &dds))
    }

    /// The largest mip level of `layer` as a PNG file.
    #[pyo3(signature = (layer = 0))]
    fn to_png<'py>(&self, py: Python<'py>, layer: usize) -> PyResult<Bound<'py, PyBytes>> {
        let texture = &self.texture;
        let png = py.detach(|| texture.to_png(layer)).py()?;
        Ok(util::bytes(py, &png))
    }

    /// The largest mip level of `layer` as `(width, height, rgba)`: 8-bit RGBA
    /// pixels, row by row (`numpy.frombuffer(rgba, numpy.uint8).reshape(height, width, 4)`).
    #[pyo3(signature = (layer = 0))]
    fn rgba<'py>(
        &self,
        py: Python<'py>,
        layer: usize,
    ) -> PyResult<(usize, usize, Bound<'py, PyBytes>)> {
        let texture = &self.texture;
        let (w, h, rgba) = py.detach(|| texture.rgba(layer)).py()?;
        Ok((w, h, util::bytes(py, &rgba)))
    }

    /// A replacement for this texture made from a PNG or DDS file: name and
    /// parameters are kept, the format family must fit (see the library's
    /// `texture::from_png` / `from_dds`). Nothing is recorded; use
    /// `Workspace.import_texture` to record it.
    fn replaced_by(&self, py: Python<'_>, image: &[u8]) -> PyResult<Texture> {
        let header = &self.texture.header;
        let replacement = py.detach(|| import_image(header, image)).py()?;
        Ok(Texture::new(replacement, self.keys.clone()))
    }

    /// Header fields as a dict.
    fn to_dict<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let dict = PyDict::new(py);
        dict.set_item("name", self.name())?;
        dict.set_item("package", self.package())?;
        dict.set_item("name_hash", self.name_hash())?;
        dict.set_item("width", self.width())?;
        dict.set_item("height", self.height())?;
        dict.set_item("format", self.format())?;
        dict.set_item("kind", self.kind())?;
        dict.set_item("mip_count", self.mip_count())?;
        dict.set_item("layer_count", self.layer_count())?;
        dict.set_item("srgb", self.srgb())?;
        dict.set_item("data_size", self.data_size())?;
        dict.set_item("params", self.params(py)?)?;
        dict.set_item("keys", self.keys())?;
        Ok(dict)
    }

    fn __repr__(&self) -> String {
        format!(
            "Texture({:?}, {}x{}, {})",
            self.texture.header.name,
            self.texture.header.width,
            self.texture.header.height,
            self.format()
        )
    }
}

/// A replacement texture from a PNG (by its signature) or DDS file.
pub fn import_image(header: &tg4::Tg4Header, image: &[u8]) -> doctor_toolkit::Result<tex::Texture> {
    if is_png(image)? {
        tex::from_png(header, image)
    } else {
        tex::from_dds(header, image)
    }
}

/// Whether `image` is a PNG file (`false` for DDS); other files are refused.
pub fn is_png(image: &[u8]) -> doctor_toolkit::Result<bool> {
    if image.starts_with(b"\x89PNG\r\n\x1a\n") {
        Ok(true)
    } else if image.starts_with(b"DDS ") {
        Ok(false)
    } else {
        Err(doctor_toolkit::Error::InvalidArgument(
            "the image must be a PNG or DDS file".into(),
        ))
    }
}

/// The texture pair resource `index` belongs to.
pub fn pair(loc: &Loc, index: usize) -> PyResult<TexturePair> {
    let type_id = loc.header(index).type_id;
    if type_id != TYPE_TG4H && type_id != TYPE_TG4D {
        return Err(invalid(format!(
            "{} is a {}, not a texture (tg4h or tg4d)",
            loc.key(index),
            type_tags().display(type_id)
        )));
    }
    tex::pair_of(loc.file.layout(), index)
        .ok_or_else(|| invalid(format!("{} has no matching tg4h/tg4d", loc.key(index))))
}

pub fn texture(py: Python<'_>, loc: &Loc, index: usize) -> PyResult<Texture> {
    let pair = pair(loc, index)?;
    load_texture(py, loc, pair)
}

fn load_texture(py: Python<'_>, loc: &Loc, pair: TexturePair) -> PyResult<Texture> {
    let file = &loc.file;
    let texture = py
        .detach(|| tex::Texture::load(file.data(), file.layout(), pair))
        .py()?;
    let keys = (
        loc.key(pair.header).to_string(),
        loc.key(pair.data).to_string(),
    );
    Ok(Texture::new(texture, Some(keys)))
}

pub fn textures(py: Python<'_>, loc: &Loc) -> PyResult<Vec<Texture>> {
    tex::pairs(loc.file.layout())
        .into_iter()
        .map(|pair| load_texture(py, loc, pair))
        .collect()
}

// ----------------------------------------------------------- string tables

/// A string table (`lh2`): entries by hash, one column per language.
///
/// Changes are kept in the object; `Workspace.set_string_table` records them.
#[pyclass(module = "doctor_toolkit", skip_from_py_object)]
#[derive(Clone)]
pub struct StringTable {
    pub table: lh2::StringTable,
    /// Key of the resource the table was read from.
    pub key: Option<String>,
}

impl StringTable {
    fn column(&self, language: usize) -> PyResult<&Vec<String>> {
        self.table.strings.get(language).ok_or_else(|| {
            PyIndexError::new_err(format!(
                "the table has {} language column(s), not {}",
                self.table.strings.len(),
                language + 1
            ))
        })
    }

    fn entry(&self, hash: u32) -> PyResult<usize> {
        self.table
            .find(hash)
            .ok_or_else(|| PyKeyError::new_err(format!("no string {hash:08x}")))
    }
}

#[pymethods]
impl StringTable {
    /// Parses `lh2` resource data.
    #[classmethod]
    fn parse(_cls: &Bound<'_, PyType>, data: &[u8]) -> PyResult<Self> {
        Ok(Self {
            table: lh2::StringTable::parse(data).py()?,
            key: None,
        })
    }

    /// Key of the resource the table was read from, or `None`.
    #[getter]
    fn key(&self) -> Option<String> {
        self.key.clone()
    }

    #[getter]
    fn version(&self) -> u16 {
        self.table.version
    }

    #[getter]
    fn flags(&self) -> u16 {
        self.table.flags
    }

    /// Language codes; empty when the table does not name its language.
    #[getter]
    fn languages(&self) -> Vec<String> {
        self.table.languages.clone()
    }

    /// Number of language columns.
    #[getter]
    fn language_count(&self) -> usize {
        self.table.strings.len()
    }

    /// Entry hashes, ascending.
    #[getter]
    fn hashes(&self) -> Vec<u32> {
        self.table.hashes.clone()
    }

    fn __len__(&self) -> usize {
        self.table.hashes.len()
    }

    fn __contains__(&self, hash: u32) -> bool {
        self.table.find(hash).is_some()
    }

    /// The string `hash` of the first language; `KeyError` when there is none.
    fn __getitem__(&self, hash: u32) -> PyResult<String> {
        let entry = self.entry(hash)?;
        Ok(self.column(0)?[entry].clone())
    }

    /// Sets string `hash` in every language column.
    fn __setitem__(&mut self, hash: u32, text: String) -> PyResult<()> {
        let entry = self.entry(hash)?;
        for column in &mut self.table.strings {
            column[entry] = text.clone();
        }
        Ok(())
    }

    /// The string `hash` of column `language`, or `None`.
    #[pyo3(signature = (hash, language = 0))]
    fn get(&self, hash: u32, language: usize) -> PyResult<Option<String>> {
        let column = self.column(language)?;
        Ok(self.table.find(hash).map(|i| column[i].clone()))
    }

    /// Sets string `hash` of column `language`; `KeyError` for an unknown hash.
    #[pyo3(signature = (hash, text, language = 0))]
    fn set(&mut self, hash: u32, text: String, language: usize) -> PyResult<()> {
        let entry = self.entry(hash)?;
        self.column(language)?;
        self.table.strings[language][entry] = text;
        Ok(())
    }

    /// `(hash, text)` of every entry of column `language`.
    #[pyo3(signature = (language = 0))]
    fn items(&self, language: usize) -> PyResult<Vec<(u32, String)>> {
        let column = self.column(language)?;
        Ok(self
            .table
            .hashes
            .iter()
            .copied()
            .zip(column.iter().cloned())
            .collect())
    }

    /// `{"version", "flags", "languages", "strings": {hash: [text per language]}}`.
    fn to_dict<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let strings = PyDict::new(py);
        for (i, hash) in self.table.hashes.iter().enumerate() {
            let texts: Vec<&str> = self.table.strings.iter().map(|c| c[i].as_str()).collect();
            strings.set_item(hash, texts)?;
        }
        let dict = PyDict::new(py);
        dict.set_item("version", self.table.version)?;
        dict.set_item("flags", self.table.flags)?;
        dict.set_item("languages", &self.table.languages)?;
        dict.set_item("strings", strings)?;
        Ok(dict)
    }

    /// The table as `lh2` resource data.
    fn to_bytes<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        Ok(util::bytes(py, &self.table.to_bytes().py()?))
    }

    /// A copy that can be changed independently.
    fn copy(&self) -> Self {
        self.clone()
    }

    fn __eq__(&self, other: &Self) -> bool {
        self.table == other.table
    }

    fn __repr__(&self) -> String {
        format!(
            "StringTable({} string(s), {} language(s))",
            self.table.hashes.len(),
            self.table.strings.len()
        )
    }
}

pub fn string_table(py: Python<'_>, loc: &Loc, index: usize) -> PyResult<StringTable> {
    let data = loc.read(py, index)?;
    Ok(StringTable {
        table: lh2::StringTable::parse(&data).py()?,
        key: Some(loc.key(index).to_string()),
    })
}

// ------------------------------------------------------------------ models

/// A model (`mesh` with its `meshvolatile`): draw records with vertices and
/// triangles, and export to binary glTF.
#[pyclass(module = "doctor_toolkit", frozen)]
pub struct Model {
    model: mesh::Model,
    key: String,
}

impl Model {
    fn record(&self, index: usize) -> PyResult<&mesh::DrawRecord> {
        self.model.records.get(index).ok_or_else(|| {
            PyIndexError::new_err(format!(
                "the model has {} draw record(s)",
                self.model.records.len()
            ))
        })
    }
}

fn f32_bytes<const N: usize>(values: &[[f32; N]]) -> Vec<u8> {
    values
        .iter()
        .flatten()
        .flat_map(|v| v.to_le_bytes())
        .collect()
}

#[pymethods]
impl Model {
    #[getter]
    fn name(&self) -> &str {
        &self.model.name
    }

    /// Key of the `mesh` resource.
    #[getter]
    fn key(&self) -> &str {
        &self.key
    }

    /// Centre and radius.
    #[getter]
    fn bounding_sphere(&self) -> [f32; 4] {
        self.model.bounding_sphere
    }

    #[getter]
    fn aabb_min(&self) -> [f32; 3] {
        self.model.aabb_min
    }

    #[getter]
    fn aabb_max(&self) -> [f32; 3] {
        self.model.aabb_max
    }

    /// Bone table: `(bone name hash, skeleton bone index)`.
    #[getter]
    fn bones(&self) -> Vec<(u32, u32)> {
        self.model.bones.clone()
    }

    /// The draw records as dicts: `name`, `material_hash`, `primitive`,
    /// `index_count`, `vertex_count`, `format`, `influences`, `bounding_sphere`,
    /// `render_flags` (int) and `render_flag_names` (e.g. `"AlphaSort | LowLODOnly"`),
    /// `overlay_id` (decal layer, 0 for ordinary geometry).
    #[getter]
    fn records<'py>(&self, py: Python<'py>) -> PyResult<Vec<Bound<'py, PyDict>>> {
        self.model
            .records
            .iter()
            .map(|r| {
                let dict = PyDict::new(py);
                dict.set_item("name", &r.name)?;
                dict.set_item("material_hash", r.material_hash)?;
                dict.set_item(
                    "primitive",
                    match r.primitive {
                        mesh::Primitive::TriangleList => "triangle_list",
                        mesh::Primitive::TriangleStrip => "triangle_strip",
                    },
                )?;
                dict.set_item("index_count", r.index_count)?;
                dict.set_item("vertex_count", r.vertex_count)?;
                dict.set_item("format", r.format)?;
                dict.set_item("influences", r.influences)?;
                dict.set_item("render_flags", r.render_flags)?;
                dict.set_item("render_flag_names", mesh::render_flag_names(r.render_flags))?;
                dict.set_item("overlay_id", r.overlay_id)?;
                dict.set_item("bounding_sphere", r.bounding_sphere)?;
                dict.set_item("rigid_bone", r.rigid_bone)?;
                dict.set_item("palette", r.palette.clone())?;
                Ok(dict)
            })
            .collect()
    }

    /// Decoded vertices of draw record `record` as packed little-endian
    /// buffers: `count`, `positions` (float32 × 3), `normals` (float32 × 3),
    /// `tangents` (float32 × 4), `uv0`, `uv1` (float32 × 2), `colors` (uint8 × 4),
    /// `joints` (uint8 × 4), `weights` (float32 × 4); absent attributes are `None`.
    fn vertices<'py>(&self, py: Python<'py>, record: usize) -> PyResult<Bound<'py, PyDict>> {
        let r = self.record(record)?;
        let model = &self.model;
        let v = py.detach(|| model.vertices(r)).py()?;
        let dict = PyDict::new(py);
        dict.set_item("count", v.positions.len())?;
        dict.set_item("positions", util::bytes(py, &f32_bytes(&v.positions)))?;
        let opt3 = |x: &Option<Vec<[f32; 3]>>| x.as_ref().map(|x| util::bytes(py, &f32_bytes(x)));
        let opt4 = |x: &Option<Vec<[f32; 4]>>| x.as_ref().map(|x| util::bytes(py, &f32_bytes(x)));
        let opt2 = |x: &Option<Vec<[f32; 2]>>| x.as_ref().map(|x| util::bytes(py, &f32_bytes(x)));
        let u8s = |x: &Option<Vec<[u8; 4]>>| x.as_ref().map(|x| util::bytes(py, x.as_flattened()));
        dict.set_item("normals", opt3(&v.normals))?;
        dict.set_item("tangents", opt4(&v.tangents))?;
        dict.set_item("uv0", opt2(&v.uv0))?;
        dict.set_item("uv1", opt2(&v.uv1))?;
        dict.set_item("colors", u8s(&v.colors))?;
        dict.set_item("joints", u8s(&v.joints))?;
        dict.set_item("weights", opt4(&v.weights))?;
        Ok(dict)
    }

    /// Triangles of draw record `record` as packed little-endian uint32 × 3,
    /// indices into its vertices.
    fn triangles<'py>(&self, py: Python<'py>, record: usize) -> PyResult<Bound<'py, PyBytes>> {
        let r = self.record(record)?;
        let model = &self.model;
        let triangles = py.detach(|| model.triangles(r)).py()?;
        let data: Vec<u8> = triangles
            .iter()
            .flatten()
            .flat_map(|i| i.to_le_bytes())
            .collect();
        Ok(util::bytes(py, &data))
    }

    /// The model as binary glTF (`.glb`). With the game `index`, materials,
    /// textures and the skeleton found anywhere in the game are embedded;
    /// without it, geometry only.
    #[pyo3(signature = (index = None))]
    fn to_glb<'py>(
        &self,
        py: Python<'py>,
        index: Option<PyRef<'py, Index>>,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let model = &self.model;
        let glb = match &index {
            Some(index) => {
                let (game, idx, maps) = index.parts();
                py.detach(|| {
                    let mut assets = IndexAssets::new(game, idx, maps);
                    gltf::model_to_glb(model, &mut assets)
                })
            }
            None => py.detach(|| gltf::model_to_glb(model, &mut NoAssets)),
        }
        .py()?;
        Ok(util::bytes(py, &glb))
    }

    /// Writes the model to `path` as glTF, FBX, OBJ or USD (`format`: `glb`,
    /// `fbx`, `obj`, `usd`, `usda`, `usdc`, `usdz`; default from the
    /// extension). Textures of OBJ and USD go into `textures/` next to it.
    /// With the game `index`, materials, textures and the skeleton are
    /// included. Returns the paths written.
    #[pyo3(signature = (path, index = None, format = None))]
    fn export(
        &self,
        py: Python<'_>,
        path: std::path::PathBuf,
        index: Option<PyRef<'_, Index>>,
        format: Option<&str>,
    ) -> PyResult<Vec<std::path::PathBuf>> {
        use doctor_toolkit::scene;
        let format = format
            .map(str::parse::<scene::ModelFormat>)
            .transpose()
            .py()?;
        let model = &self.model;
        let built = match &index {
            Some(index) => {
                let (game, idx, maps) = index.parts();
                py.detach(|| {
                    let mut assets = IndexAssets::new(game, idx, maps);
                    scene::game::model_scene(model, &mut assets)
                })
            }
            None => py.detach(|| scene::game::model_scene(model, &mut NoAssets)),
        }
        .py()?;
        py.detach(|| scene::write_file(&built, &path, format)).py()
    }

    fn __repr__(&self) -> String {
        format!(
            "Model({:?}, {} record(s))",
            self.model.name,
            self.model.records.len()
        )
    }
}

pub fn model(py: Python<'_>, loc: &Loc, index: usize) -> PyResult<Model> {
    let type_id = loc.header(index).type_id;
    if type_id != mesh::TYPE_MESH {
        return Err(invalid(format!(
            "{} is a {}, not a mesh",
            loc.key(index),
            type_tags().display(type_id)
        )));
    }
    let file = &loc.file;
    let model = py.detach(|| mesh::load(file, index)).py()?;
    Ok(Model {
        model,
        key: loc.key(index).to_string(),
    })
}

// ------------------------------------------------------------------- fonts

/// A font (`ffn`): glyph metrics and the atlas texture of its stream.
#[pyclass(module = "doctor_toolkit", frozen)]
pub struct Font {
    font: ffn::Font,
    atlas: Option<Py<Texture>>,
}

#[pymethods]
impl Font {
    #[getter]
    fn name(&self) -> &str {
        &self.font.name
    }

    #[getter]
    fn version(&self) -> u16 {
        self.font.version
    }

    #[getter]
    fn metrics(&self) -> [u32; 3] {
        self.font.metrics
    }

    /// High bytes of the UTF-16 codes that have a page.
    #[getter]
    fn pages(&self) -> Vec<u8> {
        self.font.pages.clone()
    }

    /// Glyphs as dicts: `char`, `code`, `width`, `height`, `advance`,
    /// `x_offset`, `y_offset`, `rect` (atlas pixels: left, top, right, bottom).
    #[getter]
    fn glyphs<'py>(&self, py: Python<'py>) -> PyResult<Vec<Bound<'py, PyDict>>> {
        self.font
            .glyphs
            .iter()
            .map(|g| {
                let dict = PyDict::new(py);
                dict.set_item("char", g.character().map(String::from))?;
                dict.set_item("code", g.code)?;
                dict.set_item("width", g.width)?;
                dict.set_item("height", g.height)?;
                dict.set_item("advance", g.advance)?;
                dict.set_item("x_offset", g.x_offset)?;
                dict.set_item("y_offset", g.y_offset)?;
                dict.set_item("rect", g.rect)?;
                Ok(dict)
            })
            .collect()
    }

    /// The atlas texture: the texture of the stream named like the font, or `None`.
    #[getter]
    fn atlas(&self, py: Python<'_>) -> Option<Py<Texture>> {
        self.atlas.as_ref().map(|t| t.clone_ref(py))
    }

    /// Whether the font has a glyph for `char`.
    fn covers(&self, char: char) -> bool {
        self.font.covers(char)
    }

    /// Everything but the atlas as a dict.
    fn to_dict<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let value = to_python(py, &self.font)?;
        value.set_item("glyphs", self.glyphs(py)?)?;
        Ok(value)
    }

    fn __repr__(&self) -> String {
        format!(
            "Font({:?}, {} glyph(s))",
            self.font.name,
            self.font.glyphs.len()
        )
    }
}

fn font(py: Python<'_>, loc: &Loc, data: &[u8]) -> PyResult<Font> {
    let font = ffn::Font::parse(data).py()?;
    let file = &loc.file;
    let atlas = py.detach(|| font.find_atlas(file.data(), file.layout()));
    let atlas = atlas
        .map(|texture| Py::new(py, Texture::new(texture, None)))
        .transpose()?;
    Ok(Font { font, atlas })
}

// ------------------------------------------------------------------ sounds

enum SoundSource {
    /// An SNR header at this offset, followed by its SNS blocks (sound banks).
    Inline(usize),
    /// A whole `.snu` file.
    Snu,
}

/// A sound: from an `sbk` bank or a streamed `.snu` file of the `.viv` archives.
#[pyclass(module = "doctor_toolkit", frozen)]
pub struct Sound {
    data: Arc<Vec<u8>>,
    source: SoundSource,
    header: SnrHeader,
    /// Index in the bank, or `None` for a streamed sound.
    index: Option<u32>,
    name: Option<String>,
}

impl Sound {
    fn sample(&self) -> doctor_toolkit::Result<Sample<'_>> {
        match self.source {
            SoundSource::Inline(offset) => Sample::parse_inline(&self.data, offset),
            SoundSource::Snu => Sample::parse_snu(&self.data),
        }
    }

    pub fn from_snu(name: String, data: Vec<u8>) -> PyResult<Self> {
        let header = Sample::parse_snu(&data).py()?.header;
        Ok(Self {
            data: Arc::new(data),
            source: SoundSource::Snu,
            header,
            index: None,
            name: Some(name),
        })
    }
}

#[pymethods]
impl Sound {
    /// Index of the sound in its bank, `None` for a streamed sound.
    #[getter]
    fn index(&self) -> Option<u32> {
        self.index
    }

    /// Archive entry name of a streamed sound, `None` for a bank sound.
    #[getter]
    fn name(&self) -> Option<String> {
        self.name.clone()
    }

    /// Codec number; 4 is EA-XAS v1, the only one decoded.
    #[getter]
    fn codec(&self) -> u8 {
        self.header.codec
    }

    #[getter]
    fn channels(&self) -> u8 {
        self.header.channels
    }

    #[getter]
    fn sample_rate(&self) -> u32 {
        self.header.sample_rate
    }

    /// Samples per channel.
    #[getter]
    fn sample_count(&self) -> u32 {
        self.header.sample_count
    }

    /// Loop start in samples, or `None`.
    #[getter]
    fn loop_start(&self) -> Option<u32> {
        self.header.loop_start
    }

    /// Duration in seconds.
    #[getter]
    fn duration(&self) -> f64 {
        self.header.sample_count as f64 / self.header.sample_rate as f64
    }

    /// Decoded interleaved 16-bit PCM, little-endian (`numpy.int16`).
    fn pcm<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let samples = py.detach(|| self.sample()?.decode()).py()?;
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        Ok(util::bytes(py, &data))
    }

    /// The sound decoded to a RIFF WAVE file (16-bit PCM).
    fn to_wav<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let wav = py.detach(|| self.sample()?.to_wav()).py()?;
        Ok(util::bytes(py, &wav))
    }

    /// The sound as a standalone `.snu` file (playable with vgmstream).
    fn to_snu<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let snu = match self.source {
            SoundSource::Snu => self.data.to_vec(),
            SoundSource::Inline(_) => audio::to_snu(&self.data, &self.sample().py()?),
        };
        Ok(util::bytes(py, &snu))
    }

    fn __repr__(&self) -> String {
        format!(
            "Sound({}, {} channel(s), {} Hz, {:.2} s)",
            match (&self.name, self.index) {
                (Some(name), _) => format!("{name:?}"),
                (None, Some(i)) => format!("#{i}"),
                (None, None) => "?".into(),
            },
            self.header.channels,
            self.header.sample_rate,
            self.duration()
        )
    }
}

fn sounds(data: Vec<u8>) -> PyResult<Vec<Sound>> {
    let data = Arc::new(data);
    let parsed: Vec<(u32, usize, SnrHeader)> = audio::parse_sbk(&data)
        .py()?
        .into_iter()
        .map(|s| (s.index, s.sample.range.start, s.sample.header))
        .collect();
    Ok(parsed
        .into_iter()
        .map(|(index, offset, header)| Sound {
            data: data.clone(),
            source: SoundSource::Inline(offset),
            header,
            index: Some(index),
            name: None,
        })
        .collect())
}

// ------------------------------------------------------------- dispatching

fn material_dict<'py>(py: Python<'py>, m: &material::Material) -> PyResult<Bound<'py, PyAny>> {
    let params = m
        .params
        .iter()
        .map(|p| {
            let dict = PyDict::new(py);
            dict.set_item("hash", p.hash)?;
            dict.set_item("name", &p.name)?;
            dict.set_item("source", &p.source)?;
            match &p.value {
                material::ParamValue::Floats(v) => {
                    dict.set_item("type", "floats")?;
                    dict.set_item("value", v.clone())?;
                }
                material::ParamValue::Texture(h) => {
                    dict.set_item("type", "texture")?;
                    dict.set_item("value", *h)?;
                }
                material::ParamValue::Unknown(t) => {
                    dict.set_item("type", "unknown")?;
                    dict.set_item("value", *t)?;
                }
            }
            Ok(dict)
        })
        .collect::<PyResult<Vec<_>>>()?;
    let dict = PyDict::new(py);
    dict.set_item("version", m.version)?;
    dict.set_item("name", &m.name)?;
    dict.set_item("name_hash", m.name_hash)?;
    dict.set_item("shaders", m.shaders.to_vec())?;
    dict.set_item("shadow_shader", m.shadow_shader)?;
    dict.set_item("params", params)?;
    dict.set_item(
        "shading",
        util::to_python(
            py,
            &doctor_toolkit::resource::shading::Shading::from_material(m),
        )?,
    )?;
    Ok(dict.into_any())
}

fn apx_dict<'py>(py: Python<'py>, data: &[u8]) -> PyResult<Bound<'py, PyAny>> {
    let file = apx::AptFile::parse(data).py()?;
    let blocks: Vec<(String, usize)> = file
        .blocks
        .iter()
        .map(|b| (String::from_utf8_lossy(&b.tag).into_owned(), b.data.len()))
        .collect();
    let textures = file
        .textures()
        .py()?
        .into_iter()
        .map(|t| Py::new(py, Texture::new(t, None)))
        .collect::<PyResult<Vec<_>>>()?;
    let dict = PyDict::new(py);
    dict.set_item("title", file.title())?;
    dict.set_item("blocks", blocks)?;
    dict.set_item("textures", textures)?;
    Ok(dict.into_any())
}

/// Parses resource `index` of `loc` by its type; see `Resource.parse`.
pub fn parse<'py>(py: Python<'py>, loc: &Loc, index: usize) -> PyResult<Bound<'py, PyAny>> {
    let type_id = loc.header(index).type_id;
    match type_id {
        TYPE_TG4H | TYPE_TG4D => return Ok(Bound::new(py, texture(py, loc, index)?)?.into_any()),
        mesh::TYPE_MESH => return Ok(Bound::new(py, model(py, loc, index)?)?.into_any()),
        _ => {}
    }
    let data = loc.read(py, index)?;
    decode(py, Some(loc), type_id, data)
}

/// Decodes resource data of `type_id`. `loc` is needed for font atlases.
pub fn decode<'py>(
    py: Python<'py>,
    loc: Option<&Loc>,
    type_id: u32,
    data: Vec<u8>,
) -> PyResult<Bound<'py, PyAny>> {
    if let Some(json) = py.detach(|| logic::to_json(type_id, &data)) {
        return to_python(py, &json.py()?);
    }
    match type_id {
        TYPE_LH2 => Ok(Bound::new(
            py,
            StringTable {
                table: lh2::StringTable::parse(&data).py()?,
                key: None,
            },
        )?
        .into_any()),
        rcb::TYPE_RCB => to_python(py, &rcb::Rig::parse(&data).py()?),
        lua::TYPE_LUA => to_python(py, &lua::LuaChunk::parse(&data).py()?),
        tob::TYPE_TOB => to_python(py, &tob::Tob::parse(&data).py()?),
        hkx::TYPE_HKX => to_python(py, &hkx::Packfile::parse(&data).py()?),
        anim::TYPE_BNK => to_python(py, &anim::AnimBank::parse(&data).py()?),
        TYPE_MATERIAL => material_dict(py, &material::Material::parse(&data).py()?),
        TYPE_APX => apx_dict(py, &data),
        ffn::TYPE_FFN => match loc {
            Some(loc) => Ok(Bound::new(py, font(py, loc, &data)?)?.into_any()),
            None => to_python(py, &ffn::Font::parse(&data).py()?),
        },
        audio::TYPE_SBK => Ok(pyo3::types::PyList::new(py, sounds(data)?)?.into_any()),
        _ => Err(invalid(format!(
            "no parser for resources of type {}",
            type_tags().display(type_id)
        ))),
    }
}

/// Parses resource data of type `type` (a tag or type id) without a game:
/// game logic, rigs, materials, scripts, Havok and animation banks give
/// dicts, `lh2` a `StringTable`, `ffn` a dict, `apx` a dict with textures,
/// `sbk` a list of `Sound`.
#[pyfunction(name = "decode")]
fn decode_py<'py>(
    py: Python<'py>,
    r#type: &Bound<'py, PyAny>,
    data: Vec<u8>,
) -> PyResult<Bound<'py, PyAny>> {
    let type_id = util::resolve_type(r#type)?;
    decode(py, None, type_id, data)
}

/// The sound of a `.snu` file.
#[pyfunction]
fn sound_from_snu(data: Vec<u8>) -> PyResult<Sound> {
    Sound::from_snu(String::new(), data)
}

/// The structure of an `sbk` bank: `emitters` (each with `name` when the
/// name list knows it and `instrument_index`, the instrument the game
/// resolves for it), `instruments` with their `tones`, the lookup tables,
/// `samples` (with `channels`, `sample_rate`, `sample_count`, `size`) and
/// `problems` (broken references; empty for the banks of the game).
#[pyfunction]
fn sound_bank<'py>(py: Python<'py>, data: Vec<u8>) -> PyResult<Bound<'py, PyAny>> {
    use doctor_toolkit::resource::sbk::SoundBank;
    use serde_json::json;
    let bank = py.detach(|| SoundBank::parse(&data)).py()?;
    let mut value = serde_json::to_value(&bank).map_err(|e| invalid(e.to_string()))?;
    if let Some(list) = value["emitters"].as_array_mut() {
        for (i, e) in list.iter_mut().enumerate() {
            e["name"] = json!(bank.emitter_name(i));
            e["instrument_index"] = json!(bank.instrument_of(i));
        }
    }
    if let Some(list) = value["samples"].as_array_mut() {
        for (s, sample) in list.iter_mut().zip(&bank.samples) {
            s["size"] = json!(sample.bytes.len());
            if let Ok(parsed) = Sample::parse_inline(&sample.bytes, 0) {
                s["channels"] = json!(parsed.header.channels);
                s["sample_rate"] = json!(parsed.header.sample_rate);
                s["sample_count"] = json!(parsed.header.sample_count);
            }
        }
    }
    value["problems"] = json!(bank.validate());
    to_python(py, &value)
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Texture>()?;
    m.add_class::<StringTable>()?;
    m.add_class::<Model>()?;
    m.add_class::<Font>()?;
    m.add_class::<Sound>()?;
    m.add_function(wrap_pyfunction!(decode_py, m)?)?;
    m.add_function(wrap_pyfunction!(sound_from_snu, m)?)?;
    m.add_function(wrap_pyfunction!(sound_bank, m)?)?;
    Ok(())
}

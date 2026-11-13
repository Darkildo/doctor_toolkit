//! TG4 textures: a `tg4h` header resource and a `tg4d` data resource.
//!
//! The same header format describes the textures embedded in `apx` files
//! (`tx4h` blocks), where one header can hold several textures.
//!
//! # Header layout (version 3, The Godfather II)
//!
//! ```text
//! 0x00  u32  offset of the package name string ("assets_shared_fonts")
//! 0x04  u16  version, 3
//! 0x06  u16  number of textures C
//! 0x08  u32  offset of the name hash array, 0x14
//! 0x0C  u32  offset of the texture records, 0x14 + 4 * C
//! 0x10  u32  budget: 0 in tg4h, CRC-32 of `ui` in every tx4h of the apx files
//! 0x14  C × u32  texture name hashes; equal the last id of the stream resources
//!       C × texture record, 0x18 bytes:
//!       u32  offset of the texture name string
//!       u32  size of the texture data
//!       u16  width
//!       u16  height
//!       u16  padding, 0
//!       u8   surface count: mip levels (2D), faces (cube, 6) or slices (3D)
//!       u8   number of parameters
//!       u32  offset of the parameter name hashes (u32 each)
//!       u32  offset of the parameter values (u8 kind + u32 value each, 5 bytes)
//!       for each texture: parameter name hashes, parameter values, then the
//!       string values not stored yet (NUL-terminated)
//!       package name and texture names, again only if not stored yet
//! ```
//!
//! A value of kind 0 is the offset of a string, kind 1 an integer. A parameter
//! name hash is the CRC-32 of the name (`format`, `mipmap`, `color_space`,
//! `filter`, `edge`, `mode`); the name of [`PARAM_DATA_SIZE`] is not known.
//! Cube and 3D textures carry no mip levels.
//!
//! # Data layout
//!
//! Surfaces one after another without padding: mip levels from largest to
//! smallest, or faces / slices. Block-compressed levels round up to whole
//! 4×4 blocks.

use crate::io::{self, Endian};
use crate::{Error, Result};

/// Parameter `format`: pixel format (`DXT1c`, `DXT5`, ...).
pub const PARAM_FORMAT: u32 = 0xDEBA_72DF;
/// Parameter: size of the `tg4d` data (name unknown; equals the record data size).
pub const PARAM_DATA_SIZE: u32 = 0x4F8B_2A9F;
/// Parameter `color_space`: `sRGB` when the colour data is gamma-encoded.
pub const PARAM_COLOR_SPACE: u32 = 0x58BE_9388;
/// Parameter `filter`: filtering hint (`aniso`, `point`).
pub const PARAM_FILTER: u32 = 0x7FC4_5F1D;
/// Parameter `edge`: addressing mode (`clamp`, `border-black`).
pub const PARAM_EDGE: u32 = 0x7506_D366;
/// Parameter `mode`: texture kind (`cube`, `3d`); absent for 2D textures.
pub const PARAM_MODE: u32 = 0x97CA_47AB;
/// Parameter `mipmap`: `"1"` in almost every texture.
pub const PARAM_MIPMAP: u32 = 0x9659_7B2E;

/// Pixel formats found in The Godfather II.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Format {
    /// BC1 without alpha.
    Dxt1c,
    /// BC1 with one-bit alpha.
    Dxt1a,
    /// BC3.
    Dxt5,
    /// BC3 holding a normal map (X in alpha, Y in green).
    Dxt5Nm,
    /// 8-bit luminance.
    L8,
    /// 8-bit alpha.
    A8,
    /// 8-bit luminance and 8-bit alpha.
    L8A8,
    /// 32-bit colour.
    Rgba8,
    /// A format name not seen in the game files.
    Other(String),
}

impl Format {
    pub fn from_name(name: &str) -> Self {
        match name {
            "DXT1c" => Self::Dxt1c,
            "DXT1a" => Self::Dxt1a,
            "DXT5" => Self::Dxt5,
            "DXT5_NM" => Self::Dxt5Nm,
            "L8" => Self::L8,
            "A8" => Self::A8,
            "L8A8" => Self::L8A8,
            "RGBA8" => Self::Rgba8,
            other => Self::Other(other.to_owned()),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Dxt1c => "DXT1c",
            Self::Dxt1a => "DXT1a",
            Self::Dxt5 => "DXT5",
            Self::Dxt5Nm => "DXT5_NM",
            Self::L8 => "L8",
            Self::A8 => "A8",
            Self::L8A8 => "L8A8",
            Self::Rgba8 => "RGBA8",
            Self::Other(name) => name,
        }
    }

    /// Block edge in pixels and bytes per block; `None` for unknown formats.
    pub fn block(&self) -> Option<(usize, usize)> {
        Some(match self {
            Self::Dxt1c | Self::Dxt1a => (4, 8),
            Self::Dxt5 | Self::Dxt5Nm => (4, 16),
            Self::L8 | Self::A8 => (1, 1),
            Self::L8A8 => (1, 2),
            Self::Rgba8 => (1, 4),
            Self::Other(_) => return None,
        })
    }

    /// Bytes needed for one surface of the given size.
    pub fn surface_size(&self, width: usize, height: usize) -> Option<usize> {
        let (edge, bytes) = self.block()?;
        Some(width.max(1).div_ceil(edge) * height.max(1).div_ceil(edge) * bytes)
    }
}

/// Shape of a texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Flat,
    Cube,
    Volume,
}

/// The value of a texture parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamValue {
    Str(String),
    Int(u32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    pub name_hash: u32,
    pub value: ParamValue,
}

/// One texture header. A `tg4h` resource holds one; see [`parse_set`] for several.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tg4Header {
    pub version: u16,
    /// Budget hash at 0x10, shared by all textures of a set: 0 in `tg4h`,
    /// CRC-32 of `ui` in `tx4h`.
    pub budget_hash: u32,
    /// Package name, shared by all textures of a set.
    pub package: String,
    pub name_hash: u32,
    pub name: String,
    pub data_size: u32,
    pub width: u16,
    pub height: u16,
    /// Padding after the height, 0 in every file.
    pub padding: u16,
    /// Mip levels, faces or slices, see [`Tg4Header::kind`].
    pub surface_count: u8,
    pub params: Vec<Param>,
}

const HASHES_OFFSET: usize = 0x14;
const RECORD_SIZE: usize = 0x18;

/// Parses a header that may describe several textures.
pub fn parse_set(data: &[u8]) -> Result<Vec<Tg4Header>> {
    let le = Endian::Little;
    let string = |offset: u32| -> Result<String> {
        Ok(String::from_utf8_lossy(io::cstr(data, offset as usize)?).into_owned())
    };
    let version = io::read_u16(data, 4, le)?;
    if version != 3 {
        return Err(Error::UnsupportedVersion {
            what: "tg4h",
            version: version as u32,
        });
    }
    let count = io::read_u16(data, 6, le)? as usize;
    let hashes_at = io::read_u32(data, 8, le)? as usize;
    let records_at = io::read_u32(data, 0x0C, le)? as usize;
    let budget_hash = io::read_u32(data, 0x10, le)?;
    let package = string(io::read_u32(data, 0, le)?)?;
    io::slice(data, records_at, count * RECORD_SIZE)?;

    let mut textures = Vec::with_capacity(count);
    for t in 0..count {
        let r = records_at + t * RECORD_SIZE;
        let param_count = io::slice(data, r + 0x0F, 1)?[0] as usize;
        let names_at = io::read_u32(data, r + 0x10, le)? as usize;
        let values_at = io::read_u32(data, r + 0x14, le)? as usize;
        let mut params = Vec::with_capacity(param_count);
        for i in 0..param_count {
            let name_hash = io::read_u32(data, names_at + 4 * i, le)?;
            let kind = io::slice(data, values_at + 5 * i, 1)?[0];
            let raw = io::read_u32(data, values_at + 5 * i + 1, le)?;
            let value = match kind {
                0 => ParamValue::Str(string(raw)?),
                1 => ParamValue::Int(raw),
                other => {
                    return Err(Error::malformed(
                        "tg4h",
                        values_at + 5 * i,
                        format!("unknown parameter kind {other}"),
                    ));
                }
            };
            params.push(Param { name_hash, value });
        }
        textures.push(Tg4Header {
            version,
            budget_hash,
            package: package.clone(),
            name_hash: io::read_u32(data, hashes_at + 4 * t, le)?,
            name: string(io::read_u32(data, r, le)?)?,
            data_size: io::read_u32(data, r + 4, le)?,
            width: io::read_u16(data, r + 8, le)?,
            height: io::read_u16(data, r + 0x0A, le)?,
            padding: io::read_u16(data, r + 0x0C, le)?,
            surface_count: io::slice(data, r + 0x0E, 1)?[0],
            params,
        });
    }
    Ok(textures)
}

/// Serialises a set of texture headers. Version, package name and the field
/// at 0x10 are taken from the first texture.
pub fn set_to_bytes(textures: &[Tg4Header]) -> Result<Vec<u8>> {
    let le = Endian::Little;
    let first = textures.first().ok_or_else(|| {
        Error::InvalidArgument("a texture header needs at least one texture".into())
    })?;
    let count = u16::try_from(textures.len())
        .map_err(|_| Error::InvalidArgument("too many textures in one header".into()))?;
    let records_at = HASHES_OFFSET + 4 * textures.len();
    let params_at = records_at + RECORD_SIZE * textures.len();
    let check = |text: &str| -> Result<()> {
        if text.contains('\0') {
            return Err(Error::InvalidArgument("string contains a NUL byte".into()));
        }
        Ok(())
    };

    // Parameter tables and their strings, texture by texture.
    let mut tail = Vec::new();
    let mut seen: Vec<(String, u32)> = Vec::new();
    let mut tables = Vec::with_capacity(textures.len());
    for texture in textures {
        let n = u8::try_from(texture.params.len())
            .map_err(|_| Error::InvalidArgument("more than 255 texture parameters".into()))?;
        let names_at = params_at + tail.len();
        let values_at = names_at + 4 * texture.params.len();
        let mut strings_at = values_at + 5 * texture.params.len();
        let mut values = Vec::with_capacity(5 * texture.params.len());
        let mut strings = Vec::new();
        for param in &texture.params {
            match &param.value {
                ParamValue::Str(text) => {
                    check(text)?;
                    let offset = match seen.iter().find(|(s, _)| s == text) {
                        Some(&(_, offset)) => offset,
                        None => {
                            let offset = strings_at as u32;
                            strings.extend_from_slice(text.as_bytes());
                            strings.push(0);
                            strings_at += text.len() + 1;
                            seen.push((text.clone(), offset));
                            offset
                        }
                    };
                    values.push(0);
                    io::put_u32(&mut values, offset, le);
                }
                ParamValue::Int(value) => {
                    values.push(1);
                    io::put_u32(&mut values, *value, le);
                }
            }
        }
        for param in &texture.params {
            io::put_u32(&mut tail, param.name_hash, le);
        }
        tail.extend_from_slice(&values);
        tail.extend_from_slice(&strings);
        tables.push((n, names_at, values_at));
    }
    // Package and texture names come last and reuse equal strings from the pool.
    let mut intern = |text: &str, tail: &mut Vec<u8>| -> Result<u32> {
        check(text)?;
        if let Some(&(_, offset)) = seen.iter().find(|(s, _)| s == text) {
            return Ok(offset);
        }
        let offset = (params_at + tail.len()) as u32;
        tail.extend_from_slice(text.as_bytes());
        tail.push(0);
        seen.push((text.to_owned(), offset));
        Ok(offset)
    };
    let package_at = intern(&first.package, &mut tail)? as usize;
    let mut name_offsets = Vec::with_capacity(textures.len());
    for texture in textures {
        name_offsets.push(intern(&texture.name, &mut tail)?);
    }

    let mut out = Vec::with_capacity(params_at + tail.len());
    io::put_u32(&mut out, package_at as u32, le);
    io::put_u16(&mut out, first.version, le);
    io::put_u16(&mut out, count, le);
    io::put_u32(&mut out, HASHES_OFFSET as u32, le);
    io::put_u32(&mut out, records_at as u32, le);
    io::put_u32(&mut out, first.budget_hash, le);
    for texture in textures {
        io::put_u32(&mut out, texture.name_hash, le);
    }
    for ((texture, (n, names_at, values_at)), name_at) in
        textures.iter().zip(tables).zip(name_offsets)
    {
        io::put_u32(&mut out, name_at, le);
        io::put_u32(&mut out, texture.data_size, le);
        io::put_u16(&mut out, texture.width, le);
        io::put_u16(&mut out, texture.height, le);
        io::put_u16(&mut out, texture.padding, le);
        out.push(texture.surface_count);
        out.push(n);
        io::put_u32(&mut out, names_at as u32, le);
        io::put_u32(&mut out, values_at as u32, le);
    }
    debug_assert_eq!(out.len(), params_at);
    out.extend_from_slice(&tail);
    Ok(out)
}

impl Tg4Header {
    /// Parses a `tg4h` resource, which holds exactly one texture.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut textures = parse_set(data)?;
        if textures.len() != 1 {
            return Err(Error::malformed(
                "tg4h",
                6,
                format!("expected one texture, found {}", textures.len()),
            ));
        }
        Ok(textures.remove(0))
    }

    /// Serialises the header. Parsing and writing back a game file yields the same bytes.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        set_to_bytes(std::slice::from_ref(self))
    }

    /// The value of the parameter with the given name hash.
    pub fn param(&self, name_hash: u32) -> Option<&ParamValue> {
        self.params
            .iter()
            .find(|p| p.name_hash == name_hash)
            .map(|p| &p.value)
    }

    fn param_str(&self, name_hash: u32) -> Option<&str> {
        match self.param(name_hash) {
            Some(ParamValue::Str(text)) => Some(text),
            _ => None,
        }
    }

    /// Sets a parameter, adding it when missing.
    pub fn set_param(&mut self, name_hash: u32, value: ParamValue) {
        match self.params.iter_mut().find(|p| p.name_hash == name_hash) {
            Some(param) => param.value = value,
            None => self.params.push(Param { name_hash, value }),
        }
    }

    pub fn format(&self) -> Format {
        Format::from_name(self.param_str(PARAM_FORMAT).unwrap_or(""))
    }

    pub fn kind(&self) -> Kind {
        match self.param_str(PARAM_MODE) {
            Some("cube") => Kind::Cube,
            Some("3d") => Kind::Volume,
            _ => Kind::Flat,
        }
    }

    /// Whether the colour data is gamma-encoded.
    pub fn is_srgb(&self) -> bool {
        self.param_str(PARAM_COLOR_SPACE) == Some("sRGB")
    }

    /// Number of mip levels; cube and 3D textures have one.
    pub fn mip_count(&self) -> usize {
        match self.kind() {
            Kind::Flat => self.surface_count.max(1) as usize,
            Kind::Cube | Kind::Volume => 1,
        }
    }

    /// Faces of a cube map or slices of a volume; 1 for flat textures.
    pub fn layer_count(&self) -> usize {
        match self.kind() {
            Kind::Flat => 1,
            Kind::Cube | Kind::Volume => self.surface_count.max(1) as usize,
        }
    }

    /// The surfaces of the `tg4d` data, in storage order.
    pub fn surfaces(&self) -> Result<Vec<Surface>> {
        let format = self.format();
        let mut surfaces = Vec::new();
        let mut offset = 0;
        for layer in 0..self.layer_count() {
            for mip in 0..self.mip_count() {
                let width = (self.width as usize >> mip).max(1);
                let height = (self.height as usize >> mip).max(1);
                let size = format.surface_size(width, height).ok_or_else(|| {
                    Error::InvalidArgument(format!("unknown texture format {}", format.name()))
                })?;
                surfaces.push(Surface {
                    layer,
                    mip,
                    width,
                    height,
                    offset,
                    size,
                });
                offset += size;
            }
        }
        Ok(surfaces)
    }

    /// Size of the `tg4d` data implied by format, dimensions and surface count.
    pub fn expected_data_size(&self) -> Result<usize> {
        Ok(self.surfaces()?.iter().map(|s| s.size).sum())
    }
}

/// One mip level of one face or slice inside the `tg4d` data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Surface {
    pub layer: usize,
    pub mip: usize,
    pub width: usize,
    pub height: usize,
    pub offset: usize,
    pub size: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `tg4h` of the font texture `aachen_std_bold` from gf2/frontend.str.
    const AACHEN: &[u8] = &[
        0x52, 0x00, 0x00, 0x00, 0x03, 0x00, 0x01, 0x00, 0x14, 0x00, 0x00, 0x00, 0x18, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x4a, 0xd2, 0x8b, 0x07, 0x66, 0x00, 0x00, 0x00, 0xd0, 0xaa,
        0x0a, 0x00, 0x00, 0x04, 0x00, 0x02, 0x00, 0x00, 0x0b, 0x03, 0x30, 0x00, 0x00, 0x00, 0x3c,
        0x00, 0x00, 0x00, 0xdf, 0x72, 0xba, 0xde, 0x2e, 0x7b, 0x59, 0x96, 0x9f, 0x2a, 0x8b, 0x4f,
        0x00, 0x4b, 0x00, 0x00, 0x00, 0x00, 0x50, 0x00, 0x00, 0x00, 0x01, 0xd0, 0xaa, 0x0a, 0x00,
        0x44, 0x58, 0x54, 0x35, 0x00, 0x31, 0x00, 0x61, 0x73, 0x73, 0x65, 0x74, 0x73, 0x5f, 0x73,
        0x68, 0x61, 0x72, 0x65, 0x64, 0x5f, 0x66, 0x6f, 0x6e, 0x74, 0x73, 0x00, 0x61, 0x61, 0x63,
        0x68, 0x65, 0x6e, 0x5f, 0x73, 0x74, 0x64, 0x5f, 0x62, 0x6f, 0x6c, 0x64, 0x00,
    ];

    #[test]
    fn parses_a_game_header() {
        let h = Tg4Header::parse(AACHEN).unwrap();
        assert_eq!(h.package, "assets_shared_fonts");
        assert_eq!(h.name, "aachen_std_bold");
        assert_eq!(h.name_hash, 0x078B_D24A);
        assert_eq!((h.width, h.height), (1024, 512));
        assert_eq!(h.format(), Format::Dxt5);
        assert_eq!(h.kind(), Kind::Flat);
        assert_eq!(h.mip_count(), 11);
        assert_eq!(h.data_size, 699_088);
        assert_eq!(h.expected_data_size().unwrap(), 699_088);
        assert_eq!(h.param(PARAM_DATA_SIZE), Some(&ParamValue::Int(699_088)));
        let surfaces = h.surfaces().unwrap();
        assert_eq!(
            surfaces[0],
            Surface {
                layer: 0,
                mip: 0,
                width: 1024,
                height: 512,
                offset: 0,
                size: 524_288
            }
        );
        assert_eq!(
            surfaces.last().unwrap().size,
            16,
            "1×1 still takes a whole block"
        );
    }

    #[test]
    fn writes_the_same_bytes() {
        assert_eq!(
            Tg4Header::parse(AACHEN).unwrap().to_bytes().unwrap(),
            AACHEN
        );
    }

    #[test]
    fn edits_roundtrip() {
        let mut h = Tg4Header::parse(AACHEN).unwrap();
        h.set_param(PARAM_FORMAT, ParamValue::Str("DXT1c".into()));
        h.set_param(PARAM_MODE, ParamValue::Str("cube".into()));
        h.surface_count = 6;
        let parsed = Tg4Header::parse(&h.to_bytes().unwrap()).unwrap();
        assert_eq!(parsed, h);
        assert_eq!(parsed.kind(), Kind::Cube);
        assert_eq!((parsed.mip_count(), parsed.layer_count()), (1, 6));
        assert_eq!(parsed.expected_data_size().unwrap(), 6 * 256 * 128 * 8);
    }

    #[test]
    fn rejects_malformed_headers() {
        assert!(Tg4Header::parse(&[]).is_err());
        let mut bad = AACHEN.to_vec();
        bad[4] = 4;
        assert!(matches!(
            Tg4Header::parse(&bad),
            Err(Error::UnsupportedVersion { .. })
        ));
        let mut bad = AACHEN.to_vec();
        bad[0x3C] = 7; // parameter kind
        assert!(Tg4Header::parse(&bad).is_err());
        for len in 0..AACHEN.len() {
            let _ = Tg4Header::parse(&AACHEN[..len]);
        }
    }
}

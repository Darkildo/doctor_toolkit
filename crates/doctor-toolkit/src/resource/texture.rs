//! Textures assembled from `tg4h` / `tg4d` resource pairs of a stream.

use crate::container::ustream::StreamLayout;
use crate::hash::sdbm;
use crate::resource::{dds, pixels, tg4::Tg4Header};
use crate::{Error, Result};

/// Type id of texture headers.
pub const TYPE_TG4H: u32 = sdbm(b"tg4h");
/// Type id of texture data.
pub const TYPE_TG4D: u32 = sdbm(b"tg4d");

/// Indices of the two resources that make up one texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TexturePair {
    pub header: usize,
    pub data: usize,
}

/// Pairs every `tg4h` of a stream with its `tg4d`. Both carry the texture's
/// name hash as their last id; the data normally follows the header directly.
pub fn pairs(layout: &StreamLayout) -> Vec<TexturePair> {
    let resources = &layout.resources;
    let mut out = Vec::new();
    for (header, r) in resources.iter().enumerate() {
        if r.header.type_id != TYPE_TG4H {
            continue;
        }
        let id = r.header.ids[3];
        let is_data = |i: &usize| {
            resources[*i].header.type_id == TYPE_TG4D && resources[*i].header.ids[3] == id
        };
        let data = (header + 1..resources.len())
            .find(is_data)
            .or_else(|| (0..header).rev().find(is_data));
        if let Some(data) = data {
            out.push(TexturePair { header, data });
        }
    }
    out
}

/// Finds the pair that contains resource `index`, as header or as data.
pub fn pair_of(layout: &StreamLayout, index: usize) -> Option<TexturePair> {
    pairs(layout)
        .into_iter()
        .find(|p| p.header == index || p.data == index)
}

/// A texture loaded from a stream.
#[derive(Debug, Clone)]
pub struct Texture {
    pub header: Tg4Header,
    pub data: Vec<u8>,
}

impl Texture {
    /// Reads the texture of `pair` from the stream file `file`.
    pub fn load(file: &[u8], layout: &StreamLayout, pair: TexturePair) -> Result<Self> {
        let header = Tg4Header::parse(&layout.resource_data(file, pair.header)?)?;
        let data = layout.resource_data(file, pair.data)?;
        if data.len() != header.data_size as usize {
            return Err(Error::malformed(
                "tg4d",
                0,
                format!(
                    "{} byte(s) of data, header declares {}",
                    data.len(),
                    header.data_size
                ),
            ));
        }
        Ok(Self { header, data })
    }

    /// The texture as a DDS file.
    pub fn to_dds(&self) -> Result<Vec<u8>> {
        dds::write(&self.header, &self.data)
    }

    /// The largest mip level of `layer` (face or slice) as RGBA pixels.
    pub fn rgba(&self, layer: usize) -> Result<(usize, usize, Vec<u8>)> {
        self.rgba_level(layer, 0)
    }

    /// Mip level `mip` of `layer` (face or slice) as RGBA pixels.
    pub fn rgba_level(&self, layer: usize, mip: usize) -> Result<(usize, usize, Vec<u8>)> {
        let surface = self
            .header
            .surfaces()?
            .into_iter()
            .find(|s| s.layer == layer && s.mip == mip)
            .ok_or_else(|| {
                Error::InvalidArgument(format!("texture has no layer {layer} with mip {mip}"))
            })?;
        // The header's data size need not agree with its format and dimensions.
        let bytes = self
            .data
            .get(surface.offset..surface.offset + surface.size)
            .ok_or_else(|| {
                Error::malformed(
                    "tg4d",
                    surface.offset,
                    format!(
                        "{} byte(s) of data, the surface needs {}",
                        self.data.len(),
                        surface.offset + surface.size
                    ),
                )
            })?;
        let rgba = pixels::decode(&self.header.format(), surface.width, surface.height, bytes)?;
        Ok((surface.width, surface.height, rgba))
    }

    /// The largest mip level of `layer` as a PNG file.
    pub fn to_png(&self, layer: usize) -> Result<Vec<u8>> {
        let (width, height, rgba) = self.rgba(layer)?;
        pixels::encode_png(width, height, &rgba)
    }
}

/// Builds a replacement for `original` from a DDS file.
///
/// Name, hash, package and the other parameters are kept. Size, surface count
/// and data change. The format must belong to the same family as a DDS can
/// express it: `DXT1` keeps `DXT1c` or `DXT1a`, `DXT5` keeps `DXT5` or
/// `DXT5_NM`. The shape (flat, cube, volume) must not change.
pub fn from_dds(original: &Tg4Header, dds_file: &[u8]) -> Result<Texture> {
    use crate::resource::tg4::{Format, Kind, PARAM_DATA_SIZE, PARAM_FORMAT, ParamValue};

    let image = dds::read(dds_file)?;
    if image.kind != original.kind() {
        return Err(Error::InvalidArgument(format!(
            "texture `{}` is {:?}, the DDS file is {:?}",
            original.name,
            original.kind(),
            image.kind
        )));
    }
    let original_format = original.format();
    let format = match (&image.format, &original_format) {
        (Format::Dxt1c, Format::Dxt1a | Format::Dxt1c) => original_format.clone(),
        (Format::Dxt5, Format::Dxt5 | Format::Dxt5Nm) => original_format.clone(),
        (new, _) => new.clone(),
    };
    let surface_count = match image.kind {
        Kind::Flat => image.mip_count,
        Kind::Cube | Kind::Volume => image.layer_count,
    };
    let mut header = original.clone();
    header.width = image.width;
    header.height = image.height;
    header.surface_count = u8::try_from(surface_count)
        .map_err(|_| Error::InvalidArgument("too many mip levels or layers".into()))?;
    header.data_size = image.data.len() as u32;
    header.set_param(PARAM_FORMAT, ParamValue::Str(format.name().to_owned()));
    header.set_param(PARAM_DATA_SIZE, ParamValue::Int(header.data_size));
    debug_assert_eq!(header.expected_data_size()?, image.data.len());
    Ok(Texture {
        header,
        data: image.data.to_vec(),
    })
}

/// Builds a replacement for a flat texture from a PNG image.
///
/// The image is encoded in the original format with a full mip chain.
/// Normal maps (`DXT5_NM`) are expected as ordinary RGB normal maps.
pub fn from_png(original: &Tg4Header, png_file: &[u8]) -> Result<Texture> {
    from_png_with_mips(original, png_file, None)
}

/// Like [`from_png`], with at most `max_mips` mip levels (`None`: a full
/// chain). Textures embedded in `apx` files keep their mip count: the game
/// crashes when an embedded texture has more data than it had.
pub fn from_png_with_mips(
    original: &Tg4Header,
    png_file: &[u8],
    max_mips: Option<usize>,
) -> Result<Texture> {
    use crate::resource::tg4::Kind;

    if original.kind() != Kind::Flat {
        return Err(Error::InvalidArgument(format!(
            "texture `{}` is a cube or volume texture; import a DDS file instead",
            original.name
        )));
    }
    let (width, height, rgba) = pixels::decode_png(png_file)?;
    from_rgba(original, width, height, rgba, max_mips)
}

/// Like [`from_png_with_mips`], from decoded RGBA pixels.
pub fn from_rgba(
    original: &Tg4Header,
    width: usize,
    height: usize,
    rgba: Vec<u8>,
    max_mips: Option<usize>,
) -> Result<Texture> {
    use crate::resource::tg4::{Kind, PARAM_DATA_SIZE, ParamValue};

    if original.kind() != Kind::Flat {
        return Err(Error::InvalidArgument(format!(
            "texture `{}` is a cube or volume texture",
            original.name
        )));
    }
    if rgba.len() != width * height * 4 || width == 0 || height == 0 {
        return Err(Error::InvalidArgument(format!(
            "image of {width}×{height} pixels with {} bytes",
            rgba.len()
        )));
    }
    let to_u16 = |v: usize| {
        u16::try_from(v).map_err(|_| Error::InvalidArgument(format!("image size {v} too large")))
    };
    let format = original.format();
    let full = (usize::BITS - width.max(height).leading_zeros()) as usize;
    let mips = max_mips.map_or(full, |m| m.clamp(1, full));
    let mut data = Vec::new();
    let (mut w, mut h, mut level) = (width, height, rgba);
    for mip in 0..mips {
        data.extend(pixels::encode(&format, w, h, &level)?);
        if mip + 1 < mips {
            (w, h, level) = pixels::downsample(w, h, &level);
        }
    }
    let mut header = original.clone();
    header.width = to_u16(width)?;
    header.height = to_u16(height)?;
    header.surface_count = mips as u8;
    header.data_size = data.len() as u32;
    header.set_param(PARAM_DATA_SIZE, ParamValue::Int(header.data_size));
    debug_assert_eq!(header.expected_data_size()?, data.len());
    Ok(Texture { header, data })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::tg4::{Format, PARAM_DATA_SIZE, PARAM_FORMAT, ParamValue};

    fn header(format: &str, width: u16, height: u16, mips: u8) -> Tg4Header {
        let mut h = Tg4Header {
            version: 3,
            budget_hash: 0,
            package: "pkg".into(),
            name_hash: 7,
            name: "wall_n".into(),
            data_size: 0,
            width,
            height,
            padding: 0,
            surface_count: mips,
            params: Vec::new(),
        };
        h.set_param(PARAM_FORMAT, ParamValue::Str(format.into()));
        h.data_size = h.expected_data_size().unwrap() as u32;
        h.set_param(PARAM_DATA_SIZE, ParamValue::Int(h.data_size));
        h
    }

    #[test]
    fn dds_replacement_keeps_identity_and_variant() {
        let original = header("DXT5_NM", 64, 64, 7);
        let bigger = header("DXT5", 128, 64, 8);
        let data = vec![0xAB; bigger.data_size as usize];
        let file = dds::write(&bigger, &data).unwrap();
        let texture = from_dds(&original, &file).unwrap();
        assert_eq!(texture.header.name, "wall_n");
        assert_eq!(texture.header.name_hash, 7);
        assert_eq!(
            texture.header.format(),
            Format::Dxt5Nm,
            "normal-map variant kept"
        );
        assert_eq!(
            (
                texture.header.width,
                texture.header.height,
                texture.header.mip_count()
            ),
            (128, 64, 8)
        );
        assert_eq!(
            texture.header.param(PARAM_DATA_SIZE),
            Some(&ParamValue::Int(data.len() as u32))
        );
        assert_eq!(texture.data, data);
        assert_eq!(
            Tg4Header::parse(&texture.header.to_bytes().unwrap()).unwrap(),
            texture.header
        );
    }

    #[test]
    fn png_replacement_builds_full_mip_chain() {
        let original = header("DXT1c", 64, 64, 7);
        let rgba: Vec<u8> = (0..100 * 50)
            .flat_map(|i| [(i % 256) as u8, 50, 90, 255])
            .collect();
        let png = pixels::encode_png(100, 50, &rgba).unwrap();
        let texture = from_png(&original, &png).unwrap();
        assert_eq!((texture.header.width, texture.header.height), (100, 50));
        assert_eq!(
            texture.header.mip_count(),
            7,
            "100 → 50 → 25 → 12 → 6 → 3 → 1"
        );
        assert_eq!(texture.data.len(), texture.header.data_size as usize);
        let (w, h, back) = texture.rgba(0).unwrap();
        assert_eq!((w, h), (100, 50));
        assert!(back[0].abs_diff(rgba[0]) < 16);
        let (w, h, level) = texture.rgba_level(0, 3).unwrap();
        assert_eq!((w, h, level.len()), (12, 6, 12 * 6 * 4));
        assert!(texture.rgba_level(0, 7).is_err());
    }

    #[test]
    fn dds_replacement_can_change_format_family() {
        let original = header("DXT1c", 32, 32, 6);
        let rgba = header("RGBA8", 16, 16, 1);
        let file = dds::write(&rgba, &vec![1; rgba.data_size as usize]).unwrap();
        assert_eq!(
            from_dds(&original, &file).unwrap().header.format(),
            Format::Rgba8
        );
    }

    /// Found by fuzzing: a header whose data size disagrees with its
    /// dimensions made decoding index past the data.
    #[test]
    fn decoding_short_data_is_an_error() {
        let mut h = header("DXT1c", 64, 64, 1);
        h.data_size = 8;
        let texture = Texture {
            header: h,
            data: vec![0; 8],
        };
        assert!(texture.rgba(0).is_err());
        assert!(texture.to_png(0).is_err());
    }
}

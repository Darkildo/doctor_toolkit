//! DirectDraw Surface (DDS) files for TG4 textures.
//!
//! TG4 data is stored in DDS order already (faces or slices, each with its
//! mip chain from largest to smallest), so conversion only adds or strips the
//! 128-byte DDS header.
//!
//! Byte order of `L8A8` and `RGBA8` texels is assumed to follow Direct3D 9
//! (`A8L8`, `A8R8G8B8`); it has not been confirmed against the game.

use crate::io::{self, Endian};
use crate::resource::tg4::{Format, Kind, Tg4Header};
use crate::{Error, Result};

const MAGIC: &[u8; 4] = b"DDS ";
const HEADER_SIZE: u32 = 124;
const PIXEL_FORMAT_SIZE: u32 = 32;
/// Magic plus header.
pub const PREFIX_SIZE: usize = 128;

const DDSD_CAPS: u32 = 0x1;
const DDSD_HEIGHT: u32 = 0x2;
const DDSD_WIDTH: u32 = 0x4;
const DDSD_PITCH: u32 = 0x8;
const DDSD_PIXELFORMAT: u32 = 0x1000;
const DDSD_MIPMAPCOUNT: u32 = 0x2_0000;
const DDSD_LINEARSIZE: u32 = 0x8_0000;
const DDSD_DEPTH: u32 = 0x80_0000;

const DDPF_ALPHAPIXELS: u32 = 0x1;
const DDPF_ALPHA: u32 = 0x2;
const DDPF_FOURCC: u32 = 0x4;
const DDPF_RGB: u32 = 0x40;
const DDPF_LUMINANCE: u32 = 0x2_0000;

const DDSCAPS_COMPLEX: u32 = 0x8;
const DDSCAPS_TEXTURE: u32 = 0x1000;
const DDSCAPS_MIPMAP: u32 = 0x40_0000;
const DDSCAPS2_CUBEMAP_ALL_FACES: u32 = 0xFE00;
const DDSCAPS2_VOLUME: u32 = 0x20_0000;

/// Pixel format fields of a DDS header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PixelFormat {
    flags: u32,
    four_cc: [u8; 4],
    bits: u32,
    masks: [u32; 4],
}

fn pixel_format(format: &Format) -> Result<PixelFormat> {
    let four_cc = |code: &[u8; 4]| PixelFormat {
        flags: DDPF_FOURCC,
        four_cc: *code,
        bits: 0,
        masks: [0; 4],
    };
    let masked = |flags, bits, masks| PixelFormat {
        flags,
        four_cc: [0; 4],
        bits,
        masks,
    };
    Ok(match format {
        Format::Dxt1c | Format::Dxt1a => four_cc(b"DXT1"),
        Format::Dxt5 | Format::Dxt5Nm => four_cc(b"DXT5"),
        Format::L8 => masked(DDPF_LUMINANCE, 8, [0xFF, 0, 0, 0]),
        Format::A8 => masked(DDPF_ALPHA, 8, [0, 0, 0, 0xFF]),
        Format::L8A8 => masked(DDPF_LUMINANCE | DDPF_ALPHAPIXELS, 16, [0xFF, 0, 0, 0xFF00]),
        Format::Rgba8 => masked(
            DDPF_RGB | DDPF_ALPHAPIXELS,
            32,
            [0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0xFF00_0000],
        ),
        Format::Other(name) => {
            return Err(Error::InvalidArgument(format!(
                "no DDS mapping for format {name}"
            )));
        }
    })
}

/// Writes a DDS file with the texture described by `header` and its `tg4d` data.
pub fn write(header: &Tg4Header, data: &[u8]) -> Result<Vec<u8>> {
    let format = header.format();
    let pf = pixel_format(&format)?;
    let expected = header.expected_data_size()?;
    if data.len() != expected {
        return Err(Error::InvalidArgument(format!(
            "texture data has {} byte(s), the header implies {expected}",
            data.len()
        )));
    }
    let (width, height) = (header.width as u32, header.height as u32);
    let mips = header.mip_count() as u32;
    let compressed = pf.flags & DDPF_FOURCC != 0;
    let mut flags = DDSD_CAPS | DDSD_HEIGHT | DDSD_WIDTH | DDSD_PIXELFORMAT;
    flags |= if compressed {
        DDSD_LINEARSIZE
    } else {
        DDSD_PITCH
    };
    let pitch = if compressed {
        format
            .surface_size(width as usize, height as usize)
            .unwrap_or(0) as u32
    } else {
        width * pf.bits / 8
    };
    let mut caps = DDSCAPS_TEXTURE;
    let mut caps2 = 0;
    let mut depth = 0;
    if mips > 1 {
        flags |= DDSD_MIPMAPCOUNT;
        caps |= DDSCAPS_MIPMAP | DDSCAPS_COMPLEX;
    }
    match header.kind() {
        Kind::Flat => {}
        Kind::Cube => {
            caps |= DDSCAPS_COMPLEX;
            caps2 |= DDSCAPS2_CUBEMAP_ALL_FACES;
        }
        Kind::Volume => {
            flags |= DDSD_DEPTH;
            caps |= DDSCAPS_COMPLEX;
            caps2 |= DDSCAPS2_VOLUME;
            depth = header.layer_count() as u32;
        }
    }

    let le = Endian::Little;
    let mut out = Vec::with_capacity(PREFIX_SIZE + data.len());
    out.extend_from_slice(MAGIC);
    for value in [
        HEADER_SIZE,
        flags,
        height,
        width,
        pitch,
        depth,
        if mips > 1 { mips } else { 0 },
    ] {
        io::put_u32(&mut out, value, le);
    }
    out.resize(out.len() + 11 * 4, 0);
    io::put_u32(&mut out, PIXEL_FORMAT_SIZE, le);
    io::put_u32(&mut out, pf.flags, le);
    out.extend_from_slice(&pf.four_cc);
    io::put_u32(&mut out, pf.bits, le);
    for mask in pf.masks {
        io::put_u32(&mut out, mask, le);
    }
    for value in [caps, caps2, 0, 0, 0] {
        io::put_u32(&mut out, value, le);
    }
    debug_assert_eq!(out.len(), PREFIX_SIZE);
    out.extend_from_slice(data);
    Ok(out)
}

/// The parts of a DDS file needed to import it as a TG4 texture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DdsImage<'a> {
    /// The format family. DXT1 files read as [`Format::Dxt1c`] and DXT5 files as
    /// [`Format::Dxt5`]; the caller decides about the alpha and normal-map variants.
    pub format: Format,
    pub kind: Kind,
    pub width: u16,
    pub height: u16,
    pub mip_count: usize,
    /// Faces or slices.
    pub layer_count: usize,
    pub data: &'a [u8],
}

/// Reads a DDS file written by [`write`] or by a common DDS tool.
pub fn read(file: &[u8]) -> Result<DdsImage<'_>> {
    let le = Endian::Little;
    if io::slice(file, 0, 4)? != MAGIC {
        return Err(Error::BadMagic {
            what: "DDS",
            found: io::read_u32(file, 0, le)?,
        });
    }
    let field = |index: usize| io::read_u32(file, 4 + 4 * index, le);
    if field(0)? != HEADER_SIZE {
        return Err(Error::malformed("DDS", 4, "unexpected header size"));
    }
    let flags = field(1)?;
    let height = field(2)?;
    let width = field(3)?;
    let depth = field(5)?;
    let mips = if flags & DDSD_MIPMAPCOUNT != 0 {
        field(6)?.max(1)
    } else {
        1
    };
    let pf_flags = field(19)?;
    let four_cc: [u8; 4] = io::slice(file, 84, 4)?.try_into().expect("four bytes");
    let bits = field(21)?;
    let masks = [field(22)?, field(23)?, field(24)?, field(25)?];
    let caps2 = field(27)?;
    if &four_cc == b"DX10" {
        return Err(Error::InvalidArgument(
            "DDS files with a DX10 header are not supported; save as legacy DXT1/DXT5".into(),
        ));
    }

    let format = if pf_flags & DDPF_FOURCC != 0 {
        match &four_cc {
            b"DXT1" => Format::Dxt1c,
            b"DXT5" => Format::Dxt5,
            other => {
                return Err(Error::InvalidArgument(format!(
                    "unsupported DDS compression {}",
                    String::from_utf8_lossy(other)
                )));
            }
        }
    } else {
        let candidates = [Format::L8, Format::A8, Format::L8A8, Format::Rgba8];
        candidates
            .into_iter()
            .find(|f| {
                pixel_format(f).is_ok_and(|pf| {
                    pf.bits == bits
                        && pf.masks == masks
                        && (pf.flags & !DDPF_ALPHAPIXELS) == (pf_flags & !DDPF_ALPHAPIXELS)
                })
            })
            .ok_or_else(|| {
                Error::InvalidArgument(format!(
                    "unsupported uncompressed DDS layout: {bits} bit(s), masks {masks:08x?}"
                ))
            })?
    };

    let (kind, layer_count) = if caps2 & DDSCAPS2_CUBEMAP_ALL_FACES == DDSCAPS2_CUBEMAP_ALL_FACES {
        (Kind::Cube, 6)
    } else if caps2 & DDSCAPS2_VOLUME != 0 {
        (Kind::Volume, depth.max(1) as usize)
    } else {
        (Kind::Flat, 1)
    };
    if kind != Kind::Flat && mips > 1 {
        return Err(Error::InvalidArgument(
            "cube and volume textures must have a single mip level".into(),
        ));
    }
    let too_big = |v: u32| {
        u16::try_from(v).map_err(|_| Error::InvalidArgument(format!("texture size {v} too large")))
    };
    let (width, height) = (too_big(width)?, too_big(height)?);
    // A 16-bit size has at most 17 levels; below 1x1 a mip chain cannot go.
    let max_mips = u32::BITS - u32::from(width.max(height).max(1)).leading_zeros();
    if mips > max_mips {
        return Err(Error::InvalidArgument(format!(
            "{mips} mip levels for a {width}x{height} texture"
        )));
    }
    if layer_count > usize::from(u8::MAX) {
        return Err(Error::InvalidArgument(format!(
            "{layer_count} slices; a texture has at most 255"
        )));
    }

    let mut size = 0usize;
    for mip in 0..mips as usize {
        let w = (width as usize >> mip).max(1);
        let h = (height as usize >> mip).max(1);
        size += format.surface_size(w, h).expect("known format");
    }
    size *= layer_count;
    let data = io::slice(file, PREFIX_SIZE, size).map_err(|_| {
        Error::malformed(
            "DDS",
            PREFIX_SIZE,
            "pixel data shorter than the header implies",
        )
    })?;
    Ok(DdsImage {
        format,
        kind,
        width,
        height,
        mip_count: mips as usize,
        layer_count,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::tg4::{PARAM_FORMAT, PARAM_MODE, ParamValue};

    fn header(
        format: &str,
        width: u16,
        height: u16,
        surfaces: u8,
        kind: Option<&str>,
    ) -> Tg4Header {
        let mut h = Tg4Header {
            version: 3,
            budget_hash: 0,
            package: "pkg".into(),
            name_hash: 1,
            name: "tex".into(),
            data_size: 0,
            width,
            height,
            padding: 0,
            surface_count: surfaces,
            params: Vec::new(),
        };
        h.set_param(PARAM_FORMAT, ParamValue::Str(format.into()));
        if let Some(kind) = kind {
            h.set_param(PARAM_MODE, ParamValue::Str(kind.into()));
        }
        h
    }

    fn roundtrip(h: &Tg4Header) -> (Vec<u8>, Vec<u8>) {
        let size = h.expected_data_size().unwrap();
        let data: Vec<u8> = (0..size).map(|i| (i * 7) as u8).collect();
        let file = write(h, &data).unwrap();
        assert_eq!(file.len(), PREFIX_SIZE + size);
        (file, data)
    }

    #[test]
    fn flat_compressed_texture() {
        let h = header("DXT5", 256, 64, 9, None);
        let (file, data) = roundtrip(&h);
        assert_eq!(&file[84..88], b"DXT5");
        let image = read(&file).unwrap();
        assert_eq!(image.format, Format::Dxt5);
        assert_eq!((image.width, image.height, image.mip_count), (256, 64, 9));
        assert_eq!(image.kind, Kind::Flat);
        assert_eq!(image.data, data);
    }

    #[test]
    fn cube_and_volume_textures() {
        let (file, data) = roundtrip(&header("DXT1c", 64, 64, 6, Some("cube")));
        let image = read(&file).unwrap();
        assert_eq!(
            (image.kind, image.layer_count, image.mip_count),
            (Kind::Cube, 6, 1)
        );
        assert_eq!(image.data, data);

        let (file, data) = roundtrip(&header("RGBA8", 32, 32, 32, Some("3d")));
        let image = read(&file).unwrap();
        assert_eq!(
            (image.kind, image.layer_count, image.format),
            (Kind::Volume, 32, Format::Rgba8)
        );
        assert_eq!(image.data, data);
    }

    #[test]
    fn uncompressed_formats_are_recognised() {
        for format in ["L8", "A8", "L8A8", "RGBA8"] {
            let (file, _) = roundtrip(&header(format, 16, 8, 5, None));
            assert_eq!(read(&file).unwrap().format.name(), format);
        }
    }

    #[test]
    fn rejects_bad_input() {
        let h = header("DXT1c", 16, 16, 1, None);
        assert!(write(&h, &[0; 3]).is_err(), "wrong data size");
        assert!(write(&header("XYZ", 4, 4, 1, None), &[]).is_err());
        let (file, _) = roundtrip(&h);
        assert!(read(&file[..100]).is_err());
        assert!(read(&file[..file.len() - 1]).is_err());
        assert!(read(b"PNG not a dds file at all, really").is_err());
        let mut dx10 = file.clone();
        dx10[84..88].copy_from_slice(b"DX10");
        assert!(read(&dx10).is_err());
    }

    /// Found by fuzzing: a huge mip or slice count made  loop for minutes
    /// or overflow the data size.
    #[test]
    fn rejects_implausible_mip_and_slice_counts() {
        let (file, _) = roundtrip(&header("DXT1c", 16, 16, 1, None));
        let mut mips = file.clone();
        let flags = u32::from_le_bytes(mips[8..12].try_into().unwrap()) | DDSD_MIPMAPCOUNT;
        mips[8..12].copy_from_slice(&flags.to_le_bytes());
        mips[28..32].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(read(&mips).is_err());
        let mut slices = file;
        slices[24..28].copy_from_slice(&u32::MAX.to_le_bytes());
        slices[112..116].copy_from_slice(&DDSCAPS2_VOLUME.to_le_bytes());
        assert!(read(&slices).is_err());
    }
}

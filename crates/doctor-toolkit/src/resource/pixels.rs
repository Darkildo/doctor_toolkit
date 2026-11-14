//! Decoding texture surfaces to 8-bit RGBA, for PNG export and previews.

use crate::resource::tg4::Format;
use crate::{Error, Result};

/// Decodes one surface of `format` into RGBA, four bytes per pixel, row by row.
///
/// `DXT5_NM` normal maps are returned as stored (X in alpha, Y in green).
pub fn decode(format: &Format, width: usize, height: usize, data: &[u8]) -> Result<Vec<u8>> {
    let needed = format
        .surface_size(width, height)
        .ok_or_else(|| Error::InvalidArgument(format!("cannot decode format {}", format.name())))?;
    if data.len() < needed {
        return Err(Error::InvalidArgument(format!(
            "surface needs {needed} byte(s), got {}",
            data.len()
        )));
    }
    let mut out = vec![0u8; width * height * 4];
    match format {
        Format::Dxt1c | Format::Dxt1a | Format::Dxt5 | Format::Dxt5Nm => {
            let block_bytes = if matches!(format, Format::Dxt5 | Format::Dxt5Nm) {
                16
            } else {
                8
            };
            let blocks_x = width.div_ceil(4);
            for (b, block) in data[..needed].chunks_exact(block_bytes).enumerate() {
                let texels = if block_bytes == 16 {
                    decode_bc3(block)
                } else {
                    decode_bc1(block, true)
                };
                let (bx, by) = (b % blocks_x * 4, b / blocks_x * 4);
                for (i, texel) in texels.iter().enumerate() {
                    let (x, y) = (bx + i % 4, by + i / 4);
                    if x < width && y < height {
                        out[(y * width + x) * 4..][..4].copy_from_slice(texel);
                    }
                }
            }
        }
        Format::L8 => {
            for (px, &l) in out.chunks_exact_mut(4).zip(data) {
                px.copy_from_slice(&[l, l, l, 255]);
            }
        }
        Format::A8 => {
            // Direct3D 9 samples alpha-only textures as (0, 0, 0, a): the
            // eyelashes of the game (`lash_c`, A8) are black.
            for (px, &a) in out.chunks_exact_mut(4).zip(data) {
                px.copy_from_slice(&[0, 0, 0, a]);
            }
        }
        Format::L8A8 => {
            for (px, la) in out.chunks_exact_mut(4).zip(data.chunks_exact(2)) {
                px.copy_from_slice(&[la[0], la[0], la[0], la[1]]);
            }
        }
        Format::Rgba8 => {
            // Stored as Direct3D A8R8G8B8: B, G, R, A in memory.
            for (px, bgra) in out.chunks_exact_mut(4).zip(data.chunks_exact(4)) {
                px.copy_from_slice(&[bgra[2], bgra[1], bgra[0], bgra[3]]);
            }
        }
        Format::Other(_) => unreachable!("rejected by surface_size"),
    }
    Ok(out)
}

fn rgb565(value: u16) -> [u8; 3] {
    let r = (value >> 11) & 0x1F;
    let g = (value >> 5) & 0x3F;
    let b = value & 0x1F;
    [
        ((r << 3) | (r >> 2)) as u8,
        ((g << 2) | (g >> 4)) as u8,
        ((b << 3) | (b >> 2)) as u8,
    ]
}

/// Decodes a BC1 block. `allow_alpha` enables the three-colour mode with transparency.
fn decode_bc1(block: &[u8], allow_alpha: bool) -> [[u8; 4]; 16] {
    let c0 = u16::from_le_bytes([block[0], block[1]]);
    let c1 = u16::from_le_bytes([block[2], block[3]]);
    let (a, b) = (rgb565(c0), rgb565(c1));
    let mix = |wa: u16, wb: u16, div: u16| -> [u8; 4] {
        let ch = |i: usize| ((a[i] as u16 * wa + b[i] as u16 * wb) / div) as u8;
        [ch(0), ch(1), ch(2), 255]
    };
    let palette = if c0 > c1 || !allow_alpha {
        [
            [a[0], a[1], a[2], 255],
            [b[0], b[1], b[2], 255],
            mix(2, 1, 3),
            mix(1, 2, 3),
        ]
    } else {
        [
            [a[0], a[1], a[2], 255],
            [b[0], b[1], b[2], 255],
            mix(1, 1, 2),
            [0, 0, 0, 0],
        ]
    };
    let indices = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
    std::array::from_fn(|i| palette[((indices >> (2 * i)) & 3) as usize])
}

/// Decodes a BC3 block: interpolated alpha followed by a BC1 colour block.
fn decode_bc3(block: &[u8]) -> [[u8; 4]; 16] {
    let (a0, a1) = (block[0] as u16, block[1] as u16);
    let alphas: [u8; 8] = if a0 > a1 {
        std::array::from_fn(|i| match i {
            0 => a0 as u8,
            1 => a1 as u8,
            _ => (((8 - i) as u16 * a0 + (i - 1) as u16 * a1) / 7) as u8,
        })
    } else {
        std::array::from_fn(|i| match i {
            0 => a0 as u8,
            1 => a1 as u8,
            6 => 0,
            7 => 255,
            _ => (((6 - i) as u16 * a0 + (i - 1) as u16 * a1) / 5) as u8,
        })
    };
    let bits = block[2..8]
        .iter()
        .rev()
        .fold(0u64, |acc, &b| (acc << 8) | b as u64);
    let mut texels = decode_bc1(&block[8..16], false);
    for (i, texel) in texels.iter_mut().enumerate() {
        texel[3] = alphas[((bits >> (3 * i)) & 7) as usize];
    }
    texels
}

/// Encodes one surface of RGBA pixels into `format`. The inverse of [`decode`].
///
/// For `DXT5_NM` the input is an ordinary normal map (X in red, Y in green);
/// it is stored with X in alpha and Y in green.
pub fn encode(format: &Format, width: usize, height: usize, rgba: &[u8]) -> Result<Vec<u8>> {
    if rgba.len() != width * height * 4 {
        return Err(Error::InvalidArgument(
            "pixel buffer does not match the size".into(),
        ));
    }
    let compress = |kind: texpresso::Format, pixels: &[u8]| {
        let mut out = vec![0u8; kind.compressed_size(width, height)];
        let params = texpresso::Params {
            algorithm: texpresso::Algorithm::ClusterFit,
            ..Default::default()
        };
        kind.compress(pixels, width, height, params, &mut out);
        out
    };
    Ok(match format {
        Format::Dxt1c => {
            // Fully opaque input keeps the encoder in four-colour mode.
            let opaque: Vec<u8> = rgba
                .chunks_exact(4)
                .flat_map(|p| [p[0], p[1], p[2], 255])
                .collect();
            compress(texpresso::Format::Bc1, &opaque)
        }
        Format::Dxt1a => compress(texpresso::Format::Bc1, rgba),
        Format::Dxt5 => compress(texpresso::Format::Bc3, rgba),
        Format::Dxt5Nm => {
            let swizzled: Vec<u8> = rgba
                .chunks_exact(4)
                .flat_map(|p| [0, p[1], 0, p[0]])
                .collect();
            compress(texpresso::Format::Bc3, &swizzled)
        }
        Format::L8 => rgba.chunks_exact(4).map(|p| p[0]).collect(),
        Format::A8 => rgba.chunks_exact(4).map(|p| p[3]).collect(),
        Format::L8A8 => rgba.chunks_exact(4).flat_map(|p| [p[0], p[3]]).collect(),
        Format::Rgba8 => rgba
            .chunks_exact(4)
            .flat_map(|p| [p[2], p[1], p[0], p[3]])
            .collect(),
        Format::Other(name) => {
            return Err(Error::InvalidArgument(format!(
                "cannot encode format {name}"
            )));
        }
    })
}

/// Halves an RGBA image with a 2×2 box filter; an odd last row or column is dropped.
pub fn downsample(width: usize, height: usize, rgba: &[u8]) -> (usize, usize, Vec<u8>) {
    let (w, h) = ((width / 2).max(1), (height / 2).max(1));
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            for c in 0..4 {
                let mut sum = 0u32;
                let mut n = 0u32;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (sx, sy) = ((2 * x + dx).min(width - 1), (2 * y + dy).min(height - 1));
                    sum += rgba[(sy * width + sx) * 4 + c] as u32;
                    n += 1;
                }
                out[(y * w + x) * 4 + c] = ((sum + n / 2) / n) as u8;
            }
        }
    }
    (w, h, out)
}

/// Largest width or height of an imported PNG image. Textures of the game are
/// at most 2048 pixels wide; the limit keeps a forged header from making the
/// decoder allocate gigabytes.
pub const MAX_PNG_SIDE: u32 = 4096;

/// Decodes a PNG file into RGBA pixels.
pub fn decode_png(file: &[u8]) -> Result<(usize, usize, Vec<u8>)> {
    let error = |e: png::DecodingError| Error::InvalidArgument(format!("PNG: {e}"));
    let mut decoder = png::Decoder::new(std::io::Cursor::new(file));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(error)?;
    let (width, height) = (reader.info().width, reader.info().height);
    if width > MAX_PNG_SIDE || height > MAX_PNG_SIDE {
        return Err(Error::InvalidArgument(format!(
            "PNG image of {width}x{height} pixels; at most {MAX_PNG_SIDE}x{MAX_PNG_SIDE} is supported"
        )));
    }
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| Error::InvalidArgument("PNG image too large".into()))?;
    let mut buffer = vec![0u8; size];
    let info = reader.next_frame(&mut buffer).map_err(error)?;
    let (width, height) = (info.width as usize, info.height as usize);
    let pixels = &buffer[..info.buffer_size()];
    let rgba = match info.color_type {
        png::ColorType::Rgba => pixels.to_vec(),
        png::ColorType::Rgb => pixels
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => pixels
            .chunks_exact(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Grayscale => pixels.iter().flat_map(|&l| [l, l, l, 255]).collect(),
        png::ColorType::Indexed => {
            return Err(Error::InvalidArgument(
                "indexed PNG was not expanded".into(),
            ));
        }
    };
    Ok((width, height, rgba))
}

/// Turns a decoded `DXT5_NM` surface (X in alpha, Y in green) into an
/// ordinary RGB normal map with Z rebuilt.
pub fn rebuild_normal_map(rgba: &mut [u8]) {
    for px in rgba.chunks_exact_mut(4) {
        let x = px[3] as f32 / 127.5 - 1.0;
        let y = px[1] as f32 / 127.5 - 1.0;
        let z = (1.0 - x * x - y * y).max(0.0).sqrt();
        px[0] = px[3];
        px[2] = ((z + 1.0) * 127.5) as u8;
        px[3] = 255;
    }
}

/// Encodes RGBA pixels as a PNG file.
pub fn encode_png(width: usize, height: usize, rgba: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width as u32, height as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let error = |e: png::EncodingError| Error::InvalidArgument(format!("PNG: {e}"));
    let mut writer = encoder.write_header().map_err(error)?;
    writer.write_image_data(rgba).map_err(error)?;
    writer.finish().map_err(error)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpha_only_textures_are_black() {
        // Direct3D 9 samples A8 as (0, 0, 0, a); encoding keeps the alpha.
        let px = decode(&Format::A8, 2, 1, &[0, 200]).unwrap();
        assert_eq!(px, [0, 0, 0, 0, 0, 0, 0, 200]);
        assert_eq!(encode(&Format::A8, 2, 1, &px).unwrap(), [0, 200]);
    }

    #[test]
    fn bc1_opaque_and_transparent_modes() {
        // c0 = pure red, c1 = pure blue, indices 0,1,2,3 in the first row.
        let block = [0x00, 0xF8, 0x1F, 0x00, 0b1110_0100, 0, 0, 0];
        let px = decode(&Format::Dxt1c, 4, 4, &block).unwrap();
        assert_eq!(&px[0..4], &[255, 0, 0, 255]);
        assert_eq!(&px[4..8], &[0, 0, 255, 255]);
        assert_eq!(&px[8..12], &[170, 0, 85, 255]);
        assert_eq!(&px[12..16], &[85, 0, 170, 255]);
        assert_eq!(&px[16..20], &[255, 0, 0, 255], "index 0 in the second row");

        // c0 <= c1 selects the three-colour mode; index 3 is transparent.
        let block = [0x1F, 0x00, 0x00, 0xF8, 0b1100_0000, 0, 0, 0];
        let px = decode(&Format::Dxt1a, 4, 4, &block).unwrap();
        assert_eq!(&px[12..16], &[0, 0, 0, 0]);
    }

    #[test]
    fn bc3_alpha_interpolation() {
        // a0 = 255, a1 = 0: index 1 gives 0, index 2 gives 219.
        let mut block = [0u8; 16];
        block[0] = 255;
        block[1] = 0;
        block[2] = 0b0001_0001; // texel 0 -> index 1, texel 1 -> index 2
        block[8..16].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0]);
        let px = decode(&Format::Dxt5, 4, 4, &block).unwrap();
        assert_eq!(&px[0..4], &[255, 255, 255, 0]);
        assert_eq!(px[7], 218, "(6 * 255 + 1 * 0) / 7");
        assert_eq!(px[11], 255, "index 0 is a0");
    }

    #[test]
    fn small_and_odd_sizes() {
        // A 2×1 BC1 surface still uses a whole block.
        let block = [0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0];
        assert_eq!(decode(&Format::Dxt1c, 2, 1, &block).unwrap(), [255; 8]);
        assert!(decode(&Format::Dxt1c, 8, 8, &block).is_err());
        assert_eq!(decode(&Format::L8A8, 1, 1, &[9, 7]).unwrap(), [9, 9, 9, 7]);
        assert_eq!(
            decode(&Format::Rgba8, 1, 1, &[1, 2, 3, 4]).unwrap(),
            [3, 2, 1, 4]
        );
    }

    #[test]
    fn encode_roundtrips_through_decode() {
        // A smooth gradient survives BC compression with small errors.
        let (w, h) = (16, 8);
        let rgba: Vec<u8> = (0..w * h)
            .flat_map(|i| [(i * 2) as u8, 100, 200, 255])
            .collect();
        for format in [Format::Dxt1c, Format::Dxt5] {
            let packed = encode(&format, w, h, &rgba).unwrap();
            assert_eq!(packed.len(), format.surface_size(w, h).unwrap());
            let back = decode(&format, w, h, &packed).unwrap();
            let max_error = rgba
                .iter()
                .zip(&back)
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap();
            assert!(max_error <= 12, "{} error {max_error}", format.name());
        }
        for format in [Format::L8A8, Format::Rgba8] {
            let packed = encode(&format, w, h, &rgba).unwrap();
            let back = decode(&format, w, h, &packed).unwrap();
            if format == Format::Rgba8 {
                assert_eq!(back, rgba);
            }
        }
    }

    #[test]
    fn downsample_and_png_roundtrip() {
        let rgba = [0u8, 0, 0, 255, 200, 200, 200, 255, 100, 100, 100, 255];
        let (w, h, half) = downsample(3, 1, &rgba);
        assert_eq!((w, h), (1, 1));
        assert_eq!(
            half,
            [100, 100, 100, 255],
            "average of the first two pixels"
        );
        let png = encode_png(3, 1, &rgba).unwrap();
        assert_eq!(decode_png(&png).unwrap(), (3, 1, rgba.to_vec()));
        assert!(decode_png(b"not a png").is_err());
    }

    #[test]
    fn png_signature() {
        let png = encode_png(2, 2, &[0; 16]).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    }

    /// Found by fuzzing: the size in the header was allocated before any check.
    #[test]
    fn huge_png_is_refused_before_decoding() {
        let mut png = encode_png(2, 2, &[0; 16]).unwrap();
        // IHDR: type at 12, width and height at 16 and 20, CRC at 29.
        png[16..20].copy_from_slice(&100_000u32.to_be_bytes());
        png[20..24].copy_from_slice(&100_000u32.to_be_bytes());
        let crc = crate::hash::crc32(&png[12..29]);
        png[29..33].copy_from_slice(&crc.to_be_bytes());
        let error = decode_png(&png).unwrap_err().to_string();
        assert!(error.contains("100000x100000"), "{error}");
    }
}

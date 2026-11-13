//! On2 VP6 video: frame headers, a preview decoder and an encoder.
//!
//! The game's movies use fourcc `vp60` (VP6 sub-version 7, no filter header):
//! the picture is stored bottom-up, so the coded picture is the displayed one
//! flipped vertically ([`Picture::flipped`]).

pub mod dct;
pub mod decode;
pub mod encode;
pub mod huffman;
pub mod mc;
pub mod model;
pub mod rac;
pub mod tables;

pub use decode::{Decoder, FrameStats};
pub use encode::{EncodedFrame, Encoder, EncoderConfig};

use crate::{Error, Result};

/// The fixed part of a VP6 frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    pub key: bool,
    pub quantizer: u8,
    /// Coefficients are in a second partition.
    pub separated: bool,
    /// Key frames only.
    pub sub_version: u8,
    pub filter_header: u8,
    pub interlaced: bool,
    /// Coded and displayed size in macroblocks (key frames only).
    pub mb_rows: u8,
    pub mb_cols: u8,
    pub display_rows: u8,
    pub display_cols: u8,
    /// Absolute offset of the coefficient partition.
    pub coeff_offset: Option<usize>,
    /// Offset where the first range-coded partition starts.
    pub rac_start: usize,
}

impl FrameHeader {
    /// Parses a frame header. Inter frames are read as in streams whose key
    /// frames have no filter header (all the game's movies).
    pub fn parse(data: &[u8]) -> Result<Self> {
        Self::parse_in(data, 0)
    }

    /// Parses a frame header of a stream whose key frames have
    /// `filter_header` (inter frames depend on it).
    pub fn parse_in(data: &[u8], filter_header: u8) -> Result<Self> {
        let short = || Error::malformed("VP6", 0, "frame too short");
        let b0 = *data.first().ok_or_else(short)?;
        let key = b0 & 0x80 == 0;
        let separated = b0 & 1 != 0;
        let mut h = Self {
            key,
            quantizer: b0 >> 1 & 0x3F,
            separated,
            sub_version: 0,
            filter_header: 0,
            interlaced: false,
            mb_rows: 0,
            mb_cols: 0,
            display_rows: 0,
            display_cols: 0,
            coeff_offset: None,
            rac_start: 1,
        };
        let be16 = |at: usize| -> Result<usize> {
            let s = data.get(at..at + 2).ok_or_else(short)?;
            Ok(u16::from_be_bytes([s[0], s[1]]) as usize)
        };
        if key {
            let b1 = *data.get(1).ok_or_else(short)?;
            h.sub_version = b1 >> 3;
            h.filter_header = b1 & 6;
            h.interlaced = b1 & 1 != 0;
            let mut base = 2;
            if separated || h.filter_header == 0 {
                let off = be16(2)?;
                h.coeff_offset = (off != 2).then_some(off);
                base = 4;
            }
            let dims = data.get(base..base + 4).ok_or_else(short)?;
            h.mb_rows = dims[0];
            h.mb_cols = dims[1];
            h.display_rows = dims[2];
            h.display_cols = dims[3];
            if h.mb_rows == 0 || h.mb_cols == 0 {
                return Err(Error::malformed("VP6", base, "zero frame size"));
            }
            h.rac_start = base + 4;
        } else if separated || filter_header == 0 {
            let off = be16(1)?;
            h.coeff_offset = (off != 2).then_some(off);
            h.rac_start = 3;
        } else {
            h.filter_header = filter_header;
        }
        if h.rac_start >= data.len() {
            return Err(short());
        }
        Ok(h)
    }
}

/// A YUV 4:2:0 picture with 8-bit planes (luma `width`×`height`, chroma half size).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    pub width: usize,
    pub height: usize,
    /// Y, U, V.
    pub planes: [Vec<u8>; 3],
}

impl Picture {
    /// A black picture (`width` and `height` even).
    pub fn new(width: usize, height: usize) -> Self {
        let c = (width / 2) * (height / 2);
        Self {
            width,
            height,
            planes: [vec![16; width * height], vec![128; c], vec![128; c]],
        }
    }

    pub fn stride(&self, plane: usize) -> usize {
        if plane == 0 {
            self.width
        } else {
            self.width / 2
        }
    }

    /// Plane, offset and stride of block `b` (0..3 luma, 4 U, 5 V) of a macroblock.
    pub fn block_at(&self, row: usize, col: usize, b: usize) -> (usize, usize, usize) {
        if b < 4 {
            let s = self.width;
            let y = row * 16 + if b >= 2 { 8 } else { 0 };
            let x = col * 16 + if b & 1 == 1 { 8 } else { 0 };
            (0, y * s + x, s)
        } else {
            let s = self.width / 2;
            (b - 3, row * 8 * s + col * 8, s)
        }
    }

    /// The picture upside down.
    pub fn flipped(&self) -> Self {
        let mut out = self.clone();
        for p in 0..3 {
            let s = self.stride(p);
            let rows = self.planes[p].len() / s.max(1);
            for y in 0..rows {
                out.planes[p][y * s..(y + 1) * s]
                    .copy_from_slice(&self.planes[p][(rows - 1 - y) * s..(rows - y) * s]);
            }
        }
        out
    }

    /// Copies the top-left `width`×`height` area into a new picture.
    pub fn cropped(&self, width: usize, height: usize) -> Self {
        let mut out = Self::new(width, height);
        for p in 0..3 {
            let (w, h) = if p == 0 {
                (width, height)
            } else {
                (width / 2, height / 2)
            };
            let s = self.stride(p);
            for y in 0..h {
                out.planes[p][y * w..(y + 1) * w]
                    .copy_from_slice(&self.planes[p][y * s..y * s + w]);
            }
        }
        out
    }

    /// Pads to `width`×`height` by repeating the last row and column.
    pub fn padded(&self, width: usize, height: usize) -> Self {
        let mut out = Self::new(width, height);
        for p in 0..3 {
            let (sw, sh) = if p == 0 {
                (self.width, self.height)
            } else {
                (self.width / 2, self.height / 2)
            };
            let (w, h) = if p == 0 {
                (width, height)
            } else {
                (width / 2, height / 2)
            };
            for y in 0..h {
                let sy = y.min(sh - 1);
                for x in 0..w {
                    out.planes[p][y * w + x] = self.planes[p][sy * sw + x.min(sw - 1)];
                }
            }
        }
        out
    }

    /// RGBA8 pixels (BT.601 limited range, opaque) of the top-left
    /// `width`×`height` area into `out`, bottom row first when `flip`.
    /// Fixed-point arithmetic, two output rows per chroma row.
    pub fn rgba_into(&self, width: usize, height: usize, flip: bool, out: &mut Vec<u8>) {
        let width = width.min(self.width) & !1;
        let height = height.min(self.height) & !1;
        out.clear();
        out.resize(width * height * 4, 255);
        self.rgba_to_slice(width, height, flip, out);
    }

    /// [`Self::rgba_into`] into a buffer of exactly `width`×`height`×4
    /// bytes (`width` and `height` even and within the picture).
    pub fn rgba_to_slice(&self, width: usize, height: usize, flip: bool, out: &mut [u8]) {
        let width = width.min(self.width) & !1;
        let height = height.min(self.height) & !1;
        let out = &mut out[..width * height * 4];
        let cs = self.width / 2;
        // 13-bit fixed point: 1.164, 1.596, 0.392, 0.813, 2.017.
        const Y: i32 = 9535;
        const RV: i32 = 13074;
        const GU: i32 = 3211;
        const GV: i32 = 6660;
        const BU: i32 = 16523;
        // Two output rows per chroma row, chroma rows in parallel.
        use rayon::prelude::*;
        out.par_chunks_exact_mut(width * 8)
            .enumerate()
            .with_min_len(8)
            .for_each(|(pair, rows)| {
                let (top, bottom) = rows.split_at_mut(width * 4);
                let (y0, y1) = if flip {
                    (height - 1 - 2 * pair, height - 2 - 2 * pair)
                } else {
                    (2 * pair, 2 * pair + 1)
                };
                let c = y0 / 2;
                let u = &self.planes[1][c * cs..c * cs + width / 2];
                let v = &self.planes[2][c * cs..c * cs + width / 2];
                let l0 = &self.planes[0][y0 * self.width..y0 * self.width + width];
                let l1 = &self.planes[0][y1 * self.width..y1 * self.width + width];
                for (x, (&u, &v)) in u.iter().zip(v).enumerate() {
                    let (u, v) = (u as i32 - 128, v as i32 - 128);
                    let r = RV * v + (1 << 12);
                    let g = -GU * u - GV * v + (1 << 12);
                    let b = BU * u + (1 << 12);
                    for (row, luma) in [(&mut *top, l0), (&mut *bottom, l1)] {
                        for k in 0..2 {
                            let c = Y * (luma[2 * x + k] as i32 - 16);
                            let px = &mut row[(2 * x + k) * 4..(2 * x + k) * 4 + 4];
                            px[0] = ((c + r) >> 13).clamp(0, 255) as u8;
                            px[1] = ((c + g) >> 13).clamp(0, 255) as u8;
                            px[2] = ((c + b) >> 13).clamp(0, 255) as u8;
                            px[3] = 255;
                        }
                    }
                }
            });
    }

    /// RGB8 pixels (BT.601 limited range), `width`×`height`×3.
    pub fn to_rgb(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.width * self.height * 3);
        let cs = self.width / 2;
        for y in 0..self.height {
            for x in 0..self.width {
                let yy = self.planes[0][y * self.width + x] as f32 - 16.0;
                let u = self.planes[1][(y / 2) * cs + x / 2] as f32 - 128.0;
                let v = self.planes[2][(y / 2) * cs + x / 2] as f32 - 128.0;
                let c = 1.164 * yy;
                out.push((c + 1.596 * v).round().clamp(0.0, 255.0) as u8);
                out.push((c - 0.392 * u - 0.813 * v).round().clamp(0.0, 255.0) as u8);
                out.push((c + 2.017 * u).round().clamp(0.0, 255.0) as u8);
            }
        }
        out
    }

    /// Peak signal-to-noise ratio of the luma plane against `other`, in dB.
    pub fn psnr_y(&self, other: &Self) -> f64 {
        let n = self.planes[0].len().min(other.planes[0].len()).max(1);
        let se: f64 = self.planes[0]
            .iter()
            .zip(&other.planes[0])
            .map(|(&a, &b)| {
                let d = a as f64 - b as f64;
                d * d
            })
            .sum();
        if se == 0.0 {
            return f64::INFINITY;
        }
        10.0 * (255.0 * 255.0 / (se / n as f64)).log10()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_picture(w: usize, h: usize, t: usize) -> Picture {
        let mut p = Picture::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = 128.0
                    + 60.0 * ((x as f32 + t as f32 * 2.0) / 9.0).sin()
                    + 40.0 * (y as f32 / 13.0).cos();
                p.planes[0][y * w + x] = v.clamp(0.0, 255.0) as u8;
            }
        }
        for y in 0..h / 2 {
            for x in 0..w / 2 {
                p.planes[1][y * (w / 2) + x] = (100 + (x + t) % 50) as u8;
                p.planes[2][y * (w / 2) + x] = (150 - y % 40) as u8;
            }
        }
        p
    }

    #[test]
    fn encoder_output_decodes_to_its_reconstruction() {
        let (w, h) = (64, 48);
        let mut enc = Encoder::new(
            w,
            h,
            EncoderConfig {
                quantizer: 50,
                keyframe_interval: 4,
                scene_cut: 255,
            },
        );
        let mut dec = Decoder::new();
        let mut keys = 0;
        for t in 0..9 {
            let src = test_picture(w, h, t);
            let frame = enc.encode(&src);
            keys += frame.key as usize;
            let header = FrameHeader::parse(&frame.data).unwrap();
            assert_eq!(header.key, frame.key);
            let decoded = dec.decode(&frame.data).unwrap().clone();
            assert_eq!(&decoded, enc.reconstruction().unwrap(), "frame {t}");
            let psnr = decoded.psnr_y(&src);
            assert!(psnr > 30.0, "frame {t}: PSNR {psnr}");
        }
        assert_eq!(keys, 3);
    }

    #[test]
    fn flip_crop_pad() {
        let p = test_picture(32, 16, 0);
        assert_eq!(p.flipped().flipped(), p);
        assert_eq!(p.padded(48, 32).cropped(32, 16), p);
        assert_eq!(p.flipped().planes[0][0..32], p.planes[0][15 * 32..16 * 32]);
    }
}

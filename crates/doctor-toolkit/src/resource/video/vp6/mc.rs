//! Motion compensation of VP6 inter macroblocks: the 8×8 prediction of a
//! block from a reference picture at a quarter-pel (luma) or eighth-pel
//! (chroma) vector, with the stream's filters:
//!
//! * bilinear interpolation (always for chroma; for luma with filter mode 0
//!   or when the bicubic filter is switched off for the block);
//! * the 4-tap "bicubic" filter (`BLOCK_COPY_FILTER`), luma only: mode 1
//!   always, mode 2 unless the vector is long or the source block is flat;
//! * the loop filter across the 8-pixel grid inside the 12×12 source area
//!   (deblocking flag of inter frames in streams with a filter header).
//!
//! The source area is the 12×12 window around the block (2 pixels on the
//! left and top, 2 on the right and bottom); outside the picture its pixels
//! repeat the nearest edge pixel. All arithmetic follows the reference
//! decoder bit for bit.

use super::tables::BLOCK_COPY_FILTER;

/// A motion vector in quarter-pel (luma) units, as the bitstream codes it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mv {
    pub x: i16,
    pub y: i16,
}

/// Interpolation settings of the current frame.
#[derive(Debug, Clone, Copy)]
pub struct Filters {
    /// 0 bilinear, 1 bicubic, 2 bicubic unless the vector is long or the block flat.
    pub mode: u8,
    pub sample_variance_threshold: i32,
    pub max_vector_length: i32,
    /// Row of `BLOCK_COPY_FILTER` (0..=16).
    pub selection: usize,
    /// Loop filter limit, `Some` when deblocking is on.
    pub deblock: Option<i32>,
}

const WIN: usize = 12;

/// Writes the prediction of the 8×8 block at `(bx, by)` of a plane of
/// `width`×`height` into `dst` (at `dst_off`, row stride `dst_stride`).
#[allow(clippy::too_many_arguments)]
pub fn predict(
    dst: &mut [u8],
    dst_off: usize,
    dst_stride: usize,
    src: &[u8],
    width: usize,
    height: usize,
    bx: usize,
    by: usize,
    mv: Mv,
    luma: bool,
    filters: &Filters,
) {
    let div: i32 = if luma { 4 } else { 8 };
    let mask = div - 1;
    let (mvx, mvy) = (mv.x as i32, mv.y as i32);
    let dx = mvx / div;
    let dy = mvy / div;
    let x = bx as i64 + dx as i64 - 2;
    let y = by as i64 + dy as i64 - 2;
    let (w, h) = (width as i64, height as i64);

    let mut window = [0u8; WIN * WIN];
    // As the reference: a window touching the last column or row is copied too.
    let inside = x >= 0 && x + (WIN as i64) < w && y >= 0 && y + (WIN as i64) < h;
    // The 12×12 source area: (buffer, offset of its top-left pixel, stride).
    let (area, base, stride): (&[u8], usize, usize) = if inside && filters.deblock.is_none() {
        (src, y as usize * width + x as usize, width)
    } else {
        if inside {
            let start = y as usize * width + x as usize;
            for r in 0..WIN {
                let s = start + r * width;
                window[r * WIN..(r + 1) * WIN].copy_from_slice(&src[s..s + WIN]);
            }
        } else {
            for r in 0..WIN {
                let sy = (y + r as i64).clamp(0, h - 1) as usize * width;
                for c in 0..WIN {
                    let sx = (x + c as i64).clamp(0, w - 1) as usize;
                    window[r * WIN + c] = src[sy + sx];
                }
            }
        }
        if let Some(limit) = filters.deblock {
            deblock(&mut window, dx & 7, dy & 7, limit);
        }
        (&window[..], 0, WIN)
    };
    // The block itself starts at (2, 2) of the area.
    let at = |cx: usize, cy: usize| base + cy * stride + cx;

    let mut x8 = mvx & mask;
    let mut y8 = mvy & mask;
    if x8 == 0 && y8 == 0 {
        let s = at(2, 2);
        for r in 0..8 {
            let d = dst_off + r * dst_stride;
            dst[d..d + 8].copy_from_slice(&area[s + r * stride..s + r * stride + 8]);
        }
        return;
    }
    // Second source position, one pixel towards the vector.
    let ox: isize = if x8 != 0 {
        if mvx > 0 { 1 } else { -1 }
    } else {
        0
    };
    let oy: isize = if y8 != 0 {
        if mvy > 0 { 1 } else { -1 }
    } else {
        0
    };

    let mut filter4 = 0;
    if luma {
        x8 *= 2;
        y8 *= 2;
        filter4 = filters.mode;
        // Mode 2: bilinear for long vectors and flat blocks.
        let long = filters.max_vector_length != 0
            && (mvx.abs() > filters.max_vector_length || mvy.abs() > filters.max_vector_length);
        if filter4 == 2
            && (long
                || filters.sample_variance_threshold != 0
                    && block_variance(area, at(2, 2), stride) < filters.sample_variance_threshold)
        {
            filter4 = 0;
        }
    }
    // Start from the second position when it is above, or left on a row.
    let (mut cx, mut cy) = (2usize, 2usize);
    if (y8 != 0 && oy < 0) || (y8 == 0 && ox < 0) {
        cx = (2 + ox) as usize;
        cy = (2 + oy) as usize;
    }
    // Diagonal filters start one pixel left when the components differ in sign.
    let diag_x = if (mvx ^ mvy) < 0 { cx - 1 } else { cx };

    if filter4 != 0 {
        let taps = &BLOCK_COPY_FILTER[filters.selection.min(16)];
        if y8 == 0 {
            filter_hv4(
                dst,
                dst_off,
                dst_stride,
                area,
                at(cx, cy),
                stride,
                1,
                &taps[x8 as usize],
            );
        } else if x8 == 0 {
            filter_hv4(
                dst,
                dst_off,
                dst_stride,
                area,
                at(cx, cy),
                stride,
                stride,
                &taps[y8 as usize],
            );
        } else {
            filter_diag4(
                dst,
                dst_off,
                dst_stride,
                area,
                at(diag_x, cy),
                stride,
                &taps[x8 as usize],
                &taps[y8 as usize],
            );
        }
    } else if x8 == 0 || y8 == 0 {
        bilinear(
            dst,
            dst_off,
            dst_stride,
            area,
            at(cx, cy),
            stride,
            8,
            x8,
            y8,
        );
    } else {
        // Horizontal pass into 9 rows, then vertical.
        let mut tmp = [0u8; 8 * 9];
        bilinear(&mut tmp, 0, 8, area, at(diag_x, cy), stride, 9, x8, 0);
        bilinear(dst, dst_off, dst_stride, &tmp, 0, 8, 8, 0, y8);
    }
}

/// `h264_chroma_mc8` with rounding: `x`, `y` in eighths (one of them may be 0).
#[allow(clippy::too_many_arguments)]
fn bilinear(
    dst: &mut [u8],
    dst_off: usize,
    dst_stride: usize,
    src: &[u8],
    src_off: usize,
    stride: usize,
    rows: usize,
    x: i32,
    y: i32,
) {
    let a = ((8 - x) * (8 - y)) as u32;
    let b = (x * (8 - y)) as u32;
    let c = ((8 - x) * y) as u32;
    let d = (x * y) as u32;
    for r in 0..rows {
        let s0 = &src[src_off + r * stride..];
        let out = &mut dst[dst_off + r * dst_stride..dst_off + r * dst_stride + 8];
        if d != 0 {
            let s1 = &src[src_off + (r + 1) * stride..];
            for (i, o) in out.iter_mut().enumerate() {
                let v = a * s0[i] as u32
                    + b * s0[i + 1] as u32
                    + c * s1[i] as u32
                    + d * s1[i + 1] as u32;
                *o = ((v + 32) >> 6) as u8;
            }
        } else if b + c != 0 {
            let e = b + c;
            let step = if c != 0 { stride } else { 1 };
            let s1 = &src[src_off + r * stride + step..];
            for (i, o) in out.iter_mut().enumerate() {
                let v = a * s0[i] as u32 + e * s1[i] as u32;
                *o = ((v + 32) >> 6) as u8;
            }
        } else {
            out.copy_from_slice(&s0[..8]);
        }
    }
}

#[inline]
fn clip8(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// The 4-tap filter along one direction (`delta` 1: horizontal, stride: vertical).
#[allow(clippy::too_many_arguments)]
fn filter_hv4(
    dst: &mut [u8],
    dst_off: usize,
    dst_stride: usize,
    src: &[u8],
    src_off: usize,
    stride: usize,
    delta: usize,
    w: &[i16; 4],
) {
    let w = w.map(|v| v as i32);
    for r in 0..8 {
        let s = src_off + r * stride;
        let d = dst_off + r * dst_stride;
        for x in 0..8 {
            let p = s + x;
            let v = src[p - delta] as i32 * w[0]
                + src[p] as i32 * w[1]
                + src[p + delta] as i32 * w[2]
                + src[p + 2 * delta] as i32 * w[3];
            dst[d + x] = clip8((v + 64) >> 7);
        }
    }
}

/// The 4-tap filter horizontally into 11 rows, then vertically.
#[allow(clippy::too_many_arguments)]
fn filter_diag4(
    dst: &mut [u8],
    dst_off: usize,
    dst_stride: usize,
    src: &[u8],
    src_off: usize,
    stride: usize,
    hw: &[i16; 4],
    vw: &[i16; 4],
) {
    let hw = hw.map(|v| v as i32);
    let vw = vw.map(|v| v as i32);
    let mut tmp = [0i32; 8 * 11];
    for r in 0..11 {
        let s = src_off + r * stride - stride;
        for x in 0..8 {
            let p = s + x;
            let v = src[p - 1] as i32 * hw[0]
                + src[p] as i32 * hw[1]
                + src[p + 1] as i32 * hw[2]
                + src[p + 2] as i32 * hw[3];
            tmp[r * 8 + x] = clip8((v + 64) >> 7) as i32;
        }
    }
    for r in 0..8 {
        let d = dst_off + r * dst_stride;
        let t = (r + 1) * 8;
        for x in 0..8 {
            let v = tmp[t + x - 8] * vw[0]
                + tmp[t + x] * vw[1]
                + tmp[t + x + 8] * vw[2]
                + tmp[t + x + 16] * vw[3];
            dst[d + x] = clip8((v + 64) >> 7);
        }
    }
}

/// Variance estimate of an 8×8 block from every second pixel of every second row.
fn block_variance(src: &[u8], off: usize, stride: usize) -> i32 {
    let (mut sum, mut square) = (0i32, 0i32);
    for r in (0..8).step_by(2) {
        for x in (0..8).step_by(2) {
            let v = src[off + r * stride + x] as i32;
            sum += v;
            square += v * v;
        }
    }
    (16 * square - sum * sum) >> 8
}

/// The VP3 loop filter correction for a filter value and a limit.
fn bounding(value: i32, limit: i32) -> i32 {
    let a = value.abs();
    let v = if a < limit {
        a
    } else if a < 2 * limit {
        2 * limit - a
    } else {
        0
    };
    if value < 0 { -v } else { v }
}

/// Filters the edges of the 8-pixel grid crossing the 12×12 window: the
/// column `10 - dx` (if `dx` is not 0) and the row `10 - dy`.
fn deblock(win: &mut [u8; WIN * WIN], dx: i32, dy: i32, limit: i32) {
    if dx != 0 {
        let c = (10 - dx) as usize;
        for r in 0..WIN {
            let p = r * WIN + c;
            let f =
                (win[p - 2] as i32 - win[p + 1] as i32) + (win[p] as i32 - win[p - 1] as i32) * 3;
            let f = bounding((f + 4) >> 3, limit);
            win[p - 1] = clip8(win[p - 1] as i32 + f);
            win[p] = clip8(win[p] as i32 - f);
        }
    }
    if dy != 0 {
        let r = (10 - dy) as usize;
        for c in 0..WIN {
            let p = r * WIN + c;
            let f = (win[p - 2 * WIN] as i32 - win[p + WIN] as i32)
                + (win[p] as i32 - win[p - WIN] as i32) * 3;
            let f = bounding((f + 4) >> 3, limit);
            win[p - WIN] = clip8(win[p - WIN] as i32 + f);
            win[p] = clip8(win[p] as i32 - f);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plane(w: usize, h: usize) -> Vec<u8> {
        (0..w * h)
            .map(|i| ((i % w) * 7 + (i / w) * 13 + (i * 31) % 17) as u8)
            .collect()
    }

    fn off() -> Filters {
        Filters {
            mode: 0,
            sample_variance_threshold: 0,
            max_vector_length: 0,
            selection: 16,
            deblock: None,
        }
    }

    /// Bilinear prediction against a direct formula with clamped coordinates.
    #[test]
    fn bilinear_matches_formula_inside_and_at_edges() {
        let (w, h) = (48usize, 40usize);
        let src = plane(w, h);
        let px = |x: i64, y: i64| {
            src[y.clamp(0, h as i64 - 1) as usize * w + x.clamp(0, w as i64 - 1) as usize] as u32
        };
        for &(bx, by) in &[(16usize, 16usize), (0, 0), (40, 32)] {
            for mvx in -9i16..=9 {
                for mvy in -9i16..=9 {
                    let mut dst = vec![0u8; 64];
                    let mv = Mv {
                        x: mvx * 2,
                        y: mvy * 2,
                    };
                    predict(&mut dst, 0, 8, &src, w, h, bx, by, mv, false, &off());
                    // Chroma: eighth-pel, floor position.
                    let fx = (mv.x as i64).div_euclid(8);
                    let fy = (mv.y as i64).div_euclid(8);
                    let (ax, ay) = (
                        (mv.x as i64).rem_euclid(8) as u32,
                        (mv.y as i64).rem_euclid(8) as u32,
                    );
                    for r in 0..8i64 {
                        for c in 0..8i64 {
                            let x = bx as i64 + c + fx;
                            let y = by as i64 + r + fy;
                            let v = (8 - ax) * (8 - ay) * px(x, y)
                                + ax * (8 - ay) * px(x + 1, y)
                                + (8 - ax) * ay * px(x, y + 1)
                                + ax * ay * px(x + 1, y + 1);
                            let want = if ax != 0 && ay != 0 {
                                // Two passes, each rounded.
                                let hrow = |yy: i64| {
                                    ((8 - ax) * 8 * px(x, yy) + ax * 8 * px(x + 1, yy) + 32) >> 6
                                };
                                ((8 - ay) * 8 * hrow(y) + ay * 8 * hrow(y + 1) + 32) >> 6
                            } else {
                                (v + 32) >> 6
                            };
                            assert_eq!(
                                dst[(r * 8 + c) as usize] as u32,
                                want,
                                "block {bx},{by} mv {mvx},{mvy} at {c},{r}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// A flat picture stays flat through every filter, vector and edge.
    #[test]
    fn flat_stays_flat() {
        let (w, h) = (32usize, 32usize);
        let src = vec![77u8; w * h];
        for mode in 0..3 {
            for deblock in [None, Some(8)] {
                let f = Filters {
                    mode,
                    sample_variance_threshold: 0,
                    max_vector_length: 0,
                    selection: 3,
                    deblock,
                };
                for mvx in -40i16..40 {
                    let mut dst = vec![0u8; 64];
                    predict(
                        &mut dst,
                        0,
                        8,
                        &src,
                        w,
                        h,
                        8,
                        24,
                        Mv {
                            x: mvx,
                            y: -mvx * 3,
                        },
                        true,
                        &f,
                    );
                    assert!(dst.iter().all(|&v| v == 77), "mode {mode} mv {mvx}");
                }
            }
        }
    }

    #[test]
    fn whole_pel_vectors_copy() {
        let (w, h) = (64usize, 48usize);
        let src = plane(w, h);
        let mut dst = vec![0u8; 64];
        predict(
            &mut dst,
            0,
            8,
            &src,
            w,
            h,
            16,
            16,
            Mv { x: 12, y: -8 },
            true,
            &off(),
        );
        for r in 0..8 {
            assert_eq!(
                dst[r * 8..r * 8 + 8],
                src[(14 + r) * w + 19..(14 + r) * w + 27]
            );
        }
    }

    #[test]
    fn bounding_values() {
        let limit = 5;
        let got: Vec<i32> = (-12..=12).map(|v| bounding(v, limit)).collect();
        assert_eq!(
            got,
            [
                0, 0, 0, -1, -2, -3, -4, -5, -4, -3, -2, -1, 0, 1, 2, 3, 4, 5, 4, 3, 2, 1, 0, 0, 0
            ]
        );
    }
}

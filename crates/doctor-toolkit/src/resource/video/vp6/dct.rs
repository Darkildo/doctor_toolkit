//! VP3-family 8×8 transform: the bit-exact inverse used by VP6 decoders and a
//! floating-point forward transform scaled to match it.
//!
//! Coefficients are stored transposed: index `u * 8 + v` holds horizontal
//! frequency `u`, vertical frequency `v` (see [`super::tables::ZIGZAG`]).

const C1S7: i32 = 64277;
const C2S6: i32 = 60547;
const C3S5: i32 = 54491;
const C4S4: i32 = 46341;
const C5S3: i32 = 36410;
const C6S2: i32 = 25080;
const C7S1: i32 = 12785;

#[inline]
fn m(a: i32, b: i32) -> i32 {
    (a.wrapping_mul(b)) >> 16
}

#[inline]
fn clip(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// Writes (`add == false`, intra: +128) or adds the inverse transform of
/// `block` to the 8×8 area of `dst` at `offset` with `stride`.
pub fn idct(dst: &mut [u8], offset: usize, stride: usize, block: &[i16; 64], add: bool) {
    let mut ip = *block;
    for i in 0..8 {
        let c = |k: usize| ip[k * 8 + i] as i32;
        if (0..8).any(|k| c(k) != 0) {
            let a = m(C1S7, c(1)) + m(C7S1, c(7));
            let b = m(C7S1, c(1)) - m(C1S7, c(7));
            let cc = m(C3S5, c(3)) + m(C5S3, c(5));
            let d = m(C3S5, c(5)) - m(C5S3, c(3));
            let ad = m(C4S4, a - cc);
            let bd = m(C4S4, b - d);
            let cd = a + cc;
            let dd = b + d;
            let e = m(C4S4, c(0) + c(4));
            let f = m(C4S4, c(0) - c(4));
            let g = m(C2S6, c(2)) + m(C6S2, c(6));
            let h = m(C6S2, c(2)) - m(C2S6, c(6));
            let ed = e - g;
            let gd = e + g;
            let add_ = f + ad;
            let bdd = bd - h;
            let fd = f - ad;
            let hd = bd + h;
            let out = [
                gd + cd,
                add_ + hd,
                add_ - hd,
                ed + dd,
                ed - dd,
                fd + bdd,
                fd - bdd,
                gd - cd,
            ];
            for (k, v) in out.into_iter().enumerate() {
                ip[k * 8 + i] = v as i16;
            }
        }
    }
    for i in 0..8 {
        let r = |k: usize| ip[i * 8 + k] as i32;
        let col = offset + i;
        let mut put = |k: usize, v: i32| {
            let p = &mut dst[col + k * stride];
            *p = if add { clip(*p as i32 + v) } else { clip(v) };
        };
        if (1..8).any(|k| r(k) != 0) {
            let a = m(C1S7, r(1)) + m(C7S1, r(7));
            let b = m(C7S1, r(1)) - m(C1S7, r(7));
            let cc = m(C3S5, r(3)) + m(C5S3, r(5));
            let d = m(C3S5, r(5)) - m(C5S3, r(3));
            let ad = m(C4S4, a - cc);
            let bd = m(C4S4, b - d);
            let cd = a + cc;
            let dd = b + d;
            let mut e = m(C4S4, r(0) + r(4)) + 8;
            let mut f = m(C4S4, r(0) - r(4)) + 8;
            if !add {
                e += 16 * 128;
                f += 16 * 128;
            }
            let g = m(C2S6, r(2)) + m(C6S2, r(6));
            let h = m(C6S2, r(2)) - m(C2S6, r(6));
            let ed = e - g;
            let gd = e + g;
            let add_ = f + ad;
            let bdd = bd - h;
            let fd = f - ad;
            let hd = bd + h;
            let out = [
                gd + cd,
                add_ + hd,
                add_ - hd,
                ed + dd,
                ed - dd,
                fd + bdd,
                fd - bdd,
                gd - cd,
            ];
            for (k, v) in out.into_iter().enumerate() {
                put(k, v >> 4);
            }
        } else {
            let v = (C4S4.wrapping_mul(r(0)) + (8 << 16)) >> 20;
            if !add {
                for k in 0..8 {
                    put(k, 128 + v);
                }
            } else if r(0) != 0 {
                for k in 0..8 {
                    put(k, v);
                }
            }
        }
    }
}

/// The reduced transform decoders use for blocks whose coefficients end
/// early (selector 2–10): only the top-left 4×4 coefficients count. Equal
/// to [`idct`] when the other coefficients are zero; when they are not
/// (sub-version 6 streams), they are ignored, as by the reference decoder.
pub fn idct10(dst: &mut [u8], offset: usize, stride: usize, block: &[i16; 64], add: bool) {
    let mut ip = *block;
    for i in 0..4 {
        let c = |k: usize| ip[k * 8 + i] as i32;
        if (0..4).any(|k| c(k) != 0) {
            let a = m(C1S7, c(1));
            let b = m(C7S1, c(1));
            let cc = m(C3S5, c(3));
            let d = -m(C5S3, c(3));
            let ad = m(C4S4, a - cc);
            let bd = m(C4S4, b - d);
            let cd = a + cc;
            let dd = b + d;
            let e = m(C4S4, c(0));
            let f = e;
            let g = m(C2S6, c(2));
            let h = m(C6S2, c(2));
            let ed = e - g;
            let gd = e + g;
            let add_ = f + ad;
            let bdd = bd - h;
            let fd = f - ad;
            let hd = bd + h;
            let out = [
                gd + cd,
                add_ + hd,
                add_ - hd,
                ed + dd,
                ed - dd,
                fd + bdd,
                fd - bdd,
                gd - cd,
            ];
            for (k, v) in out.into_iter().enumerate() {
                ip[k * 8 + i] = v as i16;
            }
        }
    }
    for i in 0..8 {
        let r = |k: usize| ip[i * 8 + k] as i32;
        let col = offset + i;
        let mut put = |k: usize, v: i32| {
            let p = &mut dst[col + k * stride];
            *p = if add { clip(*p as i32 + v) } else { clip(v) };
        };
        if (0..4).any(|k| r(k) != 0) {
            let a = m(C1S7, r(1));
            let b = m(C7S1, r(1));
            let cc = m(C3S5, r(3));
            let d = -m(C5S3, r(3));
            let ad = m(C4S4, a - cc);
            let bd = m(C4S4, b - d);
            let cd = a + cc;
            let dd = b + d;
            let mut e = m(C4S4, r(0));
            if !add {
                e += 16 * 128;
            }
            let f = e;
            let g = m(C2S6, r(2));
            let h = m(C6S2, r(2));
            let ed = e - g + 8;
            let gd = e + g + 8;
            let add_ = f + ad + 8;
            let bdd = bd - h;
            let fd = f - ad + 8;
            let hd = bd + h;
            let out = [
                gd + cd,
                add_ + hd,
                add_ - hd,
                ed + dd,
                ed - dd,
                fd + bdd,
                fd - bdd,
                gd - cd,
            ];
            for (k, v) in out.into_iter().enumerate() {
                put(k, v >> 4);
            }
        } else if !add {
            for k in 0..8 {
                put(k, 128);
            }
        }
    }
}

/// DC-only add used by decoders for blocks whose selector is 1 or less.
pub fn idct_dc_add(dst: &mut [u8], offset: usize, stride: usize, block: &[i16; 64]) {
    let dc = (block[0] as i32 + 15) >> 5;
    for y in 0..8 {
        for x in 0..8 {
            let p = &mut dst[offset + y * stride + x];
            *p = clip(*p as i32 + dc);
        }
    }
}

/// Forward transform of an 8×8 block of samples (row-major) into coefficients
/// in the decoder's transposed layout, scaled so that [`idct`] inverts it.
pub fn fdct(src: &[i32; 64]) -> [f32; 64] {
    static BASIS: std::sync::OnceLock<[[f32; 8]; 8]> = std::sync::OnceLock::new();
    let basis = BASIS.get_or_init(|| {
        let mut b = [[0f32; 8]; 8];
        for (k, row) in b.iter_mut().enumerate() {
            let c = if k == 0 { (0.5f64).sqrt() } else { 1.0 };
            for (n, v) in row.iter_mut().enumerate() {
                *v = (c * ((2 * n + 1) as f64 * k as f64 * std::f64::consts::PI / 16.0).cos())
                    as f32;
            }
        }
        b
    });
    // Orthonormal DCT is (1/2)·basis per dimension; VP3 coefficients are 4× that.
    let mut tmp = [0f32; 64];
    for y in 0..8 {
        for u in 0..8 {
            let mut s = 0f32;
            for x in 0..8 {
                s += src[y * 8 + x] as f32 * basis[u][x];
            }
            tmp[y * 8 + u] = s;
        }
    }
    let mut out = [0f32; 64];
    for u in 0..8 {
        for v in 0..8 {
            let mut s = 0f32;
            for y in 0..8 {
                s += tmp[y * 8 + u] * basis[v][y];
            }
            out[u * 8 + v] = s;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_then_inverse_is_close() {
        let mut src = [0i32; 64];
        for (i, v) in src.iter_mut().enumerate() {
            *v = ((i * 37 + (i / 8) * 11) % 200) as i32 - 100;
        }
        let f = fdct(&src);
        let mut block = [0i16; 64];
        for i in 0..64 {
            block[i] = f[i].round() as i16;
        }
        let mut dst = [0u8; 64];
        idct(&mut dst, 0, 8, &block, false);
        for i in 0..64 {
            let want = src[i] + 128;
            assert!(
                (dst[i] as i32 - want).abs() <= 1,
                "{i}: {} vs {want}",
                dst[i]
            );
        }
        // DC only: 32 per unit of mean.
        let mut dc = [0i16; 64];
        dc[0] = 32 * 10;
        idct(&mut dst, 0, 8, &dc, false);
        assert!(dst.iter().all(|&p| p == 138));
    }
}

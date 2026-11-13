//! Encoder of animation clips: bone tracks into the channel streams of
//! `bnk` clips (schemes 1, 3 and 4; codecs in `resource::anim`).
//!
//! Every channel gets the cheapest encoding whose error at every frame stays
//! within the tolerance: zero / one (0xC / 0xD), a constant, a polynomial of
//! degree 1–3 over the whole clip, quantised values (u8 / u16), or segments
//! of these (codec 5), as the game's own clips do; f32 per frame (codec 4) is
//! the last resort. Rotations are made continuous (`q` and `-q` are the same
//! rotation; the sign that keeps neighbouring frames close is chosen), the
//! sign of `w` goes into the fourth channel (1 for a negative `w` in scheme 4,
//! for a positive one in scheme 1; one bit per frame when it
//! changes). After encoding, every bone is decoded again with the game's
//! formulas; a bone whose angle error exceeds the budget is encoded again
//! with tighter tolerances.

use serde::Serialize;

use crate::resource::anim::{BoneTrack, stream};
use crate::{Error, Result};

/// Encoder settings.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct EncodeOptions {
    /// Compression scheme: 4 (current clips), 1 (piecewise streams without a
    /// length) or 3 (the fourth channel stores `w`).
    pub scheme: u32,
    /// Largest angle between a decoded and a source rotation, in radians.
    pub max_angle: f32,
    /// Largest difference of a translation component, in model units (metres).
    pub max_translation: f32,
}

impl Default for EncodeOptions {
    fn default() -> Self {
        Self {
            scheme: 4,
            max_angle: 0.002,
            max_translation: 0.0005,
        }
    }
}

/// Encoded channel streams of a clip and how close they are to the source.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct EncodedClip {
    pub streams: Vec<u8>,
    /// Format word of the clip record: `0x80000000 | scheme`.
    pub format: u32,
    pub frames: u32,
    /// Largest angle between decoded and source rotations, radians.
    pub max_angle: f32,
    /// Largest difference of a translation component.
    pub max_translation: f32,
    /// Number of top-level streams and segments per codec.
    pub codecs: [usize; 16],
}

/// The upper 24 bits of `v`, rounded to nearest.
fn f24_bits(v: f32) -> u32 {
    let bits = v.to_bits();
    if !v.is_finite() {
        return bits & 0xFFFF_FF00;
    }
    let rounded = bits.wrapping_add(0x80) & 0xFFFF_FF00;
    // Rounding must not turn a finite value into infinity.
    if f32::from_bits(rounded).is_finite() {
        rounded
    } else {
        bits & 0xFFFF_FF00
    }
}

fn f24(v: f32) -> f32 {
    f32::from_bits(f24_bits(v))
}

fn put_f24(out: &mut Vec<u8>, v: f32) {
    let b = f24_bits(v).to_le_bytes();
    out.extend_from_slice(&b[1..4]);
}

fn put_header(out: &mut Vec<u8>, codec: u8, n: usize) {
    debug_assert!(n < 4096);
    let h = (codec as u16) | ((n as u16) << 4);
    out.extend_from_slice(&h.to_le_bytes());
}

/// One encoded piece: a simple stream of `n` frames.
#[derive(Debug, Clone)]
struct Piece {
    codec: u8,
    n: usize,
    bytes: Vec<u8>,
}

/// Evaluates a polynomial (coefficients highest first) like the game: f32
/// Horner steps.
fn horner(c: &[f32], t: f32) -> f32 {
    let mut v = c[0];
    for &k in &c[1..] {
        v = v * t + k;
    }
    v
}

/// Least-squares polynomial of `degree` through `v` at t = 0, 1, …;
/// coefficients highest first, rounded to f24.
fn fit_poly(v: &[f32], degree: usize) -> Vec<f32> {
    let n = v.len();
    let d = degree.min(n.saturating_sub(1));
    // Fit in s = t / scale for conditioning, then convert.
    let scale = (n.max(2) - 1) as f64;
    let m = d + 1;
    let mut a = vec![[0f64; 5]; m];
    for (i, &y) in v.iter().enumerate() {
        let s = i as f64 / scale;
        let mut p = [1f64; 4];
        for k in 1..m {
            p[k] = p[k - 1] * s;
        }
        for r in 0..m {
            for c in 0..m {
                a[r][c] += p[r] * p[c];
            }
            a[r][m] += p[r] * y as f64;
        }
    }
    // Gaussian elimination with partial pivoting.
    for col in 0..m {
        let piv = (col..m)
            .max_by(|&x, &y| a[x][col].abs().total_cmp(&a[y][col].abs()))
            .unwrap_or(col);
        a.swap(col, piv);
        if a[col][col].abs() < 1e-300 {
            continue;
        }
        for r in 0..m {
            if r != col {
                let f = a[r][col] / a[col][col];
                let pivot = a[col];
                for (x, p) in a[r].iter_mut().zip(pivot).take(m + 1).skip(col) {
                    *x -= f * p;
                }
            }
        }
    }
    let b: Vec<f64> = (0..m)
        .map(|k| {
            if a[k][k].abs() < 1e-300 {
                0.0
            } else {
                a[k][m] / a[k][k]
            }
        })
        .collect();
    // b in powers of s (ascending) → powers of t, highest first.
    let mut out = vec![0f32; degree + 1];
    for (k, &bk) in b.iter().enumerate() {
        out[degree - k] = f24((bk / scale.powi(k as i32)) as f32);
    }
    // Re-fit the constant term after rounding the others: the mean residual.
    let mean: f64 = v
        .iter()
        .enumerate()
        .map(|(i, &y)| (y - horner(&out, i as f32) + out[degree]) as f64)
        .sum::<f64>()
        / n as f64;
    out[degree] = f24(mean as f32);
    out
}

fn max_err(v: &[f32], f: impl Fn(usize) -> f32) -> f32 {
    v.iter()
        .enumerate()
        .map(|(i, &y)| (f(i) - y).abs())
        .fold(0.0, f32::max)
}

/// The cheapest simple encoding of `v` within `tol`, if any is cheaper than
/// `budget` bytes. f32 per frame always fits.
fn simple_piece(v: &[f32], tol: f32, allow_quantised: bool) -> Piece {
    let n = v.len();
    let hn = if n < 4096 { n } else { 1 };
    // Zero, one, constant.
    if v.iter().all(|&x| x.abs() <= tol) {
        return Piece {
            codec: 0xC,
            n: hn,
            bytes: Vec::new(),
        };
    }
    if v.iter().all(|&x| (x - 1.0).abs() <= tol) {
        return Piece {
            codec: 0xD,
            n: hn,
            bytes: Vec::new(),
        };
    }
    let mut candidates: Vec<Piece> = Vec::new();
    for degree in 0..=3usize.min(n.saturating_sub(1)) {
        let c = fit_poly(v, degree);
        if max_err(v, |i| horner(&c, i as f32)) <= tol {
            let mut bytes = Vec::new();
            for &k in &c {
                put_f24(&mut bytes, k);
            }
            candidates.push(Piece {
                codec: degree as u8,
                n: if degree == 0 { hn } else { n },
                bytes,
            });
            break;
        }
    }
    if n < 4096 && allow_quantised {
        let lo = v.iter().copied().fold(f32::INFINITY, f32::min);
        let hi = v.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        for (codec, levels) in [(6u8, 255.0f32), (7, 65535.0)] {
            let offset = f24(lo);
            let mut scale = f24((hi - offset) / levels);
            if scale <= 0.0 {
                scale = f24(f32::MIN_POSITIVE.max(tol));
            }
            let q: Vec<u32> = v
                .iter()
                .map(|&x| ((x - offset) / scale).round().clamp(0.0, levels) as u32)
                .collect();
            if max_err(v, |i| q[i] as f32 * scale + offset) <= tol {
                let mut bytes = Vec::new();
                put_f24(&mut bytes, offset);
                put_f24(&mut bytes, scale);
                for &x in &q {
                    if codec == 6 {
                        bytes.push(x as u8);
                    } else {
                        bytes.extend_from_slice(&(x as u16).to_le_bytes());
                    }
                }
                candidates.push(Piece { codec, n, bytes });
                break;
            }
        }
    }
    if let Some(p) = candidates.into_iter().min_by_key(|p| p.bytes.len()) {
        return p;
    }
    let mut bytes = Vec::with_capacity(4 * n);
    for &x in v {
        bytes.extend_from_slice(&x.to_le_bytes());
    }
    Piece { codec: 4, n, bytes }
}

/// Size of a piece with its header.
fn piece_size(p: &Piece) -> usize {
    2 + p.bytes.len()
}

/// Whether a polynomial of degree ≤ 3 fits `v`.
fn poly_fits(v: &[f32], tol: f32) -> bool {
    let c = fit_poly(v, 3.min(v.len().saturating_sub(1)));
    max_err(v, |i| horner(&c, i as f32)) <= tol
}

/// Splits `v` into segments that share their end frames.
fn segments(v: &[f32], tol: f32) -> Vec<Piece> {
    let n = v.len();
    let mut out = Vec::new();
    let mut s = 0;
    while s < n - 1 {
        let max_len = (n - s).min(4095);
        // Longest polynomial segment: grow exponentially, then bisect.
        let mut good = 2.min(max_len);
        let mut len = good;
        while len < max_len {
            let next = (len * 2).min(max_len);
            if poly_fits(&v[s..s + next], tol) {
                good = next;
                len = next;
            } else {
                let (mut lo, mut hi) = (good, next);
                while hi - lo > 1 {
                    let mid = (lo + hi) / 2;
                    if poly_fits(&v[s..s + mid], tol) {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                good = lo;
                break;
            }
        }
        // A short polynomial segment of noisy data: quantise a longer run.
        let mut piece = simple_piece(&v[s..s + good], tol, true);
        let poly_rate = piece_size(&piece) as f32 / (good - 1).max(1) as f32;
        if good < 8 {
            let run = max_len.min(64);
            let q = simple_piece(&v[s..s + run], tol, true);
            if (piece_size(&q) as f32 / (run - 1).max(1) as f32) < poly_rate {
                piece = q;
            }
        }
        s += piece.n.max(2) - 1;
        out.push(piece);
    }
    out
}

/// Encodes one channel into `out`.
fn encode_channel(v: &[f32], tol: f32, layout: u32, out: &mut Vec<u8>, codecs: &mut [usize; 16]) {
    let whole = simple_piece(v, tol, true);
    let n = v.len();
    let mut best_pieces = None;
    if n > 2 && !matches!(whole.codec, 0 | 0xC | 0xD) {
        let pieces = segments(v, tol);
        let size: usize =
            pieces.iter().map(piece_size).sum::<usize>() + if layout == 4 { 4 } else { 2 };
        if size < piece_size(&whole) || n >= 4096 {
            best_pieces = Some(pieces);
        }
    }
    match best_pieces {
        None => {
            codecs[whole.codec as usize] += 1;
            put_header(out, whole.codec, whole.n);
            out.extend_from_slice(&whole.bytes);
        }
        Some(pieces) => {
            codecs[5] += 1;
            let start = out.len();
            put_header(out, 5, pieces.len());
            if layout == 4 {
                out.extend_from_slice(&[0, 0]);
            }
            for p in &pieces {
                codecs[p.codec as usize] += 1;
                put_header(out, p.codec, p.n);
                out.extend_from_slice(&p.bytes);
            }
            if layout == 4 {
                let len = (out.len() - start).min(0xFFFF) as u16;
                out[start + 2..start + 4].copy_from_slice(&len.to_le_bytes());
            }
        }
    }
}

/// Encodes the sign of `w` (0 positive, 1 negative) for every frame.
fn encode_signs(neg: &[bool], layout: u32, out: &mut Vec<u8>, codecs: &mut [usize; 16]) {
    let n = neg.len();
    if n < 4096 && neg.iter().any(|&x| x) && !neg.iter().all(|&x| x) {
        codecs[0xE] += 1;
        put_header(out, 0xE, n);
        let mut bits = vec![0u8; n.div_ceil(8)];
        for (i, _) in neg.iter().enumerate().filter(|(_, x)| **x) {
            bits[i / 8] |= 1 << (i % 8);
        }
        out.extend_from_slice(&bits);
        return;
    }
    // Long clips: the bits of each segment.
    let v: Vec<f32> = neg.iter().map(|&x| x as u8 as f32).collect();
    if n >= 4096 {
        let mut pieces = Vec::new();
        let mut s = 0;
        while s < n - 1 {
            let e = (s + 4094).min(n - 1);
            let seg = &neg[s..=e];
            let mut bits = vec![0u8; seg.len().div_ceil(8)];
            for (i, _) in seg.iter().enumerate().filter(|(_, x)| **x) {
                bits[i / 8] |= 1 << (i % 8);
            }
            pieces.push(Piece {
                codec: 0xE,
                n: seg.len(),
                bytes: bits,
            });
            s = e;
        }
        codecs[5] += 1;
        let start = out.len();
        put_header(out, 5, pieces.len());
        if layout == 4 {
            out.extend_from_slice(&[0, 0]);
        }
        for p in &pieces {
            codecs[0xE] += 1;
            put_header(out, p.codec, p.n);
            out.extend_from_slice(&p.bytes);
        }
        if layout == 4 {
            let len = (out.len() - start).min(0xFFFF) as u16;
            out[start + 2..start + 4].copy_from_slice(&len.to_le_bytes());
        }
        return;
    }
    let p = simple_piece(&v, 0.0, false);
    codecs[p.codec as usize] += 1;
    put_header(out, p.codec, p.n);
    out.extend_from_slice(&p.bytes);
}

/// Rotations of a track made unit length and continuous: each frame takes
/// the sign closest to the previous one; the first has `w ≥ 0`.
fn continuous(rotations: &[[f32; 4]]) -> Vec<[f32; 4]> {
    let mut out: Vec<[f32; 4]> = Vec::with_capacity(rotations.len());
    for q in rotations {
        let n = q.iter().map(|v| v * v).sum::<f32>().sqrt();
        let mut q = if n > 1e-6 {
            q.map(|v| v / n)
        } else {
            [0.0, 0.0, 0.0, 1.0]
        };
        let flip = match out.last() {
            Some(p) => p.iter().zip(&q).map(|(a, b)| a * b).sum::<f32>() < 0.0,
            None => q[3] < 0.0,
        };
        if flip {
            q = q.map(|v| -v);
        }
        out.push(q);
    }
    out
}

/// Angle between the rotations of two quaternions (normalised first),
/// accurate for small angles: `2·atan2(|v|, |w|)` of `a⁻¹·b`.
pub fn quat_angle(a: [f32; 4], b: [f32; 4]) -> f32 {
    let a = a.map(f64::from);
    let b = b.map(f64::from);
    // conj(a) · b; atan2 does not depend on the lengths.
    let w = a[3] * b[3] + a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let x = a[3] * b[0] - a[0] * b[3] - a[1] * b[2] + a[2] * b[1];
    let y = a[3] * b[1] + a[0] * b[2] - a[1] * b[3] - a[2] * b[0];
    let z = a[3] * b[2] - a[0] * b[1] + a[1] * b[0] - a[2] * b[3];
    let v = (x * x + y * y + z * z).sqrt();
    (2.0 * v.atan2(w.abs())) as f32
}

/// Decodes the seven streams of one bone at `data` with the game's formulas.
fn decode_bone(data: &[u8], layout: u32, stored_w: bool, frames: usize) -> Result<Vec<[f32; 7]>> {
    let mut at = 0;
    let mut channels = Vec::with_capacity(7);
    for _ in 0..7 {
        let s = stream(data, at, layout, 0)?;
        at += s.len;
        channels.push(s);
    }
    (0..frames)
        .map(|f| {
            let mut v = [0f32; 7];
            for (c, s) in channels.iter().enumerate() {
                v[c] = s.eval(data, f as f32)?;
            }
            let (x, y, z) = (v[0], v[1], v[2]);
            let q = if stored_w {
                let n = (x * x + y * y + z * z + v[3] * v[3]).sqrt().max(1e-6);
                [x / n, y / n, z / n, v[3] / n]
            } else {
                let w = (1.0 - x * x - y * y - z * z).max(0.0).sqrt();
                // Scheme 1 (layout 1) stores 1 for a positive `w`, scheme 4 for a
                // negative one (as the decoder in `resource::anim`).
                let negative = (v[3] != 0.0) != (layout == 1);
                [x, y, z, if negative { -w } else { w }]
            };
            Ok([q[0], q[1], q[2], q[3], v[4], v[5], v[6]])
        })
        .collect()
}

/// Encodes the tracks of a clip: one track per bone of the bone set, in the
/// order of the set, each with `frames` rotations and translations (shorter
/// tracks repeat their last frame).
pub fn encode_clip(
    tracks: &[BoneTrack],
    frames: usize,
    options: &EncodeOptions,
) -> Result<EncodedClip> {
    if !matches!(options.scheme, 1 | 3 | 4) {
        return Err(Error::InvalidArgument(format!(
            "unknown compression scheme {}",
            options.scheme
        )));
    }
    if frames == 0 || frames > 100_000 {
        return Err(Error::InvalidArgument(format!("{frames} frames")));
    }
    let layout = if options.scheme == 1 { 1 } else { 4 };
    let stored_w = options.scheme == 3;
    let mut result = EncodedClip {
        format: 0x8000_0000 | options.scheme,
        frames: frames as u32,
        ..Default::default()
    };
    for track in tracks {
        let at = |v: &[[f32; 4]], f: usize| {
            v.get(f)
                .or(v.last())
                .copied()
                .unwrap_or([0.0, 0.0, 0.0, 1.0])
        };
        let raw: Vec<[f32; 4]> = (0..frames).map(|f| at(&track.rotations, f)).collect();
        let rotations = continuous(&raw);
        let translations: Vec<[f32; 3]> = (0..frames)
            .map(|f| {
                track
                    .translations
                    .get(f)
                    .or(track.translations.last())
                    .copied()
                    .unwrap_or_default()
            })
            .collect();
        if rotations
            .iter()
            .flatten()
            .chain(translations.iter().flatten())
            .any(|v| !v.is_finite())
        {
            return Err(Error::InvalidArgument(format!(
                "bone {}: values are not finite",
                track.bone
            )));
        }
        let mut rot_tol = options.max_angle / 4.0;
        let tra_tol = options.max_translation;
        let mut attempt = 0;
        loop {
            let mut bone = Vec::new();
            let mut codecs = [0usize; 16];
            for c in 0..3 {
                let v: Vec<f32> = rotations.iter().map(|q| q[c]).collect();
                encode_channel(&v, rot_tol, layout, &mut bone, &mut codecs);
            }
            if stored_w {
                let v: Vec<f32> = rotations.iter().map(|q| q[3]).collect();
                encode_channel(&v, rot_tol, layout, &mut bone, &mut codecs);
            } else {
                // The bit is 1 for a negative `w` in scheme 4, for a positive one in
                // scheme 1.
                let neg: Vec<bool> = rotations
                    .iter()
                    .map(|q| (q[3] < 0.0) != (layout == 1))
                    .collect();
                encode_signs(&neg, layout, &mut bone, &mut codecs);
            }
            for c in 0..3 {
                let v: Vec<f32> = translations.iter().map(|t| t[c]).collect();
                encode_channel(&v, tra_tol, layout, &mut bone, &mut codecs);
            }
            let decoded = decode_bone(&bone, layout, stored_w, frames)?;
            let angle = decoded
                .iter()
                .zip(&rotations)
                .map(|(d, q)| quat_angle([d[0], d[1], d[2], d[3]], *q))
                .fold(0.0, f32::max);
            let shift = decoded
                .iter()
                .zip(&translations)
                .flat_map(|(d, t)| (0..3).map(move |c| (d[4 + c] - t[c]).abs()))
                .fold(0.0, f32::max);
            // Near w = 0 small errors of x, y, z become large errors of w:
            // tighten the rotation channels until the angle fits.
            if angle > options.max_angle && attempt < 7 {
                // The last try stores x, y, z as they are.
                rot_tol = if attempt == 6 { 0.0 } else { rot_tol / 4.0 };

                attempt += 1;
                continue;
            }
            result.max_angle = result.max_angle.max(angle);
            result.max_translation = result.max_translation.max(shift);
            for (a, b) in result.codecs.iter_mut().zip(codecs) {
                *a += b;
            }
            result.streams.extend_from_slice(&bone);
            break;
        }
    }
    Ok(result)
}

/// How a game clip survives decoding and encoding again.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Reencoded {
    pub frames: u32,
    pub bones: usize,
    /// Scheme of the game's clip.
    pub scheme: u32,
    /// Bytes of the game's streams and of the encoder's.
    pub original_size: usize,
    pub encoded_size: usize,
    /// Largest angle between the game's and the re-encoded rotations (radians)
    /// and largest translation difference, both decoded with the game's formulas.
    pub max_angle: f32,
    pub max_translation: f32,
    pub codecs: [usize; 16],
}

/// Decodes `clip` of the bank `data`, encodes its tracks again and decodes
/// the result: the error against the game's own decoded values.
pub fn reencode(
    clip: &crate::resource::anim::Clip,
    data: &[u8],
    bones: &[usize],
    options: &EncodeOptions,
) -> Result<Reencoded> {
    let tracks = clip.decode(data, bones)?;
    let frames = clip.frames.clamp(1, 100_000) as usize;
    let encoded = encode_clip(&tracks, frames, options)?;
    let mut copy = clip.clone();
    copy.streams_at = 0;
    copy.scheme = options.scheme;
    copy.format = encoded.format;
    let back = copy.decode(&encoded.streams, bones)?;
    let mut out = Reencoded {
        frames: clip.frames,
        bones: bones.len(),
        scheme: clip.scheme,
        encoded_size: encoded.streams.len(),
        codecs: encoded.codecs,
        ..Default::default()
    };
    for (a, b) in tracks.iter().zip(&back) {
        let src = continuous(&a.rotations);
        for (q, r) in src.iter().zip(&b.rotations) {
            out.max_angle = out.max_angle.max(quat_angle(*q, *r));
        }
        for (s, t) in a.translations.iter().zip(&b.translations) {
            for c in 0..3 {
                out.max_translation = out.max_translation.max((s[c] - t[c]).abs());
            }
        }
    }
    out.original_size = crate::resource::anim::streams_len(clip, data, bones.len())?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(frames: usize, f: impl Fn(f32) -> ([f32; 4], [f32; 3])) -> BoneTrack {
        let (rotations, translations) = (0..frames).map(|i| f(i as f32)).unzip();
        BoneTrack {
            bone: 0,
            rotations,
            translations,
        }
    }

    fn axis_angle(axis: [f32; 3], a: f32) -> [f32; 4] {
        let s = (a / 2.0).sin();
        [axis[0] * s, axis[1] * s, axis[2] * s, (a / 2.0).cos()]
    }

    #[test]
    fn f24_rounds_to_nearest() {
        assert_eq!(f24(1.0), 1.0);
        let v = 0.123_456_79f32;
        assert!((f24(v) - v).abs() <= v * 2f32.powi(-16));
        assert!(f24(f32::MAX).is_finite());
    }

    #[test]
    fn encodes_within_budget_and_compresses() {
        let options = EncodeOptions::default();
        let tracks = vec![
            // Still bone: constants.
            track(60, |_| ([0.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0])),
            // A full turn: w changes sign; smooth translation.
            track(60, |t| {
                (
                    axis_angle([0.0, 0.0, 1.0], t * std::f32::consts::TAU / 59.0),
                    [t * 0.01, (t * 0.2).sin(), 0.5],
                )
            }),
            // Noise.
            track(60, |t| {
                let n = ((t * 12.9898).sin() * 43758.547).fract() - 0.5;
                (axis_angle([1.0, 0.0, 0.0], n), [n, -n, n * 0.5])
            }),
        ];
        for scheme in [1, 3, 4] {
            let options = EncodeOptions { scheme, ..options };
            let clip = encode_clip(&tracks, 60, &options).unwrap();
            assert!(clip.max_angle <= options.max_angle, "{scheme}: {clip:?}");
            assert!(clip.max_translation <= options.max_translation);
            // The game's decoder agrees.
            let c = crate::resource::anim::Clip {
                offset: 0,
                hash: 0,
                name: None,
                frames: 60,
                bone_set: 0,
                scheme,
                format: clip.format,
                streams_at: 0,
                old_layout: false,
                events: Vec::new(),
            };
            let decoded = c.decode(&clip.streams, &[0, 1, 2]).unwrap();
            for (d, t) in decoded.iter().zip(&tracks) {
                let src = continuous(&t.rotations);
                for (a, b) in d.rotations.iter().zip(&src) {
                    assert!(quat_angle(*a, *b) <= options.max_angle * 1.01);
                }
            }
            // Smaller than f32 per frame.
            assert!(
                clip.streams.len() < 3 * 7 * (2 + 4 * 60) / 3,
                "{}",
                clip.streams.len()
            );
        }
        // The still bone is seven tiny streams.
        let still = encode_clip(&tracks[..1], 60, &options).unwrap();
        assert!(still.streams.len() <= 7 * 5, "{}", still.streams.len());
    }

    #[test]
    fn long_clips_use_segments() {
        let tracks = vec![track(5000, |t| {
            (
                axis_angle([0.0, 1.0, 0.0], (t * 0.01).sin()),
                [t * 0.001, 0.0, 0.0],
            )
        })];
        let clip = encode_clip(&tracks, 5000, &EncodeOptions::default()).unwrap();
        assert!(clip.codecs[5] > 0);
        let c = crate::resource::anim::Clip {
            offset: 0,
            hash: 0,
            name: None,
            frames: 5000,
            bone_set: 0,
            scheme: 4,
            format: clip.format,
            streams_at: 0,
            old_layout: false,
            events: Vec::new(),
        };
        assert_eq!(
            c.decode(&clip.streams, &[0]).unwrap()[0].rotations.len(),
            5000
        );
    }
}

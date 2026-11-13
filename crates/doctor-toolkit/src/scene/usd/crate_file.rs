//! USD crate files (`.usdc`, binary): reader (versions 0.4–0.10) and writer
//! (version 0.8.0) of the [`Layer`] model.
//!
//! ```text
//! bootstrap  "PXR-USDC", u8 version[8] (major, minor, patch), i64 TOC offset, 64 reserved bytes
//! TOC        u64 count, {char name[16], i64 start, i64 size}
//! TOKENS     u64 count, u64 raw size, u64 packed size, LZ4: NUL-terminated strings
//! STRINGS    u64 count, u32 token index
//! FIELDS     u64 count, ints(token index), u64 packed size, LZ4: u64 value reps
//! FIELDSETS  u64 count, ints(field index, ~0 ends a set)
//! PATHS      u64 count, u64 count, ints(path index), ints(element token, < 0: property),
//!            ints(jump: -1 child only, 0 sibling only, -2 leaf, > 0 both, sibling at +jump)
//! SPECS      u64 count, ints(path), ints(field set), ints(spec type)
//! ints(n)    u64 packed size, LZ4 of integer coding: common delta, 2-bit codes
//!            (common, 8/16/32-bit; 16/32/64 for 64-bit ints), the deltas
//! LZ4        u8 chunk count (0: one block), LZ4 block(s)
//! value rep  u64: bit 63 array, 62 inlined, 61 compressed, bits 48–55 type, 0–47 payload
//! ```
//!
//! Arrays are a count (u32 before 0.7.0) and elements; compressed integer
//! arrays use the integer coding, compressed float arrays an `i` (integers)
//! or `t` (lookup table) code. Time samples are a jump to the times rep, a
//! jump to a count and the value reps.

use std::collections::HashMap;

use super::{Attribute, Layer, Prim, Property, Specifier, Value, bad};
use crate::Result;

pub const MAGIC: &[u8] = b"PXR-USDC";
/// Version written.
pub const VERSION: [u8; 3] = [0, 8, 0];

const MIN_COMPRESSED_ARRAY: usize = 16;
const MAX_ELEMENTS: usize = 1 << 28;
const MAX_DEPTH: usize = 64;

// Value types.
const BOOL: u8 = 1;
const UCHAR: u8 = 2;
const INT: u8 = 3;
const UINT: u8 = 4;
const INT64: u8 = 5;
const UINT64: u8 = 6;
const HALF: u8 = 7;
const FLOAT: u8 = 8;
const DOUBLE: u8 = 9;
const STRING: u8 = 10;
const TOKEN: u8 = 11;
const ASSET: u8 = 12;
const MATRIX2D: u8 = 13;
const MATRIX3D: u8 = 14;
const MATRIX4D: u8 = 15;
const QUATD: u8 = 16;
const QUATF: u8 = 17;
const QUATH: u8 = 18;
const VEC2D: u8 = 19;
const VEC2F: u8 = 20;
const VEC2H: u8 = 21;
const VEC2I: u8 = 22;
const VEC3D: u8 = 23;
const VEC3F: u8 = 24;
const VEC3H: u8 = 25;
const VEC3I: u8 = 26;
const VEC4D: u8 = 27;
const VEC4F: u8 = 28;
const VEC4H: u8 = 29;
const VEC4I: u8 = 30;
const DICTIONARY: u8 = 31;
const TOKEN_LIST_OP: u8 = 32;
const STRING_LIST_OP: u8 = 33;
const PATH_LIST_OP: u8 = 34;
const PATH_VECTOR: u8 = 40;
const TOKEN_VECTOR: u8 = 41;
const SPECIFIER: u8 = 42;
const PERMISSION: u8 = 43;
const VARIABILITY: u8 = 44;
const TIME_SAMPLES: u8 = 46;
const DOUBLE_VECTOR: u8 = 48;
const STRING_VECTOR: u8 = 50;
const VALUE_BLOCK: u8 = 51;
const TIME_CODE: u8 = 56;

// Spec types.
const SPEC_ATTRIBUTE: u32 = 1;
const SPEC_PRIM: u32 = 6;
const SPEC_PSEUDO_ROOT: u32 = 7;
const SPEC_RELATIONSHIP: u32 = 8;

/// Scalar component kinds.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Scalar {
    U8,
    I32,
    U32,
    I64,
    U64,
    F16,
    F32,
    F64,
}

impl Scalar {
    fn size(self) -> usize {
        match self {
            Self::U8 => 1,
            Self::F16 => 2,
            Self::I32 | Self::U32 | Self::F32 => 4,
            Self::I64 | Self::U64 | Self::F64 => 8,
        }
    }
}

/// Component kind and count of a numeric value type.
fn numeric(ty: u8) -> Option<(Scalar, usize)> {
    use Scalar::*;
    Some(match ty {
        BOOL | UCHAR => (U8, 1),
        INT => (I32, 1),
        UINT => (U32, 1),
        INT64 => (I64, 1),
        UINT64 => (U64, 1),
        HALF => (F16, 1),
        FLOAT => (F32, 1),
        DOUBLE | TIME_CODE => (F64, 1),
        MATRIX2D => (F64, 4),
        MATRIX3D => (F64, 9),
        MATRIX4D => (F64, 16),
        QUATD => (F64, 4),
        QUATF => (F32, 4),
        QUATH => (F16, 4),
        VEC2D => (F64, 2),
        VEC2F => (F32, 2),
        VEC2H => (F16, 2),
        VEC2I => (I32, 2),
        VEC3D => (F64, 3),
        VEC3F => (F32, 3),
        VEC3H => (F16, 3),
        VEC3I => (I32, 3),
        VEC4D => (F64, 4),
        VEC4F => (F32, 4),
        VEC4H => (F16, 4),
        VEC4I => (I32, 4),
        _ => return None,
    })
}

pub(crate) fn half_to_f32(h: u16) -> f32 {
    let sign = (h >> 15) as u32;
    let exp = ((h >> 10) & 0x1F) as u32;
    let frac = (h & 0x3FF) as u32;
    let bits = if exp == 0 {
        if frac == 0 {
            sign << 31
        } else {
            // Subnormal.
            let mut e = 127 - 15 + 1;
            let mut f = frac;
            while f & 0x400 == 0 {
                f <<= 1;
                e -= 1;
            }
            (sign << 31) | (e << 23) | ((f & 0x3FF) << 13)
        }
    } else if exp == 31 {
        (sign << 31) | (0xFF << 23) | (frac << 13)
    } else {
        (sign << 31) | ((exp + 127 - 15) << 23) | (frac << 13)
    };
    f32::from_bits(bits)
}

pub(crate) fn f32_to_half(f: f32) -> u16 {
    let bits = f.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xFF) as i32;
    let frac = bits & 0x7F_FFFF;
    if exp == 0xFF {
        return sign | 0x7C00 | if frac != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 31 {
        return sign | 0x7C00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = frac | 0x80_0000;
        let shift = (14 - e) as u32;
        let half = (m >> shift) as u16;
        let round = (m >> (shift - 1)) & 1;
        return sign | (half + round as u16);
    }
    let half = sign | ((e as u16) << 10) | ((frac >> 13) as u16);
    // Round to nearest.
    if frac & 0x1000 != 0 { half + 1 } else { half }
}

// ------------------------------------------------------------ compression

/// LZ4 data as written by `TfFastCompression`.
fn decompress(src: &[u8], size: usize) -> Result<Vec<u8>> {
    if size > 1 << 31 {
        return Err(bad("compressed block too large"));
    }
    // `size` may be an upper bound; LZ4 packs at most 255:1.
    let size = size.min(src.len().saturating_mul(255).saturating_add(64));
    let (&chunks, rest) = src
        .split_first()
        .ok_or_else(|| bad("empty compressed block"))?;
    let mut out = vec![0u8; size];
    if chunks == 0 {
        let n = lz4_flex::decompress_into(rest, &mut out).map_err(|e| bad(format!("LZ4: {e}")))?;
        out.truncate(n);
        return Ok(out);
    }
    let mut at = 0;
    let mut written = 0;
    for _ in 0..chunks {
        let len = rest
            .get(at..at + 4)
            .map(|b| i32::from_le_bytes(b.try_into().unwrap()))
            .ok_or_else(|| bad("truncated compressed chunk"))?;
        let len = usize::try_from(len).map_err(|_| bad("bad compressed chunk"))?;
        let chunk = rest
            .get(at + 4..at + 4 + len)
            .ok_or_else(|| bad("truncated compressed chunk"))?;
        let n = lz4_flex::decompress_into(chunk, &mut out[written..])
            .map_err(|e| bad(format!("LZ4: {e}")))?;
        written += n;
        at += 4 + len;
    }
    out.truncate(written);
    Ok(out)
}

fn compress(src: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8];
    out.extend(lz4_flex::compress(src));
    out
}

/// Decodes `n` integers of the integer coding (32- or 64-bit).
fn decode_ints(buf: &[u8], n: usize, wide: bool) -> Result<Vec<i64>> {
    let common_size = if wide { 8 } else { 4 };
    let codes_size = (n * 2).div_ceil(8);
    let common = match buf.get(..common_size) {
        Some(b) if wide => i64::from_le_bytes(b.try_into().unwrap()),
        Some(b) => i32::from_le_bytes(b.try_into().unwrap()) as i64,
        None => return Err(bad("truncated integers")),
    };
    let codes = buf
        .get(common_size..common_size + codes_size)
        .ok_or_else(|| bad("truncated integer codes"))?;
    let mut at = common_size + codes_size;
    let mut out = Vec::with_capacity(n);
    let mut prev = 0i64;
    for i in 0..n {
        let code = (codes[i / 4] >> (2 * (i % 4))) & 3;
        let size = match (code, wide) {
            (0, _) => 0,
            (1, false) => 1,
            (2, false) | (1, true) => 2,
            (3, false) | (2, true) => 4,
            _ => 8,
        };
        let delta = if size == 0 {
            common
        } else {
            let b = buf
                .get(at..at + size)
                .ok_or_else(|| bad("truncated integer data"))?;
            at += size;
            match size {
                1 => b[0] as i8 as i64,
                2 => i16::from_le_bytes(b.try_into().unwrap()) as i64,
                4 => i32::from_le_bytes(b.try_into().unwrap()) as i64,
                _ => i64::from_le_bytes(b.try_into().unwrap()),
            }
        };
        prev = if wide {
            prev.wrapping_add(delta)
        } else {
            (prev as i32).wrapping_add(delta as i32) as i64
        };
        out.push(prev);
    }
    Ok(out)
}

/// Integer coding of 32-bit values.
fn encode_ints(values: &[i64], wide: bool) -> Vec<u8> {
    let mut counts: HashMap<i64, usize> = HashMap::new();
    let mut deltas = Vec::with_capacity(values.len());
    let mut prev = 0i64;
    let mut common = 0i64;
    let mut best = 0usize;
    for &v in values {
        let d = if wide {
            v.wrapping_sub(prev)
        } else {
            (v as i32).wrapping_sub(prev as i32) as i64
        };
        prev = v;
        deltas.push(d);
        let c = counts.entry(d).or_insert(0);
        *c += 1;
        if *c > best || (*c == best && d > common) {
            best = *c;
            common = d;
        }
    }
    let mut out = Vec::new();
    if wide {
        out.extend_from_slice(&common.to_le_bytes());
    } else {
        out.extend_from_slice(&(common as i32).to_le_bytes());
    }
    let codes_at = out.len();
    out.resize(codes_at + (values.len() * 2).div_ceil(8), 0);
    for (i, &d) in deltas.iter().enumerate() {
        let code: u8 = if d == common {
            0
        } else if !wide && i8::try_from(d).is_ok() || wide && i16::try_from(d).is_ok() {
            if wide {
                out.extend_from_slice(&(d as i16).to_le_bytes());
            } else {
                out.push(d as i8 as u8);
            }
            1
        } else if !wide && i16::try_from(d).is_ok() || wide && i32::try_from(d).is_ok() {
            if wide {
                out.extend_from_slice(&(d as i32).to_le_bytes());
            } else {
                out.extend_from_slice(&(d as i16).to_le_bytes());
            }
            2
        } else {
            if wide {
                out.extend_from_slice(&d.to_le_bytes());
            } else {
                out.extend_from_slice(&(d as i32).to_le_bytes());
            }
            3
        };
        out[codes_at + i / 4] |= code << (2 * (i % 4));
    }
    out
}

// ----------------------------------------------------------------- reader

struct Reader<'a> {
    data: &'a [u8],
    version: (u8, u8, u8),
    tokens: Vec<String>,
    strings: Vec<u32>,
    paths: Vec<Option<String>>,
}

impl<'a> Reader<'a> {
    fn slice(&self, at: usize, len: usize) -> Result<&'a [u8]> {
        at.checked_add(len)
            .and_then(|end| self.data.get(at..end))
            .ok_or_else(|| bad(format!("truncated at {at:#x}")))
    }

    fn u64(&self, at: usize) -> Result<u64> {
        Ok(u64::from_le_bytes(self.slice(at, 8)?.try_into().unwrap()))
    }

    fn u32(&self, at: usize) -> Result<u32> {
        Ok(u32::from_le_bytes(self.slice(at, 4)?.try_into().unwrap()))
    }

    fn count(&self, at: usize) -> Result<usize> {
        let n = self.u64(at)?;
        usize::try_from(n)
            .ok()
            .filter(|&n| n <= MAX_ELEMENTS)
            .ok_or_else(|| bad(format!("count {n} too large")))
    }

    /// Compressed integers at `at`: returns them and the end.
    fn ints(&self, at: usize, n: usize, wide: bool) -> Result<(Vec<i64>, usize)> {
        if n > MAX_ELEMENTS {
            return Err(bad("too many integers"));
        }
        let size = self.count(at)?;
        let packed = self.slice(at + 8, size)?;
        let raw_size =
            if wide { 8 } else { 4 } + (n * 2).div_ceil(8) + n * if wide { 8 } else { 4 };
        let raw = decompress(packed, raw_size)?;
        Ok((decode_ints(&raw, n, wide)?, at + 8 + size))
    }

    fn token(&self, index: u64) -> Result<String> {
        self.tokens
            .get(index as usize)
            .cloned()
            .ok_or_else(|| bad(format!("token {index} out of range")))
    }

    fn string(&self, index: u64) -> Result<String> {
        let t = *self
            .strings
            .get(index as usize)
            .ok_or_else(|| bad(format!("string {index} out of range")))?;
        self.token(t as u64)
    }

    fn path(&self, index: u64) -> Result<String> {
        self.paths
            .get(index as usize)
            .cloned()
            .flatten()
            .ok_or_else(|| bad(format!("path {index} out of range")))
    }

    fn scalars(&self, at: usize, kind: Scalar, n: usize) -> Result<Vec<f64>> {
        let bytes = self.slice(
            at,
            n.checked_mul(kind.size())
                .ok_or_else(|| bad("array too large"))?,
        )?;
        Ok(match kind {
            Scalar::U8 => bytes.iter().map(|&b| b as f64).collect(),
            Scalar::I32 => bytes
                .chunks_exact(4)
                .map(|c| i32::from_le_bytes(c.try_into().unwrap()) as f64)
                .collect(),
            Scalar::U32 => bytes
                .chunks_exact(4)
                .map(|c| u32::from_le_bytes(c.try_into().unwrap()) as f64)
                .collect(),
            Scalar::I64 => bytes
                .chunks_exact(8)
                .map(|c| i64::from_le_bytes(c.try_into().unwrap()) as f64)
                .collect(),
            Scalar::U64 => bytes
                .chunks_exact(8)
                .map(|c| u64::from_le_bytes(c.try_into().unwrap()) as f64)
                .collect(),
            Scalar::F16 => bytes
                .chunks_exact(2)
                .map(|c| half_to_f32(u16::from_le_bytes(c.try_into().unwrap())) as f64)
                .collect(),
            Scalar::F32 => bytes
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes(c.try_into().unwrap()) as f64)
                .collect(),
            Scalar::F64 => bytes
                .chunks_exact(8)
                .map(|c| f64::from_le_bytes(c.try_into().unwrap()))
                .collect(),
        })
    }

    fn array_count(&self, at: usize) -> Result<(usize, usize)> {
        if self.version < (0, 7, 0) {
            Ok((self.u32(at)? as usize, at + 4))
        } else {
            Ok((self.count(at)?, at + 8))
        }
    }

    fn list_op(&self, at: usize, item: impl Fn(u64) -> Result<String>) -> Result<Vec<String>> {
        let header = *self.slice(at, 1)?.first().unwrap();
        let mut pos = at + 1;
        let mut out = Vec::new();
        // explicit, added, prepended, appended, deleted, ordered
        for bit in [1u8 << 1, 1 << 2, 1 << 5, 1 << 6, 1 << 3, 1 << 4] {
            if header & bit == 0 {
                continue;
            }
            let n = self.count(pos)?;
            pos += 8;
            for k in 0..n {
                let index = self.u32(pos + 4 * k)? as u64;
                // Deleted items are not part of the value.
                if bit != 1 << 3 {
                    out.push(item(index)?);
                }
            }
            pos += 4 * n;
        }
        Ok(out)
    }

    fn value(&self, rep: u64, depth: usize) -> Result<Value> {
        if depth > MAX_DEPTH {
            return Err(bad("values nested too deeply"));
        }
        let array = rep >> 63 & 1 == 1;
        let inlined = rep >> 62 & 1 == 1;
        let compressed = rep >> 61 & 1 == 1;
        let ty = (rep >> 48 & 0xFF) as u8;
        let payload = rep & ((1 << 48) - 1);
        if ty == VALUE_BLOCK {
            return Ok(Value::Blocked);
        }
        if inlined {
            return Ok(match ty {
                BOOL => Value::Bool(payload != 0),
                UCHAR => Value::Int(payload as u8 as i64),
                INT => Value::Int(payload as u32 as i32 as i64),
                UINT => Value::Int(payload as u32 as i64),
                INT64 | UINT64 => Value::Int(payload as i64),
                HALF => Value::Float(half_to_f32(payload as u16) as f64),
                FLOAT | DOUBLE | TIME_CODE => Value::Float(f32::from_bits(payload as u32) as f64),
                TOKEN => Value::Token(self.token(payload)?),
                STRING => Value::String(self.string(payload)?),
                ASSET => Value::Asset(self.token(payload)?),
                SPECIFIER | VARIABILITY | PERMISSION => Value::Int(payload as i64),
                MATRIX2D | MATRIX3D | MATRIX4D => {
                    let n = match ty {
                        MATRIX2D => 2,
                        MATRIX3D => 3,
                        _ => 4,
                    };
                    let mut m = vec![0.0; n * n];
                    for k in 0..n {
                        m[k * n + k] = (payload >> (8 * k)) as u8 as i8 as f64;
                    }
                    Value::Tuple(m)
                }
                DICTIONARY => Value::Dictionary(Vec::new()),
                _ => match numeric(ty) {
                    Some((_, n)) => Value::Tuple(
                        (0..n)
                            .map(|k| (payload >> (8 * k)) as u8 as i8 as f64)
                            .collect(),
                    ),
                    None => Value::Blocked,
                },
            });
        }
        let at = usize::try_from(payload).map_err(|_| bad("bad offset"))?;
        if array {
            if at == 0 {
                return Ok(match ty {
                    TOKEN | STRING | ASSET => Value::Strings(Vec::new()),
                    _ => Value::Array {
                        values: Vec::new(),
                        width: numeric(ty).map_or(1, |n| n.1),
                    },
                });
            }
            let (n, pos) = self.array_count(at)?;
            return match ty {
                TOKEN | ASSET => Ok(Value::Strings(
                    (0..n)
                        .map(|k| self.u32(pos + 4 * k).and_then(|t| self.token(t as u64)))
                        .collect::<Result<_>>()?,
                )),
                STRING => Ok(Value::Strings(
                    (0..n)
                        .map(|k| self.u32(pos + 4 * k).and_then(|t| self.string(t as u64)))
                        .collect::<Result<_>>()?,
                )),
                _ => {
                    let (kind, width) =
                        numeric(ty).ok_or_else(|| bad(format!("array of type {ty}")))?;
                    let total = n.checked_mul(width).ok_or_else(|| bad("array too large"))?;
                    let values = if compressed && n >= MIN_COMPRESSED_ARRAY {
                        match kind {
                            Scalar::I32 | Scalar::U32 | Scalar::I64 | Scalar::U64 => {
                                let wide = matches!(kind, Scalar::I64 | Scalar::U64);
                                self.ints(pos, total, wide)?
                                    .0
                                    .into_iter()
                                    .map(|v| {
                                        if kind == Scalar::U32 {
                                            v as u32 as f64
                                        } else {
                                            v as f64
                                        }
                                    })
                                    .collect()
                            }
                            Scalar::F16 | Scalar::F32 | Scalar::F64 => {
                                let code = *self.slice(pos, 1)?.first().unwrap();
                                match code {
                                    b'i' => self
                                        .ints(pos + 1, total, false)?
                                        .0
                                        .into_iter()
                                        .map(|v| v as f64)
                                        .collect(),
                                    b't' => {
                                        let lut_size = self.u32(pos + 1)? as usize;
                                        if lut_size > MAX_ELEMENTS {
                                            return Err(bad("lookup table too large"));
                                        }
                                        let lut = self.scalars(pos + 5, kind, lut_size)?;
                                        let indices = self
                                            .ints(pos + 5 + lut_size * kind.size(), total, false)?
                                            .0;
                                        indices
                                            .into_iter()
                                            .map(|i| {
                                                lut.get(i as u32 as usize)
                                                    .copied()
                                                    .ok_or_else(|| bad("lookup index out of range"))
                                            })
                                            .collect::<Result<_>>()?
                                    }
                                    other => {
                                        return Err(bad(format!(
                                            "unknown array compression {:?}",
                                            other as char
                                        )));
                                    }
                                }
                            }
                            Scalar::U8 => self.scalars(pos, kind, total)?,
                        }
                    } else {
                        self.scalars(pos, kind, total)?
                    };
                    Ok(Value::Array { values, width })
                }
            };
        }
        Ok(match ty {
            TOKEN_VECTOR | PATH_VECTOR | STRING_VECTOR => {
                let n = self.count(at)?;
                let items = (0..n)
                    .map(|k| {
                        let i = self.u32(at + 8 + 4 * k)? as u64;
                        match ty {
                            TOKEN_VECTOR => self.token(i),
                            PATH_VECTOR => self.path(i),
                            _ => self.string(i),
                        }
                    })
                    .collect::<Result<Vec<_>>>()?;
                if ty == PATH_VECTOR {
                    Value::Paths(items)
                } else {
                    Value::Strings(items)
                }
            }
            DOUBLE_VECTOR => {
                let n = self.count(at)?;
                Value::Array {
                    values: self.scalars(at + 8, Scalar::F64, n)?,
                    width: 1,
                }
            }
            TOKEN_LIST_OP => Value::Strings(self.list_op(at, |i| self.token(i))?),
            STRING_LIST_OP => Value::Strings(self.list_op(at, |i| self.string(i))?),
            PATH_LIST_OP => Value::Paths(self.list_op(at, |i| self.path(i))?),
            DICTIONARY => {
                let n = self.count(at)?;
                let mut pos = at + 8;
                let mut entries = Vec::new();
                for _ in 0..n {
                    let key = self.string(self.u32(pos)? as u64)?;
                    let jump = i64::from_le_bytes(self.slice(pos + 4, 8)?.try_into().unwrap());
                    let rep_at = (pos as i64 + 4)
                        .checked_add(jump)
                        .and_then(|p| usize::try_from(p).ok())
                        .ok_or_else(|| bad("bad dictionary offset"))?;
                    let rep = self.u64(rep_at)?;
                    entries.push((key, self.value(rep, depth + 1)?));
                    pos = rep_at + 8;
                }
                Value::Dictionary(entries)
            }
            _ => match numeric(ty) {
                Some((kind, n)) => {
                    let v = self.scalars(at, kind, n)?;
                    match ty {
                        INT | UINT | INT64 | UINT64 | UCHAR => Value::Int(v[0] as i64),
                        BOOL => Value::Bool(v[0] != 0.0),
                        HALF | FLOAT | DOUBLE | TIME_CODE => Value::Float(v[0]),
                        _ => Value::Tuple(v),
                    }
                }
                None => Value::Blocked,
            },
        })
    }

    fn time_samples(&self, rep: u64) -> Result<Vec<(f64, Value)>> {
        let at = usize::try_from(rep & ((1 << 48) - 1)).map_err(|_| bad("bad offset"))?;
        let jump = |pos: usize| -> Result<usize> {
            let j = i64::from_le_bytes(self.slice(pos, 8)?.try_into().unwrap());
            (pos as i64)
                .checked_add(j)
                .and_then(|p| usize::try_from(p).ok())
                .ok_or_else(|| bad("bad time samples offset"))
        };
        let times_at = jump(at)?;
        let times = self.value(self.u64(times_at)?, 1)?;
        let values_at = jump(times_at + 8)?;
        let n = self.count(values_at)?;
        self.slice(values_at + 8, n * 8)?;
        let times = match times {
            Value::Array { values, .. } => values,
            Value::Float(f) => vec![f],
            _ => Vec::new(),
        };
        if times.len() != n {
            return Err(bad("time samples: times and values differ"));
        }
        let mut out = Vec::with_capacity(n);
        for (k, t) in times.into_iter().enumerate() {
            let rep = self.u64(values_at + 8 + 8 * k)?;
            out.push((t, self.value(rep, 1)?));
        }
        Ok(out)
    }
}

/// Reads a crate file.
pub fn read(data: &[u8]) -> Result<Layer> {
    if !data.starts_with(MAGIC) || data.len() < 88 {
        return Err(bad("not a crate file"));
    }
    let version = (data[8], data[9], data[10]);
    if version < (0, 4, 0) || version.0 > 0 {
        return Err(bad(format!(
            "crate version {}.{}.{} is not supported",
            version.0, version.1, version.2
        )));
    }
    let mut r = Reader {
        data,
        version,
        tokens: Vec::new(),
        strings: Vec::new(),
        paths: Vec::new(),
    };
    let toc = usize::try_from(r.u64(16)?).map_err(|_| bad("bad TOC offset"))?;
    let sections = r.count(toc)?;
    let mut table: HashMap<String, usize> = HashMap::new();
    for k in 0..sections.min(64) {
        let at = toc + 8 + k * 32;
        let name = r.slice(at, 16)?;
        let end = name.iter().position(|&b| b == 0).unwrap_or(16);
        let start = usize::try_from(r.u64(at + 16)?).map_err(|_| bad("bad section"))?;
        let size = usize::try_from(r.u64(at + 24)?).map_err(|_| bad("bad section"))?;
        r.slice(start, size)?;
        table.insert(String::from_utf8_lossy(&name[..end]).into_owned(), start);
    }
    let section = |name: &str| {
        table
            .get(name)
            .copied()
            .ok_or_else(|| bad(format!("no {name} section")))
    };
    // Tokens.
    let at = section("TOKENS")?;
    let n = r.count(at)?;
    let raw_size = r.count(at + 8)?;
    let packed_size = r.count(at + 16)?;
    let raw = decompress(r.slice(at + 24, packed_size)?, raw_size)?;
    r.tokens = raw
        .split(|&b| b == 0)
        .take(n)
        .map(|t| String::from_utf8_lossy(t).into_owned())
        .collect();
    if r.tokens.len() < n {
        return Err(bad("fewer tokens than counted"));
    }
    // Strings.
    let at = section("STRINGS")?;
    let n = r.count(at)?;
    r.strings = (0..n)
        .map(|k| r.u32(at + 8 + 4 * k))
        .collect::<Result<_>>()?;
    // Fields.
    let at = section("FIELDS")?;
    let n = r.count(at)?;
    let (names, end) = r.ints(at + 8, n, false)?;
    let reps_size = r.count(end)?;
    let reps_raw = decompress(r.slice(end + 8, reps_size)?, n * 8)?;
    if reps_raw.len() < n * 8 {
        return Err(bad("truncated field values"));
    }
    let fields: Vec<(u32, u64)> = names
        .iter()
        .zip(reps_raw.chunks_exact(8))
        .map(|(&t, r)| (t as u32, u64::from_le_bytes(r.try_into().unwrap())))
        .collect();
    // Field sets.
    let at = section("FIELDSETS")?;
    let n = r.count(at)?;
    let (fieldsets, _) = r.ints(at + 8, n, false)?;
    // Paths.
    let at = section("PATHS")?;
    let n = r.count(at)?;
    let encoded = r.count(at + 8)?;
    let (path_indexes, end) = r.ints(at + 16, encoded, false)?;
    let (elements, end) = r.ints(end, encoded, false)?;
    let (jumps, _) = r.ints(end, encoded, false)?;
    r.paths = vec![None; n.min(encoded)];
    build_paths(&mut r, &path_indexes, &elements, &jumps)?;
    // Specs.
    let at = section("SPECS")?;
    let n = r.count(at)?;
    let (spec_paths, end) = r.ints(at + 8, n, false)?;
    let (spec_sets, end) = r.ints(end, n, false)?;
    let (spec_types, _) = r.ints(end, n, false)?;

    // Fields of every spec, by path.
    let mut specs: Specs = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for k in 0..n {
        let path = r.path(spec_paths[k] as u64)?;
        let mut list = Vec::new();
        let mut i = usize::try_from(spec_sets[k]).map_err(|_| bad("bad field set"))?;
        while let Some(&f) = fieldsets.get(i) {
            if f as u32 == u32::MAX {
                break;
            }
            let (name, rep) = *fields
                .get(f as usize)
                .ok_or_else(|| bad("field out of range"))?;
            list.push((r.token(name as u64)?, rep));
            i += 1;
        }
        order.push(path.clone());
        specs.insert(path, (spec_types[k] as u32, list));
    }
    let mut layer = Layer::default();
    if let Some((_, fields)) = specs.get("/") {
        for (name, rep) in fields {
            if name == "primChildren" {
                continue;
            }
            layer.metadata.push((name.clone(), r.value(*rep, 0)?));
        }
    }
    let children = |path: &str| -> Result<Vec<String>> {
        let listed = specs.get(path).and_then(|(_, f)| {
            f.iter()
                .find(|(n, _)| n == "primChildren")
                .map(|(_, rep)| *rep)
        });
        Ok(match listed {
            Some(rep) => r
                .value(rep, 0)?
                .strings()
                .map(<[String]>::to_vec)
                .unwrap_or_default(),
            None => order
                .iter()
                .filter(|p| specs.get(*p).is_some_and(|s| s.0 == SPEC_PRIM) && parent_of(p) == path)
                .map(|p| p.rsplit('/').next().unwrap_or("").to_owned())
                .collect(),
        })
    };
    layer.prims = build_prims(&r, &specs, &order, "/", &children, 0)?;
    Ok(layer)
}

/// Spec type and fields (name, value rep) by path.
type Specs = HashMap<String, (u32, Vec<(String, u64)>)>;

fn parent_of(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) => "/",
        Some(i) => &path[..i],
        None => "",
    }
}

fn build_prims(
    r: &Reader<'_>,
    specs: &Specs,
    order: &[String],
    parent: &str,
    children: &dyn Fn(&str) -> Result<Vec<String>>,
    depth: usize,
) -> Result<Vec<Prim>> {
    if depth > 256 {
        return Err(bad("prims nested too deeply"));
    }
    let mut out = Vec::new();
    for name in children(parent)? {
        let path = if parent == "/" {
            format!("/{name}")
        } else {
            format!("{parent}/{name}")
        };
        let Some((kind, fields)) = specs.get(&path) else {
            continue;
        };
        if *kind != SPEC_PRIM {
            continue;
        }
        let mut prim = Prim {
            name: name.clone(),
            ..Default::default()
        };
        let mut property_names: Option<Vec<String>> = None;
        for (field, rep) in fields {
            match field.as_str() {
                "specifier" => {
                    prim.specifier = match r.value(*rep, 0)? {
                        Value::Int(1) => Specifier::Over,
                        Value::Int(2) => Specifier::Class,
                        _ => Specifier::Def,
                    }
                }
                "typeName" => {
                    prim.type_name = r.value(*rep, 0)?.as_str().unwrap_or("").to_owned();
                }
                "primChildren" => {}
                "properties" => {
                    property_names = r.value(*rep, 0)?.strings().map(<[String]>::to_vec);
                }
                _ => prim.metadata.push((field.clone(), r.value(*rep, 0)?)),
            }
        }
        let property_names = property_names.unwrap_or_else(|| {
            order
                .iter()
                .filter_map(|p| {
                    p.strip_prefix(&path)
                        .and_then(|rest| rest.strip_prefix('.'))
                })
                .filter(|p| !p.contains(['[', '.']))
                .map(str::to_owned)
                .collect()
        });
        for property in property_names {
            let ppath = format!("{path}.{property}");
            let Some((kind, fields)) = specs.get(&ppath) else {
                continue;
            };
            match *kind {
                SPEC_ATTRIBUTE => {
                    let mut a = Attribute::default();
                    for (field, rep) in fields {
                        match field.as_str() {
                            "typeName" => {
                                a.type_name = r.value(*rep, 0)?.as_str().unwrap_or("").to_owned();
                            }
                            "variability" => a.uniform = r.value(*rep, 0)? == Value::Int(1),
                            "custom" => a.custom = r.value(*rep, 0)? == Value::Bool(true),
                            "default" => a.default = Some(r.value(*rep, 0)?),
                            "timeSamples" => a.samples = r.time_samples(*rep)?,
                            "connectionPaths" => {
                                if let Value::Paths(p) = r.value(*rep, 0)? {
                                    a.connections = p;
                                }
                            }
                            "connectionChildren" | "targetChildren" => {}
                            _ => a.metadata.push((field.clone(), r.value(*rep, 0)?)),
                        }
                    }
                    a.samples.sort_by(|x, y| x.0.total_cmp(&y.0));
                    prim.properties.push((property, Property::Attribute(a)));
                }
                SPEC_RELATIONSHIP => {
                    let mut targets = Vec::new();
                    for (field, rep) in fields {
                        if field == "targetPaths"
                            && let Value::Paths(p) = r.value(*rep, 0)?
                        {
                            targets = p;
                        }
                    }
                    prim.properties
                        .push((property, Property::Relationship(targets)));
                }
                _ => {}
            }
        }
        prim.children = build_prims(r, specs, order, &path, children, depth + 1)?;
        out.push(prim);
    }
    Ok(out)
}

fn build_paths(r: &mut Reader<'_>, indexes: &[i64], elements: &[i64], jumps: &[i64]) -> Result<()> {
    let n = indexes.len();
    if elements.len() < n || jumps.len() < n {
        return Err(bad("paths: lists differ"));
    }
    // (start index, parent path) work items instead of recursion.
    let mut work: Vec<(usize, Option<String>)> = vec![(0, None)];
    let mut steps = 0usize;
    while let Some((mut i, mut parent)) = work.pop() {
        loop {
            steps += 1;
            if i >= n || steps > 4 * n + 16 {
                return Err(bad("paths: bad tree"));
            }
            let slot = usize::try_from(indexes[i]).map_err(|_| bad("bad path index"))?;
            let path = match &parent {
                None => "/".to_owned(),
                Some(p) => {
                    let token = r.token(elements[i].unsigned_abs())?;
                    if elements[i] < 0 {
                        format!("{p}.{token}")
                    } else if p == "/" {
                        format!("/{token}")
                    } else {
                        format!("{p}/{token}")
                    }
                }
            };
            *r.paths
                .get_mut(slot)
                .ok_or_else(|| bad("path index out of range"))? = Some(path.clone());
            let jump = jumps[i];
            let has_child = jump > 0 || jump == -1;
            let has_sibling = jump >= 0;
            if has_child {
                if has_sibling {
                    let sibling = i
                        .checked_add(usize::try_from(jump).map_err(|_| bad("bad jump"))?)
                        .ok_or_else(|| bad("bad jump"))?;
                    work.push((sibling, parent.clone()));
                }
                parent = Some(path);
                i += 1;
            } else if has_sibling {
                i += 1;
            } else {
                break;
            }
        }
    }
    Ok(())
}

// ----------------------------------------------------------------- writer

#[derive(Default)]
struct PathNode {
    name: String,
    property: bool,
    children: Vec<usize>,
}

struct Writer {
    out: Vec<u8>,
    tokens: Vec<String>,
    token_index: HashMap<String, u32>,
    strings: Vec<u32>,
    string_index: HashMap<String, u32>,
    fields: Vec<(u32, u64)>,
    field_index: HashMap<(u32, u64), u32>,
    fieldsets: Vec<u32>,
    fieldset_index: HashMap<Vec<u32>, u32>,
    nodes: Vec<PathNode>,
    node_of: HashMap<String, usize>,
    specs: Vec<(u32, u32, u32)>,
}

fn rep(ty: u8, array: bool, inlined: bool, payload: u64) -> u64 {
    (array as u64) << 63 | (inlined as u64) << 62 | (ty as u64) << 48 | (payload & ((1 << 48) - 1))
}

/// Crate type and whether it is an array, from a USD type name.
fn crate_type(type_name: &str) -> (u8, bool) {
    let array = type_name.ends_with("[]");
    let base = type_name.trim_end_matches("[]");
    let ty = match base {
        "bool" => BOOL,
        "uchar" => UCHAR,
        "int" => INT,
        "uint" => UINT,
        "int64" => INT64,
        "uint64" => UINT64,
        "half" => HALF,
        "float" => FLOAT,
        "double" => DOUBLE,
        "timecode" => TIME_CODE,
        "string" => STRING,
        "token" => TOKEN,
        "asset" => ASSET,
        "matrix2d" => MATRIX2D,
        "matrix3d" => MATRIX3D,
        "matrix4d" | "frame4d" => MATRIX4D,
        "quatd" => QUATD,
        "quatf" => QUATF,
        "quath" => QUATH,
        "double2" | "texCoord2d" => VEC2D,
        "float2" | "texCoord2f" => VEC2F,
        "half2" | "texCoord2h" => VEC2H,
        "int2" => VEC2I,
        "double3" | "point3d" | "normal3d" | "vector3d" | "color3d" | "texCoord3d" => VEC3D,
        "float3" | "point3f" | "normal3f" | "vector3f" | "color3f" | "texCoord3f" => VEC3F,
        "half3" | "point3h" | "normal3h" | "vector3h" | "color3h" | "texCoord3h" => VEC3H,
        "int3" => VEC3I,
        "double4" | "color4d" => VEC4D,
        "float4" | "color4f" => VEC4F,
        "half4" | "color4h" => VEC4H,
        "int4" => VEC4I,
        _ => TOKEN,
    };
    (ty, array)
}

impl Writer {
    fn token(&mut self, t: &str) -> u32 {
        if let Some(&i) = self.token_index.get(t) {
            return i;
        }
        self.tokens.push(t.to_owned());
        let i = (self.tokens.len() - 1) as u32;
        self.token_index.insert(t.to_owned(), i);
        i
    }

    fn string(&mut self, s: &str) -> u32 {
        if let Some(&i) = self.string_index.get(s) {
            return i;
        }
        let t = self.token(s);
        self.strings.push(t);
        let i = (self.strings.len() - 1) as u32;
        self.string_index.insert(s.to_owned(), i);
        i
    }

    /// Adds a path (and its ancestors) to the tree; returns its node.
    fn path(&mut self, path: &str) -> usize {
        if let Some(&n) = self.node_of.get(path) {
            return n;
        }
        if self.nodes.is_empty() {
            self.nodes.push(PathNode::default());
            self.node_of.insert("/".into(), 0);
            if path == "/" {
                return 0;
            }
        }
        let (parent, name, property) = match path.rfind(['.', '/']) {
            Some(i) if path.as_bytes()[i] == b'.' && i > 0 => (&path[..i], &path[i + 1..], true),
            Some(0) => ("/", &path[1..], false),
            Some(i) => (&path[..i], &path[i + 1..], false),
            None => ("/", path, false),
        };
        let parent = self.path(parent);
        self.token(name);
        self.nodes.push(PathNode {
            name: name.to_owned(),
            property,
            children: Vec::new(),
        });
        let n = self.nodes.len() - 1;
        self.nodes[parent].children.push(n);
        self.node_of.insert(path.to_owned(), n);
        n
    }

    fn here(&self) -> u64 {
        self.out.len() as u64
    }

    fn put_u32(&mut self, v: u32) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    fn put_u64(&mut self, v: u64) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    fn put_scalars(&mut self, kind: Scalar, values: &[f64]) {
        for &v in values {
            match kind {
                Scalar::U8 => self.out.push(v as u8),
                Scalar::I32 => self.out.extend_from_slice(&(v as i32).to_le_bytes()),
                Scalar::U32 => self.out.extend_from_slice(&(v as u32).to_le_bytes()),
                Scalar::I64 => self.out.extend_from_slice(&(v as i64).to_le_bytes()),
                Scalar::U64 => self.out.extend_from_slice(&(v as u64).to_le_bytes()),
                Scalar::F16 => self
                    .out
                    .extend_from_slice(&f32_to_half(v as f32).to_le_bytes()),
                Scalar::F32 => self.out.extend_from_slice(&(v as f32).to_le_bytes()),
                Scalar::F64 => self.out.extend_from_slice(&v.to_le_bytes()),
            }
        }
    }

    /// Writes a value of a USD type; returns its rep.
    fn value(&mut self, v: &Value, type_name: &str) -> u64 {
        let (ty, array) = crate_type(type_name);
        match v {
            Value::Blocked => rep(VALUE_BLOCK, false, true, 0),
            Value::Bool(b) => rep(BOOL, false, true, *b as u64),
            Value::Int(i) if ty == INT64 || ty == UINT64 => {
                let at = self.here();
                self.put_u64(*i as u64);
                rep(ty, false, false, at)
            }
            Value::Int(i) => match ty {
                FLOAT | DOUBLE | HALF | TIME_CODE => {
                    self.value(&Value::Float(*i as f64), type_name)
                }
                BOOL => rep(BOOL, false, true, (*i != 0) as u64),
                UINT | UCHAR => rep(ty, false, true, *i as u32 as u64),
                _ => rep(INT, false, true, *i as i32 as u32 as u64),
            },
            Value::Float(f) => match ty {
                FLOAT => rep(FLOAT, false, true, (*f as f32).to_bits() as u64),
                HALF => rep(HALF, false, true, f32_to_half(*f as f32) as u64),
                INT | UINT | INT64 | UINT64 => self.value(&Value::Int(*f as i64), type_name),
                _ => {
                    let at = self.here();
                    self.out.extend_from_slice(&f.to_le_bytes());
                    rep(
                        if ty == TIME_CODE { TIME_CODE } else { DOUBLE },
                        false,
                        false,
                        at,
                    )
                }
            },
            Value::Token(s) => match ty {
                STRING => rep(STRING, false, true, self.string(s) as u64),
                ASSET => rep(ASSET, false, true, self.token(s) as u64),
                _ => rep(TOKEN, false, true, self.token(s) as u64),
            },
            Value::String(s) => match ty {
                TOKEN => rep(TOKEN, false, true, self.token(s) as u64),
                ASSET => rep(ASSET, false, true, self.token(s) as u64),
                _ => rep(STRING, false, true, self.string(s) as u64),
            },
            Value::Asset(s) => rep(ASSET, false, true, self.token(s) as u64),
            Value::Tuple(values) => {
                let (kind, _) = numeric(ty).unwrap_or((Scalar::F64, values.len()));
                let at = self.here();
                self.put_scalars(kind, values);
                rep(ty, false, false, at)
            }
            Value::Array { values, .. } if array => {
                let (kind, width) = numeric(ty).unwrap_or((Scalar::F32, 1));
                if values.is_empty() {
                    return rep(ty, true, false, 0);
                }
                let at = self.here();
                self.put_u64((values.len() / width.max(1)) as u64);
                self.put_scalars(kind, values);
                rep(ty, true, false, at)
            }
            Value::Array { values, .. } => {
                // A double vector (time sample times).
                let at = self.here();
                self.put_u64(values.len() as u64);
                self.put_scalars(Scalar::F64, values);
                rep(DOUBLE_VECTOR, false, false, at)
            }
            Value::Strings(items) => {
                let indices: Vec<u32> = items
                    .iter()
                    .map(|s| {
                        if ty == STRING {
                            self.string(s)
                        } else {
                            self.token(s)
                        }
                    })
                    .collect();
                if !array {
                    let at = self.here();
                    self.put_u64(indices.len() as u64);
                    for i in indices {
                        self.put_u32(i);
                    }
                    return rep(TOKEN_VECTOR, false, false, at);
                }
                if indices.is_empty() {
                    return rep(ty, true, false, 0);
                }
                let at = self.here();
                self.put_u64(indices.len() as u64);
                for i in indices {
                    self.put_u32(i);
                }
                rep(
                    if ty == STRING || ty == ASSET {
                        ty
                    } else {
                        TOKEN
                    },
                    true,
                    false,
                    at,
                )
            }
            Value::Paths(paths) => self.path_list_op(paths),
            Value::Dictionary(_) => rep(DICTIONARY, false, true, 0),
        }
    }

    fn path_list_op(&mut self, paths: &[String]) -> u64 {
        let indices: Vec<usize> = paths.iter().map(|p| self.node_of[p.as_str()]).collect();
        let at = self.here();
        // Explicit list.
        self.out.push(1 | 2);
        self.put_u64(indices.len() as u64);
        for i in indices {
            self.put_u32(i as u32);
        }
        rep(PATH_LIST_OP, false, false, at)
    }

    fn token_list_op(&mut self, tokens: &[String]) -> u64 {
        let indices: Vec<u32> = tokens.iter().map(|t| self.token(t)).collect();
        let at = self.here();
        // Prepended items.
        self.out.push(1 << 5);
        self.put_u64(indices.len() as u64);
        for i in indices {
            self.put_u32(i);
        }
        rep(TOKEN_LIST_OP, false, false, at)
    }

    fn time_samples(&mut self, samples: &[(f64, Value)], type_name: &str) -> u64 {
        let times: Vec<f64> = samples.iter().map(|s| s.0).collect();
        let times_rep = self.value(
            &Value::Array {
                values: times,
                width: 1,
            },
            "double",
        );
        let reps: Vec<u64> = samples
            .iter()
            .map(|(_, v)| self.value(v, type_name))
            .collect();
        let at = self.here();
        // Jump to the times rep (right after), jump to the values.
        self.put_u64(8);
        self.put_u64(times_rep);
        self.put_u64(8);
        self.put_u64(reps.len() as u64);
        for r in reps {
            self.put_u64(r);
        }
        rep(TIME_SAMPLES, false, false, at)
    }

    fn field(&mut self, name: &str, rep: u64) -> u32 {
        let t = self.token(name);
        if let Some(&i) = self.field_index.get(&(t, rep)) {
            return i;
        }
        self.fields.push((t, rep));
        let i = (self.fields.len() - 1) as u32;
        self.field_index.insert((t, rep), i);
        i
    }

    fn spec(&mut self, path: &str, kind: u32, fields: Vec<u32>) {
        let set = match self.fieldset_index.get(&fields) {
            Some(&s) => s,
            None => {
                let s = self.fieldsets.len() as u32;
                self.fieldsets.extend_from_slice(&fields);
                self.fieldsets.push(u32::MAX);
                self.fieldset_index.insert(fields, s);
                s
            }
        };
        let node = self.node_of[path] as u32;
        self.specs.push((node, set, kind));
    }

    fn metadata_value(&mut self, key: &str, v: &Value) -> u64 {
        match (key, v) {
            ("apiSchemas", Value::Strings(s)) => self.token_list_op(s),
            (_, Value::Strings(s)) => {
                let s = s.clone();
                self.value(&Value::Strings(s), "token")
            }
            (_, Value::Token(_)) => self.value(v, "token"),
            (_, Value::String(_)) => self.value(v, "string"),
            (_, Value::Float(_)) => self.value(v, "double"),
            (_, Value::Int(_)) => self.value(v, "int"),
            _ => self.value(v, "token"),
        }
    }

    fn prim(&mut self, prim: &Prim, parent: &str) {
        let path = if parent == "/" {
            format!("/{}", prim.name)
        } else {
            format!("{parent}/{}", prim.name)
        };
        let mut fields = Vec::new();
        let specifier = match prim.specifier {
            Specifier::Def => 0,
            Specifier::Over => 1,
            Specifier::Class => 2,
        };
        let r = rep(SPECIFIER, false, true, specifier);
        fields.push(self.field("specifier", r));
        if !prim.type_name.is_empty() {
            let r = rep(TOKEN, false, true, self.token(&prim.type_name) as u64);
            fields.push(self.field("typeName", r));
        }
        for (key, v) in &prim.metadata {
            if matches!(v, Value::Dictionary(_)) {
                continue;
            }
            let r = self.metadata_value(key, v);
            fields.push(self.field(key, r));
        }
        if !prim.properties.is_empty() {
            let names: Vec<String> = prim.properties.iter().map(|p| p.0.clone()).collect();
            let r = self.value(&Value::Strings(names), "token");
            fields.push(self.field("properties", r));
        }
        if !prim.children.is_empty() {
            let names: Vec<String> = prim.children.iter().map(|c| c.name.clone()).collect();
            let r = self.value(&Value::Strings(names), "token");
            fields.push(self.field("primChildren", r));
        }
        self.spec(&path, SPEC_PRIM, fields);
        for (name, property) in &prim.properties {
            let ppath = format!("{path}.{name}");
            let mut fields = Vec::new();
            match property {
                Property::Attribute(a) => {
                    if a.custom {
                        let r = rep(BOOL, false, true, 1);
                        fields.push(self.field("custom", r));
                    }
                    let r = rep(TOKEN, false, true, self.token(&a.type_name) as u64);
                    fields.push(self.field("typeName", r));
                    if a.uniform {
                        let r = rep(VARIABILITY, false, true, 1);
                        fields.push(self.field("variability", r));
                    }
                    if let Some(v) = &a.default {
                        let r = self.value(v, &a.type_name);
                        fields.push(self.field("default", r));
                    }
                    if !a.samples.is_empty() {
                        let r = self.time_samples(&a.samples, &a.type_name);
                        fields.push(self.field("timeSamples", r));
                    }
                    if !a.connections.is_empty() {
                        let r = self.path_list_op(&a.connections);
                        fields.push(self.field("connectionPaths", r));
                    }
                    for (key, v) in &a.metadata {
                        let r = match (key.as_str(), v) {
                            ("elementSize", _) => self.value(v, "int"),
                            _ => self.metadata_value(key, v),
                        };
                        fields.push(self.field(key, r));
                    }
                    self.spec(&ppath, SPEC_ATTRIBUTE, fields);
                }
                Property::Relationship(targets) => {
                    let r = rep(VARIABILITY, false, true, 1);
                    fields.push(self.field("variability", r));
                    if !targets.is_empty() {
                        let r = self.path_list_op(targets);
                        fields.push(self.field("targetPaths", r));
                    }
                    self.spec(&ppath, SPEC_RELATIONSHIP, fields);
                }
            }
        }
        for child in &prim.children {
            self.prim(child, &path);
        }
    }

    fn collect_paths(&mut self, prim: &Prim, parent: &str) {
        let path = if parent == "/" {
            format!("/{}", prim.name)
        } else {
            format!("{parent}/{}", prim.name)
        };
        self.path(&path);
        for (name, property) in &prim.properties {
            self.path(&format!("{path}.{name}"));
            let targets = match property {
                Property::Attribute(a) => &a.connections,
                Property::Relationship(t) => t,
            };
            for t in targets {
                self.path(t);
            }
        }
        for child in &prim.children {
            self.collect_paths(child, &path);
        }
    }

    fn compressed_ints(&mut self, values: &[i64]) {
        let packed = compress(&encode_ints(values, false));
        self.put_u64(packed.len() as u64);
        self.out.extend_from_slice(&packed);
    }
}

/// Writes a crate file (version 0.8.0).
pub fn write(layer: &Layer) -> Result<Vec<u8>> {
    let mut w = Writer {
        out: vec![0u8; 88],
        tokens: Vec::new(),
        token_index: HashMap::new(),
        strings: Vec::new(),
        string_index: HashMap::new(),
        fields: Vec::new(),
        field_index: HashMap::new(),
        fieldsets: Vec::new(),
        fieldset_index: HashMap::new(),
        nodes: Vec::new(),
        node_of: HashMap::new(),
        specs: Vec::new(),
    };
    // Token 0 is the empty token, so that no property name is token 0
    // (whose negation would be lost).
    w.token("");
    w.path("/");
    for prim in &layer.prims {
        w.collect_paths(prim, "/");
    }
    // Paths in depth-first order: renumber the nodes.
    let mut order = Vec::with_capacity(w.nodes.len());
    let mut stack = vec![0usize];
    while let Some(n) = stack.pop() {
        order.push(n);
        for &c in w.nodes[n].children.iter().rev() {
            stack.push(c);
        }
    }
    let mut number = vec![0usize; w.nodes.len()];
    for (k, &n) in order.iter().enumerate() {
        number[n] = k;
    }
    for v in w.node_of.values_mut() {
        *v = number[*v];
    }
    // Pseudo-root.
    let mut fields = Vec::new();
    for (key, v) in &layer.metadata {
        if matches!(v, Value::Dictionary(_)) {
            continue;
        }
        let r = match key.as_str() {
            "metersPerUnit" | "timeCodesPerSecond" | "framesPerSecond" | "startTimeCode"
            | "endTimeCode" => {
                let f = v.as_f64().unwrap_or(0.0);
                w.value(&Value::Float(f), "double")
            }
            _ => w.metadata_value(key, v),
        };
        fields.push(w.field(key, r));
    }
    if !layer.prims.is_empty() {
        let names: Vec<String> = layer.prims.iter().map(|p| p.name.clone()).collect();
        let r = w.value(&Value::Strings(names), "token");
        fields.push(w.field("primChildren", r));
    }
    w.spec("/", SPEC_PSEUDO_ROOT, fields);
    for prim in &layer.prims {
        w.prim(prim, "/");
    }

    // Sections.
    let mut sections: Vec<(&str, u64, u64)> = Vec::new();
    // TOKENS
    let start = w.here();
    let mut raw = Vec::new();
    for t in &w.tokens {
        raw.extend_from_slice(t.as_bytes());
        raw.push(0);
    }
    let packed = compress(&raw);
    let count = w.tokens.len() as u64;
    w.put_u64(count);
    w.put_u64(raw.len() as u64);
    w.put_u64(packed.len() as u64);
    w.out.extend_from_slice(&packed);
    sections.push(("TOKENS", start, w.here() - start));
    // STRINGS
    let start = w.here();
    let strings = std::mem::take(&mut w.strings);
    w.put_u64(strings.len() as u64);
    for s in strings {
        w.put_u32(s);
    }
    sections.push(("STRINGS", start, w.here() - start));
    // FIELDS
    let start = w.here();
    let fields = std::mem::take(&mut w.fields);
    w.put_u64(fields.len() as u64);
    w.compressed_ints(&fields.iter().map(|f| f.0 as i64).collect::<Vec<_>>());
    let reps: Vec<u8> = fields.iter().flat_map(|f| f.1.to_le_bytes()).collect();
    let packed = compress(&reps);
    w.put_u64(packed.len() as u64);
    w.out.extend_from_slice(&packed);
    sections.push(("FIELDS", start, w.here() - start));
    // FIELDSETS
    let start = w.here();
    let sets: Vec<i64> = w.fieldsets.iter().map(|&f| f as i64).collect();
    w.put_u64(sets.len() as u64);
    w.compressed_ints(&sets);
    sections.push(("FIELDSETS", start, w.here() - start));
    // PATHS
    let start = w.here();
    let n = order.len();
    let mut indexes = Vec::with_capacity(n);
    let mut elements = Vec::with_capacity(n);
    let mut jumps = Vec::with_capacity(n);
    // Next sibling of each node in depth-first order.
    let mut sibling = vec![None; w.nodes.len()];
    for node in &w.nodes {
        for pair in node.children.windows(2) {
            sibling[pair[0]] = Some(pair[1]);
        }
    }
    for (k, &node) in order.iter().enumerate() {
        indexes.push(k as i64);
        let element = if node == 0 {
            0
        } else {
            let t = w.token_index[&w.nodes[node].name] as i64;
            if w.nodes[node].property { -t } else { t }
        };
        elements.push(element);
        let has_child = !w.nodes[node].children.is_empty();
        let jump = match (has_child, sibling[node]) {
            (true, Some(s)) => (number[s] - k) as i64,
            (true, None) => -1,
            (false, Some(_)) => 0,
            (false, None) => -2,
        };
        jumps.push(jump);
    }
    w.put_u64(n as u64);
    w.put_u64(n as u64);
    w.compressed_ints(&indexes);
    w.compressed_ints(&elements);
    w.compressed_ints(&jumps);
    sections.push(("PATHS", start, w.here() - start));
    // SPECS
    let start = w.here();
    let specs = std::mem::take(&mut w.specs);
    w.put_u64(specs.len() as u64);
    w.compressed_ints(&specs.iter().map(|s| s.0 as i64).collect::<Vec<_>>());
    w.compressed_ints(&specs.iter().map(|s| s.1 as i64).collect::<Vec<_>>());
    w.compressed_ints(&specs.iter().map(|s| s.2 as i64).collect::<Vec<_>>());
    sections.push(("SPECS", start, w.here() - start));
    // TOC and bootstrap.
    let toc = w.here();
    w.put_u64(sections.len() as u64);
    for (name, start, size) in &sections {
        let mut field = [0u8; 16];
        field[..name.len()].copy_from_slice(name.as_bytes());
        w.out.extend_from_slice(&field);
        w.put_u64(*start);
        w.put_u64(*size);
    }
    w.out[..8].copy_from_slice(MAGIC);
    w.out[8..11].copy_from_slice(&VERSION);
    w.out[16..24].copy_from_slice(&toc.to_le_bytes());
    Ok(w.out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_coding_round_trip() {
        let values: Vec<i64> = vec![0, 1, 2, 3, 3, 1000, -5, 70000, 70001, -2_000_000_000, 7];
        let encoded = encode_ints(&values, false);
        assert_eq!(decode_ints(&encoded, values.len(), false).unwrap(), values);
        let wide: Vec<i64> = vec![1 << 40, -(1 << 40), 5, 6, 7];
        let encoded = encode_ints(&wide, true);
        assert_eq!(decode_ints(&encoded, wide.len(), true).unwrap(), wide);
        let packed = compress(&encoded);
        assert_eq!(decompress(&packed, encoded.len()).unwrap(), encoded);
    }

    #[test]
    fn halves() {
        for v in [0.0f32, 1.0, -2.5, 0.000_061_035_156, 65504.0, 0.333_251_95] {
            assert_eq!(half_to_f32(f32_to_half(v)), v, "{v}");
        }
        assert_eq!(f32_to_half(1.0), 0x3C00);
    }

    #[test]
    fn layer_round_trip() {
        let mut mesh = Prim::new("m", "Mesh");
        mesh.metadata.push((
            "apiSchemas".into(),
            Value::Strings(vec!["SkelBindingAPI".into()]),
        ));
        mesh.set(
            "points",
            Attribute::new(
                "point3f[]",
                Value::Array {
                    values: (0..60).map(|i| i as f64 * 0.25).collect(),
                    width: 3,
                },
            ),
        );
        let mut idx = Attribute::new(
            "int[]",
            Value::Array {
                values: vec![0.0, 1.0, 2.0],
                width: 1,
            },
        );
        idx.metadata.push(("elementSize".into(), Value::Int(1)));
        idx.metadata
            .push(("interpolation".into(), Value::Token("vertex".into())));
        mesh.set("faceVertexIndices", idx);
        let mut rot = Attribute {
            uniform: false,
            type_name: "quatf[]".into(),
            ..Default::default()
        };
        rot.samples = vec![
            (
                0.0,
                Value::Array {
                    values: vec![0.0, 0.0, 0.0, 1.0],
                    width: 4,
                },
            ),
            (
                1.0,
                Value::Array {
                    values: vec![0.5, 0.5, 0.5, 0.5],
                    width: 4,
                },
            ),
        ];
        mesh.set("rotations", rot);
        mesh.set(
            "m",
            Attribute::new(
                "matrix4d",
                Value::Tuple((0..16).map(|i| i as f64).collect()),
            ),
        );
        mesh.set(
            "joints",
            Attribute {
                uniform: true,
                ..Attribute::new("token[]", Value::Strings(vec!["a".into(), "a/b".into()]))
            },
        );
        mesh.set(
            "info:id",
            Attribute::new("token", Value::Token("UsdPreviewSurface".into())),
        );
        mesh.set(
            "inputs:varname",
            Attribute::new("string", Value::String("st".into())),
        );
        mesh.set(
            "inputs:file",
            Attribute::new("asset", Value::Asset("t/a.png".into())),
        );
        mesh.set(
            "inputs:roughness",
            Attribute::new("float", Value::Float(0.5)),
        );
        let mut c = Attribute::new("color3f", Value::Tuple(vec![0.5, 0.25, 1.0]));
        c.connections.push("/r/m.points".into());
        mesh.set("inputs:diffuseColor", c);
        mesh.relate("skel:skeleton", vec!["/r".into()]);
        let mut root = Prim::new("r", "SkelRoot");
        root.children.push(mesh);
        let layer = Layer {
            metadata: vec![
                ("upAxis".into(), Value::Token("Y".into())),
                ("metersPerUnit".into(), Value::Float(1.0)),
                ("defaultPrim".into(), Value::Token("r".into())),
            ],
            prims: vec![root],
        };
        let bytes = write(&layer).unwrap();
        let back = read(&bytes).unwrap();
        assert_eq!(back, layer);
    }

    #[test]
    fn rejects_damaged_files() {
        let layer = Layer {
            metadata: vec![],
            prims: vec![Prim::new("a", "Xform")],
        };
        let bytes = write(&layer).unwrap();
        for cut in [10, 88, bytes.len() / 2, bytes.len() - 1] {
            assert!(read(&bytes[..cut]).is_err(), "{cut}");
        }
    }
}

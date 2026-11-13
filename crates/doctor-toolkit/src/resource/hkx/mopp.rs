//! MOPP: Havok's compressed bounding-volume tree over the triangles of a mesh.
//!
//! An `hkpMoppBvTreeShape` answers "which triangles may touch this box" by
//! running a byte code (`hkpMoppCode`). The machine below is a port of the
//! game's AABB query (`godfather2.exe` `0x00C61BA0`, `0x00C61050`), so its
//! answers are the game's answers. [`build`] writes new code that uses a small
//! subset of the instructions.
//!
//! The code works in integer space: a point `p` maps to `(p - offset) * scale`,
//! 24 bits per axis. The query box is turned into 8-bit cells (`>> 16`), with
//! one extra unit of slack on each side. Instructions (big-endian arguments):
//!
//! | Op | Bytes | Meaning |
//! |---|---|---|
//! | `00` | 1 | return |
//! | `01`–`04` | 4 | rescale by 2^op: `offset = (offset + xyz) << op`, cells get finer |
//! | `05`, `06`, `07` | 2, 3, 4 | jump forward by an 8-, 16-, 24-bit distance |
//! | `09`, `0A` | 2, 3 | add to the key offset; `0B` (5) sets it |
//! | `0C` | 3 | continue at chunk `n` (`n × 512` bytes from the start) |
//! | `0D` | 5 | no operation |
//! | `10`–`12` | 4 | split on x, y, z: `lo hi jump`; the first branch (next byte) holds keys below `lo`, the second (after `jump`) keys from `hi` up |
//! | `13`–`1C` | 4 | the same on the diagonals `y+z`, `y−z`, `x+z`, `x−z`, `x+y`, `x−y`, `x+y+z`, `x+y−z`, `x+z−y`, `x−y−z` |
//! | `20`–`22` | 3 | single-plane split `v jump` |
//! | `23`–`25` | 7 | split with 16-bit jumps for both branches |
//! | `26`–`28` | 3 | cut: continue only if the box overlaps `[lo, hi)` on the axis |
//! | `29`–`2B` | 7 | cut with 24-bit bounds in unscaled space |
//! | `30`–`4F` | 1 | key `op − 0x30` (+ key offset) |
//! | `50`–`53` | 2–5 | key with an 8-, 16-, 24-, 32-bit argument |
//! | `60`–`6B` | 2, 3, 5 | properties (ignored for keys) |

use serde::Serialize;

use crate::{Error, Result};

/// The integer space spans 2^24 units per axis.
const SPACE: f64 = 16_777_216.0;

/// MOPP code with its coordinate frame.
#[derive(Debug, Clone, Serialize)]
pub struct MoppCode {
    /// Origin of the code's integer space.
    pub offset: [f32; 3],
    /// Integer units per metre.
    pub scale: f32,
    #[serde(skip)]
    pub data: Vec<u8>,
    /// 1 for codes over extended meshes, 0 otherwise.
    pub build_type: u32,
    /// The bytes of the `hkpMoppCode` object as read.
    #[serde(skip)]
    pub raw: Vec<u8>,
}

impl PartialEq for MoppCode {
    fn eq(&self, other: &Self) -> bool {
        self.offset == other.offset
            && self.scale.to_bits() == other.scale.to_bits()
            && self.data == other.data
            && self.build_type == other.build_type
    }
}

impl MoppCode {
    /// The keys whose boxes may overlap the box `min..max`, as the game finds them.
    pub fn query(&self, min: [f32; 3], max: [f32; 3]) -> Result<Vec<u32>> {
        // Clip the query to the code's space, as hkpMoppBvTreeShape::queryAabb does.
        let extent = (SPACE / f64::from(self.scale)) as f32;
        let mut lo = [0f32; 3];
        let mut hi = [0f32; 3];
        for a in 0..3 {
            lo[a] = min[a].max(self.offset[a]);
            hi[a] = max[a].min(self.offset[a] + extent);
        }
        let scale = f64::from(self.scale);
        let to_int = |v: f32, a: usize| -> i32 {
            let x = (f64::from(v) - f64::from(self.offset[a])) * scale;
            x.round_ties_even()
                .clamp(-(1 << 30) as f64, (1 << 30) as f64) as i32
        };
        let query = Query {
            min: [0, 1, 2].map(|a| to_int(lo[a], a) - 1),
            max: [0, 1, 2].map(|a| to_int(hi[a], a) + 1),
        };
        let state = State {
            max: query.max.map(|v| (v >> 16) + 1),
            min: query.min.map(|v| v >> 16),
            offset: [0; 3],
            key_offset: 0,
            shift: 0,
        };
        let mut keys = Vec::new();
        run(&self.data, &query, state, &mut keys)?;
        Ok(keys)
    }

    /// Every key the code can report (no culling).
    pub fn keys(&self) -> Result<Vec<u32>> {
        let state = State {
            max: [i32::MAX / 4; 3],
            min: [i32::MIN / 4; 3],
            offset: [0; 3],
            key_offset: 0,
            shift: 0,
        };
        let query = Query {
            min: [i32::MIN / 4; 3],
            max: [i32::MAX / 4; 3],
        };
        let mut keys = Vec::new();
        run(&self.data, &query, state, &mut keys)?;
        keys.sort_unstable();
        keys.dedup();
        Ok(keys)
    }
}

/// The query box in full 24-bit precision (with its slack).
struct Query {
    min: [i32; 3],
    max: [i32; 3],
}

#[derive(Clone, Copy)]
struct State {
    /// Query box in the current cells: `max` is one past the last cell.
    max: [i32; 3],
    min: [i32; 3],
    offset: [i32; 3],
    key_offset: u32,
    shift: u32,
}

fn malformed(pc: usize, reason: &str) -> Error {
    Error::malformed("MOPP code", pc, reason)
}

const MAX_STEPS: usize = 1 << 24;

fn run(code: &[u8], query: &Query, state: State, keys: &mut Vec<u32>) -> Result<()> {
    let byte = |pc: usize| -> Result<i32> {
        code.get(pc)
            .map(|&b| i32::from(b))
            .ok_or_else(|| malformed(pc, "runs past the end"))
    };
    let be = |pc: usize, n: usize| -> Result<u32> {
        let mut v = 0u32;
        for k in 0..n {
            v = (v << 8) | byte(pc + k)? as u32;
        }
        Ok(v)
    };
    let mut stack = vec![(0usize, state)];
    let mut steps = 0usize;
    while let Some((mut pc, mut st)) = stack.pop() {
        loop {
            steps += 1;
            if steps > MAX_STEPS {
                return Err(malformed(pc, "does not terminate"));
            }
            let op = byte(pc)?;
            match op {
                0x00 => break,
                0x01..=0x04 => {
                    let shift = st.shift + op as u32;
                    if shift > 16 {
                        return Err(malformed(pc, "rescaled past 24 bits"));
                    }
                    let s = 16 - shift;
                    for a in 0..3 {
                        let offset = (byte(pc + 1 + a)? + st.offset[a]) << op;
                        st.offset[a] = offset;
                        st.min[a] = (query.min[a] >> s) - offset;
                        st.max[a] = (query.max[a] >> s) - offset + 1;
                    }
                    st.shift = shift;
                    pc += 4;
                }
                0x05 => pc += 2 + byte(pc + 1)? as usize,
                0x06 => pc += 3 + be(pc + 1, 2)? as usize,
                0x07 => pc += 4 + be(pc + 1, 3)? as usize,
                0x09 => {
                    st.key_offset = st.key_offset.wrapping_add(byte(pc + 1)? as u32);
                    pc += 2;
                }
                0x0A => {
                    st.key_offset = st.key_offset.wrapping_add(be(pc + 1, 2)?);
                    pc += 3;
                }
                0x0B => {
                    st.key_offset = be(pc + 1, 4)?;
                    pc += 5;
                }
                0x0C => pc = be(pc + 1, 2)? as usize * 512,
                0x0D => pc += 5,
                0x10..=0x1C => {
                    let lo = byte(pc + 1)?;
                    let hi = byte(pc + 2)?;
                    let jump = byte(pc + 3)? as usize;
                    // Projected box (min, max + 1 in cell units) and plane values.
                    let (pmin, pmax, lo, hi) = match op {
                        0x10..=0x12 => {
                            let a = (op - 0x10) as usize;
                            (st.min[a], st.max[a], lo, hi)
                        }
                        _ => diagonal(op, &st, lo, hi),
                    };
                    if hi < pmax {
                        if pmin < lo {
                            stack.push((pc + 4, st));
                        }
                        pc += 4 + jump;
                    } else {
                        if lo <= pmin {
                            break;
                        }
                        pc += 4;
                    }
                }
                0x20..=0x22 => {
                    let a = (op - 0x20) as usize;
                    let v = byte(pc + 1)?;
                    let jump = byte(pc + 2)? as usize;
                    if v < st.max[a] {
                        if st.min[a] <= v {
                            stack.push((pc + 3, st));
                        }
                        pc += 3 + jump;
                    } else {
                        pc += 3;
                    }
                }
                0x23..=0x25 => {
                    let a = (op - 0x23) as usize;
                    let lo = byte(pc + 1)?;
                    let hi = byte(pc + 2)?;
                    let first = be(pc + 3, 2)? as usize;
                    let second = be(pc + 5, 2)? as usize;
                    if hi < st.max[a] {
                        if st.min[a] < lo {
                            stack.push((pc + 7 + first, st));
                        }
                        pc += 7 + second;
                    } else {
                        if lo <= st.min[a] {
                            break;
                        }
                        pc += 7 + first;
                    }
                }
                0x26..=0x28 => {
                    let a = (op - 0x26) as usize;
                    if st.max[a] < byte(pc + 1)? || byte(pc + 2)? <= st.min[a] {
                        break;
                    }
                    pc += 3;
                }
                0x29..=0x2B => {
                    let a = (op - 0x29) as usize;
                    let lo = be(pc + 1, 3)? as i32;
                    let hi = be(pc + 4, 3)? as i32;
                    if query.max[a] < lo || hi < query.min[a] {
                        break;
                    }
                    pc += 7;
                }
                0x30..=0x53 => {
                    let value = match op {
                        0x30..=0x4F => (op - 0x30) as u32,
                        0x50 => be(pc + 1, 1)?,
                        0x51 => be(pc + 1, 2)?,
                        0x52 => be(pc + 1, 3)?,
                        0x53 => be(pc + 1, 4)?,
                        _ => return Err(malformed(pc, "unknown instruction")),
                    };
                    keys.push(st.key_offset.wrapping_add(value));
                    break;
                }
                0x60..=0x63 => pc += 2,
                0x64..=0x67 => pc += 3,
                0x68..=0x6B => pc += 5,
                _ => return Err(malformed(pc, "unknown instruction")),
            }
        }
    }
    Ok(())
}

/// Projection of the query box on a diagonal plane and the scaled plane values.
fn diagonal(op: i32, st: &State, lo: i32, hi: i32) -> (i32, i32, i32, i32) {
    let [x0, y0, z0] = st.min;
    let [x1, y1, z1] = st.max;
    match op {
        0x13 => (y0 + z0, y1 + z1, 2 * lo, 2 * hi),
        0x14 => (y0 - z1, y1 - z0, 2 * lo - 255, 2 * hi - 255),
        0x15 => (x0 + z0, x1 + z1, 2 * lo, 2 * hi),
        0x16 => (x0 - z1, x1 - z0, 2 * lo - 255, 2 * hi - 255),
        0x17 => (x0 + y0, x1 + y1, 2 * lo, 2 * hi),
        0x18 => (x0 - y1, x1 - y0, 2 * lo - 255, 2 * hi - 255),
        0x19 => (x0 + y0 + z0, x1 + y1 + z1, 3 * lo, 3 * hi),
        0x1A => (x0 + y0 - z1, x1 + y1 - z0, 3 * (lo - 0x55), 3 * (hi - 0x55)),
        0x1B => (x0 + z0 - y1, x1 + z1 - y0, 3 * (lo - 0x55), 3 * (hi - 0x55)),
        _ => (x0 - z1 - y1, x1 - z0 - y0, 3 * (lo - 0xAA), 3 * (hi - 0xAA)),
    }
}

/// A primitive for [`build`]: its key and bounding box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Primitive {
    pub key: u32,
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// Builds MOPP code over `primitives`, each box grown by `margin`.
///
/// The tree splits the primitives at the median of the longest axis of their
/// centres (`10`–`12`, or `23`–`25` with a 24-bit jump when a branch is
/// longer than 255 bytes); every leaf cuts the query on all three axes
/// (`26`–`28`) before its key. Bounds are rounded outwards, so a box that
/// touches a primitive always reaches its key. The frame is chosen like
/// Havok's: the offset is the minimum corner and the largest extent maps to
/// 254 cells of 2^16 units.
pub fn build(primitives: &[Primitive], margin: f32) -> MoppCode {
    if primitives.is_empty() {
        return MoppCode {
            offset: [0.0; 3],
            scale: 1.0,
            data: vec![0x00],
            build_type: 0,
            raw: Vec::new(),
        };
    }
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for p in primitives {
        for a in 0..3 {
            lo[a] = lo[a].min(p.min[a] - margin);
            hi[a] = hi[a].max(p.max[a] + margin);
        }
    }
    let extent = (0..3).map(|a| hi[a] - lo[a]).fold(1e-3f32, f32::max);
    let scale = (254.0 * 65536.0 / f64::from(extent)) as f32;
    let cells: Vec<Cells> = primitives
        .iter()
        .map(|p| {
            let mut c = Cells {
                key: p.key,
                lo: [0; 3],
                hi: [0; 3],
                centre: [0.0; 3],
            };
            for (a, &origin) in lo.iter().enumerate() {
                let f = |v: f32| (f64::from(v) - f64::from(origin)) * f64::from(scale);
                let l = f(p.min[a] - margin).floor().clamp(0.0, 254.0 * 65536.0) as i32;
                let h = f(p.max[a] + margin)
                    .ceil()
                    .clamp(0.0, 254.0 * 65536.0 + 65535.0) as i32;
                c.lo[a] = (l >> 16) as u8;
                c.hi[a] = (h >> 16).min(254) as u8;
                c.centre[a] = (p.min[a] + p.max[a]) * 0.5;
            }
            c
        })
        .collect();
    let mut order: Vec<usize> = (0..cells.len()).collect();
    let mut data = Vec::new();
    emit(&cells, &mut order, &mut data);
    MoppCode {
        offset: lo,
        scale,
        data,
        build_type: 0,
        raw: Vec::new(),
    }
}

struct Cells {
    key: u32,
    lo: [u8; 3],
    hi: [u8; 3],
    centre: [f32; 3],
}

fn emit(cells: &[Cells], set: &mut [usize], out: &mut Vec<u8>) {
    if let [only] = set {
        let c = &cells[*only];
        for a in 0..3u8 {
            out.extend_from_slice(&[0x26 + a, c.lo[a as usize], c.hi[a as usize] + 1]);
        }
        let key = c.key;
        match key {
            0..=0x1F => out.push(0x30 + key as u8),
            0x20..=0xFF => out.extend_from_slice(&[0x50, key as u8]),
            0x100..=0xFFFF => {
                out.push(0x51);
                out.extend_from_slice(&(key as u16).to_be_bytes());
            }
            0x1_0000..=0xFF_FFFF => {
                out.push(0x52);
                out.extend_from_slice(&key.to_be_bytes()[1..]);
            }
            _ => {
                out.push(0x53);
                out.extend_from_slice(&key.to_be_bytes());
            }
        }
        return;
    }
    // Split at the median of the longest axis of the centres.
    let mut c_lo = [f32::MAX; 3];
    let mut c_hi = [f32::MIN; 3];
    for &i in set.iter() {
        for a in 0..3 {
            c_lo[a] = c_lo[a].min(cells[i].centre[a]);
            c_hi[a] = c_hi[a].max(cells[i].centre[a]);
        }
    }
    let axis = (0..3)
        .max_by(|&a, &b| (c_hi[a] - c_lo[a]).total_cmp(&(c_hi[b] - c_lo[b])))
        .unwrap_or(0);
    set.sort_by(|&a, &b| {
        cells[a].centre[axis]
            .total_cmp(&cells[b].centre[axis])
            .then(a.cmp(&b))
    });
    let mid = set.len() / 2;
    let (left, right) = set.split_at_mut(mid);
    let left_hi = left.iter().map(|&i| cells[i].hi[axis]).max().unwrap_or(0);
    let right_lo = right.iter().map(|&i| cells[i].lo[axis]).min().unwrap_or(0);
    let mut first = Vec::new();
    emit(cells, left, &mut first);
    let mut second = Vec::new();
    emit(cells, right, &mut second);
    // The first branch is taken when the box starts below `left_hi + 1`,
    // the second when it ends at `right_lo` or above.
    let a = axis as u8;
    if first.len() <= 0xFF {
        out.extend_from_slice(&[0x10 + a, left_hi + 1, right_lo, first.len() as u8]);
        out.extend_from_slice(&first);
    } else {
        // 16-bit jumps: the first branch starts with a 24-bit jump to the second.
        out.extend_from_slice(&[0x23 + a, left_hi + 1, right_lo, 0, 4, 0, 0]);
        out.push(0x07);
        out.extend_from_slice(&(first.len() as u32).to_be_bytes()[1..]);
        out.extend_from_slice(&first);
    }
    out.extend_from_slice(&second);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlaps(p: &Primitive, min: [f32; 3], max: [f32; 3]) -> bool {
        (0..3).all(|a| p.min[a] <= max[a] && min[a] <= p.max[a])
    }

    /// A small deterministic generator.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> f32 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((self.0 >> 40) as f32) / (1u64 << 24) as f32
        }
    }

    #[test]
    fn built_code_reports_every_overlapping_primitive() {
        let mut rng = Rng(7);
        for count in [1usize, 2, 3, 17, 300, 2000] {
            let size = 10.0 + 90.0 * rng.next();
            let prims: Vec<Primitive> = (0..count)
                .map(|i| {
                    let c = [0, 1, 2].map(|_| size * rng.next() - 3.0);
                    let e = [0, 1, 2].map(|_| 0.5 * rng.next());
                    Primitive {
                        key: (i * 7) as u32,
                        min: [c[0] - e[0], c[1] - e[1], c[2] - e[2]],
                        max: [c[0] + e[0], c[1] + e[1], c[2] + e[2]],
                    }
                })
                .collect();
            let code = build(&prims, 0.0);
            let mut all = code.keys().unwrap();
            all.sort_unstable();
            let mut expected: Vec<u32> = prims.iter().map(|p| p.key).collect();
            expected.sort_unstable();
            assert_eq!(all, expected);
            for p in &prims {
                assert!(code.query(p.min, p.max).unwrap().contains(&p.key));
            }
            let mut culled = 0;
            for _ in 0..200 {
                let c = [0, 1, 2].map(|_| size * rng.next() - 3.0);
                let e = 2.0 * rng.next();
                let (min, max) = (c.map(|v| v - e), c.map(|v| v + e));
                let found = code.query(min, max).unwrap();
                for p in prims.iter().filter(|p| overlaps(p, min, max)) {
                    assert!(
                        found.contains(&p.key),
                        "{count} primitives: key {} missed",
                        p.key
                    );
                }
                culled += prims.len() - found.len();
            }
            if count > 100 {
                assert!(culled > 0, "the tree culls nothing");
            }
        }
    }

    #[test]
    fn rejects_bad_code() {
        let mut code = build(
            &[Primitive {
                key: 1,
                min: [0.0; 3],
                max: [1.0; 3],
            }],
            0.0,
        );
        assert_eq!(code.keys().unwrap(), vec![1]);
        code.data = vec![0x10, 1, 2];
        assert!(code.keys().is_err());
        code.data = vec![0xFF];
        assert!(code.keys().is_err());
        code.data = vec![0x0C, 0, 0];
        assert!(code.keys().is_err());
    }
}

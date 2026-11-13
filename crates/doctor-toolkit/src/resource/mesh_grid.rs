//! Triangle grids of static draw records (record field `0x8C`).
//!
//! Every static (strip) record of the game has one; skinned records have
//! none. The grid splits the record's box into cells and lists, per cell,
//! the strip runs that touch it:
//!
//! ```text
//! 0x00  vec4  minimum (w = 0)
//! 0x10  vec4  maximum (w = 0)
//! 0x20  vec4  cells per unit: dims / (max − min); +inf for an empty extent
//! 0x30  u16 × 3  dims (x, y, z)
//! 0x36  u16   entry count E
//! 0x38  u16   cell count C = dims x × y × z
//! 0x3A  u16 × E  entries: strip position / 2 of a run start
//!       u16 × (C + 1)  first entry of every cell; the last one is E
//!       zero padding to 0x10
//! ```
//!
//! Cells go x fastest, then z, then y. A run is a sub-strip that starts at
//! an even index position (so its winding is the strip's) and ends where the
//! next run starts; a cell lists every run whose box overlaps it. In the
//! corpus 83 % of the grids are reproduced exactly by this rule; the rest
//! differ in rounding at cell borders.

use crate::Result;
use crate::io::{self, Endian};

const HEADER: usize = 0x3A;

/// Size in bytes of the grid at `at`, padding included.
pub fn size_at(data: &[u8], at: usize) -> Result<usize> {
    let le = Endian::Little;
    let entries = io::read_u16(data, at + 0x36, le)? as usize;
    let cells = io::read_u16(data, at + 0x38, le)? as usize;
    let size = HEADER + 2 * entries + 2 * (cells + 1);
    Ok(size.div_ceil(0x10) * 0x10)
}

/// A decoded grid.
#[derive(Debug, Clone, PartialEq)]
pub struct Grid {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub dims: [u16; 3],
    /// Entries of every cell (x fastest, then z, then y): run start / 2.
    pub cells: Vec<Vec<u16>>,
}

impl Grid {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let le = Endian::Little;
        let f = |o: usize| io::read_u32(data, o, le).map(f32::from_bits);
        let u = |o: usize| io::read_u16(data, o, le);
        let dims = [u(0x30)?, u(0x32)?, u(0x34)?];
        let entries = u(0x36)? as usize;
        let cells = u(0x38)? as usize;
        let list: Vec<u16> = (0..entries)
            .map(|i| u(HEADER + 2 * i))
            .collect::<Result<_>>()?;
        let firsts: Vec<usize> = (0..=cells)
            .map(|i| u(HEADER + 2 * entries + 2 * i).map(usize::from))
            .collect::<Result<_>>()?;
        let mut out = Vec::with_capacity(cells);
        for w in firsts.windows(2) {
            let cell = list.get(w[0]..w[1]).ok_or_else(|| {
                crate::Error::malformed("mesh grid", HEADER, "cell outside the entry list")
            })?;
            out.push(cell.to_vec());
        }
        Ok(Self {
            min: [f(0)?, f(4)?, f(8)?],
            max: [f(0x10)?, f(0x14)?, f(0x18)?],
            dims,
            cells: out,
        })
    }

    /// Builds the grid of a strip. `positions` are the record's vertices
    /// (strip index − `base`), `runs` the index positions where runs start
    /// (even, ascending, the first one 0).
    pub fn build(positions: &[[f32; 3]], strip: &[u16], base: u16, runs: &[usize]) -> Self {
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for &i in strip {
            if let Some(p) = positions.get(i.wrapping_sub(base) as usize) {
                for a in 0..3 {
                    min[a] = min[a].min(p[a]);
                    max[a] = max[a].max(p[a]);
                }
            }
        }
        if min[0] > max[0] {
            min = [0.0; 3];
            max = [0.0; 3];
        }
        let triangles = strip.len().saturating_sub(2).max(1);
        let dims = choose_dims(min, max, triangles);
        let per_unit = per_unit(min, max, dims);
        let cell_of = |v: f32, a: usize| -> usize {
            if !per_unit[a].is_finite() {
                return 0;
            }
            (((v - min[a]) * per_unit[a]) as isize).clamp(0, dims[a] as isize - 1) as usize
        };
        let (nx, ny, nz) = (dims[0] as usize, dims[1] as usize, dims[2] as usize);
        let mut cells = vec![Vec::new(); nx * ny * nz];
        for (k, &start) in runs.iter().enumerate() {
            let end = runs.get(k + 1).copied().unwrap_or(strip.len());
            let mut lo = [f32::INFINITY; 3];
            let mut hi = [f32::NEG_INFINITY; 3];
            for q in start..end.min(strip.len().saturating_sub(2)) {
                let t = [strip[q], strip[q + 1], strip[q + 2]];
                if t[0] == t[1] || t[1] == t[2] || t[0] == t[2] {
                    continue;
                }
                for i in t {
                    if let Some(p) = positions.get(i.wrapping_sub(base) as usize) {
                        for a in 0..3 {
                            lo[a] = lo[a].min(p[a]);
                            hi[a] = hi[a].max(p[a]);
                        }
                    }
                }
            }
            if lo[0] > hi[0] {
                continue;
            }
            let r = |a: usize| cell_of(lo[a], a)..=cell_of(hi[a], a);
            for y in r(1) {
                for z in r(2) {
                    for x in r(0) {
                        cells[x + nx * (z + nz * y)].push((start / 2) as u16);
                    }
                }
            }
        }
        Self {
            min,
            max,
            dims,
            cells,
        }
    }

    /// Encodes the grid with its padding.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let per_unit = per_unit(self.min, self.max, self.dims);
        for v in [self.min, self.max, per_unit] {
            for x in v {
                out.extend_from_slice(&x.to_le_bytes());
            }
            out.extend_from_slice(&0f32.to_le_bytes());
        }
        let entries: usize = self.cells.iter().map(Vec::len).sum();
        for v in self.dims {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&(entries as u16).to_le_bytes());
        out.extend_from_slice(&(self.cells.len() as u16).to_le_bytes());
        for cell in &self.cells {
            for e in cell {
                out.extend_from_slice(&e.to_le_bytes());
            }
        }
        let mut first = 0u16;
        out.extend_from_slice(&first.to_le_bytes());
        for cell in &self.cells {
            first += cell.len() as u16;
            out.extend_from_slice(&first.to_le_bytes());
        }
        out.resize(out.len().div_ceil(0x10) * 0x10, 0);
        out
    }
}

fn per_unit(min: [f32; 3], max: [f32; 3], dims: [u16; 3]) -> [f32; 3] {
    // An empty extent gives +inf, as in the game files.
    [0, 1, 2].map(|a| dims[a] as f32 / (max[a] - min[a]))
}

/// Cells of roughly cubic shape with about 16 strip positions each, at most
/// 64 per axis and 4096 in total (entry counts are 16-bit).
fn choose_dims(min: [f32; 3], max: [f32; 3], triangles: usize) -> [u16; 3] {
    let ext = [0, 1, 2].map(|a| (max[a] - min[a]).max(0.0));
    let target_cells = (triangles as f32 / 16.0).clamp(1.0, 4096.0);
    let largest = ext.iter().copied().fold(0.0f32, f32::max);
    if largest <= 0.0 {
        return [1, 1, 1];
    }
    // Ignore axes much thinner than the largest one.
    let used: Vec<f32> = ext
        .iter()
        .map(|&e| if e > largest * 0.05 { e } else { 0.0 })
        .collect();
    let volume: f32 = used.iter().filter(|&&e| e > 0.0).product();
    let axes = used.iter().filter(|&&e| e > 0.0).count() as f32;
    let cell = (volume / target_cells).powf(1.0 / axes);
    [0, 1, 2].map(|a| {
        if used[a] > 0.0 {
            ((used[a] / cell).round() as u16).clamp(1, 64)
        } else {
            1
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_and_parse() {
        // Two quads far apart along x, each a run of one sub-strip.
        let positions = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
            [10.0, 0.0, 0.0],
            [11.0, 0.0, 0.0],
            [10.0, 1.0, 0.0],
            [11.0, 1.0, 0.0],
        ];
        let strip = [0u16, 1, 2, 3, 3, 4, 4, 5, 6, 7];
        let grid = Grid::build(&positions, &strip, 0, &[0, 4]);
        let bytes = grid.to_bytes();
        assert_eq!(bytes.len() % 0x10, 0);
        assert_eq!(size_at(&bytes, 0).unwrap(), bytes.len());
        let parsed = Grid::parse(&bytes).unwrap();
        assert_eq!(parsed, grid);
        assert!(grid.dims[0] > 1);
        assert_eq!(grid.cells.first().unwrap(), &vec![0]);
        assert_eq!(grid.cells.last().unwrap(), &vec![2]);
        // z has no extent: +inf cells per unit, as the game writes it.
        assert!(f32::from_le_bytes(bytes[0x28..0x2C].try_into().unwrap()).is_infinite());
    }
}

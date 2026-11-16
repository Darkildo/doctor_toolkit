//! `wgi` resources: the world grid of a level (`docs/formats/wgi.md`).
//!
//! A `WorldGraphPartition` object of the level root owns one `wgi`. Its cells
//! are the streams the partition loads by the position of the player; its
//! stream table lists the other streams of the level with the streams they
//! need.
//!
//! # Layout
//!
//! All values are little-endian; offsets are absolute and become pointers
//! when the game loads the resource (`0x0088CC10`).
//!
//! ```text
//! 0x00  u32  type id 0x3B004EB9 (SDBM of "wgi")
//! 0x04  u32  version, 7
//! 0x08  u32  offset of the stream table, 0x20
//! 0x0C  u32  stream count S
//! 0x10  u32  offset of the cells
//! 0x14  u32  cell count C
//! 0x18  u32  offset of the pool
//! 0x1C  u32  pool word count P
//! 0x20  S × 24-byte stream entries
//!       C × 128-byte cells
//!       P × u32 pool words: offsets of stream entries
//! ```
//!
//! Stream entry: 16-byte GUID of a stream, u32 count and u32 offset of its
//! list in the pool (both 0 without a list). Cell: see [`Cell`].

use serde::Serialize;

use crate::io::{self, Endian};
use crate::resource::simgroup::{Guid, Hash32};
use crate::{Error, Result};

/// Type id of the resource, also its first word.
pub const TYPE_WGI: u32 = 0x3B00_4EB9;
pub const VERSION: u32 = 7;
/// "No stream" in [`Cell::linked_stream`].
pub const NO_STREAM: u32 = 0xFE16_702F;

const HEADER_SIZE: usize = 0x20;
const STREAM_SIZE: usize = 24;
const CELL_SIZE: usize = 0x80;
const WHAT: &str = "wgi";

/// Cells of this kind are enabled from the start; every other kind waits
/// for a `ZoneStreamEnable` object that names the cell (`0x0088C4F0`).
pub const KIND_WORLD: u8 = 0;

/// The game loads the cell whenever its partition is enabled, at any
/// distance ([`Cell::flags`], `0x0088A960`).
pub const FLAG_ALWAYS: u8 = 0x01;

/// Level-of-detail bits of [`Cell::lod_mask`] and of attribute 0 of
/// `WorldGraphMetrics`.
pub mod lod {
    pub const HIGH: u32 = 0x01;
    pub const MEDIUM: u32 = 0x02;
    pub const LOW: u32 = 0x04;
    /// The map of the Don's View: the one cell of the fourth grid of a city.
    pub const MAP: u32 = 0x08;
    pub const INTERIOR: u32 = 0x10;
}

/// A list of streams in the pool: `count` words from word `start`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PoolList {
    pub start: u32,
    pub count: u32,
}

/// A stream of the level that is not a cell, with the streams it needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GridStream {
    /// GUID of the stream; [`crate::toc::guid_hash`] of it is the hash of
    /// its entry in the tables of contents.
    pub guid: Guid,
    pub needs: Option<PoolList>,
}

impl GridStream {
    pub fn hash(&self) -> u32 {
        crate::toc::guid_hash(&self.guid.0)
    }
}

/// One stream the partition loads by the position of the player.
///
/// ```text
/// 0x00  16   GUID of the stream
/// 0x10  u32  hash of the stream in the tables of contents (fold of the GUID)
/// 0x14  3f   centre: distance to the player is measured to it
/// 0x20  f32  radius of the bounding sphere
/// 0x24  12   not read by the game
/// 0x30  4f   box minimum, w = 1 (minimum above maximum: no box)
/// 0x40  4f   box maximum, w = 1
/// 0x50  u8   kind
/// 0x51  u8   flags
/// 0x52  u16  number of words of the list at 0x6C
/// 0x54  u32  level-of-detail mask
/// 0x58  f32  nearest distance the cell is loaded at (0: none)
/// 0x5C  f32  farthest distance (0: the radius of the partition)
/// 0x60  u32  SDBM of the lower-case name `ZoneStreamEnable` finds it by
/// 0x64  u32  0
/// 0x68  u32  hash of a linked cell stream, or 0xFE16702F
/// 0x6C  u32  offset of the list of needed streams in the pool, or 0
/// 0x70  u32  offset of the next cell of the ring
/// 0x74  12   not read by the game
/// ```
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Cell {
    pub guid: Guid,
    pub stream_hash: Hash32,
    pub centre: [f32; 3],
    pub radius: f32,
    #[serde(skip)]
    pub unused_24: [u8; 12],
    pub box_min: [f32; 4],
    pub box_max: [f32; 4],
    pub kind: u8,
    pub flags: u8,
    pub lod_mask: u32,
    pub min_distance: f32,
    pub max_distance: f32,
    pub name_hash: Hash32,
    pub reserved_64: u32,
    pub linked_stream: Hash32,
    /// Streams the cell needs: `count` is the u16 at 0x52.
    pub needs: Option<PoolList>,
    /// Index of the next cell of the ring; the cell itself when alone.
    pub ring_next: u32,
    #[serde(skip)]
    pub unused_74: [u8; 12],
}

impl Cell {
    /// Whether the cell has a bounding box.
    pub fn has_box(&self) -> bool {
        (0..3).all(|k| self.box_min[k] <= self.box_max[k])
    }
}

/// A cell for [`WorldGrid::add_cell`].
#[derive(Debug, Clone, PartialEq)]
pub struct NewCell {
    /// GUID of the stream; its fold must be the hash of the stream in the
    /// tables of contents.
    pub guid: [u8; 16],
    pub box_min: [f32; 3],
    pub box_max: [f32; 3],
    pub kind: u8,
    pub flags: u8,
    pub lod_mask: u32,
    /// Name for `ZoneStreamEnable`; hashed in lower case.
    pub name: Option<String>,
    /// Hash of the stream the cell needs (its parent); it must be in the
    /// stream table.
    pub needs: Option<u32>,
}

/// A parsed world grid.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WorldGrid {
    pub streams: Vec<GridStream>,
    pub cells: Vec<Cell>,
    /// Indices into [`Self::streams`].
    pub pool: Vec<u32>,
}

fn bad(offset: usize, reason: impl Into<String>) -> Error {
    Error::malformed(WHAT, offset, reason)
}

fn f32_at(data: &[u8], offset: usize) -> Result<f32> {
    Ok(f32::from_bits(io::read_u32(data, offset, Endian::Little)?))
}

fn bytes<const N: usize>(data: &[u8], offset: usize) -> Result<[u8; N]> {
    Ok(io::slice(data, offset, N)?
        .try_into()
        .expect("slice of N bytes"))
}

impl WorldGrid {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let le = Endian::Little;
        let u32_at = |offset| io::read_u32(data, offset, le).map(|v| v as usize);
        if u32_at(0)? != TYPE_WGI as usize {
            return Err(bad(0, "not a world grid"));
        }
        let version = u32_at(4)? as u32;
        if version != VERSION {
            return Err(Error::UnsupportedVersion {
                what: WHAT,
                version,
            });
        }
        let (streams_at, stream_count) = (u32_at(0x08)?, u32_at(0x0C)?);
        let (cells_at, cell_count) = (u32_at(0x10)?, u32_at(0x14)?);
        let (pool_at, pool_count) = (u32_at(0x18)?, u32_at(0x1C)?);
        let section = |count: usize, size: usize, start: usize| {
            count
                .checked_mul(size)
                .and_then(|n| n.checked_add(start))
                .ok_or_else(|| bad(0x08, "section size overflows"))
        };
        if streams_at != HEADER_SIZE
            || cells_at != section(stream_count, STREAM_SIZE, streams_at)?
            || pool_at != section(cell_count, CELL_SIZE, cells_at)?
            || data.len() != section(pool_count, 4, pool_at)?
        {
            return Err(bad(0x08, "sections are not contiguous"));
        }
        // A list of `count` words at `offset`, both zero for no list.
        let list = |at: usize, offset: usize, count: usize| -> Result<Option<PoolList>> {
            if count == 0 && offset == 0 {
                return Ok(None);
            }
            let inside = offset >= pool_at
                && (offset - pool_at).is_multiple_of(4)
                && count > 0
                && (offset - pool_at) / 4 + count <= pool_count;
            if !inside {
                return Err(bad(at, "list outside the pool"));
            }
            Ok(Some(PoolList {
                start: ((offset - pool_at) / 4) as u32,
                count: count as u32,
            }))
        };
        let streams = (0..stream_count)
            .map(|k| {
                let at = streams_at + STREAM_SIZE * k;
                Ok(GridStream {
                    guid: Guid(bytes(data, at)?),
                    needs: list(at + 0x10, u32_at(at + 0x14)?, u32_at(at + 0x10)?)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let cells = (0..cell_count)
            .map(|k| {
                let at = cells_at + CELL_SIZE * k;
                let vec4 = |o: usize| -> Result<[f32; 4]> {
                    Ok([
                        f32_at(data, at + o)?,
                        f32_at(data, at + o + 4)?,
                        f32_at(data, at + o + 8)?,
                        f32_at(data, at + o + 12)?,
                    ])
                };
                let centre = vec4(0x14)?;
                let next = u32_at(at + 0x70)?;
                if next < cells_at
                    || next >= pool_at
                    || !(next - cells_at).is_multiple_of(CELL_SIZE)
                {
                    return Err(bad(at + 0x70, "ring link is not a cell"));
                }
                Ok(Cell {
                    guid: Guid(bytes(data, at)?),
                    stream_hash: Hash32(io::read_u32(data, at + 0x10, le)?),
                    centre: [centre[0], centre[1], centre[2]],
                    radius: centre[3],
                    unused_24: bytes(data, at + 0x24)?,
                    box_min: vec4(0x30)?,
                    box_max: vec4(0x40)?,
                    kind: data[at + 0x50],
                    flags: data[at + 0x51],
                    lod_mask: io::read_u32(data, at + 0x54, le)?,
                    min_distance: f32_at(data, at + 0x58)?,
                    max_distance: f32_at(data, at + 0x5C)?,
                    name_hash: Hash32(io::read_u32(data, at + 0x60, le)?),
                    reserved_64: io::read_u32(data, at + 0x64, le)?,
                    linked_stream: Hash32(io::read_u32(data, at + 0x68, le)?),
                    needs: list(
                        at + 0x6C,
                        u32_at(at + 0x6C)?,
                        usize::from(io::read_u16(data, at + 0x52, le)?),
                    )?,
                    ring_next: ((next - cells_at) / CELL_SIZE) as u32,
                    unused_74: bytes(data, at + 0x74)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let pool = (0..pool_count)
            .map(|k| {
                let at = pool_at + 4 * k;
                let target = u32_at(at)?;
                if target < streams_at
                    || target >= cells_at
                    || !(target - streams_at).is_multiple_of(STREAM_SIZE)
                {
                    return Err(bad(at, "pool word is not a stream entry"));
                }
                Ok(((target - streams_at) / STREAM_SIZE) as u32)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            streams,
            cells,
            pool,
        })
    }

    /// Writes the grid; a parsed grid is written back byte for byte.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let le = Endian::Little;
        let cells_at = HEADER_SIZE + STREAM_SIZE * self.streams.len();
        let pool_at = cells_at + CELL_SIZE * self.cells.len();
        let size = pool_at + 4 * self.pool.len();
        let word = |value: usize| {
            u32::try_from(value).map_err(|_| Error::InvalidArgument("wgi exceeds 4 GiB".into()))
        };
        word(size)?;
        let list = |needs: &Option<PoolList>| -> Result<(u32, u32)> {
            match needs {
                None => Ok((0, 0)),
                Some(l) => {
                    let end = l.start as usize + l.count as usize;
                    if l.count == 0 || end > self.pool.len() {
                        return Err(Error::InvalidArgument("wgi list outside the pool".into()));
                    }
                    Ok((l.count, word(pool_at + 4 * l.start as usize)?))
                }
            }
        };
        let mut out = Vec::with_capacity(size);
        for value in [
            TYPE_WGI as usize,
            VERSION as usize,
            HEADER_SIZE,
            self.streams.len(),
            cells_at,
            self.cells.len(),
            pool_at,
            self.pool.len(),
        ] {
            io::put_u32(&mut out, word(value)?, le);
        }
        for stream in &self.streams {
            let (count, offset) = list(&stream.needs)?;
            out.extend_from_slice(&stream.guid.0);
            io::put_u32(&mut out, count, le);
            io::put_u32(&mut out, offset, le);
        }
        for cell in &self.cells {
            let (count, offset) = list(&cell.needs)?;
            let count = u16::try_from(count)
                .map_err(|_| Error::InvalidArgument("wgi cell list is too long".into()))?;
            if cell.ring_next as usize >= self.cells.len() {
                return Err(Error::InvalidArgument("wgi ring link is not a cell".into()));
            }
            out.extend_from_slice(&cell.guid.0);
            io::put_u32(&mut out, cell.stream_hash.0, le);
            for value in cell.centre.iter().chain([&cell.radius]) {
                io::put_u32(&mut out, value.to_bits(), le);
            }
            out.extend_from_slice(&cell.unused_24);
            for value in cell.box_min.iter().chain(&cell.box_max) {
                io::put_u32(&mut out, value.to_bits(), le);
            }
            out.push(cell.kind);
            out.push(cell.flags);
            io::put_u16(&mut out, count, le);
            io::put_u32(&mut out, cell.lod_mask, le);
            io::put_u32(&mut out, cell.min_distance.to_bits(), le);
            io::put_u32(&mut out, cell.max_distance.to_bits(), le);
            io::put_u32(&mut out, cell.name_hash.0, le);
            io::put_u32(&mut out, cell.reserved_64, le);
            io::put_u32(&mut out, cell.linked_stream.0, le);
            io::put_u32(&mut out, offset, le);
            io::put_u32(
                &mut out,
                word(cells_at + CELL_SIZE * cell.ring_next as usize)?,
                le,
            );
            out.extend_from_slice(&cell.unused_74);
        }
        for &stream in &self.pool {
            if stream as usize >= self.streams.len() {
                return Err(Error::InvalidArgument(
                    "wgi pool word is not a stream".into(),
                ));
            }
            io::put_u32(
                &mut out,
                word(HEADER_SIZE + STREAM_SIZE * stream as usize)?,
                le,
            );
        }
        Ok(out)
    }

    /// Index of the stream-table entry whose GUID folds to `hash`.
    pub fn find_stream(&self, hash: u32) -> Option<usize> {
        self.streams.iter().position(|s| s.hash() == hash)
    }

    /// Index of the cell of the stream with table-of-contents hash `hash`.
    pub fn find_cell(&self, hash: u32) -> Option<usize> {
        self.cells.iter().position(|c| c.stream_hash.0 == hash)
    }

    /// The streams of a pool list, as indices into [`Self::streams`].
    pub fn list(&self, needs: &Option<PoolList>) -> &[u32] {
        needs.map_or(&[], |l| {
            &self.pool[l.start as usize..(l.start + l.count) as usize]
        })
    }

    /// The most common level-of-detail mask of the world cells
    /// ([`KIND_WORLD`]): what the partition of this grid is for.
    pub fn main_lod_mask(&self) -> Option<u32> {
        let mut counts = std::collections::BTreeMap::<u32, usize>::new();
        for cell in self.cells.iter().filter(|c| c.kind == KIND_WORLD) {
            *counts.entry(cell.lod_mask).or_default() += 1;
        }
        counts.into_iter().max_by_key(|&(_, n)| n).map(|(m, _)| m)
    }

    /// Appends a cell in a ring of its own. The centre is the centre of the
    /// box and the radius half its diagonal. Returns the index of the cell.
    pub fn add_cell(&mut self, new: &NewCell) -> Result<usize> {
        let hash = crate::toc::guid_hash(&new.guid);
        if self.find_cell(hash).is_some() {
            return Err(Error::InvalidArgument(format!(
                "the grid already has a cell of stream {hash:08x}"
            )));
        }
        if !(0..3).all(|k| new.box_min[k] <= new.box_max[k]) {
            return Err(Error::InvalidArgument(
                "the box of a cell needs minimum <= maximum".into(),
            ));
        }
        let needs = match new.needs {
            None => None,
            Some(parent) => {
                let stream = self.find_stream(parent).ok_or_else(|| {
                    Error::InvalidArgument(format!(
                        "stream {parent:08x} is not in the stream table of the grid"
                    ))
                })? as u32;
                let start = match self.pool.iter().position(|&s| s == stream) {
                    Some(at) => at,
                    None => {
                        self.pool.push(stream);
                        self.pool.len() - 1
                    }
                };
                Some(PoolList {
                    start: start as u32,
                    count: 1,
                })
            }
        };
        let (min, max) = (new.box_min, new.box_max);
        let centre = [0, 1, 2].map(|k| (min[k] + max[k]) / 2.0);
        let radius = (0..3)
            .map(|k| ((max[k] - min[k]) / 2.0).powi(2))
            .sum::<f32>()
            .sqrt();
        let index = self.cells.len();
        self.cells.push(Cell {
            guid: Guid(new.guid),
            stream_hash: Hash32(hash),
            centre,
            radius,
            unused_24: [0; 12],
            box_min: [min[0], min[1], min[2], 1.0],
            box_max: [max[0], max[1], max[2], 1.0],
            kind: new.kind,
            flags: new.flags,
            lod_mask: new.lod_mask,
            min_distance: 0.0,
            max_distance: 0.0,
            name_hash: Hash32(
                new.name
                    .as_deref()
                    .map_or(0, |n| crate::hash::sdbm(n.to_ascii_lowercase().as_bytes())),
            ),
            reserved_64: 0,
            linked_stream: Hash32(NO_STREAM),
            needs,
            ring_next: index as u32,
            unused_74: [0; 12],
        });
        Ok(index)
    }

    /// Removes cell `index`, taking it out of its ring. Pool words stay.
    pub fn remove_cell(&mut self, index: usize) -> Result<Cell> {
        if index >= self.cells.len() {
            return Err(Error::InvalidArgument(format!("no cell {index}")));
        }
        let next = self.cells[index].ring_next;
        for cell in &mut self.cells {
            if cell.ring_next as usize == index {
                cell.ring_next = next;
            }
        }
        let removed = self.cells.remove(index);
        for cell in &mut self.cells {
            if cell.ring_next as usize > index {
                cell.ring_next -= 1;
            }
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guid(seed: u8) -> [u8; 16] {
        let mut g = [0u8; 16];
        g[0] = seed;
        g[8..].copy_from_slice(b"XDOCTORT");
        g
    }

    fn sample() -> WorldGrid {
        WorldGrid {
            streams: vec![
                GridStream {
                    guid: Guid(guid(1)),
                    needs: None,
                },
                GridStream {
                    guid: Guid(guid(2)),
                    needs: Some(PoolList { start: 0, count: 1 }),
                },
            ],
            cells: Vec::new(),
            pool: vec![0],
        }
    }

    fn new_cell(seed: u8, needs: Option<u32>) -> NewCell {
        NewCell {
            guid: guid(seed),
            box_min: [-10.0, 0.0, -20.0],
            box_max: [10.0, 4.0, 20.0],
            kind: KIND_WORLD,
            flags: 0,
            lod_mask: lod::HIGH,
            name: Some("Doctor_Zone".into()),
            needs,
        }
    }

    #[test]
    fn a_grid_with_new_cells_round_trips() {
        let mut grid = sample();
        let parent = grid.streams[0].hash();
        let other = grid.streams[1].hash();
        assert_eq!(grid.add_cell(&new_cell(10, Some(parent))).unwrap(), 0);
        assert_eq!(grid.add_cell(&new_cell(11, Some(other))).unwrap(), 1);
        assert_eq!(grid.add_cell(&new_cell(12, None)).unwrap(), 2);
        // The parent's pool word is reused, the other stream gets a new one.
        assert_eq!(grid.pool, [0, 1]);
        let cell = &grid.cells[0];
        assert_eq!(cell.centre, [0.0, 2.0, 0.0]);
        assert!((cell.radius - 504f32.sqrt()).abs() < 1e-4);
        assert_eq!(cell.stream_hash.0, crate::toc::guid_hash(&guid(10)));
        assert_eq!(cell.name_hash.0, crate::hash::sdbm(b"doctor_zone"));
        assert_eq!(cell.ring_next, 0);
        let bytes = grid.to_bytes().unwrap();
        assert_eq!(bytes.len(), 0x20 + 2 * 24 + 3 * 0x80 + 2 * 4);
        let again = WorldGrid::parse(&bytes).unwrap();
        assert_eq!(again, grid);
        assert_eq!(again.to_bytes().unwrap(), bytes);
        assert_eq!(again.list(&again.cells[1].needs), [1]);
        assert_eq!(again.main_lod_mask(), Some(lod::HIGH));
    }

    #[test]
    fn add_cell_rejects_duplicates_and_unknown_parents() {
        let mut grid = sample();
        grid.add_cell(&new_cell(10, None)).unwrap();
        assert!(grid.add_cell(&new_cell(10, None)).is_err());
        assert!(grid.add_cell(&new_cell(11, Some(0x1234_5678))).is_err());
        let mut flat = new_cell(12, None);
        flat.box_min[0] = 50.0;
        assert!(grid.add_cell(&flat).is_err());
    }

    #[test]
    fn remove_cell_mends_the_ring() {
        let mut grid = sample();
        for seed in 10..13 {
            grid.add_cell(&new_cell(seed, None)).unwrap();
        }
        // Ring 0 -> 1 -> 2 -> 0.
        grid.cells[0].ring_next = 1;
        grid.cells[1].ring_next = 2;
        grid.cells[2].ring_next = 0;
        let removed = grid.remove_cell(1).unwrap();
        assert_eq!(removed.guid.0, guid(11));
        assert_eq!(grid.cells[0].ring_next, 1);
        assert_eq!(grid.cells[1].ring_next, 0);
        WorldGrid::parse(&grid.to_bytes().unwrap()).unwrap();
        assert!(grid.remove_cell(5).is_err());
    }

    #[test]
    fn parse_rejects_broken_layouts() {
        let mut grid = sample();
        grid.add_cell(&new_cell(10, None)).unwrap();
        let bytes = grid.to_bytes().unwrap();
        assert!(WorldGrid::parse(&bytes[..bytes.len() - 1]).is_err());
        let mut wrong = bytes.clone();
        wrong[4] = 6;
        assert!(matches!(
            WorldGrid::parse(&wrong),
            Err(Error::UnsupportedVersion { version: 6, .. })
        ));
        let mut wrong = bytes.clone();
        // The ring link of the cell: outside the cells.
        let at = 0x20 + 2 * 24 + 0x70;
        wrong[at..at + 4].copy_from_slice(&4u32.to_le_bytes());
        assert!(WorldGrid::parse(&wrong).is_err());
        let mut wrong = bytes;
        // The pool word: not a stream entry.
        let at = wrong.len() - 4;
        wrong[at..].copy_from_slice(&0x21u32.to_le_bytes());
        assert!(WorldGrid::parse(&wrong).is_err());
    }
}

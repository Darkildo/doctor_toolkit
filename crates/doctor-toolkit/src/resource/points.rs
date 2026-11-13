//! The type `covernodetable` (`0xDF14DA75`): tables of placed points of a level.
//!
//! One resource per stream (682 in the game, 52 162 points, in terrain, filler and
//! objective streams). Each record is a point with a GUID of the author's
//! machine (`ANDAYA68`), an orientation and a position in world coordinates,
//! followed by two cosines (35° and 10–45°), a radius of 25 and a squared
//! distance (40², 60², 90²), a second position 1.3 above the first and a
//! kind (1, 2, 4 or 8). None of the GUIDs is a SimGroup object, metamodel
//! part or state; nothing refers to a model. They are most likely AI points
//! (cover or combat positions with view cones); the level view shows them as
//! markers. See `docs/formats/logic-types.md` §4.
//!
//! # Layout
//!
//! All values are little-endian.
//!
//! ```text
//! 0x00  u32  2
//! 0x04  u32  record count N
//! 0x08  u32  small count (meaning unknown)
//! 0x14  u32, u32  small count and a hash (meaning unknown)
//! 0x28  N records:
//!       16 bytes  prefix: u32 0, u32 0, u32 k, u32; when k != 0 a
//!                 28-byte block follows (links, not decoded)
//!       0x7C bytes body:
//!         0x00 16 bytes  GUID
//!         0x10 9 × f32   rotation, rows right, up, at
//!         0x34 3 × f32   position (Y up)
//!         0x40 2 × f32   cosines
//!         0x48 2 × f32   radius, squared distance
//!         0x50 8 bytes
//!         0x58 2 × 3 × f32  positions
//!         0x70 2 × f32
//!         0x78 u16 kind (1, 2, 4, 8; 0x2000 set in 21), u16 10
//! ```
//!
//! The walk is checked on every resource of the game: it finds exactly N
//! records, every GUID ends in a machine name, and it ends at the end of
//! the data (`tests/corpus.rs`, `every_point_table_parses`).

use serde::Serialize;

use crate::io::{self, Endian};
use crate::resource::simgroup::Guid;
use crate::{Error, Result};

/// Type id of the point tables, `covernodetable` (`0xDF14DA75`).
pub const TYPE_POINTS: u32 = crate::hash::sdbm(b"covernodetable");
const HEADER_SIZE: usize = 0x28;
const PREFIX_SIZE: usize = 16;
const LINK_BLOCK_SIZE: usize = 28;
const BODY_SIZE: usize = 0x7C;

/// One point.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Point {
    pub guid: Guid,
    /// Rows right, up, at.
    pub rotation: [[f32; 3]; 3],
    pub position: [f32; 3],
    /// Kind: 1, 2, 4 or 8 in the game (21 points also have bit 0x2000).
    pub kind: u16,
    /// Offset of the record body in the resource.
    #[serde(skip)]
    pub offset: usize,
}

impl Point {
    /// The world matrix, row-major like a SimGroup transform.
    pub fn matrix(&self) -> [[f32; 4]; 4] {
        let r = &self.rotation;
        [
            [r[0][0], r[0][1], r[0][2], 0.0],
            [r[1][0], r[1][1], r[1][2], 0.0],
            [r[2][0], r[2][1], r[2][2], 0.0],
            [self.position[0], self.position[1], self.position[2], 1.0],
        ]
    }
}

/// A parsed point table.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PointTable {
    pub points: Vec<Point>,
}

impl PointTable {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let le = Endian::Little;
        let version = io::read_u32(data, 0, le)?;
        if version != 2 {
            return Err(Error::UnsupportedVersion {
                what: "point table",
                version,
            });
        }
        let count = io::read_u32(data, 4, le)? as usize;
        // Each record takes at least the prefix and the body.
        if count > data.len() / (PREFIX_SIZE + BODY_SIZE) {
            return Err(Error::malformed("point table", 4, "too many records"));
        }
        let f32_at = |o: usize| io::read_u32(data, o, le).map(f32::from_bits);
        let mut points = Vec::with_capacity(count);
        let mut at = HEADER_SIZE;
        for _ in 0..count {
            let links = io::read_u32(data, at + 8, le)?;
            at += PREFIX_SIZE;
            if links != 0 {
                at += LINK_BLOCK_SIZE;
            }
            let body = io::slice(data, at, BODY_SIZE)?;
            let guid = Guid(body[..16].try_into().expect("16 bytes"));
            let mut rotation = [[0f32; 3]; 3];
            for (r, row) in rotation.iter_mut().enumerate() {
                for (c, value) in row.iter_mut().enumerate() {
                    *value = f32_at(at + 0x10 + 4 * (3 * r + c))?;
                }
            }
            let position = [f32_at(at + 0x34)?, f32_at(at + 0x38)?, f32_at(at + 0x3C)?];
            let kind = io::read_u16(data, at + 0x78, le)?;
            points.push(Point {
                guid,
                rotation,
                position,
                kind,
                offset: at,
            });
            at += BODY_SIZE;
        }
        if at != data.len() {
            return Err(Error::malformed(
                "point table",
                at,
                "data after the last record",
            ));
        }
        Ok(Self { points })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(out: &mut Vec<u8>, links: bool, x: f32, kind: u16) {
        out.extend_from_slice(&[0; 8]);
        out.extend_from_slice(&(links as u32 * 3).to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        if links {
            out.extend_from_slice(&[0; LINK_BLOCK_SIZE]);
        }
        let start = out.len();
        out.extend_from_slice(b"\x01\x02\x03\x04\x05\x06\x07\x08ANDAYA68");
        for v in [1.0f32, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, x, 2.0, 3.0] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.resize(start + 0x78, 0);
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(&10u16.to_le_bytes());
    }

    #[test]
    fn walks_records_with_and_without_link_blocks() {
        let mut data = vec![0u8; HEADER_SIZE];
        data[0] = 2;
        data[4] = 3;
        record(&mut data, false, 1.0, 1);
        record(&mut data, true, 5.0, 4);
        record(&mut data, false, 9.0, 8);
        let table = PointTable::parse(&data).unwrap();
        assert_eq!(table.points.len(), 3);
        assert_eq!(table.points[1].position, [5.0, 2.0, 3.0]);
        assert_eq!(table.points[2].kind, 8);
        assert_eq!(&table.points[0].guid.0[8..], b"ANDAYA68");
        assert_eq!(table.points[0].matrix()[3], [1.0, 2.0, 3.0, 1.0]);
        // Truncated or padded data is refused.
        assert!(PointTable::parse(&data[..data.len() - 1]).is_err());
        let mut longer = data.clone();
        longer.push(0);
        assert!(PointTable::parse(&longer).is_err());
        data[4] = 0xFF;
        assert!(PointTable::parse(&data).is_err());
    }
}

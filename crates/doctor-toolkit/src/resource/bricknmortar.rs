//! The `bricknmortarres` resource: the venues (businesses, rackets, family
//! compounds) of a city. Every city root stream (`gf2/queens`, `gf2/miami`,
//! `gf2/cuba`) and the multiplayer maps hold one.
//!
//! ```text
//! 0x00  u32  16
//! 0x04  u32  number of venues N
//! 0x08  N × 0x70 bytes:
//!       +0x00 16 bytes  GUID of the venue (`Building.5` of its building)
//!       +0x10 u32  owner: sdbm of the lower-case family name
//!                  (`rosato_carmine` 529e27ee), 0 for none
//!       +0x14 i32  venue number (`Building.0`, the `venue` value list)
//!       +0x18 u32  city: sdbm of `newyork`, `florida`, `cuba`
//!       +0x1C u32  flags (`Building.2`)
//!       +0x20 i32  type 0..9 (`Building.1`)
//!       +0x24 u32  0, 4 or 8; the game keeps it at +0x50 of the venue
//!       +0x28 u32  20 (50 in one venue of Havana); the loader does not read it
//!       +0x2C 3 × f32  position of the building
//!       +0x38 3 × f32  a second point
//!       +0x44 3 × f32  a third point
//!       +0x50 2 × f32  8 and 22 in every venue (`Building.39`, `Building.40`)
//!       +0x58 6 × u32  offsets from the start of the resource of the names
//!                      of the Don's View effects (`dv_dons_light`,
//!                      `dv_building_fire`, `dv_building_embattled`,
//!                      `dv_building_repairing`, `dv_venue_closed`,
//!                      `dv_building_bombers`; `Building.6`..`11`); 0: none
//! 0x08 + N × 0x70: the names, zero-terminated, the six of every venue in
//!                  the order of the venues
//! ```
//!
//! `godfather2.exe` 1.0.789.0: the manager `BricknMortarRes` (constructor
//! `0x006B01E0`) turns the offsets into pointers (`0x006B1000`) and makes one
//! venue of 0x1A0 bytes of every record (`0x006AF5E0` → `0x006B2670`); the
//! name on screen is the string `building_name_%03u` of the venue number.
//! A `Building` object finds its venue by number when its stream loads
//! (`0x006AE7C0` → `0x006B0EE0`) and uses it without a check
//! (`0x006B5B80`): **a building with a number that no record has crashes
//! the game** (read of `+0x16C` of a null venue, `0x006B5B87`).
//!
//! [`VenueTable::to_bytes`] reproduces the resources of the game byte for
//! byte.

use serde::{Deserialize, Serialize};

use crate::hash::sdbm;
use crate::io::{self, Endian};
use crate::{Error, Result};

/// Type id of `bricknmortarres` resources.
pub const TYPE_BRICKNMORTARRES: u32 = sdbm(b"bricknmortarres");

const RECORD_SIZE: usize = 0x70;
const HEADER: u32 = 16;
/// Number of effect names of a venue.
pub const EFFECTS: usize = 6;

/// One venue.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Venue {
    /// GUID as 32 hexadecimal digits.
    pub guid: String,
    /// sdbm of the lower-case name of the owning family; 0 for none.
    pub owner: u32,
    /// Venue number.
    pub id: i32,
    /// sdbm of the city name.
    pub city: u32,
    pub flags: u32,
    /// Type 0..9 (`Building.1`).
    pub kind: i32,
    /// 0, 4 or 8; meaning not established.
    pub unknown_24: u32,
    /// 20 in almost every venue; the loader does not read it.
    pub unknown_28: u32,
    pub position: [f32; 3],
    pub point_b: [f32; 3],
    pub point_c: [f32; 3],
    /// 8 and 22 in every venue of the game.
    pub values: [f32; 2],
    /// Names of the Don's View effects; `None` for an offset of 0.
    pub effects: [Option<String>; EFFECTS],
}

/// The parsed `bricknmortarres` resource.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VenueTable {
    pub venues: Vec<Venue>,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl VenueTable {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let le = Endian::Little;
        let header = io::read_u32(data, 0, le)?;
        if header != HEADER {
            return Err(Error::malformed(
                "bricknmortarres",
                0,
                format!("header {header}"),
            ));
        }
        let n = io::read_u32(data, 4, le)? as usize;
        let records = io::slice(data, 8, n.saturating_mul(RECORD_SIZE))?;
        let mut venues = Vec::with_capacity(n);
        for (i, r) in records.chunks_exact(RECORD_SIZE).enumerate() {
            let at = 8 + i * RECORD_SIZE;
            let w = |k: usize| u32::from_le_bytes([r[k], r[k + 1], r[k + 2], r[k + 3]]);
            let f = |k: usize| f32::from_bits(w(k));
            let v3 = |k: usize| [f(k), f(k + 4), f(k + 8)];
            let mut effects: [Option<String>; EFFECTS] = Default::default();
            for (e, slot) in effects.iter_mut().enumerate() {
                let offset = w(0x58 + 4 * e) as usize;
                if offset == 0 {
                    continue;
                }
                let text = io::cstr(data, offset).map_err(|_| {
                    Error::malformed(
                        "bricknmortarres",
                        at + 0x58 + 4 * e,
                        "name out of the resource",
                    )
                })?;
                *slot = Some(String::from_utf8_lossy(text).into_owned());
            }
            venues.push(Venue {
                guid: hex(&r[..16]),
                owner: w(0x10),
                id: w(0x14) as i32,
                city: w(0x18),
                flags: w(0x1C),
                kind: w(0x20) as i32,
                unknown_24: w(0x24),
                unknown_28: w(0x28),
                position: v3(0x2C),
                point_b: v3(0x38),
                point_c: v3(0x44),
                values: [f(0x50), f(0x54)],
                effects,
            });
        }
        Ok(Self { venues })
    }

    /// Serialises the table in the game's layout: the records, then the six
    /// names of every venue in their order.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let le = Endian::Little;
        let n = self.venues.len();
        let mut out = Vec::with_capacity(8 + n * (RECORD_SIZE + 100));
        io::put_u32(&mut out, HEADER, le);
        io::put_u32(&mut out, n as u32, le);
        out.resize(8 + n * RECORD_SIZE, 0);
        for (i, v) in self.venues.iter().enumerate() {
            let guid = crate::create::parse_guid(&v.guid)?;
            let mut r = Vec::with_capacity(RECORD_SIZE);
            r.extend_from_slice(&guid);
            for word in [
                v.owner,
                v.id as u32,
                v.city,
                v.flags,
                v.kind as u32,
                v.unknown_24,
                v.unknown_28,
            ] {
                io::put_u32(&mut r, word, le);
            }
            for x in v
                .position
                .iter()
                .chain(&v.point_b)
                .chain(&v.point_c)
                .chain(&v.values)
            {
                io::put_u32(&mut r, x.to_bits(), le);
            }
            for name in &v.effects {
                match name {
                    None => io::put_u32(&mut r, 0, le),
                    Some(text) => {
                        if text.as_bytes().contains(&0) {
                            return Err(Error::InvalidArgument(format!(
                                "venue {}: an effect name holds a zero byte",
                                v.id
                            )));
                        }
                        io::put_u32(&mut r, out.len() as u32, le);
                        out.extend_from_slice(text.as_bytes());
                        out.push(0);
                    }
                }
            }
            debug_assert_eq!(r.len(), RECORD_SIZE);
            out[8 + i * RECORD_SIZE..8 + (i + 1) * RECORD_SIZE].copy_from_slice(&r);
        }
        Ok(out)
    }

    /// The venue with number `id`.
    pub fn find(&self, id: i32) -> Option<&Venue> {
        self.venues.iter().find(|v| v.id == id)
    }

    /// Mutable access to the venue with number `id`.
    pub fn find_mut(&mut self, id: i32) -> Option<&mut Venue> {
        self.venues.iter_mut().find(|v| v.id == id)
    }

    /// Problems the game would trip over: repeated numbers or GUIDs (the
    /// zero GUID, which several venues of the game have, is not counted).
    pub fn validate(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let zero = |g: &str| g.bytes().all(|c| c == b'0');
        for (i, a) in self.venues.iter().enumerate() {
            for b in &self.venues[i + 1..] {
                if a.id == b.id {
                    problems.push(format!("two venues have the number {}", a.id));
                }
                if !zero(&a.guid) && a.guid.eq_ignore_ascii_case(&b.guid) {
                    problems.push(format!(
                        "venues {} and {} have the same GUID {}",
                        a.id, b.id, a.guid
                    ));
                }
            }
        }
        problems
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn venue(id: i32) -> Venue {
        Venue {
            guid: format!("931bc932{id:08x}52454953444f5246"),
            owner: sdbm(b"rosato_carmine"),
            id,
            city: sdbm(b"newyork"),
            flags: 0,
            kind: 9,
            unknown_24: 0,
            unknown_28: 20,
            position: [-92.4, 6.2, -255.1],
            point_b: [1.0, 2.0, 3.0],
            point_c: [4.0, 5.0, 6.0],
            values: [8.0, 22.0],
            effects: [
                Some("dv_dons_light".into()),
                Some("dv_building_fire".into()),
                None,
                Some("dv_building_repairing".into()),
                Some("dv_venue_closed".into()),
                Some("dv_building_bombers".into()),
            ],
        }
    }

    #[test]
    fn round_trips_with_names_after_the_records() {
        let table = VenueTable {
            venues: vec![venue(202), venue(230)],
        };
        assert!(table.validate().is_empty());
        let bytes = table.to_bytes().unwrap();
        assert_eq!(&bytes[..8], &[16, 0, 0, 0, 2, 0, 0, 0]);
        // The first name follows the two records.
        let first = u32::from_le_bytes(bytes[8 + 0x58..8 + 0x5C].try_into().unwrap());
        assert_eq!(first as usize, 8 + 2 * RECORD_SIZE);
        // A missing name is offset 0.
        assert_eq!(&bytes[8 + 0x60..8 + 0x64], &[0, 0, 0, 0]);
        let back = VenueTable::parse(&bytes).unwrap();
        assert_eq!(back, table);
        assert_eq!(back.to_bytes().unwrap(), bytes);
        assert_eq!(back.find(230).unwrap().kind, 9);
        assert_eq!(sdbm(b"rosato_carmine"), 0x529e27ee);
        assert_eq!(TYPE_BRICKNMORTARRES, 0xa31ff01e);
    }

    #[test]
    fn rejects_bad_data_and_reports_repeats() {
        let table = VenueTable {
            venues: vec![venue(202)],
        };
        let bytes = table.to_bytes().unwrap();
        assert!(VenueTable::parse(&bytes[..0x40]).is_err());
        let mut bad = bytes.clone();
        bad[0] = 17;
        assert!(VenueTable::parse(&bad).is_err());
        let mut far = bytes;
        far[8 + 0x58..8 + 0x5C].copy_from_slice(&0xFFFFu32.to_le_bytes());
        assert!(VenueTable::parse(&far).is_err());
        let twice = VenueTable {
            venues: vec![venue(202), venue(202)],
        };
        assert_eq!(twice.validate().len(), 2);
    }
}

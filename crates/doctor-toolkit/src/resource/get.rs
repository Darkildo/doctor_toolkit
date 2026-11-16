//! The `get` resource: the table of speech events of the whole game. A
//! record tells when an event may be said (cool-downs, groups, flags) and how
//! important it is; the lines themselves are in the `chv` of the speaker.
//!
//! ```text
//! 0x00  u16  version = 3
//! 0x02  u16  number of records N
//! 0x04  u32  offset of the records = 8
//! 0x08  N × 36 bytes, sorted by hash (unsigned):
//!       +0x00 u32 sdbm(lower(event name))
//!       +0x04 f32 1.0 in most records
//!       +0x08 f32 cool-down in seconds: the event is not said again before
//!                 it passes
//!       +0x0C f32 a second time in seconds, -1 when there is none
//!       +0x10 u32 1 in most records
//!       +0x14 f32 100 in most records (90..170)
//!       +0x18 i32 rank, 0..4 (4 in most records)
//!       +0x1C u32 flags; bits 12..19 are groups: a said event marks its
//!                 groups (`0x006F58D0`), and an event whose group is marked
//!                 is not said (`0x006F5940`)
//!       +0x20 u32 priority (300, 356, 400…); the game adds its low
//!                 byte to the priority of the line (`0x006F5530`)
//! ```
//!
//! The fields at 0x0C..0x18 are read for a fixed list of events
//! (`0x007E85B0`); what they mean is not established.
//!
//! `godfather2.exe` 1.0.789.0: loader `0x006F5FA0` (version check), binary
//! search `0x006F5D80` behind `0x006F5DD0`. An event that is not in the
//! table is never said through the common path (`0x006F3EB0` returns at
//! once), and the line player reads byte 0x20 of the record without a check
//! (`0x006F5530`): **a new event of a `chv` needs a record here**.
//!
//! [`SpeechEventTable::to_bytes`] reproduces the resource byte for byte.

use serde::{Deserialize, Serialize};

use crate::hash::{sdbm, string_id};
use crate::io::{self, Endian};
use crate::{Error, Result};

/// Type id of `get` resources.
pub const TYPE_GET: u32 = sdbm(b"get");

const VERSION: u16 = 3;
const RECORD_SIZE: usize = 36;

/// The cool-down starts when the event is asked for, not when it is said.
pub const FLAG_COOLDOWN_ON_REQUEST: u32 = 0x80;
/// Bits of the groups of an event.
pub const GROUP_MASK: u32 = 0xF_F000;

/// The parameters of one speech event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpeechEvent {
    /// `sdbm` of the lower-case event name.
    pub hash: u32,
    /// 1.0 in most records.
    pub weight: f32,
    /// Seconds before the event may be said again.
    pub cooldown: f32,
    /// A second time in seconds; −1 in most records.
    pub second: f32,
    /// 1 in most records.
    pub count: u32,
    /// 100 in most records.
    pub range: f32,
    pub rank: i32,
    pub flags: u32,
    /// The game adds the low byte to the priority of the line.
    pub priority: u32,
}

impl SpeechEvent {
    /// A record with the most common values of the game for a new event.
    pub fn new(name: &str) -> Self {
        Self {
            hash: string_id(name),
            weight: 1.0,
            cooldown: 1.0,
            second: -1.0,
            count: 1,
            range: 100.0,
            rank: 4,
            flags: 0,
            priority: 356,
        }
    }
}

/// The parsed `get` resource.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpeechEventTable {
    pub version: u16,
    pub events: Vec<SpeechEvent>,
}

impl SpeechEventTable {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let le = Endian::Little;
        let version = io::read_u16(data, 0, le)?;
        if version != VERSION {
            return Err(Error::malformed("get", 0, format!("version {version}")));
        }
        let n = io::read_u16(data, 2, le)? as usize;
        let at = io::read_u32(data, 4, le)? as usize;
        if at != 8 {
            return Err(Error::malformed("get", 4, format!("records at {at}")));
        }
        let records = io::slice(data, at, n * RECORD_SIZE)?;
        let events = records
            .chunks_exact(RECORD_SIZE)
            .map(|r| {
                let w = |k: usize| u32::from_le_bytes([r[k], r[k + 1], r[k + 2], r[k + 3]]);
                SpeechEvent {
                    hash: w(0),
                    weight: f32::from_bits(w(4)),
                    cooldown: f32::from_bits(w(8)),
                    second: f32::from_bits(w(12)),
                    count: w(16),
                    range: f32::from_bits(w(20)),
                    rank: w(24) as i32,
                    flags: w(28),
                    priority: w(32),
                }
            })
            .collect();
        Ok(Self { version, events })
    }

    /// Serialises the table in the game's layout, records in their order:
    /// keep them sorted by hash ([`SpeechEventTable::insert`] does).
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let le = Endian::Little;
        let n = u16::try_from(self.events.len())
            .map_err(|_| Error::InvalidArgument("too many speech events".into()))?;
        let mut out = Vec::with_capacity(8 + RECORD_SIZE * self.events.len());
        io::put_u16(&mut out, self.version, le);
        io::put_u16(&mut out, n, le);
        io::put_u32(&mut out, 8, le);
        for e in &self.events {
            io::put_u32(&mut out, e.hash, le);
            io::put_u32(&mut out, e.weight.to_bits(), le);
            io::put_u32(&mut out, e.cooldown.to_bits(), le);
            io::put_u32(&mut out, e.second.to_bits(), le);
            io::put_u32(&mut out, e.count, le);
            io::put_u32(&mut out, e.range.to_bits(), le);
            io::put_u32(&mut out, e.rank as u32, le);
            io::put_u32(&mut out, e.flags, le);
            io::put_u32(&mut out, e.priority, le);
        }
        Ok(out)
    }

    /// The record of an event, by the game's binary search.
    pub fn find(&self, hash: u32) -> Option<&SpeechEvent> {
        self.events
            .binary_search_by_key(&hash, |e| e.hash)
            .ok()
            .map(|i| &self.events[i])
    }

    /// Mutable access to the record of an event.
    pub fn find_mut(&mut self, hash: u32) -> Option<&mut SpeechEvent> {
        self.events
            .binary_search_by_key(&hash, |e| e.hash)
            .ok()
            .map(|i| &mut self.events[i])
    }

    /// Adds a record at its place in the hash order; replaces the record of
    /// the same event. Returns whether the event is new.
    pub fn insert(&mut self, event: SpeechEvent) -> bool {
        match self.events.binary_search_by_key(&event.hash, |e| e.hash) {
            Ok(i) => {
                self.events[i] = event;
                false
            }
            Err(i) => {
                self.events.insert(i, event);
                true
            }
        }
    }

    /// Problems the game would trip over: records out of hash order or
    /// repeated (the binary search misses them).
    pub fn validate(&self) -> Vec<String> {
        self.events
            .windows(2)
            .filter(|p| p[0].hash >= p[1].hash)
            .map(|p| {
                format!(
                    "events {:08x} and {:08x} are not in ascending order",
                    p[0].hash, p[1].hash
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_inserts_in_order() {
        let mut table = SpeechEventTable {
            version: 3,
            events: Vec::new(),
        };
        for name in ["P2_LINE1", "GUARD_BEFORE_RIOT", "F2F_ENJOY_FLIGHT"] {
            assert!(table.insert(SpeechEvent::new(name)));
        }
        assert!(table.validate().is_empty());
        let mut changed = SpeechEvent::new("P2_LINE1");
        changed.cooldown = 30.0;
        changed.flags = 0x4011;
        assert!(!table.insert(changed.clone()));
        let bytes = table.to_bytes().unwrap();
        assert_eq!(bytes.len(), 8 + 36 * 3);
        let back = SpeechEventTable::parse(&bytes).unwrap();
        assert_eq!(back, table);
        assert_eq!(back.find(string_id("p2_line1")), Some(&changed));
        assert!(back.find(1).is_none());
        assert!(SpeechEventTable::parse(&bytes[..40]).is_err());
        let mut bad = bytes;
        bad[0] = 4;
        assert!(SpeechEventTable::parse(&bad).is_err());
        table.events.reverse();
        assert!(!table.validate().is_empty());
    }
}

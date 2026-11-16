//! `chv` resources: chatter (speech) event sets of a character.
//!
//! A set maps speech event names (`F2F_JOB_DETAILS`, `P2_LINE1`) to the
//! recorded lines the game picks from, at random by weight
//! (`godfather2.exe` `0x006F4840` sums the weights on load).
//!
//! ```text
//! 0x00  u16  version = 3
//! 0x02  u16  number of events N
//! 0x04  u32  offset of the event table = 8
//! 0x08  N × { u32 sdbm(lower(name)), u32 line count,
//!             u32 offset of the name, u32 offset of the lines }
//!       lines of every event in event order, 32 bytes each:
//!         4 × u32  sound id: the key of a speech bank (`smb`) entry, the
//!                  same value four times in every file of the game
//!         u32  string id of the subtitle: `string_id(name)` of the speech
//!              file without the `d_` prefix (`mob3_xxx_0001806`); the text
//!              is in the `LCH2` chunk of that `.snu` file
//!         f32  weight (1, 2 or 3)
//!         u32  1
//!         u32  index of the line in its event
//!       event names, NUL-terminated, each padded to 4 bytes
//! ```
//!
//! [`ChatterSet::to_bytes`] reproduces every `chv` of the game byte for byte.

use serde::{Deserialize, Serialize};

use crate::hash::{sdbm, string_id};
use crate::io::{self, Endian};
use crate::{Error, Result};

/// Type id of `chv` resources.
pub const TYPE_CHV: u32 = sdbm(b"chv");

const VERSION: u16 = 3;
const LINE_SIZE: usize = 32;

/// One recorded line of an event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatterLine {
    /// Sound id (speech bank key), four slots; the game files repeat one value.
    pub sounds: [u32; 4],
    /// String id of the subtitle inside the speech file.
    pub subtitle: u32,
    pub weight: f32,
    /// Always 1 in the game.
    pub flag: u32,
    /// Index of the line in its event.
    pub index: u32,
}

impl ChatterLine {
    /// The sound id the game plays (the first slot).
    pub fn sound(&self) -> u32 {
        self.sounds[0]
    }
}

/// A speech event and its lines.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatterEvent {
    pub name: String,
    /// `sdbm` of the lower-case name, as stored.
    pub hash: u32,
    pub lines: Vec<ChatterLine>,
}

/// A parsed `chv` resource.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatterSet {
    pub version: u16,
    pub events: Vec<ChatterEvent>,
}

impl ChatterSet {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let le = Endian::Little;
        let version = io::read_u16(data, 0, le)?;
        if version != VERSION {
            return Err(Error::malformed("chv", 0, format!("version {version}")));
        }
        let count = io::read_u16(data, 2, le)? as usize;
        let table = io::read_u32(data, 4, le)? as usize;
        io::slice(data, table, count * 16)?;
        let mut events = Vec::with_capacity(count);
        for i in 0..count {
            let at = table + 16 * i;
            let hash = io::read_u32(data, at, le)?;
            let n = io::read_u32(data, at + 4, le)? as usize;
            let name_at = io::read_u32(data, at + 8, le)? as usize;
            let lines_at = io::read_u32(data, at + 12, le)? as usize;
            let name = String::from_utf8(io::cstr(data, name_at)?.to_vec())
                .map_err(|_| Error::malformed("chv", name_at, "event name is not UTF-8"))?;
            io::slice(data, lines_at, n.saturating_mul(LINE_SIZE))?;
            let lines = (0..n)
                .map(|j| {
                    let at = lines_at + LINE_SIZE * j;
                    let w = |k: usize| io::read_u32(data, at + 4 * k, le);
                    Ok(ChatterLine {
                        sounds: [w(0)?, w(1)?, w(2)?, w(3)?],
                        subtitle: w(4)?,
                        weight: f32::from_bits(w(5)?),
                        flag: w(6)?,
                        index: w(7)?,
                    })
                })
                .collect::<Result<_>>()?;
            events.push(ChatterEvent { name, hash, lines });
        }
        Ok(Self { version, events })
    }

    /// Serialises the set in the game's layout.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let le = Endian::Little;
        let n = self.events.len();
        if n > u16::MAX as usize {
            return Err(Error::InvalidArgument("too many chatter events".into()));
        }
        let lines: usize = self.events.iter().map(|e| e.lines.len()).sum();
        let mut out = Vec::new();
        io::put_u16(&mut out, self.version, le);
        io::put_u16(&mut out, n as u16, le);
        io::put_u32(&mut out, 8, le);
        let mut lines_at = 8 + 16 * n;
        let mut name_at = lines_at + LINE_SIZE * lines;
        for event in &self.events {
            if event.name.contains('\0') {
                return Err(Error::InvalidArgument("event name contains NUL".into()));
            }
            io::put_u32(&mut out, event.hash, le);
            io::put_u32(&mut out, event.lines.len() as u32, le);
            io::put_u32(&mut out, name_at as u32, le);
            io::put_u32(&mut out, lines_at as u32, le);
            lines_at += LINE_SIZE * event.lines.len();
            name_at += (event.name.len() + 1).next_multiple_of(4);
        }
        for line in self.events.iter().flat_map(|e| &e.lines) {
            for s in line.sounds {
                io::put_u32(&mut out, s, le);
            }
            io::put_u32(&mut out, line.subtitle, le);
            io::put_u32(&mut out, line.weight.to_bits(), le);
            io::put_u32(&mut out, line.flag, le);
            io::put_u32(&mut out, line.index, le);
        }
        for event in &self.events {
            out.extend_from_slice(event.name.as_bytes());
            out.push(0);
            out.resize(out.len().next_multiple_of(4), 0);
        }
        Ok(out)
    }

    /// The event with this hash (`sdbm` of the lower-case name).
    pub fn event(&self, hash: u32) -> Option<&ChatterEvent> {
        self.events.iter().find(|e| e.hash == hash)
    }

    /// Adds a line to an event and returns `(event is new, line index)`.
    /// A new event goes to its place in the hash order: the game finds
    /// events with a binary search (`0x006F4C10`). The name of an existing
    /// event is kept as it is stored.
    pub fn add_line(
        &mut self,
        event: &str,
        sound: u32,
        subtitle: u32,
        weight: f32,
    ) -> Result<(bool, usize)> {
        if event.is_empty() || event.contains('\0') {
            return Err(Error::InvalidArgument("bad event name".into()));
        }
        let hash = event_hash(event);
        let (new, at) = match self.events.binary_search_by_key(&hash, |e| e.hash) {
            Ok(at) => (false, at),
            Err(at) => {
                self.events.insert(
                    at,
                    ChatterEvent {
                        name: event.to_owned(),
                        hash,
                        lines: Vec::new(),
                    },
                );
                (true, at)
            }
        };
        let lines = &mut self.events[at].lines;
        let index = lines.len();
        lines.push(ChatterLine {
            sounds: [sound; 4],
            subtitle,
            weight,
            flag: 1,
            index: index as u32,
        });
        Ok((new, index))
    }

    /// Problems the game would trip over: events out of hash order or
    /// repeated (the binary search misses them), a hash that is not the
    /// hash of the name, lines numbered out of order.
    pub fn validate(&self) -> Vec<String> {
        let mut problems = Vec::new();
        for pair in self.events.windows(2) {
            if pair[0].hash >= pair[1].hash {
                problems.push(format!(
                    "events {} and {} are not in ascending hash order",
                    pair[0].name, pair[1].name
                ));
            }
        }
        for event in &self.events {
            if event.hash != event_hash(&event.name) {
                problems.push(format!("{}: hash {:08x}", event.name, event.hash));
            }
            for (i, line) in event.lines.iter().enumerate() {
                if line.index != i as u32 {
                    problems.push(format!("{}: line {i} has index {}", event.name, line.index));
                }
            }
        }
        problems
    }
}

/// Hash of a speech event name as `chv` and `dlg` store it.
pub fn event_hash(name: &str) -> u32 {
    string_id(name)
}

/// The subtitle string id of a speech file: `string_id` of its name without
/// directories, the `d_` prefix and extensions (`audiostreams\b3_xxx_0\
/// d_mob3_xxx_0001806.exa.snu` → `mob3_xxx_0001806`).
pub fn subtitle_id(file: &str) -> u32 {
    string_id(speech_line_name(file))
}

/// The line name of a speech file, see [`subtitle_id`].
pub fn speech_line_name(file: &str) -> &str {
    let base = file.rsplit(['\\', '/']).next().unwrap_or(file);
    let stem = base.split('.').next().unwrap_or(base);
    stem.strip_prefix("d_").unwrap_or(stem)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ChatterSet {
        let line = |sound: u32, index: u32| ChatterLine {
            sounds: [sound; 4],
            subtitle: string_id("mob3_xxx_0001806"),
            weight: 1.0,
            flag: 1,
            index,
        };
        ChatterSet {
            version: 3,
            events: vec![
                ChatterEvent {
                    name: "F2F_CREW_SPECIALTIES".into(),
                    hash: event_hash("F2F_CREW_SPECIALTIES"),
                    lines: vec![line(0x5199_B99D, 0), line(0x13EC_10A6, 1)],
                },
                ChatterEvent {
                    name: "LINE01".into(),
                    hash: event_hash("LINE01"),
                    lines: vec![line(7, 0)],
                },
            ],
        }
    }

    #[test]
    fn round_trips() {
        let set = sample();
        let bytes = set.to_bytes().unwrap();
        assert_eq!(bytes.len() % 4, 0);
        assert_eq!(ChatterSet::parse(&bytes).unwrap(), set);
        assert_eq!(set.event(event_hash("line01")).unwrap().lines.len(), 1);
    }

    #[test]
    fn adds_lines_and_events_in_hash_order() {
        let mut set = ChatterSet {
            version: 3,
            events: Vec::new(),
        };
        for name in ["P2_LINE1", "DOCTOR_HELLO", "F2F_ENJOY_FLIGHT"] {
            assert_eq!(set.add_line(name, 1, 2, 1.0).unwrap(), (true, 0));
        }
        assert_eq!(set.add_line("doctor_hello", 9, 8, 2.0).unwrap(), (false, 1));
        assert!(set.validate().is_empty());
        let event = set.event(event_hash("DOCTOR_HELLO")).unwrap();
        assert_eq!(event.name, "DOCTOR_HELLO");
        assert_eq!(event.lines[1].sounds, [9; 4]);
        assert_eq!(event.lines[1].index, 1);
        let back = ChatterSet::parse(&set.to_bytes().unwrap()).unwrap();
        assert_eq!(back, set);
        assert!(set.add_line("", 1, 2, 1.0).is_err());
        set.events.reverse();
        assert!(!set.validate().is_empty());
    }

    #[test]
    fn names_speech_files() {
        let file = "audiostreams\\b3_xxx_0\\d_mob3_xxx_0001806.exa.snu";
        assert_eq!(speech_line_name(file), "mob3_xxx_0001806");
        assert_eq!(subtitle_id(file), 0x872F_5FCA);
        assert_eq!(event_hash("GUARD_BEFORE_RIOT"), 0x44C5_F8C2);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(ChatterSet::parse(&[4, 0, 0, 0, 8, 0, 0, 0]).is_err());
        let mut bytes = sample().to_bytes().unwrap();
        bytes.truncate(40);
        assert!(ChatterSet::parse(&bytes).is_err());
    }
}

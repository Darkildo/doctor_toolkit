//! `tob` resources: subtitle and title timings of the pre-rendered movies.
//!
//! ```text
//! 0x00  "TOB1"
//! 0x04  u32  size of the resource
//! 0x08  u32  version = 1
//! 0x0C  u32  sdbm of the movie name in lower case (`fmv01`, `trn05`, ...)
//! 0x10  u32  number of groups G
//! 0x14  G × { u32 sdbm(group name), u32 entry count, u32 first entry }
//!       N × { u32 string hash (key in `lh2`), u32 start (ms), u32 duration (ms), u32 0 }
//! ```
//!
//! The groups are `subtitles` and `titles`; their entry ranges follow each
//! other and cover every entry.

use serde::Serialize;

use crate::hash::sdbm;
use crate::io::{self, Endian};
use crate::{Error, Result};

/// Type id of `tob` resources.
pub const TYPE_TOB: u32 = sdbm(b"tob");

const MAGIC: &[u8; 4] = b"TOB1";

/// One timed string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TobEntry {
    /// Hash of the string, a key of the string tables (`lh2`).
    pub string: u32,
    pub start_ms: u32,
    pub duration_ms: u32,
}

/// A named list of entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TobGroup {
    /// sdbm of the group name.
    pub name_hash: u32,
    /// `subtitles`, `titles` or `None` for an unknown hash.
    pub name: Option<&'static str>,
    pub entries: Vec<TobEntry>,
}

/// A parsed `tob` resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Tob {
    pub version: u32,
    /// sdbm of the movie name in lower case.
    pub movie_hash: u32,
    pub groups: Vec<TobGroup>,
}

fn group_name(hash: u32) -> Option<&'static str> {
    ["subtitles", "titles"]
        .into_iter()
        .find(|n| sdbm(n.as_bytes()) == hash)
}

impl Tob {
    pub fn parse(data: &[u8]) -> Result<Self> {
        if io::slice(data, 0, 4)? != MAGIC {
            return Err(Error::BadMagic {
                what: "tob",
                found: io::read_u32(data, 0, Endian::Big)?,
            });
        }
        let u32_at = |at: usize| io::read_u32(data, at, Endian::Little);
        let size = u32_at(4)? as usize;
        if size != data.len() {
            return Err(Error::malformed(
                "tob",
                4,
                format!("size {size}, data {}", data.len()),
            ));
        }
        let version = u32_at(8)?;
        let movie_hash = u32_at(12)?;
        let count = u32_at(16)? as usize;
        let entries_at = count
            .checked_mul(12)
            .and_then(|n| n.checked_add(0x14))
            .filter(|&at| at <= data.len())
            .ok_or_else(|| Error::malformed("tob", 16, format!("{count} groups")))?;
        if !(data.len() - entries_at).is_multiple_of(16) {
            return Err(Error::malformed(
                "tob",
                entries_at,
                "entries are not 16 bytes each",
            ));
        }
        let total = (data.len() - entries_at) / 16;
        let mut groups = Vec::with_capacity(count);
        for g in 0..count {
            let at = 0x14 + 12 * g;
            let (hash, n, first) = (
                u32_at(at)?,
                u32_at(at + 4)? as usize,
                u32_at(at + 8)? as usize,
            );
            if first.checked_add(n).is_none_or(|end| end > total) {
                return Err(Error::malformed(
                    "tob",
                    at,
                    format!("entries {first}+{n} of {total}"),
                ));
            }
            let entries = (first..first + n)
                .map(|e| {
                    let at = entries_at + 16 * e;
                    Ok(TobEntry {
                        string: u32_at(at)?,
                        start_ms: u32_at(at + 4)?,
                        duration_ms: u32_at(at + 8)?,
                    })
                })
                .collect::<Result<_>>()?;
            groups.push(TobGroup {
                name_hash: hash,
                name: group_name(hash),
                entries,
            });
        }
        Ok(Self {
            version,
            movie_hash,
            groups,
        })
    }
}

impl Tob {
    /// The resource as stored: groups in order, each taking the next run of
    /// entries.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        let put = |out: &mut Vec<u8>, v: u32| out.extend_from_slice(&v.to_le_bytes());
        put(&mut out, 0);
        put(&mut out, self.version);
        put(&mut out, self.movie_hash);
        put(&mut out, self.groups.len() as u32);
        let mut first = 0u32;
        for g in &self.groups {
            put(&mut out, g.name_hash);
            put(&mut out, g.entries.len() as u32);
            put(&mut out, first);
            first += g.entries.len() as u32;
        }
        for e in self.groups.iter().flat_map(|g| &g.entries) {
            put(&mut out, e.string);
            put(&mut out, e.start_ms);
            put(&mut out, e.duration_ms);
            put(&mut out, 0);
        }
        let len = out.len() as u32;
        out[4..8].copy_from_slice(&len.to_le_bytes());
        out
    }

    /// The subtitle table in the metadata of a streamed sound (`.snu` of the
    /// `.viv` archives; cut-scene sound tracks carry their subtitles there):
    /// the table and its byte range in the file.
    pub fn from_snu(snu: &[u8]) -> Option<(Self, std::ops::Range<usize>)> {
        let blocks = io::read_u32(snu, 8, Endian::Little).ok()? as usize;
        if blocks <= 0x20 || snu.get(0x20..0x24)? != MAGIC {
            return None;
        }
        let size = io::read_u32(snu, 0x24, Endian::Little).ok()? as usize;
        let range = 0x20..0x20usize.checked_add(size).filter(|&e| e <= blocks)?;
        let tob = Self::parse(&snu[range.clone()]).ok()?;
        Some((tob, range))
    }

    /// `snu` with its subtitle table replaced by this one. A table of another
    /// size moves the sound data (kept on a 16-byte boundary) and updates the
    /// offset at 0x08.
    pub fn replace_in_snu(&self, snu: &[u8]) -> Result<Vec<u8>> {
        let (_, range) = Self::from_snu(snu)
            .ok_or_else(|| Error::InvalidArgument("the sound has no subtitle table".into()))?;
        let new = self.to_bytes();
        let blocks = io::read_u32(snu, 8, Endian::Little)? as usize;
        if new.len() == range.len() {
            let mut out = snu.to_vec();
            out[range].copy_from_slice(&new);
            return Ok(out);
        }
        let mut out = snu[..0x20].to_vec();
        out.extend_from_slice(&new);
        // What followed the table up to the sound data, if anything.
        out.extend_from_slice(&snu[range.end..blocks]);
        while !out.len().is_multiple_of(16) {
            out.push(0);
        }
        let new_blocks = out.len() as u32;
        out[8..12].copy_from_slice(&new_blocks.to_le_bytes());
        out.extend_from_slice(&snu[blocks..]);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        let mut d = b"TOB1".to_vec();
        let words = [
            0u32,
            1,
            sdbm(b"fmv01"),
            2,
            sdbm(b"titles"),
            1,
            0,
            sdbm(b"subtitles"),
            1,
            1,
            0xCAE0_79FD,
            0,
            1500,
            0,
            0x565D_557B,
            2000,
            3000,
            0,
        ];
        for w in words {
            d.extend_from_slice(&w.to_le_bytes());
        }
        let len = d.len() as u32;
        d[4..8].copy_from_slice(&len.to_le_bytes());
        d
    }

    #[test]
    fn parses_groups_and_entries() {
        let tob = Tob::parse(&sample()).unwrap();
        assert_eq!(tob.version, 1);
        assert_eq!(tob.movie_hash, sdbm(b"fmv01"));
        assert_eq!(tob.groups[0].name, Some("titles"));
        assert_eq!(tob.groups[1].name, Some("subtitles"));
        assert_eq!(tob.groups[1].entries[0].start_ms, 2000);
        assert_eq!(tob.groups[1].entries[0].duration_ms, 3000);
    }

    #[test]
    fn writes_back_and_lives_in_sounds() {
        let data = sample();
        let tob = Tob::parse(&data).unwrap();
        assert_eq!(tob.to_bytes(), data);
        // A sound with the table as metadata.
        let mut snu = vec![3u8, 4, 0, 6];
        snu.extend_from_slice(&1000u32.to_le_bytes());
        let blocks = (0x20 + data.len()).div_ceil(16) * 16;
        snu.extend_from_slice(&(blocks as u32).to_le_bytes());
        snu.resize(0x20, 0);
        snu.extend_from_slice(&data);
        snu.resize(blocks, 0);
        snu.extend_from_slice(b"SOUND DATA");
        let (found, range) = Tob::from_snu(&snu).unwrap();
        assert_eq!(found, tob);
        assert_eq!(range, 0x20..0x20 + data.len());
        let mut later = tob.clone();
        later.groups[1].entries[0].start_ms = 2500;
        let edited = later.replace_in_snu(&snu).unwrap();
        assert_eq!(edited.len(), snu.len());
        assert_eq!(Tob::from_snu(&edited).unwrap().0, later);
        // One entry more: the sound data moves.
        let mut more = tob.clone();
        more.groups[1].entries.push(TobEntry {
            string: 7,
            start_ms: 6000,
            duration_ms: 1000,
        });
        let grown = more.replace_in_snu(&snu).unwrap();
        assert_eq!(Tob::from_snu(&grown).unwrap().0, more);
        assert!(grown.ends_with(b"SOUND DATA"));
        let at = u32::from_le_bytes(grown[8..12].try_into().unwrap()) as usize;
        assert_eq!(at % 16, 0);
        assert_eq!(&grown[at..at + 5], b"SOUND");
        assert!(Tob::from_snu(b"no metadata").is_none());
    }

    #[test]
    fn rejects_bad_input() {
        let mut d = sample();
        d.push(0);
        assert!(Tob::parse(&d).is_err());
        let mut d = sample();
        d[16] = 9;
        assert!(Tob::parse(&d).is_err());
        assert!(Tob::parse(b"TOB").is_err());
    }
}

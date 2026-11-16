//! `smb` resources: banks of streamed sounds. An entry names a file of the
//! `.viv` archives (speech, music, ambience) and carries its playback
//! settings; `chv` lines and scripts refer to it by a 128-bit id.
//!
//! ```text
//! 0x00  u32  0 (the game puts a method table here)
//! 0x04  u32  0
//! 0x08  u32  12, offset of the body
//! 0x0C  u32  13, version
//! 0x10  u32  number of entries N
//! 0x14  u32  offset of the entry offsets = 0x18
//! 0x18  N × u32 entry offsets from the start of the file, ascending
//!       N entries of 0x70 bytes, sorted by id (four u32, compared in turn):
//!       +0x00 u16 0x11
//!       +0x02 u16 flags: 0x01 localised (the language directory is put into
//!             the path), 0x04, 0x08 playback switches, 0x40 `.snu` suffix
//!       +0x04 4 × u32 id; every file of the game repeats one value
//!       +0x14 u32 0
//!       +0x18 6 × f32 playback settings (2.0, 100.0, 0, 0, 0, -1.0 mostly);
//!             the last two are times in seconds of the recording itself
//!             (the game turns them into milliseconds, the sixth when >= 0)
//!       +0x30 u32 (90 mostly)
//!       +0x34 u32 mix category: sdbm of a category name of `auc`
//!       +0x38 u32 second category hash or 0
//!       +0x3C u32 duck: sdbm of a duck name of `auc` or 0
//!       +0x40 file name, 40 bytes with NUL (`d_mob3_xxx_0001806.exa`)
//!       +0x68 u32 0 (offset of extra data, never present)
//!       +0x6C u32 sdbm(lower(stem + ".stx"))
//! ```
//!
//! The game (`godfather2.exe` 1.0.789.0) fixes the offsets up on load
//! (`0x004CBD10`), finds an entry by id with a binary search in every loaded
//! bank (`0x00455F50`, `0x004CBDC0`) and builds the path of the file from
//! the name and the flags (`0x004A8580`), see [`SpeechBankEntry::stream_path`].
//!
//! [`SpeechBank::to_bytes`] reproduces every `smb` of the game byte for byte.

use serde::{Deserialize, Serialize};

use crate::hash::sdbm;
use crate::io::{self, Endian};
use crate::{Error, Result};

/// Type id of `smb` resources.
pub const TYPE_SMB: u32 = sdbm(b"smb");

const BODY_AT: u32 = 12;
const TABLE_AT: usize = 0x18;
const ENTRY_SIZE: usize = 0x70;
const NAME_AT: usize = 0x40;
const NAME_SIZE: usize = 0x28;
/// Version of the banks of the game.
pub const VERSION: u32 = 13;
/// First word of every entry.
pub const ENTRY_TAG: u16 = 0x11;

/// The language directory is part of the path (`audiostreams\es\…`).
pub const FLAG_LOCALISED: u16 = 0x01;
/// The file name gets the `.snu` suffix; set in every entry of the game.
pub const FLAG_SNU: u16 = 0x40;

/// One recorded line of a bank (the short form older code reads).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SpeechEntry {
    pub sound: u32,
    /// File name without directories (`d_mob3_xxx_0001806.exa`).
    pub file: String,
}

/// One entry of a bank with all its fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpeechBankEntry {
    /// Always [`ENTRY_TAG`] in the game.
    pub tag: u16,
    pub flags: u16,
    /// The id `chv` lines store; the game files repeat one value four times.
    pub id: [u32; 4],
    /// Always 0 in the game.
    pub reserved: u32,
    /// Playback settings at 0x18..0x30; the game copies them into the play
    /// request (`0x004A84D0`). By the values of the game the first two are
    /// the near and far distances. The fifth and sixth are times in seconds
    /// that differ from recording to recording (0 and −1 in three entries of
    /// four); the game multiplies them by 1000.
    pub settings: [f32; 6],
    /// 90 in most entries.
    pub level: u32,
    /// `sdbm` of the mix category name (`DIALOG`, `CHATTER`, `MUSIC`).
    pub category: u32,
    /// A second category hash, 0 in music and ambience.
    pub category2: u32,
    /// `sdbm` of a duck name (`CREW_CHATTER_DUCK`) or 0.
    pub duck: u32,
    /// File name without directories and without `.snu`.
    pub file: String,
    /// Offset of extra data; 0 in every entry of the game.
    pub extra: u32,
    /// `sdbm(lower(stem + ".stx"))`, see [`name_hash`].
    pub name_hash: u32,
}

impl SpeechBankEntry {
    /// The first word of the id.
    pub fn sound(&self) -> u32 {
        self.id[0]
    }

    /// The archive path the game opens for this entry, as `0x004A8580`
    /// builds it: `audiostreams\`, the language directory when the entry is
    /// localised and a language is given, a directory of characters 4..12 of
    /// the name (cut at its first dot) when the name with its suffix is
    /// longer than 16 characters, the name, and `.snu` with [`FLAG_SNU`].
    /// Lower case, as archive names are compared.
    pub fn stream_path(&self, language: Option<&str>) -> String {
        stream_path(&self.file, self.flags, language)
    }
}

/// See [`SpeechBankEntry::stream_path`].
pub fn stream_path(file: &str, flags: u16, language: Option<&str>) -> String {
    let mut name = file.to_owned();
    if flags & FLAG_SNU != 0 {
        name.push_str(".snu");
    }
    let mut path = String::from("audiostreams\\");
    if flags & FLAG_LOCALISED != 0
        && let Some(language) = language.filter(|l| !l.is_empty())
    {
        path.push_str(language);
        path.push('\\');
    }
    // Long names live in a directory named by characters 4..12 of the name,
    // cut at the first dot (`my_new_york.exa` → `ew_york`).
    if name.len() > 0x10 {
        let stem = file.split('.').next().unwrap_or(file);
        if let Some(dir) = stem.get(4..stem.len().min(12)).filter(|d| !d.is_empty()) {
            path.push_str(dir);
            path.push('\\');
        }
    }
    path.push_str(&name);
    path.to_ascii_lowercase()
}

/// The name hash of an entry: `sdbm` of the lower-case file name with its
/// extension replaced by `.stx` (`d_mob3_xxx_0001806.exa` →
/// `d_mob3_xxx_0001806.stx`).
pub fn name_hash(file: &str) -> u32 {
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
    sdbm(format!("{stem}.stx").to_ascii_lowercase().as_bytes())
}

/// A parsed `smb` resource.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpeechBank {
    pub version: u32,
    pub entries: Vec<SpeechBankEntry>,
}

impl SpeechBank {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let le = Endian::Little;
        let word = |at: usize| io::read_u32(data, at, le);
        if word(0)? != 0 || word(4)? != 0 || word(8)? != BODY_AT {
            return Err(Error::malformed("smb", 0, "unexpected header"));
        }
        let version = word(0x0C)?;
        let n = word(0x10)? as usize;
        let table = word(0x14)? as usize;
        if table != TABLE_AT {
            return Err(Error::malformed("smb", 0x14, format!("table at {table}")));
        }
        io::slice(data, table, n.saturating_mul(4))?;
        let mut entries = Vec::with_capacity(n);
        for i in 0..n {
            let at = word(table + 4 * i)? as usize;
            let e = io::slice(data, at, ENTRY_SIZE)?;
            let w = |k: usize| io::read_u32(e, k, le);
            let name = io::cstr(&e[..NAME_AT + NAME_SIZE], NAME_AT)?;
            let file = String::from_utf8(name.to_vec())
                .map_err(|_| Error::malformed("smb", at + NAME_AT, "file name is not UTF-8"))?;
            let mut settings = [0f32; 6];
            for (k, s) in settings.iter_mut().enumerate() {
                *s = f32::from_bits(w(0x18 + 4 * k)?);
            }
            entries.push(SpeechBankEntry {
                tag: io::read_u16(e, 0, le)?,
                flags: io::read_u16(e, 2, le)?,
                id: [w(4)?, w(8)?, w(12)?, w(16)?],
                reserved: w(0x14)?,
                settings,
                level: w(0x30)?,
                category: w(0x34)?,
                category2: w(0x38)?,
                duck: w(0x3C)?,
                file,
                extra: w(0x68)?,
                name_hash: w(0x6C)?,
            });
        }
        Ok(Self { version, entries })
    }

    /// Serialises the bank in the game's layout. The entries are written in
    /// their order: keep them sorted by id ([`SpeechBank::insert`] does).
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let le = Endian::Little;
        let n = self.entries.len();
        let mut out = Vec::with_capacity(TABLE_AT + n * (ENTRY_SIZE + 4));
        for word in [0, 0, BODY_AT, self.version, n as u32, TABLE_AT as u32] {
            io::put_u32(&mut out, word, le);
        }
        for i in 0..n {
            io::put_u32(&mut out, (TABLE_AT + 4 * n + ENTRY_SIZE * i) as u32, le);
        }
        for e in &self.entries {
            if e.file.len() >= NAME_SIZE || e.file.contains('\0') {
                return Err(Error::InvalidArgument(format!(
                    "file name `{}` does not fit {} bytes",
                    e.file,
                    NAME_SIZE - 1
                )));
            }
            io::put_u16(&mut out, e.tag, le);
            io::put_u16(&mut out, e.flags, le);
            for word in e.id {
                io::put_u32(&mut out, word, le);
            }
            io::put_u32(&mut out, e.reserved, le);
            for s in e.settings {
                io::put_u32(&mut out, s.to_bits(), le);
            }
            for word in [e.level, e.category, e.category2, e.duck] {
                io::put_u32(&mut out, word, le);
            }
            let start = out.len();
            out.extend_from_slice(e.file.as_bytes());
            out.resize(start + NAME_SIZE, 0);
            io::put_u32(&mut out, e.extra, le);
            io::put_u32(&mut out, e.name_hash, le);
        }
        Ok(out)
    }

    /// The entry with this id, by the game's binary search.
    pub fn find(&self, id: &[u32; 4]) -> Option<&SpeechBankEntry> {
        self.entries
            .binary_search_by(|e| e.id.cmp(id))
            .ok()
            .map(|i| &self.entries[i])
    }

    /// Adds an entry at its place in the id order. An entry with the same id
    /// is an error: the game would find only one of them.
    pub fn insert(&mut self, entry: SpeechBankEntry) -> Result<usize> {
        match self.entries.binary_search_by(|e| e.id.cmp(&entry.id)) {
            Ok(_) => Err(Error::InvalidArgument(format!(
                "the bank already has sound {:08x}",
                entry.id[0]
            ))),
            Err(at) => {
                self.entries.insert(at, entry);
                Ok(at)
            }
        }
    }

    /// Problems the game would trip over: entries out of id order (the
    /// binary search misses them), repeated ids, names that do not fit.
    pub fn validate(&self) -> Vec<String> {
        let mut problems = Vec::new();
        for pair in self.entries.windows(2) {
            if pair[0].id >= pair[1].id {
                problems.push(format!(
                    "entries {} and {} are not in ascending id order",
                    pair[0].file, pair[1].file
                ));
            }
        }
        for e in &self.entries {
            if e.file.is_empty() || e.file.len() >= NAME_SIZE {
                problems.push(format!("`{}`: bad file name length", e.file));
            }
            if e.extra != 0 {
                problems.push(format!("{}: extra data is not supported", e.file));
            }
        }
        problems
    }
}

/// Parses the entries of a speech bank: ids and file names only.
pub fn parse(data: &[u8]) -> Result<Vec<SpeechEntry>> {
    Ok(SpeechBank::parse(data)?
        .entries
        .into_iter()
        .map(|e| SpeechEntry {
            sound: e.id[0],
            file: e.file,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: u32, file: &str) -> SpeechBankEntry {
        SpeechBankEntry {
            tag: ENTRY_TAG,
            flags: 0x49,
            id: [id; 4],
            reserved: 0,
            settings: [2.0, 100.0, 0.0, 0.0, 0.0, -1.0],
            level: 90,
            category: sdbm(b"dialog"),
            category2: 0x3504_8678,
            duck: 0,
            file: file.into(),
            extra: 0,
            name_hash: name_hash(file),
        }
    }

    #[test]
    fn round_trips_and_keeps_the_id_order() {
        let mut bank = SpeechBank {
            version: VERSION,
            entries: vec![entry(0x09FC_9A57, "d_mob3_xxx_0001806.exa")],
        };
        bank.insert(entry(0x0000_0007, "d_doc1_xxx_0000001.exa"))
            .unwrap();
        bank.insert(entry(0xF000_0000, "wcll_03.exa")).unwrap();
        assert!(bank.insert(entry(7, "other.exa")).is_err());
        assert!(bank.validate().is_empty());
        let bytes = bank.to_bytes().unwrap();
        assert_eq!(bytes.len(), 0x18 + 3 * 0x74);
        let back = SpeechBank::parse(&bytes).unwrap();
        assert_eq!(back, bank);
        assert_eq!(back.entries[0].id[0], 7);
        assert_eq!(
            back.find(&[0xF000_0000; 4]).unwrap().file,
            "wcll_03.exa".to_owned()
        );
        assert!(back.find(&[1; 4]).is_none());
        assert_eq!(parse(&bytes).unwrap()[1].sound, 0x09FC_9A57);
        let mut bad = bytes.clone();
        bad[0x10] = 9;
        assert!(SpeechBank::parse(&bad).is_err());
        bank.entries.swap(0, 2);
        assert!(!bank.validate().is_empty());
    }

    #[test]
    fn builds_the_paths_of_the_game() {
        // Entries of the game and the archive names they play.
        assert_eq!(name_hash("d_mpd1_xxx_000289c.exa"), 0xD109_2A2E);
        assert_eq!(
            stream_path("d_mob3_xxx_0001806.exa", 0x49, None),
            "audiostreams\\b3_xxx_0\\d_mob3_xxx_0001806.exa.snu"
        );
        assert_eq!(
            stream_path("d_mob3_xxx_0001806.exa", 0x49, Some("es")),
            "audiostreams\\es\\b3_xxx_0\\d_mob3_xxx_0001806.exa.snu"
        );
        assert_eq!(
            stream_path("tv_loop_sports_distant.exa", 0x4A, Some("es")),
            "audiostreams\\oop_spor\\tv_loop_sports_distant.exa.snu"
        );
        assert_eq!(
            stream_path("my_new_york.exa", 0x40, None),
            "audiostreams\\ew_york\\my_new_york.exa.snu"
        );
        assert_eq!(
            stream_path("big_shot.exa", 0x40, None),
            "audiostreams\\big_shot.exa.snu"
        );
        assert_eq!(
            stream_path("WCLL_03.exa", 0x40, None),
            "audiostreams\\wcll_03.exa.snu"
        );
        let long = "x".repeat(40);
        assert!(
            SpeechBank {
                version: VERSION,
                entries: vec![entry(1, &long)]
            }
            .to_bytes()
            .is_err()
        );
    }
}

//! `BIGF` archives (`GODFATH0.viv`..`GODFATH2.viv`).
//!
//! In The Godfather II they hold only the streamed audio, `audiostreams\*.exa.snu`
//! (see [`crate::resource::audio`]).
//!
//! ```text
//! 0x00  "BIGF"
//! 0x04  u32 LE  archive size
//! 0x08  u32 BE  entry count
//! 0x0C  u32 BE  size of the header including the entry table
//! 0x10  entries: u32 BE offset, u32 BE size, NUL-terminated name with `\` separators
//! ```

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::io::{self, Endian};
use crate::{Error, Result};

/// One file of an archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BigEntry {
    pub offset: u64,
    pub size: u64,
    pub name: String,
}

/// An archive with its entry table; data is read on demand.
#[derive(Debug, Clone)]
pub struct BigArchive {
    pub path: PathBuf,
    pub entries: Vec<BigEntry>,
}

/// Parses the header and entry table, checking entries against `file_len`.
pub fn parse_table(header: &[u8], file_len: u64) -> Result<Vec<BigEntry>> {
    if io::slice(header, 0, 4)? != b"BIGF" {
        return Err(Error::BadMagic {
            what: "BIGF",
            found: io::read_u32(header, 0, Endian::Big)?,
        });
    }
    let count = io::read_u32(header, 8, Endian::Big)? as usize;
    let table_end = io::read_u32(header, 12, Endian::Big)? as usize;
    let table = io::slice(header, 0, table_end)?;
    let mut entries = Vec::with_capacity(count.min(table_end / 9));
    let mut at = 16;
    for _ in 0..count {
        let offset = io::read_u32(table, at, Endian::Big)? as u64;
        let size = io::read_u32(table, at + 4, Endian::Big)? as u64;
        let name = io::cstr(table, at + 8)?;
        if offset + size > file_len {
            return Err(Error::malformed("BIGF", at, "entry outside the archive"));
        }
        at += 8 + name.len() + 1;
        entries.push(BigEntry {
            offset,
            size,
            name: String::from_utf8_lossy(name).into_owned(),
        });
    }
    Ok(entries)
}

impl BigArchive {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let mut file = File::open(&path).map_err(|e| Error::io(&path, e))?;
        let len = file.metadata().map_err(|e| Error::io(&path, e))?.len();
        let mut head = [0u8; 16];
        file.read_exact(&mut head)
            .map_err(|e| Error::io(&path, e))?;
        let table_end = io::read_u32(&head, 12, Endian::Big)? as u64;
        if table_end > len || table_end < 16 {
            return Err(Error::malformed("BIGF", 12, "header size"));
        }
        let mut header = vec![0u8; table_end as usize];
        file.seek(SeekFrom::Start(0))
            .and_then(|_| file.read_exact(&mut header))
            .map_err(|e| Error::io(&path, e))?;
        let entries = parse_table(&header, len)?;
        Ok(Self { path, entries })
    }

    /// Reads the data of one entry.
    pub fn read(&self, entry: &BigEntry) -> Result<Vec<u8>> {
        let mut file = File::open(&self.path).map_err(|e| Error::io(&self.path, e))?;
        let mut data = vec![0u8; entry.size as usize];
        file.seek(SeekFrom::Start(entry.offset))
            .and_then(|_| file.read_exact(&mut data))
            .map_err(|e| Error::io(&self.path, e))?;
        Ok(data)
    }
}

/// Data alignment of the game's archives.
pub const ALIGN: usize = 0x800;
/// The game probes `GODFATH0.VIV` .. `GODFATH12.VIV` (`0x0068bd30`).
pub const MAX_ARCHIVES: usize = 13;

/// The header of a `BIGF` archive laid out like the game's: the entry table
/// in the given order, the `L283` trailer, then the data of each entry
/// aligned to [`ALIGN`] bytes. Takes names (with `\` separators) and sizes;
/// returns the header bytes and the data offset of every entry.
pub fn archive_header(entries: &[(&str, u64)]) -> Result<(Vec<u8>, Vec<u64>)> {
    let table: usize = entries.iter().map(|(n, _)| 8 + n.len() + 1).sum();
    let header_size = 16 + table + 8;
    let mut offsets = Vec::with_capacity(entries.len());
    let mut at = (header_size as u64).next_multiple_of(ALIGN as u64);
    let mut total = header_size as u64;
    for (name, size) in entries {
        if name.is_empty() || name.contains('\0') {
            return Err(Error::InvalidArgument(format!(
                "bad archive entry name {name:?}"
            )));
        }
        offsets.push(at);
        total = at + size;
        at = total.next_multiple_of(ALIGN as u64);
    }
    let too_big = || Error::InvalidArgument("archive larger than 4 GiB".into());
    let mut out = Vec::with_capacity(header_size);
    out.extend_from_slice(b"BIGF");
    out.extend_from_slice(&u32::try_from(total).map_err(|_| too_big())?.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    out.extend_from_slice(&(header_size as u32).to_be_bytes());
    for ((name, size), &offset) in entries.iter().zip(&offsets) {
        out.extend_from_slice(&(offset as u32).to_be_bytes());
        out.extend_from_slice(&u32::try_from(*size).map_err(|_| too_big())?.to_be_bytes());
        out.extend_from_slice(name.as_bytes());
        out.push(0);
    }
    out.extend_from_slice(b"L283\x15\x05\0\0");
    Ok((out, offsets))
}

/// Builds a whole `BIGF` archive in memory (see [`archive_header`]).
pub fn build_archive(entries: &[(String, Vec<u8>)]) -> Result<Vec<u8>> {
    let sizes: Vec<(&str, u64)> = entries
        .iter()
        .map(|(n, d)| (n.as_str(), d.len() as u64))
        .collect();
    let (mut out, offsets) = archive_header(&sizes)?;
    for ((_, data), &offset) in entries.iter().zip(&offsets) {
        out.resize(offset as usize, 0);
        out.extend_from_slice(data);
    }
    Ok(out)
}

/// Archive entry names are compared like the game's file system does:
/// case-insensitively, with `/` and `\` equal.
pub fn normalize_name(name: &str) -> String {
    name.replace('/', "\\").to_ascii_lowercase()
}

/// Whether an entry belongs to the toolkit, not the game (the marker of an
/// overlay archive, `_doctor_toolkit\overlay.txt`).
pub fn is_tool_entry(name: &str) -> bool {
    normalize_name(name).starts_with("_doctor_toolkit\\")
}

/// Number `n` of an archive named `GODFATHn.VIV`.
pub fn archive_number(path: &Path) -> Option<usize> {
    let name = path.file_name()?.to_str()?.to_ascii_uppercase();
    name.strip_prefix("GODFATH")?
        .strip_suffix(".VIV")?
        .parse()
        .ok()
}

/// The `.viv` archives of a game directory, in the order the game mounts
/// them (`GODFATH0`, `GODFATH1`, …; the game takes a file from the first
/// archive that has it).
pub fn find_archives(game_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(game_dir).map_err(|e| Error::io(game_dir, e))? {
        let path = entry.map_err(|e| Error::io(game_dir, e))?.path();
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("viv"))
        {
            out.push(path);
        }
    }
    out.sort_by_key(|p| (archive_number(p).unwrap_or(usize::MAX), p.clone()));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        let mut d = b"BIGF".to_vec();
        d.extend_from_slice(&0u32.to_le_bytes());
        d.extend_from_slice(&1u32.to_be_bytes());
        d.extend_from_slice(&32u32.to_be_bytes());
        d.extend_from_slice(&32u32.to_be_bytes());
        d.extend_from_slice(&3u32.to_be_bytes());
        d.extend_from_slice(b"a\\b.snu\0");
        d.extend_from_slice(b"xyz");
        d
    }

    #[test]
    fn parses_entries() {
        let d = sample();
        let entries = parse_table(&d, d.len() as u64).unwrap();
        assert_eq!(
            entries,
            vec![BigEntry {
                offset: 32,
                size: 3,
                name: "a\\b.snu".into()
            }]
        );
        assert!(parse_table(&d, 30).is_err());
        assert!(parse_table(&d[..20], 40).is_err());
        assert!(parse_table(b"BIGX", 4).is_err());
    }

    #[test]
    fn built_archive_parses_back() {
        let entries = vec![
            ("audiostreams\\a\\one.exa.snu".to_owned(), vec![1u8; 3000]),
            ("audiostreams\\b\\two.exa.snu".to_owned(), vec![2u8; 10]),
        ];
        let d = build_archive(&entries).unwrap();
        let parsed = parse_table(&d, d.len() as u64).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].offset, 0x800);
        assert_eq!(parsed[1].offset, 0x1800);
        assert_eq!(parsed[1].name, entries[1].0);
        assert_eq!(&d[0x1800..0x180A], &[2u8; 10]);
        assert_eq!(
            u32::from_le_bytes(d[4..8].try_into().unwrap()) as usize,
            d.len()
        );
        let header = u32::from_be_bytes(d[12..16].try_into().unwrap()) as usize;
        assert_eq!(&d[header - 8..header - 4], b"L283");
        assert!(build_archive(&[(String::new(), vec![])]).is_err());
    }
}

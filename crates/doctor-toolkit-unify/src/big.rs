//! `BIGF` archives of the game (`GODFATH0.viv`…): the streamed audio,
//! `audiostreams\*.exa.snu`.
//!
//! ```text
//! 0x00  "BIGF"
//! 0x04  u32 LE  archive size
//! 0x08  u32 BE  entry count
//! 0x0C  u32 BE  size of the header: the entry table and the `L283` trailer
//! 0x10  entries: u32 BE offset, u32 BE size, NUL-terminated name with `\`
//!       "L283" 15 05 00 00
//! data of each entry, aligned to 0x800
//! ```
//!
//! The layout is the one `doctor_toolkit::container::big` writes, which
//! reproduces the headers of the archives of the game byte for byte.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::support::io::{self, Endian};
use crate::{Error, Result};

/// Data alignment of the archives of the game.
pub const ALIGN: u64 = 0x800;

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

impl BigArchive {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let mut file = File::open(&path).map_err(|e| Error::io(&path, e))?;
        let len = file.metadata().map_err(|e| Error::io(&path, e))?.len();
        let mut head = [0u8; 16];
        file.read_exact(&mut head)
            .map_err(|e| Error::io(&path, e))?;
        if &head[..4] != b"BIGF" {
            return Err(Error::BadMagic {
                what: "BIGF",
                found: io::read_u32(&head, 0, Endian::Big)?,
            });
        }
        let count = io::read_u32(&head, 8, Endian::Big)? as usize;
        let table_end = io::read_u32(&head, 12, Endian::Big)? as u64;
        if table_end > len || table_end < 16 {
            return Err(Error::malformed("BIGF", 12, "header size"));
        }
        let mut table = vec![0u8; table_end as usize];
        file.seek(SeekFrom::Start(0))
            .and_then(|_| file.read_exact(&mut table))
            .map_err(|e| Error::io(&path, e))?;
        let mut entries = Vec::with_capacity(count.min(table.len() / 9));
        let mut at = 16;
        for _ in 0..count {
            let offset = io::read_u32(&table, at, Endian::Big)? as u64;
            let size = io::read_u32(&table, at + 4, Endian::Big)? as u64;
            let name = io::cstr(&table, at + 8)?;
            if offset + size > len {
                return Err(Error::malformed("BIGF", at, "entry outside the archive"));
            }
            at += 8 + name.len() + 1;
            entries.push(BigEntry {
                offset,
                size,
                name: String::from_utf8_lossy(name).into_owned(),
            });
        }
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

/// The header of an archive with the given entries (names with `\`, sizes)
/// in this order, and the data offset of every entry.
pub fn archive_header(entries: &[(&str, u64)]) -> Result<(Vec<u8>, Vec<u64>)> {
    let table: usize = entries.iter().map(|(n, _)| 8 + n.len() + 1).sum();
    let header_size = 16 + table + 8;
    let mut offsets = Vec::with_capacity(entries.len());
    let mut at = (header_size as u64).next_multiple_of(ALIGN);
    let mut total = header_size as u64;
    for (name, size) in entries {
        if name.is_empty() || name.contains('\0') {
            return Err(Error::InvalidArgument(format!(
                "bad archive entry name {name:?}"
            )));
        }
        offsets.push(at);
        total = at + size;
        at = total.next_multiple_of(ALIGN);
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

/// A whole archive in memory (tests, small archives).
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

/// Entry names are compared as the game does: without regard to case, `/`
/// and `\` equal.
pub fn normalize_name(name: &str) -> String {
    name.replace('/', "\\").to_ascii_lowercase()
}

/// Number `n` of an archive named `GODFATHn.VIV`.
pub fn archive_number(path: &Path) -> Option<usize> {
    let name = path.file_name()?.to_str()?.to_ascii_uppercase();
    name.strip_prefix("GODFATH")?
        .strip_suffix(".VIV")?
        .parse()
        .ok()
}

/// The archives of a folder in the order the game mounts them; the game
/// takes a file from the first archive that has it.
pub fn find_archives(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| Error::io(dir, e))? {
        let path = entry.map_err(|e| Error::io(dir, e))?.path();
        if archive_number(&path).is_some() {
            out.push(path);
        }
    }
    out.sort_by_key(|p| archive_number(p));
    Ok(out)
}

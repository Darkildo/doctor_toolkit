//! Microsoft cabinets (`.cab`) of the game discs: `data_*.cab`, `Group0.cab`.
//!
//! Only what those cabinets use is read: one cabinet per file set (no
//! spanning), folders compressed with MSZIP or stored.
//!
//! ```text
//! CFHEADER  "MSCF", u32 0, u32 size, u32 0, u32 offset of the first CFFILE, u32 0,
//!           u8 minor, u8 major, u16 folders, u16 files, u16 flags, u16 set id, u16 number,
//!           [u16 header reserve, u8 folder reserve, u8 data reserve, reserve bytes]  (flag 4)
//!           [previous cabinet and disk names]  (flag 1)  [next ...]  (flag 2)
//! CFFOLDER  u32 offset of the first CFDATA, u16 data blocks, u16 compression (0 none, 1 MSZIP)
//! CFFILE    u32 size, u32 offset in the uncompressed folder, u16 folder, u16 date, u16 time,
//!           u16 attributes, NUL-terminated name
//! CFDATA    u32 checksum, u16 stored size, u16 uncompressed size, [reserve], data
//! ```
//!
//! An MSZIP block is `CK` and a raw deflate stream of at most 32 768 bytes;
//! each block may refer to the 32 768 bytes before it.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use flate2::{Decompress, FlushDecompress, Status};

use crate::support::io::{self, Endian};
use crate::{Error, Result};

const WINDOW: usize = 32_768;

/// One file of a cabinet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CabFile {
    pub name: String,
    pub size: u64,
    /// Offset in the uncompressed data of its folder.
    pub offset: u64,
    pub folder: usize,
}

#[derive(Debug, Clone)]
struct Folder {
    data_offset: u64,
    blocks: usize,
    mszip: bool,
}

/// A cabinet with its file table; data is decompressed on demand.
#[derive(Debug, Clone)]
pub struct Cabinet {
    pub path: PathBuf,
    pub files: Vec<CabFile>,
    folders: Vec<Folder>,
    data_reserve: usize,
}

fn bad(offset: usize, reason: &str) -> Error {
    Error::malformed("cabinet", offset, reason)
}

impl Cabinet {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let mut file = File::open(&path).map_err(|e| Error::io(&path, e))?;
        let mut head = vec![0u8; 36];
        file.read_exact(&mut head)
            .map_err(|e| Error::io(&path, e))?;
        if &head[..4] != b"MSCF" {
            return Err(Error::BadMagic {
                what: "cabinet",
                found: io::read_u32(&head, 0, Endian::Big)?,
            });
        }
        let files_at = io::read_u32(&head, 16, Endian::Little)? as usize;
        let folder_count = io::read_u16(&head, 26, Endian::Little)? as usize;
        let file_count = io::read_u16(&head, 28, Endian::Little)? as usize;
        let flags = io::read_u16(&head, 30, Endian::Little)?;
        // The tables end where the data of the first folder starts; cabinets
        // of the game keep them within the first megabytes.
        let len = file.metadata().map_err(|e| Error::io(&path, e))?.len();
        let table_len = (files_at as u64 + file_count as u64 * 300).min(len) as usize;
        let mut data = vec![0u8; table_len];
        file.seek(SeekFrom::Start(0))
            .and_then(|_| file.read_exact(&mut data))
            .map_err(|e| Error::io(&path, e))?;

        let mut at = 36;
        let (mut folder_reserve, mut data_reserve) = (0usize, 0usize);
        if flags & 4 != 0 {
            let header_reserve = io::read_u16(&data, at, Endian::Little)? as usize;
            folder_reserve = *data.get(at + 2).ok_or_else(|| bad(at, "reserve sizes"))? as usize;
            data_reserve = *data.get(at + 3).ok_or_else(|| bad(at, "reserve sizes"))? as usize;
            at += 4 + header_reserve;
        }
        for flag in [1, 2] {
            if flags & flag != 0 {
                // Cabinet name and disk name of the neighbour.
                for _ in 0..2 {
                    at += io::cstr(&data, at)?.len() + 1;
                }
                return Err(bad(at, "a cabinet that continues in another one"));
            }
        }
        let mut folders = Vec::with_capacity(folder_count);
        for _ in 0..folder_count {
            let compression = io::read_u16(&data, at + 6, Endian::Little)? & 0x0F;
            if compression > 1 {
                return Err(bad(at + 6, "compression other than MSZIP"));
            }
            folders.push(Folder {
                data_offset: io::read_u32(&data, at, Endian::Little)? as u64,
                blocks: io::read_u16(&data, at + 4, Endian::Little)? as usize,
                mszip: compression == 1,
            });
            at += 8 + folder_reserve;
        }
        let mut files = Vec::with_capacity(file_count);
        let mut at = files_at;
        for _ in 0..file_count {
            let name = io::cstr(&data, at + 16)?;
            let folder = io::read_u16(&data, at + 8, Endian::Little)? as usize;
            if folder >= folders.len() {
                return Err(bad(at + 8, "a file of a folder in another cabinet"));
            }
            files.push(CabFile {
                name: String::from_utf8_lossy(name).into_owned(),
                size: io::read_u32(&data, at, Endian::Little)? as u64,
                offset: io::read_u32(&data, at + 4, Endian::Little)? as u64,
                folder,
            });
            at += 16 + name.len() + 1;
        }
        Ok(Self {
            path,
            files,
            folders,
            data_reserve,
        })
    }

    /// Decompresses the cabinet once and hands the data of every wanted file
    /// to `sink` in pieces, in the order the files lie in the cabinet:
    /// `sink(file, piece)`; an empty piece ends a file (and is the only call
    /// for an empty file).
    pub fn extract(
        &self,
        mut wanted: impl FnMut(&CabFile) -> bool,
        mut sink: impl FnMut(&CabFile, &[u8]) -> Result<()>,
    ) -> Result<()> {
        let file = File::open(&self.path).map_err(|e| Error::io(&self.path, e))?;
        let mut reader = BufReader::with_capacity(1 << 20, file);
        for (index, folder) in self.folders.iter().enumerate() {
            let mut files: Vec<&CabFile> = self
                .files
                .iter()
                .filter(|f| f.folder == index && wanted(f))
                .collect();
            if files.is_empty() {
                continue;
            }
            files.sort_by_key(|f| f.offset);
            for file in files.iter().filter(|f| f.size == 0) {
                sink(file, &[])?;
            }
            files.retain(|f| f.size > 0);
            let end = files.iter().map(|f| f.offset + f.size).max().unwrap_or(0);
            let mut next = 0usize;
            reader
                .seek(SeekFrom::Start(folder.data_offset))
                .map_err(|e| Error::io(&self.path, e))?;
            let mut inflate = Decompress::new(false);
            let mut window: Vec<u8> = Vec::with_capacity(WINDOW);
            let mut stored = Vec::new();
            let mut block = vec![0u8; WINDOW];
            let mut position = 0u64;
            for _ in 0..folder.blocks {
                if position >= end {
                    break;
                }
                let mut head = [0u8; 8];
                reader
                    .read_exact(&mut head)
                    .map_err(|e| Error::io(&self.path, e))?;
                let stored_size = u16::from_le_bytes([head[4], head[5]]) as usize;
                let size = u16::from_le_bytes([head[6], head[7]]) as usize;
                stored.resize(self.data_reserve + stored_size, 0);
                reader
                    .read_exact(&mut stored)
                    .map_err(|e| Error::io(&self.path, e))?;
                let input = &stored[self.data_reserve..];
                if size > WINDOW {
                    return Err(bad(position as usize, "a data block over 32 768 bytes"));
                }
                let out = &mut block[..size];
                if folder.mszip {
                    if input.len() < 2 || &input[..2] != b"CK" {
                        return Err(bad(position as usize, "an MSZIP block without `CK`"));
                    }
                    inflate.reset(false);
                    if !window.is_empty() {
                        inflate
                            .set_dictionary(&window)
                            .map_err(|e| bad(position as usize, &e.to_string()))?;
                    }
                    let status = inflate
                        .decompress(&input[2..], out, FlushDecompress::Finish)
                        .map_err(|e| bad(position as usize, &e.to_string()))?;
                    if inflate.total_out() as usize != size || status == Status::BufError {
                        return Err(bad(position as usize, "a short MSZIP block"));
                    }
                    window.clear();
                    window.extend_from_slice(out);
                } else {
                    if input.len() != size {
                        return Err(bad(position as usize, "a stored block of another size"));
                    }
                    out.copy_from_slice(input);
                }
                // Hand the parts of this block that belong to wanted files.
                let block_end = position + size as u64;
                while next < files.len() {
                    let file = files[next];
                    let from = file.offset.max(position);
                    let to = (file.offset + file.size).min(block_end);
                    if from >= block_end {
                        break;
                    }
                    if to > from {
                        sink(
                            file,
                            &out[(from - position) as usize..(to - position) as usize],
                        )?;
                    }
                    if file.offset + file.size <= block_end {
                        sink(file, &[])?;
                        next += 1;
                    } else {
                        break;
                    }
                }
                position = block_end;
            }
            if next < files.len() {
                return Err(bad(position as usize, "the folder ends before its files"));
            }
        }
        Ok(())
    }
}

/// Reads one file of a cabinet whole.
pub fn read_file(path: &Path, name: &str) -> Result<Vec<u8>> {
    let cabinet = Cabinet::open(path)?;
    let mut out = None;
    cabinet.extract(
        |f| f.name.eq_ignore_ascii_case(name),
        |_, piece| {
            out.get_or_insert_with(Vec::new).extend_from_slice(piece);
            Ok(())
        },
    )?;
    out.ok_or_else(|| Error::InvalidArgument(format!("{} has no file {name}", path.display())))
}

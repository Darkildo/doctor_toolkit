//! What the rest of the crate stands on: the error type, reads of binary
//! fields and files replaced through a temporary file.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An operating system I/O error, with the path it happened on.
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// The file does not start with the expected signature.
    #[error("bad {what} magic {found:#010x}")]
    BadMagic { what: &'static str, found: u32 },

    /// The data violates a structural rule of the format.
    #[error("malformed {what} at offset {offset:#x}: {reason}")]
    Malformed {
        what: &'static str,
        offset: usize,
        reason: String,
    },

    /// The operation cannot be carried out with what it was given.
    #[error("{0}")]
    InvalidArgument(String),
}

impl Error {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    pub fn malformed(what: &'static str, offset: usize, reason: impl Into<String>) -> Self {
        Self::Malformed {
            what,
            offset,
            reason: reason.into(),
        }
    }
}

/// Reads of binary fields, checked against the length of the data.
pub mod io {
    use super::{Error, Result};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Endian {
        Little,
        Big,
    }

    fn bytes<const N: usize>(data: &[u8], offset: usize) -> Result<[u8; N]> {
        data.get(offset..offset + N)
            .and_then(|s| s.try_into().ok())
            .ok_or_else(|| Error::malformed("data", offset, "a field past the end"))
    }

    pub fn read_u32(data: &[u8], offset: usize, endian: Endian) -> Result<u32> {
        let raw = bytes::<4>(data, offset)?;
        Ok(match endian {
            Endian::Little => u32::from_le_bytes(raw),
            Endian::Big => u32::from_be_bytes(raw),
        })
    }

    pub fn read_u16(data: &[u8], offset: usize, endian: Endian) -> Result<u16> {
        let raw = bytes::<2>(data, offset)?;
        Ok(match endian {
            Endian::Little => u16::from_le_bytes(raw),
            Endian::Big => u16::from_be_bytes(raw),
        })
    }

    /// The bytes of a NUL-terminated string, without the NUL.
    pub fn cstr(data: &[u8], offset: usize) -> Result<&[u8]> {
        let rest = data
            .get(offset..)
            .ok_or_else(|| Error::malformed("data", offset, "a string past the end"))?;
        let len = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| Error::malformed("data", offset, "a string without its end"))?;
        Ok(&rest[..len])
    }
}

/// Files replaced through a temporary file next to them, so that an
/// interrupted write never leaves half a file under the real name.
pub mod atomic {
    use super::*;

    /// Suffix of temporary files.
    pub const TEMP_SUFFIX: &str = ".gf2-unify-tmp";

    /// The temporary file used to replace `path`: same directory, hidden,
    /// with the process id so that two processes never share it.
    pub fn temp_path(path: &Path) -> Result<PathBuf> {
        let name = path.file_name().ok_or_else(|| {
            Error::InvalidArgument(format!("{} has no file name", path.display()))
        })?;
        let mut temp = std::ffi::OsString::from(".");
        temp.push(name);
        temp.push(format!(".{}{TEMP_SUFFIX}", std::process::id()));
        Ok(path.with_file_name(temp))
    }

    fn write_with(path: &Path, fill: impl FnOnce(&mut File) -> std::io::Result<()>) -> Result<()> {
        let temp = temp_path(path)?;
        let result = File::create(&temp).and_then(|mut file| {
            fill(&mut file)?;
            file.flush()
        });
        match result.and_then(|()| std::fs::rename(&temp, path)) {
            Ok(()) => Ok(()),
            Err(error) => {
                let _ = std::fs::remove_file(&temp);
                Err(Error::io(path, error))
            }
        }
    }

    /// Replaces `path` with `contents`.
    pub fn write(path: &Path, contents: &[u8]) -> Result<()> {
        write_with(path, |file| file.write_all(contents))
    }

    /// Replaces `to` with a copy of `from` (the target of a symbolic link is read).
    pub fn copy(from: &Path, to: &Path) -> Result<()> {
        let mut source = File::open(from).map_err(|e| Error::io(from, e))?;
        write_with(to, |file| std::io::copy(&mut source, file).map(drop))
    }
}

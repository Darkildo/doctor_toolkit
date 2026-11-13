//! Error type shared by all layers of the library.

use std::path::PathBuf;

use crate::compress::refpack::RefpackError;

/// Result alias used throughout the crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Everything that can go wrong while reading or writing game files.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An operating system I/O error, annotated with the path it happened on.
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// A directory given as the game directory holds no game files.
    #[error(
        "{} is not a game directory: it has no .str or .toc files. Choose the folder \
         that contains the game executable and the gf2 folder",
        path.display()
    )]
    NotAGame { path: PathBuf },

    /// A read went past the end of the available data.
    #[error(
        "unexpected end of data: need {needed} byte(s) at offset {offset:#x}, length is {len:#x}"
    )]
    UnexpectedEof {
        offset: usize,
        needed: usize,
        len: usize,
    },

    /// The file does not start with the expected signature.
    #[error("bad {what} magic {found:#010x}")]
    BadMagic { what: &'static str, found: u32 },

    /// The format version is known to exist but is not implemented.
    #[error("unsupported {what} version {version}")]
    UnsupportedVersion { what: &'static str, version: u32 },

    /// The data violates a structural rule of the format.
    #[error("malformed {what} at offset {offset:#x}: {reason}")]
    Malformed {
        what: &'static str,
        offset: usize,
        reason: String,
    },

    /// A RefPack stream could not be decoded or encoded.
    #[error("RefPack: {0}")]
    Refpack(#[from] RefpackError),

    /// A caller supplied an argument the operation cannot honour.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    /// A mod package is malformed, or a mod operation cannot be carried out safely.
    #[error("{0}")]
    Mod(String),
}

impl Error {
    /// Builds an [`Error::Io`] for `path`.
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    /// Builds an [`Error::Mod`].
    pub fn mod_error(message: impl Into<String>) -> Self {
        Self::Mod(message.into())
    }

    /// Builds an [`Error::Malformed`].
    pub fn malformed(what: &'static str, offset: usize, reason: impl Into<String>) -> Self {
        Self::Malformed {
            what,
            offset,
            reason: reason.into(),
        }
    }
}

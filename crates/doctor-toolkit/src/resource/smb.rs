//! `smb` resources: speech banks, the sound ids of recorded lines and the
//! files of the `.viv` archives that hold them. Read only.
//!
//! ```text
//! 0x00  u32  0
//! 0x04  u32  0
//! 0x08  u32  12
//! 0x0C  u32  ?
//! 0x10  u32  number of entries N
//! 0x14  u32  offset of the entry offsets = 0x18
//! 0x18  N × u32 entry offsets, ascending; entries are 0x70 bytes:
//!       +0x00 u16 0x11, u16 0x49
//!       +0x04 4 × u32 sound id (the same value; `chv` lines store it)
//!       +0x14 .. +0x40 playback settings (f32 2.0, 100.0, -1.0, ...)
//!       +0x40 file name, NUL-terminated (`d_mob3_xxx_0001806.exa`)
//! ```
//!
//! The file is `audiostreams\<group>\<name>.snu` in the archives, where the
//! group directory is named after the speaker code (`b3_xxx_0` for `mob3`).

use serde::Serialize;

use crate::hash::sdbm;
use crate::io::{self, Endian};
use crate::{Error, Result};

/// Type id of `smb` resources.
pub const TYPE_SMB: u32 = sdbm(b"smb");

const NAME_AT: usize = 0x40;

/// One recorded line of a bank.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SpeechEntry {
    pub sound: u32,
    /// File name without directories (`d_mob3_xxx_0001806.exa`).
    pub file: String,
}

/// Parses the entries of a speech bank.
pub fn parse(data: &[u8]) -> Result<Vec<SpeechEntry>> {
    let le = Endian::Little;
    let n = io::read_u32(data, 0x10, le)? as usize;
    let table = io::read_u32(data, 0x14, le)? as usize;
    io::slice(data, table, n.saturating_mul(4))?;
    (0..n)
        .map(|i| {
            let at = io::read_u32(data, table + 4 * i, le)? as usize;
            let sound = io::read_u32(data, at + 4, le)?;
            let name = io::cstr(data, at + NAME_AT)?;
            let file = String::from_utf8(name.to_vec())
                .map_err(|_| Error::malformed("smb", at + NAME_AT, "file name is not UTF-8"))?;
            Ok(SpeechEntry { sound, file })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_entries() {
        let mut d = vec![0u8; 0x18];
        d[0x08] = 12;
        d[0x10] = 1;
        d[0x14] = 0x18;
        d.extend_from_slice(&0x1Cu32.to_le_bytes());
        let mut entry = vec![0u8; 0x70];
        entry[0..4].copy_from_slice(&[0x11, 0, 0x49, 0]);
        for k in 0..4 {
            entry[4 + 4 * k..8 + 4 * k].copy_from_slice(&0x09FC_9A57u32.to_le_bytes());
        }
        entry[0x40..0x56].copy_from_slice(b"d_mob3_xxx_0001806.exa");
        d.extend_from_slice(&entry);
        let entries = parse(&d).unwrap();
        assert_eq!(entries[0].sound, 0x09FC_9A57);
        assert_eq!(entries[0].file, "d_mob3_xxx_0001806.exa");
        d[0x10] = 9;
        assert!(parse(&d).is_err());
    }
}

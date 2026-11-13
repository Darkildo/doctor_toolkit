//! Relocatable 32-bit memory images (`rcb`, `bnk`).
//!
//! Several resource types are a C++ object graph written as a 32-bit memory
//! image. Pointers are offsets from the start of the resource. The first `u32`
//! is the offset of a relocation table at the end of the image: the offsets of
//! all pointer fields in descending order, terminated by `0`.
//!
//! ```text
//! 0x00  u32  offset of the relocation table
//! 0x04  u32  format version
//! ...        objects
//!            relocation table: u32 offsets of pointer fields, descending, then u32 0
//! ```

use crate::io::{self, Endian};
use crate::{Error, Result};

/// A validated memory image.
#[derive(Debug, Clone, Copy)]
pub struct Image<'a> {
    pub data: &'a [u8],
    /// Offset of the relocation table; objects end here.
    pub table_at: usize,
}

impl<'a> Image<'a> {
    /// Checks the relocation table: every entry lies before the table, entries
    /// descend and point at pointers whose targets lie inside the objects.
    pub fn parse(data: &'a [u8], what: &'static str) -> Result<Self> {
        let le = Endian::Little;
        let table_at = io::read_u32(data, 0, le)? as usize;
        if table_at < 8 || table_at >= data.len() || !table_at.is_multiple_of(4) {
            return Err(Error::malformed(
                what,
                0,
                format!("relocation table offset {table_at:#x}"),
            ));
        }
        let mut previous = usize::MAX;
        let mut at = table_at;
        loop {
            let field = io::read_u32(data, at, le)? as usize;
            at += 4;
            if field == 0 {
                break;
            }
            if field >= previous || field + 4 > table_at {
                return Err(Error::malformed(what, at - 4, "relocation out of order"));
            }
            let target = io::read_u32(data, field, le)? as usize;
            if target > table_at {
                return Err(Error::malformed(
                    what,
                    field,
                    format!("pointer {target:#x} outside the image"),
                ));
            }
            previous = field;
        }
        Ok(Self { data, table_at })
    }

    /// Offsets of all pointer fields, ascending.
    pub fn relocations(&self) -> Vec<usize> {
        let mut out = Vec::new();
        let mut at = self.table_at;
        while let Ok(field) = self.u32(at) {
            if field == 0 {
                break;
            }
            out.push(field as usize);
            at += 4;
        }
        out.reverse();
        out
    }

    /// Format version (the second `u32`).
    pub fn version(&self) -> u32 {
        self.u32(4).unwrap_or(0)
    }

    pub fn u32(&self, offset: usize) -> Result<u32> {
        io::read_u32(self.data, offset, Endian::Little)
    }

    pub fn i32(&self, offset: usize) -> Result<i32> {
        self.u32(offset).map(|v| v as i32)
    }

    pub fn f32(&self, offset: usize) -> Result<f32> {
        self.u32(offset).map(f32::from_bits)
    }

    /// `count` records of `size` bytes at `offset`, checked against the object area.
    pub fn array(&self, offset: usize, count: usize, size: usize) -> Result<&'a [u8]> {
        let len = count.checked_mul(size).ok_or(Error::UnexpectedEof {
            offset,
            needed: usize::MAX,
            len: self.table_at,
        })?;
        io::slice(&self.data[..self.table_at], offset, len)
    }

    /// NUL-terminated string at `offset`.
    pub fn cstr(&self, offset: usize) -> Result<String> {
        Ok(String::from_utf8_lossy(io::cstr(&self.data[..self.table_at], offset)?).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny image: one pointer at 0x08 pointing to 0x0C.
    fn sample() -> Vec<u8> {
        let mut d = Vec::new();
        for v in [0x10u32, 7, 0x0C, 0xABCD, 0x08, 0] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d
    }

    #[test]
    fn parses_relocation_table() {
        let data = sample();
        let image = Image::parse(&data, "test").unwrap();
        assert_eq!(image.table_at, 0x10);
        assert_eq!(image.version(), 7);
        assert_eq!(image.u32(0x0C).unwrap(), 0xABCD);
        assert!(image.array(0x0C, 2, 4).is_err());
        assert_eq!(image.relocations(), vec![8]);
    }

    #[test]
    fn rejects_bad_tables() {
        let mut data = sample();
        data[0] = 0x40;
        assert!(Image::parse(&data, "test").is_err());
        let mut data = sample();
        data[8] = 0x80; // pointer outside the image
        assert!(Image::parse(&data, "test").is_err());
        let mut data = sample();
        data.truncate(0x14); // no terminator
        assert!(Image::parse(&data, "test").is_err());
        assert!(Image::parse(&[], "test").is_err());
    }
}

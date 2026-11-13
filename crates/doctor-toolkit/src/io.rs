//! Bounds-checked primitives for reading and writing binary data.
//!
//! Offsets and sizes found in game files are untrusted. Every helper here
//! validates them against the buffer and reports [`Error::UnexpectedEof`]
//! instead of panicking.

use std::io::Cursor;

use binrw::{BinRead, BinWrite};

use crate::{Error, Result};

pub use binrw::Endian;

/// Returns `data[offset..offset + len]`.
pub fn slice(data: &[u8], offset: usize, len: usize) -> Result<&[u8]> {
    offset
        .checked_add(len)
        .and_then(|end| data.get(offset..end))
        .ok_or(Error::UnexpectedEof {
            offset,
            needed: len,
            len: data.len(),
        })
}

/// Reads a `u32` at `offset`.
pub fn read_u32(data: &[u8], offset: usize, endian: Endian) -> Result<u32> {
    let bytes: [u8; 4] = slice(data, offset, 4)?
        .try_into()
        .expect("slice of length 4");
    Ok(match endian {
        Endian::Little => u32::from_le_bytes(bytes),
        Endian::Big => u32::from_be_bytes(bytes),
    })
}

/// Reads a `u16` at `offset`.
pub fn read_u16(data: &[u8], offset: usize, endian: Endian) -> Result<u16> {
    let bytes: [u8; 2] = slice(data, offset, 2)?
        .try_into()
        .expect("slice of length 2");
    Ok(match endian {
        Endian::Little => u16::from_le_bytes(bytes),
        Endian::Big => u16::from_be_bytes(bytes),
    })
}

/// Appends a `u32` to `out`.
pub fn put_u32(out: &mut Vec<u8>, value: u32, endian: Endian) {
    match endian {
        Endian::Little => out.extend_from_slice(&value.to_le_bytes()),
        Endian::Big => out.extend_from_slice(&value.to_be_bytes()),
    }
}

/// Appends a `u16` to `out`.
pub fn put_u16(out: &mut Vec<u8>, value: u16, endian: Endian) {
    match endian {
        Endian::Little => out.extend_from_slice(&value.to_le_bytes()),
        Endian::Big => out.extend_from_slice(&value.to_be_bytes()),
    }
}

/// Returns the NUL-terminated byte string starting at `offset`, without the terminator.
pub fn cstr(data: &[u8], offset: usize) -> Result<&[u8]> {
    let tail = data.get(offset..).ok_or(Error::UnexpectedEof {
        offset,
        needed: 1,
        len: data.len(),
    })?;
    let end = tail.iter().position(|&b| b == 0).ok_or(Error::Malformed {
        what: "string",
        offset,
        reason: "missing NUL terminator".into(),
    })?;
    Ok(&tail[..end])
}

/// Reads a fixed-size record described with `binrw` at `offset`.
pub fn read_record<T>(data: &[u8], offset: usize, endian: Endian) -> Result<T>
where
    T: for<'a> BinRead<Args<'a> = ()>,
{
    let tail = data.get(offset..).ok_or(Error::UnexpectedEof {
        offset,
        needed: 1,
        len: data.len(),
    })?;
    T::read_options(&mut Cursor::new(tail), endian, ()).map_err(|e| match e {
        binrw::Error::Io(_) => Error::UnexpectedEof {
            offset,
            needed: std::mem::size_of::<T>(),
            len: data.len(),
        },
        other => Error::malformed("record", offset, other.to_string()),
    })
}

/// Appends a fixed-size record described with `binrw` to `out`.
pub fn put_record<T>(out: &mut Vec<u8>, record: &T, endian: Endian)
where
    T: for<'a> BinWrite<Args<'a> = ()>,
{
    let mut cursor = Cursor::new(Vec::new());
    record
        .write_options(&mut cursor, endian, ())
        .expect("writing a fixed-size record to memory cannot fail");
    out.extend_from_slice(cursor.get_ref());
}

/// Rounds `value` up to the next multiple of `align` (a power of two).
pub const fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slice_rejects_out_of_range_and_overflow() {
        let data = [1u8, 2, 3, 4];
        assert_eq!(slice(&data, 1, 2).unwrap(), &[2, 3]);
        assert!(slice(&data, 3, 2).is_err());
        assert!(slice(&data, usize::MAX, 2).is_err());
    }

    #[test]
    fn integers_respect_endianness() {
        let data = [0x12u8, 0x34, 0x56, 0x78];
        assert_eq!(read_u32(&data, 0, Endian::Little).unwrap(), 0x7856_3412);
        assert_eq!(read_u32(&data, 0, Endian::Big).unwrap(), 0x1234_5678);
        assert_eq!(read_u16(&data, 2, Endian::Little).unwrap(), 0x7856);
        assert!(read_u32(&data, 1, Endian::Little).is_err());
    }

    #[test]
    fn cstr_requires_terminator() {
        assert_eq!(cstr(b"ab\0cd", 0).unwrap(), b"ab");
        assert!(cstr(b"abc", 0).is_err());
        assert!(cstr(b"abc", 9).is_err());
    }

    #[test]
    fn align_up_rounds_to_power_of_two() {
        assert_eq!(align_up(0, 4), 0);
        assert_eq!(align_up(5, 4), 8);
        assert_eq!(align_up(0x800, 0x800), 0x800);
    }
}

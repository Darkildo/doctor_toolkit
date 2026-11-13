//! A minimal zip writer for USDZ packages: stored (uncompressed) entries
//! whose data start at a multiple of an alignment, padded with an extra
//! field (id `0x1986`, as Pixar's writer does).

use crate::hash::crc32;

/// Writes a zip archive of stored files with data aligned to `align` bytes.
pub fn write_stored(files: &[(String, Vec<u8>)], align: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    let put16 = |v: &mut Vec<u8>, x: u16| v.extend_from_slice(&x.to_le_bytes());
    let put32 = |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&x.to_le_bytes());
    // 1980-01-01 00:00.
    let (time, date) = (0u16, 0x21u16);
    for (name, data) in files {
        let offset = out.len();
        let crc = crc32(data);
        let name_bytes = name.as_bytes();
        let unpadded = offset + 30 + name_bytes.len();
        let mut pad = (align - unpadded % align) % align;
        if pad != 0 && pad < 4 {
            pad += align;
        }
        put32(&mut out, 0x0403_4b50);
        put16(&mut out, 10);
        put16(&mut out, 0x0800);
        put16(&mut out, 0);
        put16(&mut out, time);
        put16(&mut out, date);
        put32(&mut out, crc);
        put32(&mut out, data.len() as u32);
        put32(&mut out, data.len() as u32);
        put16(&mut out, name_bytes.len() as u16);
        put16(&mut out, pad as u16);
        out.extend_from_slice(name_bytes);
        if pad > 0 {
            put16(&mut out, 0x1986);
            put16(&mut out, (pad - 4) as u16);
            out.extend(std::iter::repeat_n(0u8, pad - 4));
        }
        debug_assert_eq!(out.len() % align, 0);
        out.extend_from_slice(data);

        put32(&mut central, 0x0201_4b50);
        put16(&mut central, 20);
        put16(&mut central, 10);
        put16(&mut central, 0x0800);
        put16(&mut central, 0);
        put16(&mut central, time);
        put16(&mut central, date);
        put32(&mut central, crc);
        put32(&mut central, data.len() as u32);
        put32(&mut central, data.len() as u32);
        put16(&mut central, name_bytes.len() as u16);
        put16(&mut central, 0);
        put16(&mut central, 0);
        put16(&mut central, 0);
        put16(&mut central, 0);
        put32(&mut central, 0);
        put32(&mut central, offset as u32);
        central.extend_from_slice(name_bytes);
    }
    let central_offset = out.len();
    out.extend_from_slice(&central);
    put32(&mut out, 0x0605_4b50);
    put16(&mut out, 0);
    put16(&mut out, 0);
    put16(&mut out, files.len() as u16);
    put16(&mut out, files.len() as u16);
    put32(&mut out, central.len() as u32);
    put32(&mut out, central_offset as u32);
    put16(&mut out, 0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_are_aligned_and_readable() {
        let files = vec![
            ("model.usdc".to_owned(), vec![1u8; 100]),
            ("textures/a.png".to_owned(), vec![2u8; 33]),
            ("b".to_owned(), vec![]),
        ];
        let zip = write_stored(&files, 64);
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&zip)).unwrap();
        assert_eq!(archive.len(), 3);
        for (i, (name, data)) in files.iter().enumerate() {
            let mut entry = archive.by_index(i).unwrap();
            assert_eq!(entry.name().unwrap(), name.as_str());
            assert_eq!(entry.data_start().unwrap() % 64, 0, "{name}");
            let mut back = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut back).unwrap();
            assert_eq!(&back, data);
        }
    }
}

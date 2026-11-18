//! `lh2` resources: localised string tables (magic `LCH2`).
//!
//! # Layout
//!
//! ```text
//! 0x00  "LCH2"
//! 0x04  u32  total size
//! 0x08  u16  version, 1
//! 0x0A  u16  flags, 0x4000 in every file (strings are UTF-8)
//! 0x0C  u32  offset of the language table, 0 when the file has one language
//! 0x10  u32  number of entries N
//! 0x14  u32  number of languages M
//! 0x18  u32  0
//! 0x1C  u32  0
//! 0x20  N × u32   entry hashes, sorted ascending (SDBM of the lower-case
//!                 entry name, [`crate::hash::string_id`])
//!       N*M × u32 string offsets, language-major: offsets[lang * N + entry]
//!       strings: NUL-terminated UTF-8 in offset order, no sharing, no padding
//!       language table (optional): M × 8-byte NUL-padded codes ("en", "cs", ...)
//! ```
//!
//! The main text streams (`gf2/text/*`) hold one language each; the `lhr`
//! resource maps language codes to those streams.
//!
//! The game finds a string by binary search over the hashes of every loaded
//! table, the most recently loaded first (`godfather2.exe` `0x006039D0`,
//! `0x00605D00`). A new entry therefore only has to keep the hashes sorted;
//! the order of the strings follows the entries.

use crate::io::{self, Endian};
use crate::{Error, Result};

const MAGIC: &[u8; 4] = b"LCH2";
const HEADER_SIZE: usize = 0x20;
const LANGUAGE_CODE_SIZE: usize = 8;

/// Far more languages than the tables of the game have (four).
const MAX_LANGUAGES: usize = 256;

/// A parsed string table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StringTable {
    pub version: u16,
    pub flags: u16,
    /// Language codes; empty when the file does not name its language.
    pub languages: Vec<String>,
    /// Entry hashes, sorted ascending: [`crate::hash::string_id`] of the entry names.
    pub hashes: Vec<u32>,
    /// `strings[lang][entry]`.
    pub strings: Vec<Vec<String>>,
}

impl StringTable {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let le = Endian::Little;
        if io::slice(data, 0, 4)? != MAGIC {
            return Err(Error::BadMagic {
                what: "LCH2",
                found: io::read_u32(data, 0, le)?,
            });
        }
        let version = io::read_u16(data, 8, le)?;
        let flags = io::read_u16(data, 10, le)?;
        let languages_at = io::read_u32(data, 0x0C, le)? as usize;
        let n = io::read_u32(data, 0x10, le)? as usize;
        let m = io::read_u32(data, 0x14, le)? as usize;
        let offsets_at = HEADER_SIZE + 4 * n;
        // Reject counts the file cannot hold before allocating for them.
        io::slice(data, HEADER_SIZE, n.saturating_mul(4 * (m + 1)))?;
        // With no entries the line above holds for any number of languages.
        if m > MAX_LANGUAGES {
            return Err(Error::malformed("LCH2", 0x14, format!("{m} languages")));
        }
        let hashes = (0..n)
            .map(|i| io::read_u32(data, HEADER_SIZE + 4 * i, le))
            .collect::<Result<_>>()?;
        let mut strings = Vec::with_capacity(m);
        for lang in 0..m {
            let mut column = Vec::with_capacity(n);
            for entry in 0..n {
                let offset = io::read_u32(data, offsets_at + 4 * (lang * n + entry), le)? as usize;
                let bytes = io::cstr(data, offset)?;
                let text = String::from_utf8(bytes.to_vec())
                    .map_err(|_| Error::malformed("LCH2", offset, "string is not UTF-8"))?;
                column.push(text);
            }
            strings.push(column);
        }
        let languages = if languages_at == 0 {
            Vec::new()
        } else {
            (0..m)
                .map(|i| {
                    let code = io::slice(
                        data,
                        languages_at + LANGUAGE_CODE_SIZE * i,
                        LANGUAGE_CODE_SIZE,
                    )?;
                    let end = code.iter().position(|&b| b == 0).unwrap_or(code.len());
                    Ok(String::from_utf8_lossy(&code[..end]).into_owned())
                })
                .collect::<Result<_>>()?
        };
        Ok(Self {
            version,
            flags,
            languages,
            hashes,
            strings,
        })
    }

    /// Serialises the table. Parsing and writing back a game file yields the same bytes.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let le = Endian::Little;
        let n = self.hashes.len();
        let m = self.strings.len();
        if self.strings.iter().any(|column| column.len() != n) {
            return Err(Error::InvalidArgument(
                "every language needs one string per entry".into(),
            ));
        }
        if !self.languages.is_empty() && self.languages.len() != m {
            return Err(Error::InvalidArgument(
                "one language code per language column".into(),
            ));
        }
        let mut pool = Vec::new();
        let mut offsets = Vec::with_capacity(n * m);
        let pool_at = HEADER_SIZE + 4 * n + 4 * n * m;
        for column in &self.strings {
            for text in column {
                if text.contains('\0') {
                    return Err(Error::InvalidArgument(
                        "string contains a NUL character".into(),
                    ));
                }
                offsets.push((pool_at + pool.len()) as u32);
                pool.extend_from_slice(text.as_bytes());
                pool.push(0);
            }
        }
        let languages_at = if self.languages.is_empty() {
            0
        } else {
            pool_at + pool.len()
        };
        let total = pool_at + pool.len() + LANGUAGE_CODE_SIZE * self.languages.len();

        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(MAGIC);
        io::put_u32(&mut out, total as u32, le);
        io::put_u16(&mut out, self.version, le);
        io::put_u16(&mut out, self.flags, le);
        for value in [languages_at, n, m, 0, 0] {
            io::put_u32(&mut out, value as u32, le);
        }
        for &hash in &self.hashes {
            io::put_u32(&mut out, hash, le);
        }
        for offset in offsets {
            io::put_u32(&mut out, offset, le);
        }
        out.extend_from_slice(&pool);
        for code in &self.languages {
            if code.len() >= LANGUAGE_CODE_SIZE {
                return Err(Error::InvalidArgument(format!(
                    "language code `{code}` too long"
                )));
            }
            let mut field = [0u8; LANGUAGE_CODE_SIZE];
            field[..code.len()].copy_from_slice(code.as_bytes());
            out.extend_from_slice(&field);
        }
        debug_assert_eq!(out.len(), total);
        Ok(out)
    }

    /// Index of the entry with `hash`.
    pub fn find(&self, hash: u32) -> Option<usize> {
        self.hashes.binary_search(&hash).ok()
    }

    /// Inserts a new entry at its place in the hash order, with one text per
    /// language column. Returns its index. Fails when the hash is taken.
    pub fn insert(&mut self, hash: u32, texts: &[String]) -> Result<usize> {
        if texts.len() != self.strings.len() {
            return Err(Error::InvalidArgument(format!(
                "{} text(s) for {} language(s)",
                texts.len(),
                self.strings.len()
            )));
        }
        if texts.iter().any(|t| t.contains('\0')) {
            return Err(Error::InvalidArgument(
                "string contains a NUL character".into(),
            ));
        }
        let at = match self.hashes.binary_search(&hash) {
            Ok(_) => {
                return Err(Error::InvalidArgument(format!(
                    "the table already has an entry {hash:08x}"
                )));
            }
            Err(at) => at,
        };
        self.hashes.insert(at, hash);
        for (column, text) in self.strings.iter_mut().zip(texts) {
            column.insert(at, text.clone());
        }
        Ok(at)
    }

    /// Names of the language columns for addressing a text: the language
    /// codes, or the column numbers `0`, `1`, … when the table does not name
    /// its languages.
    pub fn column_names(&self) -> Vec<String> {
        if self.languages.len() == self.strings.len() && !self.languages.is_empty() {
            self.languages.clone()
        } else {
            (0..self.strings.len()).map(|i| i.to_string()).collect()
        }
    }

    /// Text of entry `hash` in every language column.
    pub fn texts(&self, hash: u32) -> Option<Vec<&str>> {
        let at = self.find(hash)?;
        Some(self.strings.iter().map(|c| c[at].as_str()).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> StringTable {
        StringTable {
            version: 1,
            flags: 0x4000,
            languages: vec!["en".into(), "cs".into()],
            hashes: vec![0x0266_681F, 0x02E1_E52A],
            strings: vec![
                vec!["Recommended Players 8-14".into(), "Cuban Streets".into()],
                vec![
                    "Doporučený počet hráčů 8-14".into(),
                    "Kubánské ulice".into(),
                ],
            ],
        }
    }

    #[test]
    fn roundtrip_with_language_table() {
        let table = sample();
        let bytes = table.to_bytes().unwrap();
        assert_eq!(&bytes[..4], b"LCH2");
        assert_eq!(
            io::read_u32(&bytes, 4, Endian::Little).unwrap() as usize,
            bytes.len()
        );
        assert_eq!(StringTable::parse(&bytes).unwrap(), table);
        // Language-major offsets: the second offset belongs to English entry 1.
        let second = io::read_u32(&bytes, 0x20 + 8 + 4, Endian::Little).unwrap() as usize;
        assert_eq!(io::cstr(&bytes, second).unwrap(), b"Cuban Streets");
        assert_eq!(table.find(0x02E1_E52A), Some(1));
    }

    #[test]
    fn inserted_entries_keep_the_hash_order() {
        let mut table = sample();
        let hash = crate::hash::string_id("doctor_new");
        let at = table
            .insert(hash, &["New".to_owned(), "Nový".to_owned()])
            .unwrap();
        assert!(table.hashes.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(table.hashes[at], hash);
        assert_eq!(table.texts(hash).unwrap(), vec!["New", "Nový"]);
        let parsed = StringTable::parse(&table.to_bytes().unwrap()).unwrap();
        assert_eq!(parsed, table);
        // A taken hash, a wrong number of texts, a NUL.
        assert!(table.insert(hash, &["a".into(), "b".into()]).is_err());
        assert!(table.insert(1, &["a".into()]).is_err());
        assert!(table.insert(1, &["a\0".into(), "b".into()]).is_err());
        assert!(table.insert(0, &["first".into(), "první".into()]).is_ok());
        assert_eq!(table.hashes[0], 0);
    }

    #[test]
    fn single_language_files_have_no_table() {
        let table = StringTable {
            languages: Vec::new(),
            strings: vec![sample().strings[0].clone()],
            ..sample()
        };
        let bytes = table.to_bytes().unwrap();
        assert_eq!(io::read_u32(&bytes, 0x0C, Endian::Little).unwrap(), 0);
        assert_eq!(StringTable::parse(&bytes).unwrap(), table);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(StringTable::parse(b"").is_err());
        assert!(StringTable::parse(b"LCH1\0\0\0\0").is_err());
        let bytes = sample().to_bytes().unwrap();
        for len in 0..bytes.len() {
            let _ = StringTable::parse(&bytes[..len]);
        }
        let mut huge = bytes.clone();
        huge[0x10..0x14].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(StringTable::parse(&huge).is_err());
        // Found by fuzzing: no entries and four billion languages.
        let mut empty = StringTable {
            languages: Vec::new(),
            hashes: Vec::new(),
            strings: Vec::new(),
            ..sample()
        }
        .to_bytes()
        .unwrap();
        empty[0x14..0x18].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(StringTable::parse(&empty).is_err());
        let mut bad = sample();
        bad.strings[1].pop();
        assert!(bad.to_bytes().is_err());
    }
}

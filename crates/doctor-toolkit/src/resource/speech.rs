//! Subtitles of recorded speech: the metadata of `.snu` files in the `.viv`
//! archives.
//!
//! A speech file has metadata between its 0x20-byte header and the SNS
//! blocks (the blocks then start after 0x20, `docs/formats/audio.md` §4):
//!
//! ```text
//! 0x20  u32  0xCAE87732
//!       u32  1
//!       u32  0xD1A3DDF5
//!       u32  number of chunks K
//!       K × { u32 chunk id, u32 offset from 0x20, u32 size }
//!       chunks, one after another; zero padding up to the SNS blocks,
//!       which start at the next multiple of 16 (header field 0x08)
//! ```
//!
//! Chunk `0x3596A436` is an `LCH2` string table ([`StringTable`]) with one
//! entry, `string_id` of the line name (`mob3_xxx_0001806`), in eight columns
//! `EN CS NL PL RU ES SS LL`: the subtitle in every language (the `SS`
//! column holds the line name). Chunk `0x30B00ADF`, present in about half of
//! the files, is not decoded (lip sync data, most likely). Music and other
//! streamed sounds have other metadata or none.

use crate::io::{self, Endian};
use crate::resource::lh2::StringTable;
use crate::{Error, Result};

const MAGIC: u32 = 0xCAE8_7732;
const TABLE_TAG: u32 = 0xD1A3_DDF5;
/// Chunk id of the subtitle table.
pub const CHUNK_SUBTITLES: u32 = 0x3596_A436;
const META_AT: usize = 0x20;

/// The metadata chunks of a speech file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechMeta {
    pub version: u32,
    /// `(chunk id, data)` in file order.
    pub chunks: Vec<(u32, Vec<u8>)>,
}

impl SpeechMeta {
    /// Reads the metadata of an `.snu` file; `None` when the file has no
    /// speech metadata (music, effects).
    pub fn parse(snu: &[u8]) -> Result<Option<Self>> {
        let le = Endian::Little;
        if snu.len() < META_AT + 16 {
            return Ok(None);
        }
        // The flags byte 1 does not tell: a quarter of the speech files with
        // metadata have 0 there. Metadata is there when the blocks start
        // after the header.
        let blocks = io::read_u32(snu, 8, le)? as usize;
        if blocks < META_AT + 16 {
            return Ok(None);
        }
        if io::read_u32(snu, META_AT, le)? != MAGIC
            || io::read_u32(snu, META_AT + 8, le)? != TABLE_TAG
        {
            return Ok(None);
        }
        let version = io::read_u32(snu, META_AT + 4, le)?;
        let count = io::read_u32(snu, META_AT + 12, le)? as usize;
        if count > 64 {
            return Err(Error::malformed(
                "snu",
                META_AT + 12,
                format!("{count} chunks"),
            ));
        }
        let mut chunks = Vec::with_capacity(count);
        for i in 0..count {
            let at = META_AT + 16 + 12 * i;
            let id = io::read_u32(snu, at, le)?;
            let offset = io::read_u32(snu, at + 4, le)? as usize;
            let size = io::read_u32(snu, at + 8, le)? as usize;
            let start = META_AT + offset;
            if start + size > blocks {
                return Err(Error::malformed("snu", at, "chunk runs into the audio"));
            }
            chunks.push((id, io::slice(snu, start, size)?.to_vec()));
        }
        Ok(Some(Self { version, chunks }))
    }

    /// The subtitle table, when the file has one.
    pub fn subtitles(&self) -> Option<Result<StringTable>> {
        self.chunks
            .iter()
            .find(|(id, _)| *id == CHUNK_SUBTITLES)
            .map(|(_, data)| StringTable::parse(data))
    }

    /// Replaces the subtitle table.
    pub fn set_subtitles(&mut self, table: &StringTable) -> Result<()> {
        let data = table.to_bytes()?;
        match self
            .chunks
            .iter_mut()
            .find(|(id, _)| *id == CHUNK_SUBTITLES)
        {
            Some(chunk) => chunk.1 = data,
            None => self.chunks.push((CHUNK_SUBTITLES, data)),
        }
        Ok(())
    }

    /// The metadata bytes as the game lays them out (from 0x20 up to the
    /// SNS blocks, padding included).
    pub fn to_bytes(&self) -> Vec<u8> {
        let le = Endian::Little;
        let mut out = Vec::new();
        io::put_u32(&mut out, MAGIC, le);
        io::put_u32(&mut out, self.version, le);
        io::put_u32(&mut out, TABLE_TAG, le);
        io::put_u32(&mut out, self.chunks.len() as u32, le);
        let mut offset = 16 + 12 * self.chunks.len();
        for (id, data) in &self.chunks {
            io::put_u32(&mut out, *id, le);
            io::put_u32(&mut out, offset as u32, le);
            io::put_u32(&mut out, data.len() as u32, le);
            offset += data.len();
        }
        for (_, data) in &self.chunks {
            out.extend_from_slice(data);
        }
        out.resize((META_AT + out.len()).next_multiple_of(16) - META_AT, 0);
        out
    }
}

/// The subtitles of a speech file; `None` when it has none.
pub fn subtitles(snu: &[u8]) -> Result<Option<StringTable>> {
    match SpeechMeta::parse(snu)? {
        Some(meta) => meta.subtitles().transpose(),
        None => Ok(None),
    }
}

/// A copy of a speech file with another subtitle table; the audio is kept.
pub fn with_subtitles(snu: &[u8], table: &StringTable) -> Result<Vec<u8>> {
    let mut meta = SpeechMeta::parse(snu)?
        .ok_or_else(|| Error::InvalidArgument("the sound has no speech metadata".into()))?;
    meta.set_subtitles(table)?;
    let blocks = io::read_u32(snu, 8, Endian::Little)? as usize;
    let audio = io::slice(snu, blocks, snu.len().saturating_sub(blocks))?;
    let mut out = snu[..META_AT].to_vec();
    out.extend_from_slice(&meta.to_bytes());
    let at = out.len() as u32;
    out[8..12].copy_from_slice(&at.to_le_bytes());
    out.extend_from_slice(audio);
    Ok(out)
}

/// The language columns of the subtitle tables of the game, in file order.
/// `SS` holds the line name, `LL` repeats the English text.
pub const LANGUAGES: [&str; 8] = ["EN", "CS", "NL", "PL", "RU", "ES", "SS", "LL"];

/// The subtitle table of a new line named `line` (`doc1_xxx_0000001`): one
/// entry, `string_id(line)`, in the columns of the game. `texts` are
/// `(language code, text)` pairs; a language without a text gets the English
/// one (or the first given).
pub fn new_table(line: &str, texts: &[(String, String)]) -> Result<StringTable> {
    let find = |code: &str| {
        texts
            .iter()
            .find(|(c, _)| c.eq_ignore_ascii_case(code))
            .map(|(_, t)| t.as_str())
    };
    if let Some((code, _)) = texts.iter().find(|(c, _)| {
        !LANGUAGES.iter().any(|l| l.eq_ignore_ascii_case(c)) || c.eq_ignore_ascii_case("SS")
    }) {
        return Err(Error::InvalidArgument(format!(
            "no subtitle language `{code}` ({})",
            LANGUAGES.join(" ")
        )));
    }
    let fallback = find("EN")
        .or_else(|| texts.first().map(|(_, t)| t.as_str()))
        .unwrap_or("");
    let strings = LANGUAGES
        .iter()
        .map(|&code| {
            vec![match code {
                "SS" => line.to_owned(),
                _ => find(code).unwrap_or(fallback).to_owned(),
            }]
        })
        .collect();
    Ok(StringTable {
        version: 1,
        flags: 0x4000,
        languages: LANGUAGES.map(String::from).to_vec(),
        hashes: vec![crate::hash::string_id(line)],
        strings,
    })
}

/// A copy of an `.snu` file with this subtitle table as its only metadata:
/// what a new speech file needs. Metadata the file had (subtitles and the
/// undecoded chunk of another line) is dropped; the audio is kept.
pub fn attach_subtitles(snu: &[u8], table: &StringTable) -> Result<Vec<u8>> {
    let blocks = io::read_u32(snu, 8, Endian::Little)? as usize;
    if blocks < META_AT {
        return Err(Error::malformed("snu", 8, "blocks start inside the header"));
    }
    let audio = io::slice(snu, blocks, snu.len().saturating_sub(blocks))?;
    let meta = SpeechMeta {
        version: 1,
        chunks: vec![(CHUNK_SUBTITLES, table.to_bytes()?)],
    };
    let mut out = snu[..META_AT].to_vec();
    out.extend_from_slice(&meta.to_bytes());
    let at = out.len() as u32;
    out[8..12].copy_from_slice(&at.to_le_bytes());
    out.extend_from_slice(audio);
    Ok(out)
}

/// Column of a subtitle table for a language code (`en`, `es`, ...), by the
/// table's own codes, case ignored.
pub fn language_column(table: &StringTable, code: &str) -> Option<usize> {
    table
        .languages
        .iter()
        .position(|l| l.eq_ignore_ascii_case(code))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::string_id;

    fn table() -> StringTable {
        StringTable {
            version: 1,
            flags: 0x4000,
            languages: ["EN", "CS", "NL", "PL", "RU", "ES", "SS", "LL"]
                .map(String::from)
                .to_vec(),
            hashes: vec![string_id("mob3_xxx_0001806")],
            strings: (0..8).map(|i| vec![format!("line {i}")]).collect(),
        }
    }

    fn snu(meta: &SpeechMeta) -> Vec<u8> {
        let mut out = vec![0u8; 0x20];
        out[0] = 3;
        out[1] = 0;
        out[3] = 1;
        out.extend_from_slice(&meta.to_bytes());
        let at = out.len() as u32;
        out[8..12].copy_from_slice(&at.to_le_bytes());
        out.extend_from_slice(&[0x80, 0, 0, 8, 1, 2, 3, 4]);
        out
    }

    #[test]
    fn replaces_subtitles_and_keeps_audio() {
        let meta = SpeechMeta {
            version: 1,
            chunks: vec![
                (0x30B0_0ADF, vec![9; 13]),
                (CHUNK_SUBTITLES, table().to_bytes().unwrap()),
            ],
        };
        let file = snu(&meta);
        assert_eq!(SpeechMeta::parse(&file).unwrap().unwrap(), meta);
        assert_eq!(subtitles(&file).unwrap().unwrap(), table());
        let mut t = table();
        t.strings[0][0] = "A much longer English subtitle than before".into();
        let edited = with_subtitles(&file, &t).unwrap();
        assert_eq!(subtitles(&edited).unwrap().unwrap(), t);
        let blocks = u32::from_le_bytes(edited[8..12].try_into().unwrap()) as usize;
        assert_eq!(blocks % 16, 0);
        assert_eq!(&edited[blocks..], &[0x80, 0, 0, 8, 1, 2, 3, 4]);
        // Same table: same bytes.
        assert_eq!(with_subtitles(&file, &table()).unwrap(), file);
        assert_eq!(language_column(&t, "es"), Some(5));
    }

    #[test]
    fn new_lines_get_a_table_of_their_own() {
        let texts = vec![
            ("en".to_owned(), "Hello.".to_owned()),
            ("ru".to_owned(), "Привет.".to_owned()),
        ];
        let t = new_table("doc1_xxx_0000001", &texts).unwrap();
        assert_eq!(t.hashes, vec![string_id("doc1_xxx_0000001")]);
        assert_eq!(t.strings[0][0], "Hello.");
        assert_eq!(t.strings[4][0], "Привет.");
        assert_eq!(t.strings[5][0], "Hello.");
        assert_eq!(t.strings[6][0], "doc1_xxx_0000001");
        assert_eq!(t.strings[7][0], "Hello.");
        assert!(new_table("x", &[("xx".to_owned(), String::new())]).is_err());
        // A sound without metadata, and one with the metadata of another line.
        let mut plain = vec![0u8; 0x20];
        plain[0] = 3;
        plain[8] = 0x20;
        plain.extend_from_slice(&[0x80, 0, 0, 8, 1, 2, 3, 4]);
        let other = snu(&SpeechMeta {
            version: 1,
            chunks: vec![(0x30B0_0ADF, vec![9; 13])],
        });
        for file in [plain, other] {
            let new = attach_subtitles(&file, &t).unwrap();
            assert_eq!(subtitles(&new).unwrap().unwrap(), t);
            assert_eq!(SpeechMeta::parse(&new).unwrap().unwrap().chunks.len(), 1);
            let blocks = u32::from_le_bytes(new[8..12].try_into().unwrap()) as usize;
            assert_eq!(blocks % 16, 0);
            assert_eq!(&new[blocks..], &[0x80, 0, 0, 8, 1, 2, 3, 4]);
        }
    }

    #[test]
    fn other_sounds_have_no_subtitles() {
        let mut file = vec![0u8; 0x40];
        file[0] = 3;
        assert!(subtitles(&file).unwrap().is_none());
        file[1] = 4;
        assert!(subtitles(&file).unwrap().is_none());
        assert!(with_subtitles(&file, &table()).is_err());
    }
}

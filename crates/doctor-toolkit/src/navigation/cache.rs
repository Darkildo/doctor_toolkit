//! On-disk cache of the game index.
//!
//! A little-endian binary file: a header with the format and harvest
//! versions, the stamp of the name lists and the game directory, then one chunk per stream with the stamp
//! of the stream file and its records. Strings of a chunk are stored once in
//! a table at the start of the chunk and referenced by number.

use std::collections::HashMap;
use std::path::Path;

use crate::container::ustream::ResourceHeader;
use crate::{Error, Result};

use super::index::{
    Anchor, Chunk, HARVEST_VERSION, HashKind, NameKind, Record, Ref, Stamp, Target, TextEntry,
    names_stamp,
};

const MAGIC: &[u8; 8] = b"DTKINDEX";
const FORMAT_VERSION: u32 = 2;

/// Encodes the chunks of the index of the game in `root`.
pub(crate) fn write(root: &Path, chunks: &[Chunk]) -> Vec<u8> {
    let mut w = Writer::default();
    w.out.extend_from_slice(MAGIC);
    w.u32(FORMAT_VERSION);
    w.u32(HARVEST_VERSION);
    w.u32(names_stamp());
    w.raw_str(&root.to_string_lossy());
    w.u32(chunks.len() as u32);
    for chunk in chunks {
        write_chunk(&mut w, chunk);
    }
    w.out
}

/// Decodes a cache file written for the game in `root`.
pub(crate) fn read(path: &Path, root: &Path) -> Result<Vec<Chunk>> {
    let data = std::fs::read(path).map_err(|e| Error::io(path, e))?;
    let mut r = Reader { data: &data, at: 0 };
    if r.bytes(MAGIC.len())? != MAGIC {
        return Err(r.bad("not an index cache"));
    }
    if r.u32()? != FORMAT_VERSION || r.u32()? != HARVEST_VERSION {
        return Err(r.bad("written by another version"));
    }
    if r.u32()? != names_stamp() {
        return Err(r.bad("written with other name lists"));
    }
    if r.raw_str()? != root.to_string_lossy() {
        return Err(r.bad("written for another game directory"));
    }
    let count = r.u32()? as usize;
    let mut chunks = Vec::with_capacity(count.min(1 << 16));
    for _ in 0..count {
        chunks.push(read_chunk(&mut r)?);
    }
    if r.at != data.len() {
        return Err(r.bad("trailing data"));
    }
    Ok(chunks)
}

// ------------------------------------------------------------------ writing

#[derive(Default)]
struct Writer {
    out: Vec<u8>,
}

impl Writer {
    fn u8(&mut self, v: u8) {
        self.out.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }
    fn raw_str(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.out.extend_from_slice(s.as_bytes());
    }
}

/// Numbers the strings of a chunk in order of first use.
#[derive(Default)]
struct Strings<'a> {
    numbers: HashMap<&'a str, u32>,
    list: Vec<&'a str>,
}

impl<'a> Strings<'a> {
    fn add(&mut self, s: &'a str) {
        if !self.numbers.contains_key(s) {
            self.numbers.insert(s, self.list.len() as u32);
            self.list.push(s);
        }
    }
    fn get(&self, s: &str) -> u32 {
        self.numbers[s]
    }
}

fn write_chunk(w: &mut Writer, chunk: &Chunk) {
    let mut strings = Strings::default();
    for r in &chunk.records {
        r.names.iter().for_each(|(_, s)| strings.add(s));
        for reference in &r.refs {
            strings.add(&reference.label);
            if let Target::Named { name, .. } = &reference.target {
                strings.add(name);
            }
        }
        r.pairs.iter().for_each(|(_, s)| strings.add(s));
        r.languages.iter().for_each(|s| strings.add(s));
        r.texts.iter().for_each(|t| strings.add(&t.text));
        if let Some(e) = &r.error {
            strings.add(e);
        }
    }
    w.raw_str(&chunk.rel_path);
    w.u64(chunk.stamp.size);
    w.u64(chunk.stamp.mtime_ns);
    w.u64(chunk.stamp.file_id);
    match &chunk.error {
        Some(e) => {
            w.u8(1);
            w.raw_str(e);
        }
        None => w.u8(0),
    }
    w.u32(strings.list.len() as u32);
    for s in &strings.list {
        w.raw_str(s);
    }
    w.u32(chunk.records.len() as u32);
    for r in &chunk.records {
        let h = &r.header;
        w.u32(r.resource);
        for v in [h.mem_policy, h.alignment, h.type_id, h.data_size] {
            w.u32(v);
        }
        h.ids.iter().for_each(|&v| w.u32(v));
        w.u32(r.stored_size);
        w.u8(u8::from(r.compressed));
        match &r.error {
            Some(e) => w.u32(strings.get(e) + 1),
            None => w.u32(0),
        }
        w.u32(r.names.len() as u32);
        for (kind, s) in &r.names {
            w.u8(*kind as u8);
            w.u32(strings.get(s));
        }
        w.u32(r.refs.len() as u32);
        for reference in &r.refs {
            w.u32(strings.get(&reference.label));
            match &reference.target {
                Target::Resource { type_id, id } => {
                    w.u8(0);
                    w.u32(*type_id);
                    w.u32(*id);
                }
                Target::Material(hash) => {
                    w.u8(1);
                    w.u32(*hash);
                }
                Target::Guid(guid) => {
                    w.u8(2);
                    w.out.extend_from_slice(guid);
                }
                Target::Bones(bones) => {
                    w.u8(3);
                    w.u32(bones.len() as u32);
                    for &(hash, index) in bones.iter() {
                        w.u32(hash);
                        w.u32(index);
                    }
                }
                Target::Named { type_id, name } => {
                    w.u8(4);
                    w.u32(*type_id);
                    w.u32(strings.get(name));
                }
            }
        }
        w.u32(r.anchors.len() as u32);
        for anchor in &r.anchors {
            match anchor {
                Anchor::Material(hash) => {
                    w.u8(0);
                    w.u32(*hash);
                }
                Anchor::Guid(guid) => {
                    w.u8(1);
                    w.out.extend_from_slice(guid);
                }
                Anchor::Skeleton(bones) => {
                    w.u8(2);
                    w.u32(bones.len() as u32);
                    bones.iter().for_each(|&b| w.u32(b));
                }
            }
        }
        w.u32(r.hashes.len() as u32);
        for (kind, hash) in &r.hashes {
            w.u8(*kind as u8);
            w.u32(*hash);
        }
        w.u32(r.pairs.len() as u32);
        for (hash, s) in &r.pairs {
            w.u32(*hash);
            w.u32(strings.get(s));
        }
        w.u32(r.languages.len() as u32);
        r.languages.iter().for_each(|s| w.u32(strings.get(s)));
        w.u32(r.texts.len() as u32);
        for t in &r.texts {
            w.u32(t.hash);
            w.u16(t.language);
            w.u32(strings.get(&t.text));
        }
    }
}

// ------------------------------------------------------------------ reading

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn bad(&self, reason: &str) -> Error {
        Error::malformed("index cache", self.at, reason)
    }
    fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let slice = crate::io::slice(self.data, self.at, n)?;
        self.at += n;
        Ok(slice)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(
            self.bytes(2)?.try_into().expect("2 bytes"),
        ))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(
            self.bytes(4)?.try_into().expect("4 bytes"),
        ))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(
            self.bytes(8)?.try_into().expect("8 bytes"),
        ))
    }
    fn guid(&mut self) -> Result<[u8; 16]> {
        Ok(self.bytes(16)?.try_into().expect("16 bytes"))
    }
    fn raw_str(&mut self) -> Result<String> {
        let len = self.u32()? as usize;
        let bytes = self.bytes(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| self.bad("string is not UTF-8"))
    }
    /// A count of items that take at least `min_size` bytes each.
    fn count(&mut self, min_size: usize) -> Result<usize> {
        let n = self.u32()? as usize;
        if n.saturating_mul(min_size) > self.data.len() - self.at {
            return Err(self.bad("count larger than the file"));
        }
        Ok(n)
    }
}

fn read_chunk(r: &mut Reader<'_>) -> Result<Chunk> {
    let rel_path = r.raw_str()?;
    let stamp = Stamp {
        size: r.u64()?,
        mtime_ns: r.u64()?,
        file_id: r.u64()?,
    };
    let error = match r.u8()? {
        0 => None,
        _ => Some(r.raw_str()?),
    };
    let string_count = r.count(4)?;
    let strings: Vec<String> = (0..string_count)
        .map(|_| r.raw_str())
        .collect::<Result<_>>()?;
    let string = |r: &mut Reader<'_>| -> Result<String> {
        let n = r.u32()? as usize;
        strings
            .get(n)
            .cloned()
            .ok_or_else(|| r.bad("string number out of range"))
    };
    let count = r.count(4 * 13)?;
    let mut records = Vec::with_capacity(count);
    for _ in 0..count {
        let resource = r.u32()?;
        let [mem_policy, alignment, type_id, data_size] = [r.u32()?, r.u32()?, r.u32()?, r.u32()?];
        let ids = [r.u32()?, r.u32()?, r.u32()?, r.u32()?];
        let header = ResourceHeader {
            mem_policy,
            alignment,
            type_id,
            ids,
            data_size,
        };
        let stored_size = r.u32()?;
        let compressed = r.u8()? != 0;
        let error = match r.u32()? {
            0 => None,
            n => Some(
                strings
                    .get(n as usize - 1)
                    .cloned()
                    .ok_or_else(|| r.bad("string number out of range"))?,
            ),
        };
        let mut record = Record {
            resource,
            header,
            stored_size,
            compressed,
            error,
            ..Record::default()
        };
        for _ in 0..r.count(5)? {
            let kind = NameKind::from_u8(r.u8()?).ok_or_else(|| r.bad("name kind"))?;
            record.names.push((kind, string(r)?));
        }
        for _ in 0..r.count(5)? {
            let label = string(r)?;
            let target = match r.u8()? {
                0 => Target::Resource {
                    type_id: r.u32()?,
                    id: r.u32()?,
                },
                1 => Target::Material(r.u32()?),
                2 => Target::Guid(r.guid()?),
                3 => {
                    let n = r.count(8)?;
                    let bones = (0..n)
                        .map(|_| Ok((r.u32()?, r.u32()?)))
                        .collect::<Result<Vec<_>>>()?;
                    Target::Bones(bones.into_boxed_slice())
                }
                4 => Target::Named {
                    type_id: r.u32()?,
                    name: string(r)?,
                },
                _ => return Err(r.bad("reference kind")),
            };
            record.refs.push(Ref { label, target });
        }
        for _ in 0..r.count(5)? {
            let anchor = match r.u8()? {
                0 => Anchor::Material(r.u32()?),
                1 => Anchor::Guid(r.guid()?),
                2 => {
                    let n = r.count(4)?;
                    let bones = (0..n).map(|_| r.u32()).collect::<Result<Vec<_>>>()?;
                    Anchor::Skeleton(bones.into_boxed_slice())
                }
                _ => return Err(r.bad("anchor kind")),
            };
            record.anchors.push(anchor);
        }
        for _ in 0..r.count(5)? {
            let kind = HashKind::from_u8(r.u8()?).ok_or_else(|| r.bad("hash kind"))?;
            record.hashes.push((kind, r.u32()?));
        }
        for _ in 0..r.count(8)? {
            let hash = r.u32()?;
            record.pairs.push((hash, string(r)?));
        }
        for _ in 0..r.count(4)? {
            record.languages.push(string(r)?);
        }
        for _ in 0..r.count(10)? {
            let hash = r.u32()?;
            let language = r.u16()?;
            let text = string(r)?;
            record.texts.push(TextEntry {
                hash,
                language,
                text,
            });
        }
        records.push(record);
    }
    Ok(Chunk {
        rel_path,
        stamp,
        error,
        records,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<Chunk> {
        let record = Record {
            resource: 3,
            header: ResourceHeader {
                mem_policy: 1,
                alignment: 16,
                type_id: 0x0196_1BED,
                ids: [7; 4],
                data_size: 100,
            },
            stored_size: 64,
            compressed: true,
            names: vec![
                (NameKind::Name, "car".into()),
                (NameKind::Part, "wheel".into()),
            ],
            refs: vec![
                Ref {
                    label: "wheel".into(),
                    target: Target::Material(5),
                },
                Ref {
                    label: "skeleton".into(),
                    target: Target::Bones(vec![(1, 0), (2, 1)].into_boxed_slice()),
                },
                Ref {
                    label: "atlas".into(),
                    target: Target::Named {
                        type_id: 9,
                        name: "car".into(),
                    },
                },
                Ref {
                    label: "x".into(),
                    target: Target::Guid([3; 16]),
                },
                Ref {
                    label: "y".into(),
                    target: Target::Resource { type_id: 0, id: 8 },
                },
            ],
            anchors: vec![
                Anchor::Material(5),
                Anchor::Guid([1; 16]),
                Anchor::Skeleton(vec![1, 2].into_boxed_slice()),
            ],
            hashes: vec![(HashKind::Bone, 1), (HashKind::StringEntry, 2)],
            pairs: vec![(0xABCD, "car".into())],
            languages: vec!["en".into()],
            texts: vec![TextEntry {
                hash: 2,
                language: 0,
                text: "Hello".into(),
            }],
            error: Some("broken".into()),
        };
        vec![
            Chunk {
                rel_path: "gf2/a.str".into(),
                stamp: Stamp {
                    size: 1,
                    mtime_ns: 2,
                    file_id: 3,
                },
                error: None,
                records: vec![record, Record::default()],
            },
            Chunk {
                rel_path: "gf2/b.str".into(),
                error: Some("cannot open".into()),
                ..Chunk::default()
            },
        ]
    }

    #[test]
    fn cache_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.bin");
        let chunks = sample();
        let root = Path::new("/games/gf2");
        std::fs::write(&path, write(root, &chunks)).unwrap();
        assert_eq!(read(&path, root).unwrap(), chunks);
        assert!(read(&path, Path::new("/elsewhere")).is_err());
    }

    #[test]
    fn cache_of_other_name_lists_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.bin");
        let root = Path::new("/g");
        let mut bytes = write(root, &sample());
        // Magic, format version, harvest version, then the stamp.
        assert_eq!(bytes[16..20], names_stamp().to_le_bytes());
        bytes[16] ^= 1;
        std::fs::write(&path, &bytes).unwrap();
        let error = read(&path, root).unwrap_err().to_string();
        assert!(error.contains("other name lists"), "{error}");
    }

    #[test]
    fn truncated_cache_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.bin");
        let root = Path::new("/g");
        let bytes = write(root, &sample());
        for cut in [0, 7, 20, bytes.len() / 2, bytes.len() - 1] {
            std::fs::write(&path, &bytes[..cut]).unwrap();
            assert!(read(&path, root).is_err(), "cut at {cut}");
        }
    }
}

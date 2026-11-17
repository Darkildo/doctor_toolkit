//! Resources of mods put into the streams the game reads, in memory
//! (stage 100): no rebuilt `.str` on disk, nothing written into the game.
//!
//! Stage 46 gives the game a whole rebuilt stream from the toolkit's folder
//! ([`crate::files`]). Here the toolkit writes only a **resource patch**
//! ([`Patch`], a `DTRP` file): the resources a stream gets replaced or
//! added, each with its key (type id and the four ids of its `SHDR`) and
//! its raw data. The module's configuration names the patch of a stream
//! (`resources=gf2/cuba.str|Z:\…\cuba.str`). When the game opens that
//! stream, the module reads the block headers of the game's own file
//! ([`scan`]), lays the stream out anew with the patched resources
//! ([`compose`]) and from then on answers the game's reads, seeks and size
//! queries of that handle from the composed **virtual stream**
//! ([`Composed::read`]): unchanged blocks come from the game's file,
//! changed resources from the patch, block headers and `FILL` padding are
//! made up.
//!
//! [`compose`] reproduces `ustream::rebuild_with` of the toolkit with raw
//! (`SDAT`) storage for every changed resource, block for block: the
//! virtual stream holds the same resources in the same order with the same
//! headers and data as the file "apply" writes, and is the same file byte
//! for byte when "apply" stores the changed resources raw. The toolkit's
//! tests compare the two on the game's streams.
//!
//! The layout rules are those of the game's files
//! (`docs/formats/stream.md` §6): pages of 128 KiB, a block never crosses a
//! page, the rest of a page is a `FILL` block.

use std::fmt;
use std::ops::Range;

use crate::DecodeError;
use crate::files::{is_absolute, key, line_safe};
use crate::wire::{Reader, Writer};

/// Size of a page of a stream file.
pub const PAGE: u64 = 0x2_0000;
/// Header of the file: `3slo`, offset of the first block, version, 3.
pub const FILE_HEADER: u64 = 12;
/// Header of a `SHOC` block: tag, size, subtype.
pub const BLOCK_HEADER: u64 = 12;
/// Header of a `FILL` block: tag, size.
pub const FILL_HEADER: u64 = 8;
/// Body of a `SHDR` block.
pub const RESOURCE_HEADER: usize = 32;

const MAGIC: u32 = u32::from_le_bytes(*b"3slo");
const TAG_SHOC: u32 = u32::from_be_bytes(*b"SHOC");
const TAG_FILL: u32 = u32::from_be_bytes(*b"FILL");
const SUB_SHDR: u32 = u32::from_be_bytes(*b"SHDR");
const SUB_SDAT: u32 = u32::from_be_bytes(*b"SDAT");
const SUB_RPAK: u32 = u32::from_be_bytes(*b"Rpak");
const SUB_LZXD: u32 = u32::from_be_bytes(*b"LZXD");

/// First bytes of a resource patch file.
pub const PATCH_MAGIC: [u8; 4] = *b"DTRP";
/// Version of the patch file this code reads and writes.
pub const PATCH_VERSION: u32 = 1;
/// Size of the header of a patch file.
pub const PATCH_HEADER: usize = 0x20;
/// Size of one entry of a patch file.
pub const PATCH_ENTRY: usize = 0x50;
/// At most this many entries in one patch.
pub const MAX_ENTRIES: usize = 1 << 20;
/// At most this many patched streams in one configuration.
pub const MAX_PATCHES: usize = 65_536;
/// At most this many streams in a [`Resources`] reply.
pub const MAX_LISTED: usize = 2_048;

/// Why a stream cannot be composed. The module then gives the game its own
/// file unchanged and counts the failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// A read of the game's file or of the patch failed.
    Read { what: &'static str, offset: u64 },
    /// The game's file is not a stream this code lays out.
    Stream { offset: u64, why: &'static str },
    /// The patch file is broken or of another version.
    Patch(&'static str),
    /// The patch was made for another file (its length differs).
    OtherOriginal { expected: u64, found: u64 },
    /// A replaced resource is not in the stream.
    Missing { type_id: u32, ids: [u32; 4] },
    /// An added resource is already in the stream.
    Present { type_id: u32, ids: [u32; 4] },
    /// Two entries replace the same resource.
    Twice { type_id: u32, ids: [u32; 4] },
    /// An entry asks for something this version does not do (the place of
    /// the SimGroup merge, [`Op::Merge`]).
    Unsupported(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let key = |f: &mut fmt::Formatter<'_>, type_id: &u32, ids: &[u32; 4]| {
            write!(
                f,
                "{type_id:#010x}:{:08x}.{:08x}.{:08x}.{:08x}",
                ids[0], ids[1], ids[2], ids[3]
            )
        };
        match self {
            Self::Read { what, offset } => write!(f, "cannot read {what} at {offset:#x}"),
            Self::Stream { offset, why } => write!(f, "stream at {offset:#x}: {why}"),
            Self::Patch(why) => write!(f, "resource patch: {why}"),
            Self::OtherOriginal { expected, found } => write!(
                f,
                "the patch was made for a stream of {expected} bytes, the game's has {found}"
            ),
            Self::Missing { type_id, ids } => {
                f.write_str("replaced resource ")?;
                key(f, type_id, ids)?;
                f.write_str(" is not in the stream")
            }
            Self::Present { type_id, ids } => {
                f.write_str("added resource ")?;
                key(f, type_id, ids)?;
                f.write_str(" is already in the stream")
            }
            Self::Twice { type_id, ids } => {
                f.write_str("resource ")?;
                key(f, type_id, ids)?;
                f.write_str(" is replaced twice")
            }
            Self::Unsupported(what) => write!(f, "not supported: {what}"),
        }
    }
}

impl std::error::Error for Error {}

/// A file read at any offset: the game's stream, the patch.
pub trait ReadAt {
    /// Fills `buf` from `offset`; false when that is not possible.
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> bool;
    /// Length of the file.
    fn len(&mut self) -> Option<u64>;
    /// Whether the file is empty (or its length is unknown).
    fn is_empty(&mut self) -> bool {
        self.len().is_none_or(|n| n == 0)
    }
}

impl ReadAt for &[u8] {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> bool {
        let Ok(at) = usize::try_from(offset) else {
            return false;
        };
        match at.checked_add(buf.len()).and_then(|end| self.get(at..end)) {
            Some(src) => {
                buf.copy_from_slice(src);
                true
            }
            None => false,
        }
    }

    fn len(&mut self) -> Option<u64> {
        Some(<[u8]>::len(self) as u64)
    }
}

/// The body of a `SHDR` block: one resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Header {
    /// SDBM hash of the memory policy name.
    pub mem_policy: u32,
    pub alignment: u32,
    /// SDBM hash of the type tag.
    pub type_id: u32,
    /// The 128-bit GUID of the resource.
    pub ids: [u32; 4],
    /// Size of the decompressed data.
    pub data_size: u32,
}

impl Header {
    pub fn to_bytes(&self) -> [u8; RESOURCE_HEADER] {
        let mut out = [0u8; RESOURCE_HEADER];
        let words = [
            self.mem_policy,
            self.alignment,
            self.type_id,
            self.ids[0],
            self.ids[1],
            self.ids[2],
            self.ids[3],
            self.data_size,
        ];
        for (i, w) in words.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
        }
        out
    }

    pub fn from_bytes(b: &[u8; RESOURCE_HEADER]) -> Self {
        let w = |i: usize| u32::from_le_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]]);
        Self {
            mem_policy: w(0),
            alignment: w(1),
            type_id: w(2),
            ids: [w(3), w(4), w(5), w(6)],
            data_size: w(7),
        }
    }

    /// Whether this is the resource with that type and those ids.
    pub fn is(&self, type_id: u32, ids: &[u32; 4]) -> bool {
        self.type_id == type_id && self.ids == *ids
    }
}

// --- The patch file ---------------------------------------------------------

/// What an entry of a patch does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// New data for a resource of the stream; its header keeps the policy
    /// and the alignment of the game's.
    Replace,
    /// A new resource, placed before the resource `before` (type id and
    /// ids) of the game's stream, or at the end when that is absent or not
    /// in the stream.
    Add { before: Option<(u32, [u32; 4])> },
    /// Reserved (stage 100 leaves the place): data to be merged with the
    /// game's resource object by object (SimGroup). Not composed yet: a
    /// patch with such an entry is refused ([`Error::Unsupported`]).
    Merge,
}

/// One resource of a patch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub op: Op,
    /// The key (type id, ids) and, for an added resource, the policy and
    /// the alignment; `data_size` is the length of the data.
    pub header: Header,
    /// Where the raw data lies in the patch file.
    pub data_offset: u64,
}

/// A resource patch of one stream: the table of a `DTRP` file.
///
/// ```text
/// 0x00  "DTRP"
/// 0x04  u32  version (1)
/// 0x08  u32  number of entries
/// 0x0C  u32  0
/// 0x10  u64  length of the stream the patch was made for (0: any)
/// 0x18  u32  offset of the path of that stream in this file, 0: the
///            game's own file
/// 0x1C  u32  length of that path (UTF-8)
/// 0x20  entries, 0x50 bytes each:
///         +0x00 u32  0 replace, 1 add, 2 merge (reserved)
///         +0x04 u32  0
///         +0x08      SHDR body: policy, alignment, type id, 4 ids, data size
///         +0x28 u32  1 when the resource goes before another one (add)
///         +0x2C u32  type id of that resource
///         +0x30      its 4 ids
///         +0x40 u64  offset of the data in this file
///         +0x48 u64  0
///       the path, zeros to a multiple of four
///       the data of the entries (raw, as the resource is in memory)
/// ```
///
/// Entries are in the order the toolkit applies them: added resources of
/// one place keep that order.
///
/// The patch is made for one file: usually the stream the game has at
/// that path. When the toolkit took another file for the original — the
/// backup of a stream that was written into the game earlier — the patch
/// names it ([`Self::base`]), and the module composes from that file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Patch {
    /// Length of the file the patch was made for; 0 when unknown.
    pub original_len: u64,
    /// The file the patch was made for, as the toolkit names it on its
    /// computer, when that is not the stream of the game folder itself.
    pub base: Option<String>,
    pub entries: Vec<Entry>,
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn le64(b: &[u8], at: usize) -> u64 {
    u64::from(le32(b, at)) | u64::from(le32(b, at + 4)) << 32
}

impl Patch {
    /// Whether a file starts like a patch.
    pub fn is_patch(first_bytes: &[u8]) -> bool {
        first_bytes.starts_with(&PATCH_MAGIC)
    }

    /// Reads the table of a patch file and checks that every entry's data
    /// lies in the file.
    pub fn read(file: &mut dyn ReadAt) -> Result<Self, Error> {
        let len = file.len().ok_or(Error::Patch("no length"))?;
        let mut head = [0u8; PATCH_HEADER];
        if !file.read_at(0, &mut head) {
            return Err(Error::Patch("shorter than its header"));
        }
        if !Self::is_patch(&head) {
            return Err(Error::Patch("no DTRP magic"));
        }
        if le32(&head, 4) != PATCH_VERSION {
            return Err(Error::Patch("another version"));
        }
        let count = le32(&head, 8) as usize;
        if count > MAX_ENTRIES || (PATCH_HEADER + count * PATCH_ENTRY) as u64 > len {
            return Err(Error::Patch("entry count out of range"));
        }
        let mut table = vec![0u8; count * PATCH_ENTRY];
        if !file.read_at(PATCH_HEADER as u64, &mut table) {
            return Err(Error::Patch("shorter than its table"));
        }
        let mut entries = Vec::with_capacity(count);
        for e in table.chunks_exact(PATCH_ENTRY) {
            let mut body = [0u8; RESOURCE_HEADER];
            body.copy_from_slice(&e[8..8 + RESOURCE_HEADER]);
            let header = Header::from_bytes(&body);
            let op = match le32(e, 0) {
                0 => Op::Replace,
                1 => Op::Add {
                    before: (le32(e, 0x28) != 0).then(|| {
                        (
                            le32(e, 0x2C),
                            [le32(e, 0x30), le32(e, 0x34), le32(e, 0x38), le32(e, 0x3C)],
                        )
                    }),
                },
                2 => Op::Merge,
                _ => return Err(Error::Patch("unknown operation")),
            };
            let data_offset = le64(e, 0x40);
            let end = data_offset.checked_add(u64::from(header.data_size));
            if end.is_none_or(|end| end > len) {
                return Err(Error::Patch("data of an entry outside the file"));
            }
            entries.push(Entry {
                op,
                header,
                data_offset,
            });
        }
        let (base_at, base_len) = (le32(&head, 0x18) as u64, le32(&head, 0x1C) as usize);
        let base = if base_at == 0 {
            None
        } else {
            if base_len > 0x8000 || base_at + base_len as u64 > len {
                return Err(Error::Patch("path of the original outside the file"));
            }
            let mut text = vec![0u8; base_len];
            if !file.read_at(base_at, &mut text) {
                return Err(Error::Patch("shorter than the path of its original"));
            }
            Some(String::from_utf8(text).map_err(|_| Error::Patch("path of the original"))?)
        };
        Ok(Self {
            original_len: le64(&head, 0x10),
            base,
            entries,
        })
    }

    /// The bytes of a patch file: `resources` in the order they apply, each
    /// with its data (`header.data_size` is set from it). `None` when a
    /// resource is larger than 4 GiB or there are too many.
    pub fn build(
        original_len: u64,
        base: Option<&str>,
        resources: &[(Op, Header, &[u8])],
    ) -> Option<Vec<u8>> {
        if resources.len() > MAX_ENTRIES || base.is_some_and(|b| b.is_empty() || b.len() > 0x8000) {
            return None;
        }
        let base_at = PATCH_HEADER + resources.len() * PATCH_ENTRY;
        let table = base_at + base.map_or(0, |b| b.len().next_multiple_of(4));
        let total: usize = resources
            .iter()
            .map(|r| r.2.len().next_multiple_of(4))
            .sum();
        let mut out = Vec::with_capacity(table + total);
        out.extend_from_slice(&PATCH_MAGIC);
        out.extend_from_slice(&PATCH_VERSION.to_le_bytes());
        out.extend_from_slice(&(resources.len() as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&original_len.to_le_bytes());
        out.extend_from_slice(&(base.map_or(0, |_| base_at) as u32).to_le_bytes());
        out.extend_from_slice(&(base.map_or(0, str::len) as u32).to_le_bytes());
        let mut at = table as u64;
        for (op, header, data) in resources {
            let header = Header {
                data_size: u32::try_from(data.len()).ok()?,
                ..*header
            };
            let (code, before) = match op {
                Op::Replace => (0u32, None),
                Op::Add { before } => (1, *before),
                Op::Merge => (2, None),
            };
            out.extend_from_slice(&code.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&header.to_bytes());
            let (flag, type_id, ids) = match before {
                Some((t, ids)) => (1u32, t, ids),
                None => (0, 0, [0; 4]),
            };
            out.extend_from_slice(&flag.to_le_bytes());
            out.extend_from_slice(&type_id.to_le_bytes());
            for id in ids {
                out.extend_from_slice(&id.to_le_bytes());
            }
            out.extend_from_slice(&at.to_le_bytes());
            out.extend_from_slice(&0u64.to_le_bytes());
            at += data.len().next_multiple_of(4) as u64;
        }
        if let Some(base) = base {
            out.extend_from_slice(base.as_bytes());
            out.resize(out.len().next_multiple_of(4), 0);
        }
        for (_, _, data) in resources {
            out.extend_from_slice(data);
            out.resize(out.len().next_multiple_of(4), 0);
        }
        Some(out)
    }
}

// --- The game's stream: its blocks ------------------------------------------

/// What a block of the game's file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `SHDR`.
    Header,
    /// `SDAT`, `Rpak` or `LZXD`: data of the resource before it.
    Data,
    /// `FILL`.
    Fill,
}

/// One block of the game's file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Block {
    pub offset: u64,
    /// Size with the block header.
    pub size: u32,
    pub kind: Kind,
}

/// One resource of the game's file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resource {
    pub header: Header,
    /// Index of its `SHDR` block.
    pub header_block: usize,
    /// The blocks after the header up to the next header (may hold `FILL`).
    pub blocks: Range<usize>,
}

/// The blocks and resources of a stream file; no data.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Layout {
    pub blocks: Vec<Block>,
    pub resources: Vec<Resource>,
    pub file_len: u64,
    /// Offset of the first block (12).
    pub first_block: u64,
    /// How many reads of the file the scan took.
    pub reads: u32,
}

/// Reads through a small window: block headers lie close together where
/// blocks are small and one read apart where they are large.
struct Window<'a> {
    file: &'a mut dyn ReadAt,
    len: u64,
    at: u64,
    buf: Vec<u8>,
    reads: u32,
}

impl Window<'_> {
    const SIZE: u64 = 4096;

    fn get(&mut self, offset: u64, n: usize) -> Option<&[u8]> {
        let end = offset.checked_add(n as u64)?;
        if end > self.len {
            return None;
        }
        if offset < self.at || end > self.at + self.buf.len() as u64 {
            let want = Self::SIZE.max(n as u64).min(self.len - offset) as usize;
            self.buf.resize(want, 0);
            self.reads += 1;
            if !self.file.read_at(offset, &mut self.buf) {
                self.buf.clear();
                return None;
            }
            self.at = offset;
        }
        let start = (offset - self.at) as usize;
        Some(&self.buf[start..start + n])
    }
}

/// Reads the block headers of a stream file (little-endian, version 2) —
/// the same checks as the toolkit's `StreamLayout::parse`, without the
/// data.
pub fn scan(file: &mut dyn ReadAt) -> Result<Layout, Error> {
    let len = file.len().ok_or(Error::Read {
        what: "the length of the stream",
        offset: 0,
    })?;
    let mut w = Window {
        file,
        len,
        at: 0,
        buf: Vec::new(),
        reads: 0,
    };
    let bad = |offset: u64, why: &'static str| Error::Stream { offset, why };
    let head = w
        .get(0, FILE_HEADER as usize)
        .ok_or(bad(0, "shorter than its header"))?;
    if le32(head, 0) != MAGIC {
        return Err(bad(0, "no 3slo magic"));
    }
    if u16::from_le_bytes([head[8], head[9]]) != 2 {
        return Err(bad(8, "not version 2"));
    }
    let first = u64::from(le32(head, 4));
    if first < FILE_HEADER || first > len {
        return Err(bad(4, "first block offset out of range"));
    }
    let mut blocks: Vec<Block> = Vec::new();
    let mut resources: Vec<Resource> = Vec::new();
    let mut pos = first;
    while pos < len {
        if len - pos < FILL_HEADER {
            let rest = w.get(pos, (len - pos) as usize).ok_or(Error::Read {
                what: "the stream",
                offset: pos,
            })?;
            if rest.iter().all(|&b| b == 0) {
                break;
            }
            return Err(bad(pos, "trailing bytes after the last block"));
        }
        let head = w.get(pos, FILL_HEADER as usize).ok_or(Error::Read {
            what: "the stream",
            offset: pos,
        })?;
        let (tag, size) = (le32(head, 0), le32(head, 4));
        let min = match tag {
            TAG_FILL => FILL_HEADER,
            TAG_SHOC => BLOCK_HEADER,
            _ => return Err(bad(pos, "unknown block tag")),
        };
        if u64::from(size) < min || u64::from(size) > len - pos {
            return Err(bad(pos, "block size out of range"));
        }
        let index = blocks.len();
        let kind = if tag == TAG_FILL {
            Kind::Fill
        } else {
            let head = w.get(pos, BLOCK_HEADER as usize).ok_or(Error::Read {
                what: "the stream",
                offset: pos,
            })?;
            match le32(head, 8) {
                SUB_SHDR => Kind::Header,
                SUB_SDAT | SUB_RPAK | SUB_LZXD => Kind::Data,
                _ => return Err(bad(pos, "unknown block subtype")),
            }
        };
        blocks.push(Block {
            offset: pos,
            size,
            kind,
        });
        match kind {
            Kind::Header => {
                if u64::from(size) != BLOCK_HEADER + RESOURCE_HEADER as u64 {
                    return Err(bad(pos, "unexpected SHDR block size"));
                }
                let body = w
                    .get(pos + BLOCK_HEADER, RESOURCE_HEADER)
                    .ok_or(Error::Read {
                        what: "the stream",
                        offset: pos,
                    })?;
                let mut bytes = [0u8; RESOURCE_HEADER];
                bytes.copy_from_slice(body);
                resources.push(Resource {
                    header: Header::from_bytes(&bytes),
                    header_block: index,
                    blocks: index + 1..index + 1,
                });
            }
            Kind::Fill => {
                if let Some(r) = resources.last_mut() {
                    r.blocks.end = index + 1;
                }
            }
            Kind::Data => match resources.last_mut() {
                Some(r) => r.blocks.end = index + 1,
                None => return Err(bad(pos, "data block before the first resource header")),
            },
        }
        pos += u64::from(size);
    }
    // A trailing FILL belongs to the file, not to the last resource.
    for r in &mut resources {
        while r.blocks.end > r.blocks.start && blocks[r.blocks.end - 1].kind == Kind::Fill {
            r.blocks.end -= 1;
        }
    }
    Ok(Layout {
        blocks,
        resources,
        file_len: len,
        first_block: first,
        reads: w.reads,
    })
}

// --- The virtual stream ------------------------------------------------------

/// Where the bytes of a piece of the virtual stream come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The game's own file, from this offset.
    Original(u64),
    /// The patch file, from this offset.
    Patch(u64),
    /// Made up (block headers), from this offset of [`Composed::own`].
    Own(u32),
    /// Zeros (padding).
    Zero,
}

/// One piece of the virtual stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    /// Offset in the virtual stream.
    pub at: u64,
    pub len: u64,
    pub source: Source,
}

/// A stream laid out anew with the resources of a patch.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Composed {
    pub segments: Vec<Segment>,
    /// The made-up bytes ([`Source::Own`]).
    pub own: Vec<u8>,
    /// Length of the virtual stream.
    pub len: u64,
    /// How many resources got new data.
    pub replaced: u32,
    /// How many resources were added.
    pub added: u32,
    /// Sum of the data sizes of all resources of the virtual stream — what
    /// the table of contents holds for the stream (its weight in the
    /// memory budget of a section of the world grid).
    pub data_size: u64,
    /// Added resources whose `before` is not in the stream (they went to
    /// the end, as "apply" puts them).
    pub misplaced: u32,
}

struct Lay {
    segments: Vec<Segment>,
    own: Vec<u8>,
    pos: u64,
}

impl Lay {
    fn push(&mut self, len: u64, source: Source) {
        if len == 0 {
            return;
        }
        if let Some(last) = self.segments.last_mut() {
            let joins = match (last.source, source) {
                (Source::Original(a), Source::Original(b)) => a + last.len == b,
                (Source::Patch(a), Source::Patch(b)) => a + last.len == b,
                (Source::Own(a), Source::Own(b)) => u64::from(a) + last.len == u64::from(b),
                (Source::Zero, Source::Zero) => true,
                _ => false,
            };
            if joins {
                last.len += len;
                self.pos += len;
                return;
            }
        }
        self.segments.push(Segment {
            at: self.pos,
            len,
            source,
        });
        self.pos += len;
    }

    fn own(&mut self, bytes: &[u8]) {
        let at = self.own.len() as u32;
        self.own.extend_from_slice(bytes);
        self.push(bytes.len() as u64, Source::Own(at));
    }

    fn remaining(&self) -> u64 {
        PAGE - self.pos % PAGE
    }

    /// The placement rule of the game's files: a block goes into the page
    /// when it ends at the page boundary or leaves room for a `FILL`
    /// header.
    fn fits(&self, size: u64) -> bool {
        let remaining = self.remaining();
        size == remaining || size + FILL_HEADER <= remaining
    }

    fn fill_page(&mut self) {
        let remaining = self.remaining();
        if remaining == PAGE {
            return;
        }
        let mut head = [0u8; FILL_HEADER as usize];
        head[..4].copy_from_slice(&TAG_FILL.to_le_bytes());
        head[4..].copy_from_slice(&(remaining as u32).to_le_bytes());
        self.own(&head);
        self.push(remaining - FILL_HEADER, Source::Zero);
    }

    fn check(size: u64, offset: u64) -> Result<(), Error> {
        // A block four bytes short of a page would leave a gap no FILL covers.
        if size > PAGE || PAGE - size == 4 || !size.is_multiple_of(4) {
            return Err(Error::Stream {
                offset,
                why: "a block that does not fit a page",
            });
        }
        Ok(())
    }

    /// A block of the game's file, as it is.
    fn original(&mut self, block: &Block) -> Result<(), Error> {
        let size = u64::from(block.size);
        Self::check(size, block.offset)?;
        if !self.fits(size) {
            self.fill_page();
        }
        self.push(size, Source::Original(block.offset));
        Ok(())
    }

    fn block_head(&mut self, size: u64, subtype: u32) {
        let mut head = [0u8; BLOCK_HEADER as usize];
        head[..4].copy_from_slice(&TAG_SHOC.to_le_bytes());
        head[4..8].copy_from_slice(&(size as u32).to_le_bytes());
        head[8..].copy_from_slice(&subtype.to_le_bytes());
        self.own(&head);
    }

    fn header(&mut self, header: &Header) {
        let size = BLOCK_HEADER + RESOURCE_HEADER as u64;
        if !self.fits(size) {
            self.fill_page();
        }
        self.block_head(size, SUB_SHDR);
        self.own(&header.to_bytes());
    }

    /// Raw data of the patch as `SDAT` blocks, split at page boundaries.
    fn raw(&mut self, mut offset: u64, mut rest: u64) -> Result<(), Error> {
        while rest > 0 {
            let remaining = self.remaining();
            if remaining < BLOCK_HEADER + 4 {
                self.fill_page();
                continue;
            }
            let room = remaining - BLOCK_HEADER;
            let mut body = rest.next_multiple_of(4).min(room);
            // Never leave a four-byte gap: it cannot hold a FILL header.
            if room - body == 4 {
                body -= 4;
            }
            if body == 0 {
                self.fill_page();
                continue;
            }
            let take = body.min(rest);
            let padded = take.next_multiple_of(4);
            let size = BLOCK_HEADER + padded;
            Self::check(size, offset)?;
            if !self.fits(size) {
                self.fill_page();
            }
            self.block_head(size, SUB_SDAT);
            self.push(take, Source::Patch(offset));
            self.push(padded - take, Source::Zero);
            offset += take;
            rest -= take;
        }
        Ok(())
    }

    fn resource(&mut self, header: &Header, data_offset: u64) -> Result<(), Error> {
        self.header(header);
        self.raw(data_offset, u64::from(header.data_size))
    }
}

/// Lays the stream `layout` out anew with the resources of `patch`.
///
/// Everything before the first changed resource stays where it is; from
/// there on every block of the game's file keeps its bytes and gets a new
/// place, replaced and added resources are raw `SDAT` blocks.
pub fn compose(layout: &Layout, patch: &Patch) -> Result<Composed, Error> {
    if patch.original_len != 0 && patch.original_len != layout.file_len {
        return Err(Error::OtherOriginal {
            expected: patch.original_len,
            found: layout.file_len,
        });
    }
    let count = layout.resources.len();
    let find = |type_id: u32, ids: &[u32; 4]| {
        layout
            .resources
            .iter()
            .position(|r| r.header.is(type_id, ids))
    };
    let mut replaced: Vec<Option<&Entry>> = vec![None; count];
    let mut inserted: Vec<Vec<&Entry>> = vec![Vec::new(); count + 1];
    let mut out = Composed::default();
    for entry in &patch.entries {
        let (type_id, ids) = (entry.header.type_id, entry.header.ids);
        match entry.op {
            Op::Replace => {
                let index = find(type_id, &ids).ok_or(Error::Missing { type_id, ids })?;
                if replaced[index].is_some() {
                    return Err(Error::Twice { type_id, ids });
                }
                replaced[index] = Some(entry);
                out.replaced += 1;
            }
            Op::Add { before } => {
                if find(type_id, &ids).is_some() {
                    return Err(Error::Present { type_id, ids });
                }
                let at = match before {
                    None => count,
                    Some((t, i)) => find(t, &i).unwrap_or_else(|| {
                        out.misplaced += 1;
                        count
                    }),
                };
                inserted[at].push(entry);
                out.added += 1;
            }
            Op::Merge => {
                return Err(Error::Unsupported(
                    "merging a resource with the game's (SimGroup by objects)",
                ));
            }
        }
    }
    out.data_size = layout
        .resources
        .iter()
        .zip(&replaced)
        .map(|(r, e)| u64::from(e.map_or(r.header.data_size, |e| e.header.data_size)))
        .sum::<u64>()
        + inserted
            .iter()
            .flatten()
            .map(|e| u64::from(e.header.data_size))
            .sum::<u64>();
    let first_replaced = replaced.iter().position(Option::is_some);
    let first_inserted = inserted.iter().position(|list| !list.is_empty());
    let first = match (first_replaced, first_inserted) {
        (None, None) => {
            out.segments.push(Segment {
                at: 0,
                len: layout.file_len,
                source: Source::Original(0),
            });
            out.len = layout.file_len;
            return Ok(out);
        }
        (Some(a), Some(b)) => a.min(b),
        (Some(a), None) | (None, Some(a)) => a,
    };
    // Cut before the header of the first changed resource, and before a
    // FILL that directly precedes it: that padding depends on the old
    // layout. When the only change is an addition at the end, cut before
    // the closing FILL.
    let mut cut_block = match layout.resources.get(first) {
        Some(r) => r.header_block,
        None => layout.blocks.len(),
    };
    if cut_block > 0 && layout.blocks[cut_block - 1].kind == Kind::Fill {
        cut_block -= 1;
    }
    let end_of_blocks = layout
        .blocks
        .last()
        .map_or(layout.first_block, |b| b.offset + u64::from(b.size));
    let cut = layout
        .blocks
        .get(cut_block)
        .map_or(end_of_blocks, |b| b.offset);
    let mut lay = Lay {
        segments: Vec::new(),
        own: Vec::new(),
        pos: 0,
    };
    lay.push(cut, Source::Original(0));
    for (index, resource) in layout.resources.iter().enumerate().skip(first) {
        for entry in &inserted[index] {
            lay.resource(&entry.header, entry.data_offset)?;
        }
        match replaced[index] {
            Some(entry) => {
                let header = Header {
                    data_size: entry.header.data_size,
                    ..resource.header
                };
                lay.resource(&header, entry.data_offset)?;
            }
            None => {
                lay.original(&layout.blocks[resource.header_block])?;
                for block in &layout.blocks[resource.blocks.clone()] {
                    if block.kind != Kind::Fill {
                        lay.original(block)?;
                    }
                }
            }
        }
    }
    for entry in &inserted[count] {
        lay.resource(&entry.header, entry.data_offset)?;
    }
    lay.fill_page();
    out.len = lay.pos;
    out.segments = lay.segments;
    out.own = lay.own;
    Ok(out)
}

impl Composed {
    /// Reads up to `buf.len()` bytes of the virtual stream from `at`;
    /// returns how many (less at the end of the stream, 0 past it).
    pub fn read(
        &self,
        at: u64,
        buf: &mut [u8],
        original: &mut dyn ReadAt,
        patch: &mut dyn ReadAt,
    ) -> Result<usize, Error> {
        if at >= self.len || buf.is_empty() {
            return Ok(0);
        }
        let want = (buf.len() as u64).min(self.len - at) as usize;
        // The segment that holds `at`.
        let mut index = self.segments.partition_point(|s| s.at + s.len <= at);
        let mut done = 0usize;
        while done < want {
            let Some(seg) = self.segments.get(index) else {
                break;
            };
            let pos = at + done as u64;
            let skip = pos - seg.at;
            let n = ((seg.len - skip).min((want - done) as u64)) as usize;
            let out = &mut buf[done..done + n];
            match seg.source {
                Source::Original(offset) => {
                    if !original.read_at(offset + skip, out) {
                        return Err(Error::Read {
                            what: "the game's stream",
                            offset: offset + skip,
                        });
                    }
                }
                Source::Patch(offset) => {
                    if !patch.read_at(offset + skip, out) {
                        return Err(Error::Read {
                            what: "the resource patch",
                            offset: offset + skip,
                        });
                    }
                }
                Source::Own(offset) => {
                    let start = offset as usize + skip as usize;
                    out.copy_from_slice(&self.own[start..start + n]);
                }
                Source::Zero => out.fill(0),
            }
            done += n;
            index += 1;
        }
        Ok(done)
    }

    /// The whole virtual stream (tests, the module's dump).
    pub fn to_vec(
        &self,
        original: &mut dyn ReadAt,
        patch: &mut dyn ReadAt,
    ) -> Result<Vec<u8>, Error> {
        let mut out = vec![0u8; self.len as usize];
        let n = self.read(0, &mut out, original, patch)?;
        out.truncate(n);
        Ok(out)
    }
}

/// Scans the game's stream and composes it with the patch.
pub fn compose_files(
    original: &mut dyn ReadAt,
    patch_file: &mut dyn ReadAt,
) -> Result<(Layout, Patch, Composed), Error> {
    let patch = Patch::read(patch_file)?;
    let layout = scan(original)?;
    let composed = compose(&layout, &patch)?;
    Ok((layout, patch, composed))
}

// --- Configuration and the module's answer -----------------------------------

/// The line `resources=FROM|TO` or `resources=FROM|TO|BASE` of the
/// configuration: a stream of the game folder and its patch file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchLine {
    /// The stream the game opens, relative to its folder (`gf2/cuba.str`).
    pub from: String,
    /// Its patch file: an absolute Windows path, or relative to the game
    /// folder.
    pub to: String,
    /// The file the patch was made for, when it is not the game's own
    /// stream at `from` (the backup of a stream written into the game
    /// earlier): the module composes from this file. As `to`.
    pub base: Option<String>,
}

impl PatchLine {
    /// The line of the configuration.
    pub fn to_line(&self) -> String {
        match &self.base {
            Some(base) => format!("{}|{}|{base}", self.from, self.to),
            None => format!("{}|{}", self.from, self.to),
        }
    }

    /// Reads `FROM|TO[|BASE]`; `None` for a line that is not one.
    pub fn parse(line: &str) -> Option<Self> {
        let mut parts = line.split('|');
        let from = key(parts.next()?.trim())?;
        let to = parts.next()?.trim();
        let base = parts.next().map(str::trim);
        if parts.next().is_some()
            || to.is_empty()
            || !line_safe(to)
            || base.is_some_and(|b| b.is_empty() || !line_safe(b))
        {
            return None;
        }
        Some(Self {
            from,
            to: to.to_owned(),
            base: base.map(str::to_owned),
        })
    }

    /// A path of the line as the module opens it: absolute as it is,
    /// relative under the game folder.
    pub fn resolve(path: &str, game_dir: &str) -> String {
        if is_absolute(path) {
            path.to_owned()
        } else {
            format!("{game_dir}\\{}", path.replace('/', "\\"))
        }
    }
}

/// Answer to [`crate::Request::Resources`] (protocol 9): the streams the
/// module composes and their use.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Resources {
    /// Whether the hooks of the game's file device are in place.
    pub hooked: bool,
    /// How many patched streams the configuration lists.
    pub total: u32,
    /// Handles the module answers for right now.
    pub open: u32,
    /// The streams, at most [`MAX_LISTED`] (those opened first).
    pub streams: Vec<StreamUse>,
}

/// One patched stream and its use.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StreamUse {
    /// The stream in the game folder.
    pub from: String,
    /// Its patch file.
    pub to: String,
    /// The file it is composed from when that is not the game's own
    /// stream; empty otherwise.
    pub base: String,
    /// How often the game opened it.
    pub opens: u32,
    /// Opens the module could not compose: the game got its own file.
    pub failed: u32,
    pub replaced: u32,
    pub added: u32,
    /// Length of the game's file and of the virtual stream.
    pub original_len: u64,
    pub virtual_len: u64,
    /// Sum of the data sizes of the virtual stream's resources.
    pub data_size: u64,
    /// Microseconds the last scan and layout took.
    pub compose_micros: u32,
    /// Bytes the game read of it.
    pub bytes_read: u64,
    /// The last error, empty when there was none.
    pub error: String,
}

/// The hash of a resource GUID in the game's table of loaded resources
/// (`godfather2.exe` `0x004209C0`): the four ids as big-endian bytes
/// (`0x004D9AF0` swaps each word), then `hash * 65599 + byte` over the 16
/// bytes. The bucket is the hash modulo the number of buckets
/// (`0x00421170`).
pub fn table_hash(ids: [u32; 4]) -> u32 {
    ids.iter()
        .flat_map(|id| id.to_be_bytes())
        .fold(0u32, |hash, b| {
            hash.wrapping_mul(65_599).wrapping_add(u32::from(b))
        })
}

/// A loaded resource in the game's table of resources
/// ([`crate::Request::Resource`], protocol 9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Loaded {
    /// The record of the table.
    pub record: u32,
    /// Its data in memory and their size.
    pub data: u32,
    pub size: u32,
    /// Reference count (streams that hold it).
    pub refs: u32,
    pub flags: u16,
}

pub(crate) fn put_resources(w: &mut Writer, r: &Resources) {
    w.bool(r.hooked);
    w.u32(r.total);
    w.u32(r.open);
    w.u32(r.streams.len() as u32);
    for s in &r.streams {
        w.str(&s.from);
        w.str(&s.to);
        w.str(&s.base);
        w.u32(s.opens);
        w.u32(s.failed);
        w.u32(s.replaced);
        w.u32(s.added);
        w.u64(s.original_len);
        w.u64(s.virtual_len);
        w.u64(s.data_size);
        w.u32(s.compose_micros);
        w.u64(s.bytes_read);
        w.str(&s.error);
    }
}

pub(crate) fn get_resources(r: &mut Reader<'_>) -> Result<Resources, DecodeError> {
    let hooked = r.bool()?;
    let total = r.u32()?;
    let open = r.u32()?;
    let n = r.count(60)?;
    let mut streams = Vec::with_capacity(n);
    for _ in 0..n {
        streams.push(StreamUse {
            from: r.str()?,
            to: r.str()?,
            base: r.str()?,
            opens: r.u32()?,
            failed: r.u32()?,
            replaced: r.u32()?,
            added: r.u32()?,
            original_len: r.u64()?,
            virtual_len: r.u64()?,
            data_size: r.u64()?,
            compose_micros: r.u32()?,
            bytes_read: r.u64()?,
            error: r.str()?,
        });
    }
    Ok(Resources {
        hooked,
        total,
        open,
        streams,
    })
}

pub(crate) fn put_loaded(w: &mut Writer, l: &Option<Loaded>) {
    match l {
        Some(l) => {
            w.u8(1);
            w.u32(l.record);
            w.u32(l.data);
            w.u32(l.size);
            w.u32(l.refs);
            w.u16(l.flags);
        }
        None => w.u8(0),
    }
}

pub(crate) fn get_loaded(r: &mut Reader<'_>) -> Result<Option<Loaded>, DecodeError> {
    Ok(if r.bool()? {
        Some(Loaded {
            record: r.u32()?,
            data: r.u32()?,
            size: r.u32()?,
            refs: r.u32()?,
            flags: r.u16()?,
        })
    } else {
        None
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn tag(t: &[u8; 4]) -> u32 {
        u32::from_be_bytes(*t)
    }

    /// A small writer of stream files with the rules of the game's (as the
    /// toolkit's `StreamWriter`; the toolkit's tests compare the composer
    /// with the real one).
    struct Stream(Vec<u8>);

    impl Stream {
        fn new() -> Self {
            let mut out = Vec::new();
            out.extend_from_slice(b"3slo");
            out.extend_from_slice(&12u32.to_le_bytes());
            out.extend_from_slice(&2u16.to_le_bytes());
            out.extend_from_slice(&3u16.to_le_bytes());
            Self(out)
        }

        fn remaining(&self) -> usize {
            PAGE as usize - self.0.len() % PAGE as usize
        }

        fn fill(&mut self) {
            let r = self.remaining();
            if r == PAGE as usize {
                return;
            }
            self.0.extend_from_slice(&TAG_FILL.to_le_bytes());
            self.0.extend_from_slice(&(r as u32).to_le_bytes());
            self.0.resize(self.0.len() + r - 8, 0);
        }

        fn block(&mut self, subtype: u32, body: &[u8]) {
            let padded = body.len().next_multiple_of(4);
            let size = 12 + padded;
            let r = self.remaining();
            if !(size == r || size + 8 <= r) {
                self.fill();
            }
            self.0.extend_from_slice(&TAG_SHOC.to_le_bytes());
            self.0.extend_from_slice(&(size as u32).to_le_bytes());
            self.0.extend_from_slice(&subtype.to_le_bytes());
            self.0.extend_from_slice(body);
            self.0.resize(self.0.len() + padded - body.len(), 0);
        }

        /// A block of another file, header and all.
        fn whole(&mut self, file: &[u8], block: &Block) {
            let size = block.size as usize;
            let r = self.remaining();
            if !(size == r || size + 8 <= r) {
                self.fill();
            }
            let at = block.offset as usize;
            self.0.extend_from_slice(&file[at..at + size]);
        }

        fn resource(&mut self, header: &Header, data: &[u8], subtype: u32) {
            let header = Header {
                data_size: data.len() as u32,
                ..*header
            };
            self.block(SUB_SHDR, &header.to_bytes());
            let mut rest = data;
            while !rest.is_empty() {
                let r = self.remaining();
                if r < 16 {
                    self.fill();
                    continue;
                }
                let room = r - 12;
                let mut body = rest.len().next_multiple_of(4).min(room);
                if room - body == 4 {
                    body -= 4;
                }
                if body == 0 {
                    self.fill();
                    continue;
                }
                let take = body.min(rest.len());
                self.block(subtype, &rest[..take]);
                rest = &rest[take..];
            }
        }

        fn finish(mut self) -> Vec<u8> {
            self.fill();
            self.0
        }
    }

    fn header(type_tag: &[u8; 4], id: u32) -> Header {
        Header {
            mem_policy: 0x1111,
            alignment: 16,
            type_id: tag(type_tag),
            ids: [id; 4],
            data_size: 0,
        }
    }

    fn bytes(seed: u32, len: usize) -> Vec<u8> {
        let mut x = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            })
            .collect()
    }

    /// Five resources, some larger than a page; the third is stored in
    /// `Rpak` blocks (opaque here: only the subtype differs).
    fn original() -> (Vec<u8>, Vec<(Header, Vec<u8>)>) {
        let list = vec![
            (header(b"tg4h", 1), bytes(1, 60)),
            (header(b"tg4d", 2), bytes(2, 200_000)),
            (header(b"mesh", 3), bytes(3, 131_003)),
            (header(b"lh2 ", 4), bytes(4, 7)),
            (header(b"sim ", 5), bytes(5, 300_001)),
        ];
        let mut s = Stream::new();
        for (i, (h, d)) in list.iter().enumerate() {
            s.resource(h, d, if i == 2 { SUB_RPAK } else { SUB_SDAT });
        }
        (s.finish(), list)
    }

    /// What the stream should be after the changes: the same writer, the
    /// changed resources raw.
    fn expected(
        list: &[(Header, Vec<u8>)],
        replace: &[(usize, Vec<u8>)],
        add: &[(usize, Header, Vec<u8>)],
        first_changed: usize,
        original: &[u8],
        layout: &Layout,
    ) -> Vec<u8> {
        // The prefix is the original's up to the cut.
        let mut cut_block = layout
            .resources
            .get(first_changed)
            .map_or(layout.blocks.len(), |r| r.header_block);
        if cut_block > 0 && layout.blocks[cut_block - 1].kind == Kind::Fill {
            cut_block -= 1;
        }
        let cut = layout.blocks.get(cut_block).map_or_else(
            || {
                let b = layout.blocks.last().unwrap();
                (b.offset + u64::from(b.size)) as usize
            },
            |b| b.offset as usize,
        );
        let mut s = Stream(original[..cut].to_vec());
        for (i, (h, d)) in list.iter().enumerate().skip(first_changed) {
            for (_, ah, ad) in add.iter().filter(|a| a.0 == i) {
                s.resource(ah, ad, SUB_SDAT);
            }
            match replace.iter().find(|r| r.0 == i) {
                Some((_, data)) => s.resource(h, data, SUB_SDAT),
                None => {
                    // Unchanged: its blocks as they are, placed anew.
                    let _ = (h, d);
                    let r = &layout.resources[i];
                    s.whole(original, &layout.blocks[r.header_block]);
                    for b in &layout.blocks[r.blocks.clone()] {
                        if b.kind != Kind::Fill {
                            s.whole(original, b);
                        }
                    }
                }
            }
        }
        for (_, ah, ad) in add.iter().filter(|a| a.0 == list.len()) {
            s.resource(ah, ad, SUB_SDAT);
        }
        s.finish()
    }

    fn composed(original: &[u8], patch_file: &[u8]) -> (Layout, Composed, Vec<u8>) {
        let (mut o, mut p) = (original, patch_file);
        let (layout, _, c) = compose_files(&mut o, &mut p).unwrap();
        let (mut o, mut p) = (original, patch_file);
        let all = c.to_vec(&mut o, &mut p).unwrap();
        (layout, c, all)
    }

    #[test]
    fn the_scan_finds_the_resources() {
        let (file, list) = original();
        let mut src = &file[..];
        let layout = scan(&mut src).unwrap();
        assert_eq!(layout.resources.len(), list.len());
        for (r, (h, d)) in layout.resources.iter().zip(&list) {
            assert!(r.header.is(h.type_id, &h.ids));
            assert_eq!(r.header.data_size as usize, d.len());
        }
        assert_eq!(layout.file_len % PAGE, 0);
        // Far fewer reads than blocks of four kilobytes.
        assert!(layout.reads < 40, "{}", layout.reads);
    }

    #[test]
    fn a_patch_without_entries_is_the_game_s_file() {
        let (file, _) = original();
        let patch = Patch::build(file.len() as u64, None, &[]).unwrap();
        let (_, c, all) = composed(&file, &patch);
        assert_eq!(all, file);
        assert_eq!(c.segments.len(), 1);
        assert_eq!((c.replaced, c.added), (0, 0));
    }

    #[test]
    fn replaced_and_added_resources_as_the_writer_lays_them_out() {
        let (file, list) = original();
        let mut src = &file[..];
        let layout = scan(&mut src).unwrap();
        // Same size, smaller, larger than a page, empty.
        type Replaced = Vec<(usize, Vec<u8>)>;
        type Added = Vec<(usize, Header, Vec<u8>)>;
        let cases: Vec<(Replaced, Added)> = vec![
            (vec![(0, bytes(10, 60))], vec![]),
            (vec![(1, bytes(11, 200_000))], vec![]),
            (vec![(1, bytes(12, 5))], vec![]),
            (vec![(3, bytes(13, 400_123))], vec![]),
            (vec![(4, Vec::new())], vec![]),
            (
                vec![(2, bytes(14, 131_060)), (4, bytes(15, 9))],
                vec![(2, header(b"new ", 77), bytes(16, 1234))],
            ),
            (
                vec![],
                vec![
                    (5, header(b"new ", 78), bytes(17, 130_000)),
                    (5, header(b"new ", 79), bytes(18, 3)),
                ],
            ),
            (vec![], vec![(0, header(b"new ", 80), bytes(19, 262_144))]),
        ];
        for (replace, add) in cases {
            let mut entries: Vec<(Op, Header, &[u8])> = Vec::new();
            for (i, data) in &replace {
                // The policy of a replacing entry is not used: the game's is.
                let h = Header {
                    mem_policy: 0xDEAD,
                    ..list[*i].0
                };
                entries.push((Op::Replace, h, data));
            }
            for (i, h, data) in &add {
                let before = list.get(*i).map(|(h, _)| (h.type_id, h.ids));
                entries.push((Op::Add { before }, *h, data));
            }
            let patch = Patch::build(file.len() as u64, None, &entries).unwrap();
            let (_, c, all) = composed(&file, &patch);
            let first = replace
                .iter()
                .map(|r| r.0)
                .chain(add.iter().map(|a| a.0))
                .min()
                .unwrap();
            let want = expected(&list, &replace, &add, first, &file, &layout);
            assert_eq!(all.len(), want.len());
            assert!(
                all == want,
                "replace {:?}",
                replace.iter().map(|r| r.0).collect::<Vec<_>>()
            );
            assert_eq!(c.replaced as usize, replace.len());
            assert_eq!(c.added as usize, add.len());
            // The result is a stream again, and its data sizes add up.
            let mut src = &all[..];
            let again = scan(&mut src).unwrap();
            assert_eq!(again.resources.len(), list.len() + add.len());
            let sum: u64 = again
                .resources
                .iter()
                .map(|r| u64::from(r.header.data_size))
                .sum();
            assert_eq!(sum, c.data_size);
        }
    }

    #[test]
    fn reads_at_any_offset_and_length() {
        let (file, list) = original();
        let data = bytes(20, 150_001);
        let patch = Patch::build(
            file.len() as u64,
            None,
            &[(Op::Replace, list[1].0, &data[..])],
        )
        .unwrap();
        let (_, c, all) = composed(&file, &patch);
        let mut x = 12345u64;
        for _ in 0..400 {
            x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            let at = (x >> 16) % (c.len + 100);
            let len = ((x >> 40) % 140_000) as usize;
            let mut buf = vec![0xAAu8; len];
            let (mut o, mut p) = (&file[..], &patch[..]);
            let n = c.read(at, &mut buf, &mut o, &mut p).unwrap();
            let end = (at as usize + len).min(all.len());
            let want = all.get(at as usize..end).unwrap_or(&[]);
            assert_eq!(n, want.len());
            assert!(&buf[..n] == want, "at {at} len {len}");
        }
    }

    #[test]
    fn what_cannot_be_composed_is_refused() {
        let (file, list) = original();
        let run = |entries: &[(Op, Header, &[u8])], len: u64| {
            let patch = Patch::build(len, None, entries).unwrap();
            let (mut o, mut p) = (&file[..], &patch[..]);
            compose_files(&mut o, &mut p).map(|_| ())
        };
        let len = file.len() as u64;
        let h = header(b"none", 9);
        assert_eq!(
            run(&[(Op::Replace, h, b"x")], len),
            Err(Error::Missing {
                type_id: h.type_id,
                ids: h.ids
            })
        );
        let h = list[0].0;
        assert_eq!(
            run(&[(Op::Add { before: None }, h, b"x")], len),
            Err(Error::Present {
                type_id: h.type_id,
                ids: h.ids
            })
        );
        assert_eq!(
            run(&[(Op::Replace, h, b"x"), (Op::Replace, h, b"y")], len),
            Err(Error::Twice {
                type_id: h.type_id,
                ids: h.ids
            })
        );
        assert!(matches!(
            run(&[(Op::Merge, h, b"x")], len),
            Err(Error::Unsupported(_))
        ));
        assert_eq!(
            run(&[(Op::Replace, h, b"x")], len + 1),
            Err(Error::OtherOriginal {
                expected: len + 1,
                found: len
            })
        );
        // Any length when the patch names none.
        assert_eq!(run(&[(Op::Replace, h, b"x")], 0), Ok(()));
        // Not a stream, not a patch.
        let patch = Patch::build(0, None, &[]).unwrap();
        let (mut o, mut p) = (&b"3slx00000000"[..], &patch[..]);
        assert!(matches!(
            compose_files(&mut o, &mut p),
            Err(Error::Stream { .. })
        ));
        let (mut o, mut p) = (&file[..], &file[..]);
        assert_eq!(
            compose_files(&mut o, &mut p).map(|_| ()),
            Err(Error::Patch("no DTRP magic"))
        );
        // Data of an entry outside the file.
        let mut cut = Patch::build(0, None, &[(Op::Replace, h, &[1u8; 64][..])]).unwrap();
        cut.truncate(cut.len() - 8);
        let mut p = &cut[..];
        assert_eq!(
            Patch::read(&mut p),
            Err(Error::Patch("data of an entry outside the file"))
        );
    }

    #[test]
    fn an_added_resource_with_an_unknown_place_goes_to_the_end() {
        let (file, list) = original();
        let h = header(b"new ", 90);
        let patch = Patch::build(
            0,
            None,
            &[(
                Op::Add {
                    before: Some((tag(b"none"), [1; 4])),
                },
                h,
                &b"abc"[..],
            )],
        )
        .unwrap();
        let (_, c, all) = composed(&file, &patch);
        assert_eq!(c.misplaced, 1);
        let mut src = &all[..];
        let again = scan(&mut src).unwrap();
        assert_eq!(again.resources.len(), list.len() + 1);
        assert!(again.resources.last().unwrap().header.is(h.type_id, &h.ids));
    }

    #[test]
    fn the_patch_table_round_trips() {
        let a = header(b"tg4h", 1);
        let b = header(b"new ", 2);
        let file = Patch::build(
            777,
            Some("/srv/game/_doctor_toolkit/backups/gf2/cuba.str"),
            &[
                (Op::Replace, a, &b"12345"[..]),
                (
                    Op::Add {
                        before: Some((a.type_id, a.ids)),
                    },
                    b,
                    &b"abcdefgh"[..],
                ),
                (Op::Add { before: None }, b, &b""[..]),
            ],
        )
        .unwrap();
        assert!(Patch::is_patch(&file));
        let mut src = &file[..];
        let patch = Patch::read(&mut src).unwrap();
        assert_eq!(patch.original_len, 777);
        assert_eq!(
            patch.base.as_deref(),
            Some("/srv/game/_doctor_toolkit/backups/gf2/cuba.str")
        );
        // The lines of the configuration, with and without another original.
        let line = PatchLine::parse("GF2\\Cuba.str | Z:\\o\\cuba.str").unwrap();
        assert_eq!((line.from.as_str(), &line.base), ("gf2/cuba.str", &None));
        assert_eq!(PatchLine::parse(&line.to_line()), Some(line));
        let line =
            PatchLine::parse("gf2/cuba.str|Z:\\o\\cuba.str|_doctor_toolkit/backups/gf2/cuba.str")
                .unwrap();
        assert_eq!(
            line.base.as_deref(),
            Some("_doctor_toolkit/backups/gf2/cuba.str")
        );
        assert_eq!(PatchLine::parse(&line.to_line()), Some(line.clone()));
        assert_eq!(
            PatchLine::resolve(line.base.as_deref().unwrap(), "C:\\game"),
            "C:\\game\\_doctor_toolkit\\backups\\gf2\\cuba.str"
        );
        assert_eq!(PatchLine::resolve("Z:\\o\\x", "C:\\game"), "Z:\\o\\x");
        for bad in [
            "gf2/cuba.str",
            "gf2/cuba.str|",
            "../x|y",
            "a.str|b|c|d",
            "a.str|b|",
        ] {
            assert_eq!(PatchLine::parse(bad), None, "{bad}");
        }
        assert_eq!(patch.entries.len(), 3);
        assert_eq!(patch.entries[0].op, Op::Replace);
        assert_eq!(patch.entries[0].header.data_size, 5);
        let at = patch.entries[1].data_offset as usize;
        assert_eq!(&file[at..at + 8], b"abcdefgh");
        assert_eq!(
            patch.entries[1].op,
            Op::Add {
                before: Some((a.type_id, a.ids))
            }
        );
        assert_eq!(patch.entries[2].op, Op::Add { before: None });
    }

    /// The calls of the file device the module patches are checked by the
    /// live module itself and by no other module (the crash module and the
    /// graphics module check their own places before they install).
    #[test]
    fn the_hooks_lie_outside_what_the_other_modules_check() {
        use crate::addr::{DEVICE_CALLS, DEVICE_READ_CALL, READ_FILE_CALLS};
        assert!(READ_FILE_CALLS.contains(&DEVICE_READ_CALL));
        let theirs = crate::crash::CHECKS
            .iter()
            .chain(crate::render::CHECKS.iter());
        for check in theirs {
            let (start, end) = (check.address, check.address + check.bytes.len() as u32);
            for at in DEVICE_CALLS.iter().chain([&DEVICE_READ_CALL]) {
                assert!(
                    at + 6 <= start || *at >= end,
                    "the hook at {at:#010x} overlaps the check of {:#010x} ({})",
                    check.address,
                    check.what
                );
            }
        }
        // Each is a `call [slot]` of six bytes the edition check knows.
        for at in DEVICE_CALLS {
            let check = crate::edition::CHECKS
                .iter()
                .find(|c| c.address == at)
                .unwrap_or_else(|| panic!("{at:#010x} is not checked"));
            assert_eq!(check.bytes.len(), 6);
            assert_eq!(check.bytes[..2], [0xFF, 0x15]);
        }
        // No two checks of the live module overlap a patched call partly:
        // a check either is the call or lies outside it.
        for check in crate::edition::CHECKS {
            let (start, end) = (check.address, check.address + check.bytes.len() as u32);
            for at in DEVICE_CALLS {
                assert!(
                    start == at || at + 6 <= start || at >= end,
                    "{:#010x} ({})",
                    check.address,
                    check.what
                );
            }
        }
    }

    #[test]
    fn the_hash_of_the_table_of_resources() {
        assert_eq!(table_hash([0; 4]), 0);
        // The last byte is added as it is; the one before it times 65599.
        assert_eq!(table_hash([0, 0, 0, 0x0000_0001]), 1);
        assert_eq!(table_hash([0, 0, 0, 0x0000_0100]), 65_599);
        // The first byte hashed is the highest byte of the first id.
        assert_eq!(
            table_hash([0x0100_0000, 0, 0, 0]),
            65_599u32.wrapping_pow(15)
        );
    }

    #[test]
    fn the_answers_round_trip() {
        let r = Resources {
            hooked: true,
            total: 2,
            open: 1,
            streams: vec![StreamUse {
                from: "gf2/cuba.str".into(),
                to: "Z:\\tmp\\cuba.str".into(),
                base: "Z:\\game\\_doctor_toolkit\\backups\\gf2\\cuba.str".into(),
                opens: 3,
                failed: 1,
                replaced: 15,
                added: 2,
                original_len: 1 << 33,
                virtual_len: (1 << 33) + 4096,
                data_size: 12345,
                compose_micros: 900,
                bytes_read: 1 << 34,
                error: "x".into(),
            }],
        };
        let mut w = Writer::default();
        put_resources(&mut w, &r);
        let mut rd = Reader::new(&w.buf);
        assert_eq!(get_resources(&mut rd), Ok(r));
        rd.finish().unwrap();
        for l in [
            None,
            Some(Loaded {
                record: 1,
                data: 2,
                size: 3,
                refs: 4,
                flags: 5,
            }),
        ] {
            let mut w = Writer::default();
            put_loaded(&mut w, &l);
            let mut rd = Reader::new(&w.buf);
            assert_eq!(get_loaded(&mut rd), Ok(l));
        }
    }
}

//! UStream, the resource container of The Godfather II and Dead Space (`.str`).
//!
//! # File layout
//!
//! ```text
//! 0x00  "3slo"            magic (little-endian files)
//! 0x04  u32               offset of the first block, always 12
//! 0x08  u16               version, 2
//! 0x0A  u16               unknown, 3 in every Godfather II file
//! 0x0C  blocks...
//! ```
//!
//! The file is a sequence of 128 KiB pages. A block never crosses a page
//! boundary; the unused tail of a page is covered by a `FILL` block.
//!
//! ```text
//! SHOC block:  "COHS"  u32 size  u32 subtype  body      (size includes the 12 header bytes)
//! FILL block:  "LLIF"  u32 size  zeros                   (size includes the 8 header bytes)
//! ```
//!
//! A resource is a `SHDR` block followed by its data blocks. The bodies of the
//! data blocks, after decompression, are concatenated:
//!
//! - `SDAT`: raw bytes, padded with zeros to a multiple of four;
//! - `Rpak`: `u32 compressed_len`, a RefPack stream, zero padding to a multiple of four;
//! - `LZXD`: LZX data (Dead Space only, not implemented).

use std::ops::Range;

use binrw::{BinRead, BinWrite};

use crate::compress::refpack;
use crate::io::{self, Endian};
use crate::{Error, Result};

/// Size of a file page. Blocks never cross a page boundary.
pub const PAGE_SIZE: usize = 0x2_0000;
/// Size of the file header.
pub const FILE_HEADER_SIZE: usize = 12;
/// Size of the header of a `SHOC` block (tag, size, subtype).
pub const BLOCK_HEADER_SIZE: usize = 12;
/// Size of the header of a `FILL` block (tag, size).
pub const FILL_HEADER_SIZE: usize = 8;
/// Size of the body of a `SHDR` block.
pub const RESOURCE_HEADER_SIZE: usize = 32;
/// Largest body a single block can carry.
pub const MAX_BLOCK_BODY: usize = PAGE_SIZE - BLOCK_HEADER_SIZE;

/// File magic, the bytes `3slo` read as a little-endian integer.
pub const MAGIC: u32 = u32::from_le_bytes(*b"3slo");

/// Block tag `SHOC`.
pub const TAG_SHOC: u32 = u32::from_be_bytes(*b"SHOC");
/// Block tag `FILL`.
pub const TAG_FILL: u32 = u32::from_be_bytes(*b"FILL");
/// Subtype of a resource header block.
pub const SUB_SHDR: u32 = u32::from_be_bytes(*b"SHDR");
/// Subtype of a raw data block.
pub const SUB_SDAT: u32 = u32::from_be_bytes(*b"SDAT");
/// Subtype of a RefPack-compressed data block.
pub const SUB_RPAK: u32 = u32::from_be_bytes(*b"Rpak");
/// Subtype of an LZX-compressed data block.
pub const SUB_LZXD: u32 = u32::from_be_bytes(*b"LZXD");

/// Upper bound for a single decompressed chunk; guards allocations on corrupt input.
const MAX_CHUNK_SIZE: usize = refpack::MAX_SIZE_NARROW;

/// The 12-byte file header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, BinRead, BinWrite)]
pub struct FileHeader {
    /// [`MAGIC`].
    pub magic: u32,
    /// Offset of the first block.
    pub first_block: u32,
    /// Container version, 2.
    pub version: u16,
    /// Unknown, 3 in every Godfather II (PC) file.
    pub unknown_0a: u16,
}

impl FileHeader {
    /// The header found in every Godfather II (PC) stream.
    pub const GF2_PC: Self = Self {
        magic: MAGIC,
        first_block: FILE_HEADER_SIZE as u32,
        version: 2,
        unknown_0a: 3,
    };
}

/// The body of a `SHDR` block: metadata of one resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, BinRead, BinWrite)]
pub struct ResourceHeader {
    /// SDBM hash of the memory policy name (`default`, `volatile`, `geometry`, ...).
    pub mem_policy: u32,
    /// Alignment the engine applies to the loaded data.
    pub alignment: u32,
    /// SDBM hash of the resource type tag (`tg4h`, `mesh`, `simgroup`, ...).
    pub type_id: u32,
    /// Four identifiers. Most resources repeat one name hash four times;
    /// textures store `0`, their type id and two ids shared by the header/data pair.
    pub ids: [u32; 4],
    /// Size of the decompressed resource data in bytes.
    pub data_size: u32,
}

/// What a block contains.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    /// `SHDR`: a resource header.
    Header,
    /// `SDAT`: raw data.
    Raw,
    /// `Rpak`: RefPack-compressed data.
    RefPack,
    /// `LZXD`: LZX-compressed data.
    Lzx,
    /// `FILL`: page padding.
    Fill,
}

impl BlockKind {
    /// The on-disk subtype of a `SHOC` block; `None` for `FILL`.
    pub fn subtype(self) -> Option<u32> {
        match self {
            Self::Header => Some(SUB_SHDR),
            Self::Raw => Some(SUB_SDAT),
            Self::RefPack => Some(SUB_RPAK),
            Self::Lzx => Some(SUB_LZXD),
            Self::Fill => None,
        }
    }
}

/// Location of one block inside the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Block {
    /// File offset of the block header.
    pub offset: usize,
    /// Total size including the block header.
    pub size: usize,
    pub kind: BlockKind,
}

impl Block {
    /// File range of the block body.
    pub fn body(&self) -> Range<usize> {
        let header = if self.kind == BlockKind::Fill {
            FILL_HEADER_SIZE
        } else {
            BLOCK_HEADER_SIZE
        };
        self.offset + header..self.offset + self.size
    }
}

/// One resource: a header block and the data blocks that follow it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resource {
    pub header: ResourceHeader,
    /// Index of the `SHDR` block in [`StreamLayout::blocks`].
    pub header_block: usize,
    /// Indices of the blocks after the header up to the next header.
    /// The range may contain `FILL` blocks.
    pub blocks: Range<usize>,
}

/// The parsed structure of a stream. It owns no file data; pass the file bytes
/// to the accessor methods.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamLayout {
    pub header: FileHeader,
    pub endian: Endian,
    pub blocks: Vec<Block>,
    pub resources: Vec<Resource>,
    /// Length of the file the layout was parsed from.
    pub file_len: usize,
}

impl StreamLayout {
    /// Parses the block structure of a stream. Resource data is not decompressed.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let magic_bytes: [u8; 4] = io::slice(data, 0, 4)?
            .try_into()
            .expect("slice of length 4");
        let endian = if u32::from_le_bytes(magic_bytes) == MAGIC {
            Endian::Little
        } else if u32::from_be_bytes(magic_bytes) == MAGIC {
            Endian::Big
        } else {
            return Err(Error::BadMagic {
                what: "UStream",
                found: u32::from_le_bytes(magic_bytes),
            });
        };
        let header: FileHeader = io::read_record(data, 0, endian)?;
        if header.version != 2 {
            return Err(Error::UnsupportedVersion {
                what: "UStream",
                version: header.version as u32,
            });
        }
        let first = header.first_block as usize;
        if first < FILE_HEADER_SIZE || first > data.len() {
            return Err(Error::malformed(
                "UStream header",
                4,
                "first block offset out of range",
            ));
        }

        let mut blocks = Vec::new();
        let mut resources: Vec<Resource> = Vec::new();
        let mut pos = first;
        while pos < data.len() {
            if data.len() - pos < FILL_HEADER_SIZE {
                if data[pos..].iter().all(|&b| b == 0) {
                    break;
                }
                return Err(Error::malformed(
                    "UStream",
                    pos,
                    "trailing bytes after the last block",
                ));
            }
            let tag = io::read_u32(data, pos, endian)?;
            let size = io::read_u32(data, pos + 4, endian)? as usize;
            let min_size = if tag == TAG_FILL {
                FILL_HEADER_SIZE
            } else {
                BLOCK_HEADER_SIZE
            };
            if tag != TAG_SHOC && tag != TAG_FILL {
                return Err(Error::malformed(
                    "UStream block",
                    pos,
                    format!("unknown block tag {tag:#010x}"),
                ));
            }
            if size < min_size || size > data.len() - pos {
                return Err(Error::malformed(
                    "UStream block",
                    pos,
                    format!("block size {size:#x} out of range"),
                ));
            }
            let kind = if tag == TAG_FILL {
                BlockKind::Fill
            } else {
                match io::read_u32(data, pos + 8, endian)? {
                    SUB_SHDR => BlockKind::Header,
                    SUB_SDAT => BlockKind::Raw,
                    SUB_RPAK => BlockKind::RefPack,
                    SUB_LZXD => BlockKind::Lzx,
                    other => {
                        return Err(Error::malformed(
                            "UStream block",
                            pos,
                            format!("unknown block subtype {other:#010x}"),
                        ));
                    }
                }
            };
            let index = blocks.len();
            blocks.push(Block {
                offset: pos,
                size,
                kind,
            });
            match kind {
                BlockKind::Header => {
                    if size != BLOCK_HEADER_SIZE + RESOURCE_HEADER_SIZE {
                        return Err(Error::malformed(
                            "UStream resource header",
                            pos,
                            format!("unexpected SHDR block size {size:#x}"),
                        ));
                    }
                    let header = io::read_record(data, pos + BLOCK_HEADER_SIZE, endian)?;
                    resources.push(Resource {
                        header,
                        header_block: index,
                        blocks: index + 1..index + 1,
                    });
                }
                BlockKind::Fill => {
                    if let Some(resource) = resources.last_mut() {
                        resource.blocks.end = index + 1;
                    }
                }
                BlockKind::Raw | BlockKind::RefPack | BlockKind::Lzx => {
                    match resources.last_mut() {
                        Some(resource) => resource.blocks.end = index + 1,
                        None => {
                            return Err(Error::malformed(
                                "UStream block",
                                pos,
                                "data block before the first resource header",
                            ));
                        }
                    }
                }
            }
            pos += size;
        }

        // A trailing FILL belongs to the file, not to the last resource.
        for resource in &mut resources {
            while resource.blocks.end > resource.blocks.start
                && blocks[resource.blocks.end - 1].kind == BlockKind::Fill
            {
                resource.blocks.end -= 1;
            }
        }

        Ok(Self {
            header,
            endian,
            blocks,
            resources,
            file_len: data.len(),
        })
    }

    /// The data blocks of resource `index`, without `FILL` blocks.
    pub fn data_blocks(&self, index: usize) -> impl Iterator<Item = &Block> {
        let range = self
            .resources
            .get(index)
            .map(|r| r.blocks.clone())
            .unwrap_or(0..0);
        self.blocks[range]
            .iter()
            .filter(|b| b.kind != BlockKind::Fill)
    }

    /// Number of bytes the data blocks of resource `index` occupy on disk,
    /// block headers included.
    pub fn stored_size(&self, index: usize) -> usize {
        self.data_blocks(index).map(|b| b.size).sum()
    }

    /// Whether any data block of resource `index` is compressed.
    pub fn is_compressed(&self, index: usize) -> bool {
        self.data_blocks(index).any(|b| b.kind != BlockKind::Raw)
    }

    /// Reads and decompresses the data of resource `index`.
    ///
    /// `data` must be the file the layout was parsed from. When the blocks hold
    /// more than the header declares, the surplus is dropped; see
    /// [`StreamLayout::resource_data_with_excess`].
    pub fn resource_data(&self, data: &[u8], index: usize) -> Result<Vec<u8>> {
        self.resource_data_with_excess(data, index)
            .map(|(out, _)| out)
    }

    /// Like [`StreamLayout::resource_data`], and also returns how many bytes the
    /// blocks hold beyond the declared size and the four-byte padding.
    ///
    /// Game files never have such a surplus. Files modified by third-party
    /// tools sometimes do: a patch for The Godfather II appends bytes to a
    /// resource without updating its header.
    pub fn resource_data_with_excess(&self, data: &[u8], index: usize) -> Result<(Vec<u8>, usize)> {
        let resource = self.resources.get(index).ok_or_else(|| {
            Error::InvalidArgument(format!("resource index {index} out of range"))
        })?;
        let expected = resource.header.data_size as usize;
        let mut out = Vec::with_capacity(expected.min(MAX_CHUNK_SIZE));
        for block in self.data_blocks(index) {
            let body = io::slice(data, block.body().start, block.body().len())?;
            match block.kind {
                BlockKind::Raw => out.extend_from_slice(body),
                BlockKind::RefPack => {
                    let compressed_len = io::read_u32(body, 0, self.endian)? as usize;
                    let stream = io::slice(body, 4, compressed_len).map_err(|_| {
                        Error::malformed(
                            "UStream Rpak block",
                            block.offset,
                            "compressed length exceeds the block",
                        )
                    })?;
                    let (chunk, _) = refpack::decompress_with_limit(stream, MAX_CHUNK_SIZE)?;
                    out.extend_from_slice(&chunk);
                }
                BlockKind::Lzx => {
                    return Err(Error::UnsupportedVersion {
                        what: "LZX compression",
                        version: 0,
                    });
                }
                BlockKind::Header | BlockKind::Fill => unreachable!("filtered by data_blocks"),
            }
        }
        if out.len() < expected {
            return Err(Error::malformed(
                "UStream resource",
                self.blocks[resource.header_block].offset,
                format!(
                    "data blocks hold {} byte(s), header declares {expected}",
                    out.len()
                ),
            ));
        }
        // Data is padded to four bytes, raw or compressed, so up to three extra bytes are expected.
        let excess = (out.len() - expected).saturating_sub(3);
        out.truncate(expected);
        Ok((out, excess))
    }

    /// Sum of the decompressed sizes of all resources. The table of contents
    /// stores this value for every stream.
    pub fn total_data_size(&self) -> u64 {
        self.resources
            .iter()
            .map(|r| r.header.data_size as u64)
            .sum()
    }
}

/// How the data of a new or replaced resource is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Storage {
    /// Uncompressed `SDAT` blocks.
    #[default]
    Raw,
    /// RefPack-compressed `Rpak` blocks.
    RefPack,
}

/// Lays out blocks into pages and inserts `FILL` blocks where needed.
///
/// The placement rule reproduces the files shipped with the game: a block is
/// placed in the current page when it ends exactly at the page boundary or
/// leaves at least eight bytes (room for a `FILL` header); otherwise the rest
/// of the page is filled first.
#[derive(Debug, Clone)]
pub struct StreamWriter {
    endian: Endian,
    out: Vec<u8>,
}

impl StreamWriter {
    /// Starts a new stream with the given file header.
    pub fn new(header: &FileHeader, endian: Endian) -> Self {
        let mut out = Vec::new();
        io::put_record(&mut out, header, endian);
        Self { endian, out }
    }

    /// Continues a stream whose first bytes are taken verbatim from an existing file.
    ///
    /// `prefix` must end on a block boundary.
    pub fn with_prefix(prefix: &[u8], endian: Endian) -> Self {
        Self {
            endian,
            out: prefix.to_vec(),
        }
    }

    /// Number of bytes written so far.
    pub fn len(&self) -> usize {
        self.out.len()
    }

    /// Whether nothing has been written yet.
    pub fn is_empty(&self) -> bool {
        self.out.is_empty()
    }

    fn remaining_in_page(&self) -> usize {
        PAGE_SIZE - self.out.len() % PAGE_SIZE
    }

    fn fits(&self, block_size: usize) -> bool {
        let remaining = self.remaining_in_page();
        block_size == remaining || block_size + FILL_HEADER_SIZE <= remaining
    }

    /// Pads the current page with a `FILL` block. Does nothing at a page boundary.
    fn fill_page(&mut self) {
        let remaining = self.remaining_in_page();
        if remaining == PAGE_SIZE {
            return;
        }
        debug_assert!(
            remaining >= FILL_HEADER_SIZE,
            "page remainder too small for FILL"
        );
        io::put_u32(&mut self.out, TAG_FILL, self.endian);
        io::put_u32(&mut self.out, remaining as u32, self.endian);
        self.out
            .resize(self.out.len() + remaining - FILL_HEADER_SIZE, 0);
    }

    /// Appends one `SHOC` block. `body` is padded with zeros to a multiple of four.
    pub fn push_block(&mut self, kind: BlockKind, body: &[u8]) -> Result<()> {
        let subtype = kind
            .subtype()
            .ok_or_else(|| Error::InvalidArgument("FILL blocks are placed automatically".into()))?;
        let padded = io::align_up(body.len(), 4);
        let size = BLOCK_HEADER_SIZE + padded;
        // A block four bytes short of a page would leave a gap no FILL can cover.
        if size > PAGE_SIZE || PAGE_SIZE - size == 4 {
            return Err(Error::InvalidArgument(format!(
                "block body of {} byte(s) does not fit a page",
                body.len()
            )));
        }
        if !self.fits(size) {
            self.fill_page();
        }
        io::put_u32(&mut self.out, TAG_SHOC, self.endian);
        io::put_u32(&mut self.out, size as u32, self.endian);
        io::put_u32(&mut self.out, subtype, self.endian);
        self.out.extend_from_slice(body);
        self.out.resize(self.out.len() + padded - body.len(), 0);
        Ok(())
    }

    /// Appends a resource header block.
    pub fn push_header(&mut self, header: &ResourceHeader) -> Result<()> {
        let mut body = Vec::with_capacity(RESOURCE_HEADER_SIZE);
        io::put_record(&mut body, header, self.endian);
        self.push_block(BlockKind::Header, &body)
    }

    /// Appends `data` as raw `SDAT` blocks, splitting it at page boundaries.
    pub fn push_raw_data(&mut self, data: &[u8]) -> Result<()> {
        let mut rest = data;
        while !rest.is_empty() {
            let remaining = self.remaining_in_page();
            if remaining < BLOCK_HEADER_SIZE + 4 {
                self.fill_page();
                continue;
            }
            let room = remaining - BLOCK_HEADER_SIZE;
            let padded_rest = io::align_up(rest.len(), 4);
            let mut body_len = padded_rest.min(room);
            // Never leave a four-byte gap: it cannot hold a FILL header.
            if room - body_len == 4 {
                body_len -= 4;
            }
            if body_len == 0 {
                self.fill_page();
                continue;
            }
            let take = body_len.min(rest.len());
            self.push_block(BlockKind::Raw, &rest[..take])?;
            rest = &rest[take..];
        }
        Ok(())
    }

    /// Appends `data` as RefPack-compressed `Rpak` blocks.
    ///
    /// Each block compresses a slice of the input sized so that the block fits
    /// the rest of the current page where possible.
    pub fn push_refpack_data(&mut self, data: &[u8]) -> Result<()> {
        /// Largest input slice compressed into one block.
        const MAX_INPUT: usize = 0x1_0000;
        /// Below this much free space it is cheaper to start a new page.
        const MIN_ROOM: usize = 0x400;

        let mut rest = data;
        while !rest.is_empty() {
            let remaining = self.remaining_in_page();
            if remaining < MIN_ROOM {
                self.fill_page();
                continue;
            }
            let mut take = rest.len().min(MAX_INPUT);
            let body = loop {
                let packed = refpack::compress(&rest[..take])?;
                let size = BLOCK_HEADER_SIZE + io::align_up(4 + packed.len(), 4);
                if self.fits(size) {
                    let mut body = Vec::with_capacity(4 + packed.len());
                    io::put_u32(&mut body, packed.len() as u32, self.endian);
                    body.extend_from_slice(&packed);
                    break Some(body);
                }
                // Shrink the input in proportion to the overshoot and retry.
                let scaled = take * (remaining - BLOCK_HEADER_SIZE - 16) / size;
                let next = scaled.min(take - 1) & !3;
                if next < 64 {
                    break None;
                }
                take = next;
            };
            match body {
                Some(body) => {
                    self.push_block(BlockKind::RefPack, &body)?;
                    rest = &rest[take..];
                }
                None => self.fill_page(),
            }
        }
        Ok(())
    }

    /// Appends a complete resource: its header and its data.
    ///
    /// `header.data_size` is overwritten with the length of `data`.
    pub fn push_resource(
        &mut self,
        header: &ResourceHeader,
        data: &[u8],
        storage: Storage,
    ) -> Result<()> {
        let data_size = u32::try_from(data.len())
            .map_err(|_| Error::InvalidArgument("resource data larger than 4 GiB".into()))?;
        self.push_header(&ResourceHeader {
            data_size,
            ..*header
        })?;
        match storage {
            Storage::Raw => self.push_raw_data(data),
            Storage::RefPack => self.push_refpack_data(data),
        }
    }

    /// Pads the last page and returns the file contents.
    pub fn finish(mut self) -> Vec<u8> {
        self.fill_page();
        self.out
    }
}

/// A replacement for the data of one resource.
#[derive(Debug, Clone)]
pub struct Replacement<'a> {
    /// Index of the resource to replace.
    pub index: usize,
    /// The new decompressed data.
    pub data: &'a [u8],
    pub storage: Storage,
}

/// A resource added to a stream by [`rebuild_with`].
#[derive(Debug, Clone)]
pub struct Insertion<'a> {
    /// Index of the existing resource the new one is placed before;
    /// `layout.resources.len()` appends it at the end. Insertions with the
    /// same position keep their order.
    pub before: usize,
    /// Header of the new resource; `data_size` is set from `data`.
    pub header: ResourceHeader,
    /// The decompressed data.
    pub data: &'a [u8],
    pub storage: Storage,
}

/// Rewrites a stream, replacing the data of some resources.
///
/// Everything before the first replaced resource is copied byte for byte, so
/// a call without replacements returns the input unchanged. Resources after
/// that point keep their blocks (compressed data is not re-encoded) but are
/// laid out again.
pub fn rebuild(
    data: &[u8],
    layout: &StreamLayout,
    replacements: &[Replacement],
) -> Result<Vec<u8>> {
    rebuild_with(data, layout, replacements, &[])
}

/// Like [`rebuild`], and also adds new resources.
///
/// Everything before the first replaced resource or insertion point is
/// copied byte for byte.
pub fn rebuild_with(
    data: &[u8],
    layout: &StreamLayout,
    replacements: &[Replacement],
    insertions: &[Insertion],
) -> Result<Vec<u8>> {
    let count = layout.resources.len();
    let mut inserted: Vec<Vec<&Insertion>> = vec![Vec::new(); count + 1];
    for insertion in insertions {
        inserted
            .get_mut(insertion.before)
            .ok_or_else(|| {
                Error::InvalidArgument(format!(
                    "insertion position {} out of range",
                    insertion.before
                ))
            })?
            .push(insertion);
    }
    let mut by_index: Vec<Option<&Replacement>> = vec![None; count];
    for replacement in replacements {
        let slot = by_index.get_mut(replacement.index).ok_or_else(|| {
            Error::InvalidArgument(format!("resource index {} out of range", replacement.index))
        })?;
        if slot.is_some() {
            return Err(Error::InvalidArgument(format!(
                "resource {} is replaced more than once",
                replacement.index
            )));
        }
        *slot = Some(replacement);
    }
    let first_replaced = by_index.iter().position(Option::is_some);
    let first_inserted = inserted.iter().position(|list| !list.is_empty());
    let first = match (first_replaced, first_inserted) {
        (None, None) => return Ok(data.to_vec()),
        (Some(a), Some(b)) => a.min(b),
        (Some(a), None) | (None, Some(a)) => a,
    };

    // Cut before the header of the first changed resource, and before a FILL
    // that directly precedes it: that padding depends on the old layout. When
    // the only change is an insertion at the end, cut before the closing FILL.
    let mut cut_block = match layout.resources.get(first) {
        Some(resource) => resource.header_block,
        None => layout.blocks.len(),
    };
    if cut_block > 0 && layout.blocks[cut_block - 1].kind == BlockKind::Fill {
        cut_block -= 1;
    }
    let end_of_blocks = layout
        .blocks
        .last()
        .map_or(layout.header.first_block as usize, |b| b.offset + b.size);
    let cut = layout
        .blocks
        .get(cut_block)
        .map_or(end_of_blocks, |b| b.offset);
    let mut writer = StreamWriter::with_prefix(io::slice(data, 0, cut)?, layout.endian);

    for (index, resource) in layout.resources.iter().enumerate().skip(first) {
        for insertion in &inserted[index] {
            writer.push_resource(&insertion.header, insertion.data, insertion.storage)?;
        }
        match by_index[index] {
            Some(replacement) => {
                writer.push_resource(&resource.header, replacement.data, replacement.storage)?;
            }
            None => {
                writer.push_header(&resource.header)?;
                for block in layout.data_blocks(index) {
                    let body = io::slice(data, block.body().start, block.body().len())?;
                    writer.push_block(block.kind, body)?;
                }
            }
        }
    }
    for insertion in &inserted[count] {
        writer.push_resource(&insertion.header, insertion.data, insertion.storage)?;
    }
    Ok(writer.finish())
}

/// Writes a new stream holding `resources` in order, with the header of the
/// game's own files.
pub fn build(resources: &[(ResourceHeader, &[u8], Storage)]) -> Result<Vec<u8>> {
    let mut writer = StreamWriter::new(&FileHeader::GF2_PC, Endian::Little);
    for (header, data, storage) in resources {
        writer.push_resource(header, data, *storage)?;
    }
    Ok(writer.finish())
}

/// Lays out every block of a stream again from scratch.
///
/// For files written by the game's own tools the result equals the input;
/// this is what validates the placement rule of [`StreamWriter`].
pub fn relayout(data: &[u8], layout: &StreamLayout) -> Result<Vec<u8>> {
    let mut writer = StreamWriter::new(&layout.header, layout.endian);
    for block in &layout.blocks {
        if block.kind == BlockKind::Fill {
            continue;
        }
        let body = io::slice(data, block.body().start, block.body().len())?;
        writer.push_block(block.kind, body)?;
    }
    Ok(writer.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::sdbm;

    fn header(type_tag: &[u8], id: u32) -> ResourceHeader {
        ResourceHeader {
            mem_policy: sdbm(b"default"),
            alignment: 0x10,
            type_id: sdbm(type_tag),
            ids: [id; 4],
            data_size: 0,
        }
    }

    fn pattern(len: usize, seed: u32) -> Vec<u8> {
        let mut state = seed;
        (0..len)
            .map(|i| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                // Mix noise with repetition so the data is partly compressible.
                if i % 64 < 40 {
                    (i % 13) as u8
                } else {
                    (state >> 24) as u8
                }
            })
            .collect()
    }

    fn build(resources: &[(ResourceHeader, Vec<u8>, Storage)]) -> Vec<u8> {
        let mut writer = StreamWriter::new(&FileHeader::GF2_PC, Endian::Little);
        for (header, data, storage) in resources {
            writer.push_resource(header, data, *storage).unwrap();
        }
        writer.finish()
    }

    /// Structural rules observed in every game file.
    fn assert_well_formed(data: &[u8], layout: &StreamLayout) {
        assert_eq!(
            data.len() % PAGE_SIZE,
            0,
            "file length is a multiple of the page size"
        );
        let mut expected_offset = FILE_HEADER_SIZE;
        for block in &layout.blocks {
            assert_eq!(block.offset, expected_offset, "blocks are contiguous");
            assert_eq!(block.size % 4, 0);
            assert_eq!(
                block.offset / PAGE_SIZE,
                (block.offset + block.size - 1) / PAGE_SIZE,
                "block crosses a page boundary"
            );
            if block.kind == BlockKind::Fill {
                assert_eq!(
                    (block.offset + block.size) % PAGE_SIZE,
                    0,
                    "FILL ends its page"
                );
                assert!(data[block.body()].iter().all(|&b| b == 0));
            }
            expected_offset += block.size;
        }
        assert_eq!(expected_offset, data.len());
    }

    #[test]
    fn empty_stream_is_one_page() {
        let data = build(&[]);
        assert_eq!(data.len(), PAGE_SIZE);
        let layout = StreamLayout::parse(&data).unwrap();
        assert!(layout.resources.is_empty());
        assert_eq!(layout.blocks.len(), 1);
        assert_well_formed(&data, &layout);
    }

    #[test]
    fn roundtrip_raw_and_compressed_resources() {
        let resources = vec![
            (header(b"tg4h", 1), pattern(118, 1), Storage::Raw),
            (header(b"tg4d", 2), pattern(300_000, 2), Storage::Raw),
            (header(b"mesh", 3), pattern(450_001, 3), Storage::RefPack),
            (header(b"lua", 4), Vec::new(), Storage::Raw),
            (header(b"hkx", 5), pattern(7, 5), Storage::RefPack),
        ];
        let data = build(&resources);
        let layout = StreamLayout::parse(&data).unwrap();
        assert_well_formed(&data, &layout);
        assert_eq!(layout.resources.len(), resources.len());
        for (index, (header, expected, storage)) in resources.iter().enumerate() {
            let resource = &layout.resources[index];
            assert_eq!(resource.header.type_id, header.type_id);
            assert_eq!(resource.header.data_size as usize, expected.len());
            assert_eq!(&layout.resource_data(&data, index).unwrap(), expected);
            if !expected.is_empty() {
                assert_eq!(layout.is_compressed(index), *storage == Storage::RefPack);
            }
        }
        assert_eq!(layout.total_data_size(), 118 + 300_000 + 450_001 + 7);
        assert_eq!(relayout(&data, &layout).unwrap(), data);
        assert_eq!(rebuild(&data, &layout, &[]).unwrap(), data);
    }

    #[test]
    fn never_leaves_a_four_byte_gap() {
        // Sweep data sizes around the page boundary to hit every remainder.
        let first_room = PAGE_SIZE - FILE_HEADER_SIZE - (BLOCK_HEADER_SIZE + RESOURCE_HEADER_SIZE);
        for delta in 0..96usize {
            let len = first_room - BLOCK_HEADER_SIZE - 64 + delta;
            let resources = vec![
                (header(b"mesh", 1), pattern(len, 7), Storage::Raw),
                (header(b"lua", 2), pattern(33, 8), Storage::Raw),
            ];
            let data = build(&resources);
            let layout = StreamLayout::parse(&data).unwrap();
            assert_well_formed(&data, &layout);
            assert_eq!(
                layout.resource_data(&data, 0).unwrap(),
                resources[0].1,
                "delta {delta}"
            );
            assert_eq!(
                layout.resource_data(&data, 1).unwrap(),
                resources[1].1,
                "delta {delta}"
            );
        }
    }

    #[test]
    fn rebuild_replaces_one_resource_and_keeps_the_rest() {
        let resources = vec![
            (header(b"tg4h", 1), pattern(200, 1), Storage::Raw),
            (header(b"tg4d", 2), pattern(140_000, 2), Storage::RefPack),
            (header(b"mesh", 3), pattern(90_000, 3), Storage::RefPack),
            (header(b"lua", 4), pattern(5_000, 4), Storage::Raw),
        ];
        let data = build(&resources);
        let layout = StreamLayout::parse(&data).unwrap();

        for storage in [Storage::Raw, Storage::RefPack] {
            let new_data = pattern(260_123, 99);
            let rebuilt = rebuild(
                &data,
                &layout,
                &[Replacement {
                    index: 1,
                    data: &new_data,
                    storage,
                }],
            )
            .unwrap();
            let new_layout = StreamLayout::parse(&rebuilt).unwrap();
            assert_well_formed(&rebuilt, &new_layout);
            assert_eq!(new_layout.resources.len(), 4);
            // The prefix before the replaced resource is untouched.
            let cut = layout.blocks[layout.resources[1].header_block].offset;
            assert_eq!(rebuilt[..cut], data[..cut]);
            assert_eq!(
                new_layout.resource_data(&rebuilt, 0).unwrap(),
                resources[0].1
            );
            assert_eq!(new_layout.resource_data(&rebuilt, 1).unwrap(), new_data);
            assert_eq!(
                new_layout.resource_data(&rebuilt, 2).unwrap(),
                resources[2].1
            );
            assert_eq!(
                new_layout.resource_data(&rebuilt, 3).unwrap(),
                resources[3].1
            );
            assert_eq!(new_layout.resources[1].header.ids, [2; 4]);
            assert_eq!(new_layout.is_compressed(1), storage == Storage::RefPack);
            // Untouched compressed resources keep their original blocks.
            assert_eq!(new_layout.stored_size(2), layout.stored_size(2));
        }
    }

    #[test]
    fn rebuild_inserts_new_resources() {
        let resources = vec![
            (header(b"tg4h", 1), pattern(200, 1), Storage::Raw),
            (header(b"tg4d", 2), pattern(140_000, 2), Storage::RefPack),
            (header(b"mesh", 3), pattern(90_000, 3), Storage::Raw),
        ];
        let data = build(&resources);
        let layout = StreamLayout::parse(&data).unwrap();
        let new_a = pattern(70_000, 7);
        let new_b = pattern(33, 8);
        let new_c = pattern(300_000, 9);
        let rebuilt = rebuild_with(
            &data,
            &layout,
            &[Replacement {
                index: 2,
                data: &new_b,
                storage: Storage::Raw,
            }],
            &[
                Insertion {
                    before: 1,
                    header: header(b"tg4h", 10),
                    data: &new_a,
                    storage: Storage::RefPack,
                },
                Insertion {
                    before: 1,
                    header: header(b"lua", 11),
                    data: &new_b,
                    storage: Storage::Raw,
                },
                Insertion {
                    before: 3,
                    header: header(b"hkx", 12),
                    data: &new_c,
                    storage: Storage::Raw,
                },
            ],
        )
        .unwrap();
        let new_layout = StreamLayout::parse(&rebuilt).unwrap();
        assert_well_formed(&rebuilt, &new_layout);
        let ids: Vec<u32> = new_layout
            .resources
            .iter()
            .map(|r| r.header.ids[0])
            .collect();
        assert_eq!(ids, [1, 10, 11, 2, 3, 12]);
        let expected: [&[u8]; 6] = [
            &resources[0].1,
            &new_a,
            &new_b,
            &resources[1].1,
            &new_b,
            &new_c,
        ];
        for (index, want) in expected.iter().enumerate() {
            assert_eq!(new_layout.resource_data(&rebuilt, index).unwrap(), *want);
            assert_eq!(
                new_layout.resources[index].header.data_size as usize,
                want.len()
            );
        }
        assert!(new_layout.is_compressed(1));
        // The prefix before the first insertion is untouched.
        let cut = layout.blocks[layout.resources[1].header_block].offset;
        assert_eq!(rebuilt[..cut], data[..cut]);
    }

    #[test]
    fn rebuild_appends_at_the_end() {
        // A stream whose last data block ends exactly at the page boundary has no closing FILL.
        let exact = MAX_BLOCK_BODY - BLOCK_HEADER_SIZE - RESOURCE_HEADER_SIZE - FILE_HEADER_SIZE;
        for len in [100, exact] {
            let resources = vec![(header(b"lua", 1), pattern(len, 1), Storage::Raw)];
            let data = build(&resources);
            let layout = StreamLayout::parse(&data).unwrap();
            let extra = pattern(5_000, 2);
            let rebuilt = rebuild_with(
                &data,
                &layout,
                &[],
                &[Insertion {
                    before: 1,
                    header: header(b"lua", 2),
                    data: &extra,
                    storage: Storage::Raw,
                }],
            )
            .unwrap();
            let new_layout = StreamLayout::parse(&rebuilt).unwrap();
            assert_well_formed(&rebuilt, &new_layout);
            assert_eq!(new_layout.resources.len(), 2);
            assert_eq!(
                new_layout.resource_data(&rebuilt, 0).unwrap(),
                resources[0].1
            );
            assert_eq!(new_layout.resource_data(&rebuilt, 1).unwrap(), extra);
        }
        let data = build(&[]);
        let layout = StreamLayout::parse(&data).unwrap();
        let bad = Insertion {
            before: 1,
            header: header(b"lua", 2),
            data: &[],
            storage: Storage::Raw,
        };
        assert!(rebuild_with(&data, &layout, &[], &[bad]).is_err());
    }

    #[test]
    fn rebuild_rejects_bad_replacements() {
        let data = build(&[(header(b"lua", 1), pattern(10, 1), Storage::Raw)]);
        let layout = StreamLayout::parse(&data).unwrap();
        let r = |index| Replacement {
            index,
            data: b"x",
            storage: Storage::Raw,
        };
        assert!(rebuild(&data, &layout, &[r(1)]).is_err());
        assert!(rebuild(&data, &layout, &[r(0), r(0)]).is_err());
    }

    #[test]
    fn parse_rejects_malformed_input() {
        assert!(StreamLayout::parse(b"").is_err());
        assert!(StreamLayout::parse(b"nope").is_err());
        assert!(matches!(
            StreamLayout::parse(b"RIFF\x0c\0\0\0\x02\0\x03\0"),
            Err(Error::BadMagic { .. })
        ));

        let good = build(&[(header(b"lua", 1), pattern(100, 1), Storage::Raw)]);

        let mut bad = good.clone();
        bad[8] = 9; // version
        assert!(matches!(
            StreamLayout::parse(&bad),
            Err(Error::UnsupportedVersion { .. })
        ));

        let mut bad = good.clone();
        bad[FILE_HEADER_SIZE] = b'X'; // block tag
        assert!(StreamLayout::parse(&bad).is_err());

        let mut bad = good.clone();
        bad[FILE_HEADER_SIZE + 4..FILE_HEADER_SIZE + 8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(StreamLayout::parse(&bad).is_err());

        let mut bad = good.clone();
        bad[FILE_HEADER_SIZE + 4..FILE_HEADER_SIZE + 8].copy_from_slice(&4u32.to_le_bytes());
        assert!(StreamLayout::parse(&bad).is_err());

        // Truncation anywhere must produce an error or a shorter valid parse, never a panic.
        for len in (0..600).chain([good.len() - 1]) {
            let _ = StreamLayout::parse(&good[..len]);
        }

        // Data block before any resource header.
        let mut writer = StreamWriter::new(&FileHeader::GF2_PC, Endian::Little);
        writer.push_block(BlockKind::Raw, b"abcd").unwrap();
        assert!(StreamLayout::parse(&writer.finish()).is_err());
    }

    #[test]
    fn resource_data_detects_size_mismatch_and_corruption() {
        let mut data = build(&[(header(b"lua", 1), pattern(100, 1), Storage::Raw)]);
        let layout = StreamLayout::parse(&data).unwrap();
        // Declare more data than the blocks hold.
        let size_field = FILE_HEADER_SIZE + BLOCK_HEADER_SIZE + 28;
        data[size_field..size_field + 4].copy_from_slice(&500u32.to_le_bytes());
        let layout_bad = StreamLayout::parse(&data).unwrap();
        assert!(layout_bad.resource_data(&data, 0).is_err());
        assert!(layout.resource_data(&data, 5).is_err());

        let mut data = build(&[(header(b"mesh", 1), pattern(50_000, 1), Storage::RefPack)]);
        let layout = StreamLayout::parse(&data).unwrap();
        let body = layout.data_blocks(0).next().unwrap().body();
        data[body.start + 5] = 0; // RefPack signature
        assert!(matches!(
            layout.resource_data(&data, 0),
            Err(Error::Refpack(_))
        ));
    }

    #[test]
    fn surplus_data_is_dropped_and_reported() {
        let mut writer = StreamWriter::new(&FileHeader::GF2_PC, Endian::Little);
        writer
            .push_header(&ResourceHeader {
                data_size: 10,
                ..header(b"vfx", 1)
            })
            .unwrap();
        writer.push_block(BlockKind::Raw, &[7u8; 28]).unwrap();
        let data = writer.finish();
        let layout = StreamLayout::parse(&data).unwrap();
        let (out, excess) = layout.resource_data_with_excess(&data, 0).unwrap();
        assert_eq!(out, [7u8; 10]);
        assert_eq!(excess, 15);
        assert_eq!(layout.resource_data(&data, 0).unwrap(), [7u8; 10]);
    }

    #[test]
    fn big_endian_streams_roundtrip() {
        let mut writer = StreamWriter::new(&FileHeader::GF2_PC, Endian::Big);
        let payload = pattern(1234, 3);
        writer
            .push_resource(&header(b"mesh", 7), &payload, Storage::Raw)
            .unwrap();
        let data = writer.finish();
        assert_eq!(&data[..4], b"ols3");
        assert_eq!(&data[FILE_HEADER_SIZE..FILE_HEADER_SIZE + 4], b"SHOC");
        let layout = StreamLayout::parse(&data).unwrap();
        assert_eq!(layout.endian, Endian::Big);
        assert_eq!(layout.resources[0].header.ids, [7; 4]);
        assert_eq!(layout.resource_data(&data, 0).unwrap(), payload);
    }
}

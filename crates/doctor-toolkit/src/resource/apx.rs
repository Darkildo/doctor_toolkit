//! `apx` resources: Flash user interface files compiled with EA's Apt tools.
//!
//! # Layout
//!
//! ```text
//! 0x00  "apxf" (stored as the little-endian u32 0x61707866)
//! 0x04  u32  total size
//! 0x08  blocks: u32 tag, u32 size including these 8 bytes, data
//! ```
//!
//! | Tag | Content |
//! |---|---|
//! | `titl` | movie name, NUL-terminated |
//! | `apti` | Apt movie data, starts with `Apt Data:` (the `.apt` file of EA's tools, not SWF) |
//! | `cons` | Apt constant table, starts with `Apt constant file` (the `.const` file) |
//! | `rubi` | unknown binary data |
//! | `tx4h` | texture headers in TG4 format, possibly several, see [`crate::resource::tg4`] |
//! | `tx4t` | data of one texture; follow their `tx4h` in order |
//! | `alig` | zero padding so that the data of the next block starts on a 16-byte boundary |

use crate::io::{self, Endian};
use crate::resource::texture::Texture;
use crate::resource::tg4::{self, Tg4Header};
use crate::{Error, Result};

const MAGIC: u32 = u32::from_be_bytes(*b"apxf");
const BLOCK_HEADER: usize = 8;
const DATA_ALIGNMENT: usize = 16;

pub const TAG_TITLE: [u8; 4] = *b"titl";
pub const TAG_APT: [u8; 4] = *b"apti";
pub const TAG_CONSTANTS: [u8; 4] = *b"cons";
pub const TAG_TEXTURE_HEADER: [u8; 4] = *b"tx4h";
pub const TAG_TEXTURE_DATA: [u8; 4] = *b"tx4t";
pub const TAG_ALIGN: [u8; 4] = *b"alig";

/// One block. `tag` is the readable form, e.g. `*b"apti"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub tag: [u8; 4],
    pub data: Vec<u8>,
}

/// A parsed `apx` resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AptFile {
    pub blocks: Vec<Block>,
}

impl AptFile {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let le = Endian::Little;
        let magic = io::read_u32(data, 0, le)?;
        if magic != MAGIC {
            return Err(Error::BadMagic {
                what: "apx",
                found: magic,
            });
        }
        let total = io::read_u32(data, 4, le)? as usize;
        if total != data.len() {
            return Err(Error::malformed(
                "apx",
                4,
                format!("total size {total}, data has {}", data.len()),
            ));
        }
        let mut blocks = Vec::new();
        let mut pos = BLOCK_HEADER;
        while pos < data.len() {
            let tag = io::read_u32(data, pos, le)?.to_be_bytes();
            let size = io::read_u32(data, pos + 4, le)? as usize;
            if size < BLOCK_HEADER {
                return Err(Error::malformed(
                    "apx block",
                    pos,
                    format!("size {size} too small"),
                ));
            }
            let body = io::slice(data, pos + BLOCK_HEADER, size - BLOCK_HEADER)?;
            blocks.push(Block {
                tag,
                data: body.to_vec(),
            });
            pos += size;
        }
        Ok(Self { blocks })
    }

    /// Serialises the file. `alig` blocks are resized to keep the following
    /// block's data 16-byte aligned, so blocks may be edited freely.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let le = Endian::Little;
        let mut out = Vec::new();
        io::put_u32(&mut out, MAGIC, le);
        io::put_u32(&mut out, 0, le);
        for block in &self.blocks {
            let data_len = if block.tag == TAG_ALIGN {
                // The next block header starts after this one; its data must be aligned.
                let next_data = out.len() + BLOCK_HEADER + BLOCK_HEADER;
                io::align_up(next_data, DATA_ALIGNMENT) - next_data
            } else {
                block.data.len()
            };
            let size = u32::try_from(BLOCK_HEADER + data_len)
                .map_err(|_| Error::InvalidArgument("apx block larger than 4 GiB".into()))?;
            io::put_u32(&mut out, u32::from_be_bytes(block.tag), le);
            io::put_u32(&mut out, size, le);
            if block.tag == TAG_ALIGN {
                out.resize(out.len() + data_len, 0);
            } else {
                out.extend_from_slice(&block.data);
            }
        }
        let total = out.len() as u32;
        out[4..8].copy_from_slice(&total.to_le_bytes());
        Ok(out)
    }

    /// The first block with `tag`.
    pub fn block(&self, tag: [u8; 4]) -> Option<&Block> {
        self.blocks.iter().find(|b| b.tag == tag)
    }

    /// The movie name from the `titl` block.
    pub fn title(&self) -> Option<String> {
        let data = &self.block(TAG_TITLE)?.data;
        let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
        Some(String::from_utf8_lossy(&data[..end]).into_owned())
    }

    /// Embedded textures: each `tx4h` block lists headers whose data follows
    /// in that many `tx4t` blocks.
    pub fn textures(&self) -> Result<Vec<Texture>> {
        let mut textures = Vec::new();
        let mut pending: std::collections::VecDeque<Tg4Header> = Default::default();
        for block in &self.blocks {
            if block.tag == TAG_TEXTURE_HEADER {
                if !pending.is_empty() {
                    return Err(Error::malformed("apx", 0, "texture headers without data"));
                }
                pending.extend(tg4::parse_set(&block.data)?);
            } else if block.tag == TAG_TEXTURE_DATA {
                let header = pending
                    .pop_front()
                    .ok_or_else(|| Error::malformed("apx", 0, "texture data without a header"))?;
                if block.data.len() != header.data_size as usize {
                    return Err(Error::malformed(
                        "apx",
                        0,
                        format!(
                            "texture {} has {} byte(s), header says {}",
                            header.name,
                            block.data.len(),
                            header.data_size
                        ),
                    ));
                }
                textures.push(Texture {
                    header,
                    data: block.data.clone(),
                });
            }
        }
        if !pending.is_empty() {
            return Err(Error::malformed("apx", 0, "texture headers without data"));
        }
        Ok(textures)
    }
}

/// Start of the `apti` block data.
pub const APT_SIGNATURE: &[u8] = b"Apt Data:";
/// Start of the `cons` block data.
pub const CONSTANTS_SIGNATURE: &[u8] = b"Apt constant file";

/// Where an embedded texture is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureSlot {
    /// Index into [`AptFile::blocks`] of the `tx4h` block with the header.
    pub header_block: usize,
    /// Position of the header in that block's set.
    pub position: usize,
    /// Index into [`AptFile::blocks`] of the `tx4t` block with the data.
    pub data_block: usize,
}

impl AptFile {
    /// Where each texture of [`AptFile::textures`] is stored, in the same order.
    pub fn texture_slots(&self) -> Result<Vec<TextureSlot>> {
        let mut slots = Vec::new();
        let mut pending = std::collections::VecDeque::new();
        for (i, block) in self.blocks.iter().enumerate() {
            if block.tag == TAG_TEXTURE_HEADER {
                let count = tg4::parse_set(&block.data)?.len();
                pending.extend((0..count).map(|position| (i, position)));
            } else if block.tag == TAG_TEXTURE_DATA {
                let (header_block, position) = pending
                    .pop_front()
                    .ok_or_else(|| Error::malformed("apx", 0, "texture data without a header"))?;
                slots.push(TextureSlot {
                    header_block,
                    position,
                    data_block: i,
                });
            }
        }
        Ok(slots)
    }

    /// Replaces embedded texture `index` (header and data). The other
    /// headers of its `tx4h` block are kept; `alig` blocks are added where a
    /// block would lose its 16-byte alignment.
    pub fn set_texture(&mut self, index: usize, texture: &Texture) -> Result<()> {
        if texture.header.expected_data_size()? != texture.data.len()
            || texture.header.data_size as usize != texture.data.len()
        {
            return Err(Error::InvalidArgument(format!(
                "texture {} has {} byte(s) of data, its header says otherwise",
                texture.header.name,
                texture.data.len()
            )));
        }
        let slot = *self.texture_slots()?.get(index).ok_or_else(|| {
            Error::InvalidArgument(format!("the apx file has no embedded texture {index}"))
        })?;
        let mut set = tg4::parse_set(&self.blocks[slot.header_block].data)?;
        set[slot.position] = texture.header.clone();
        self.blocks[slot.header_block].data = tg4::set_to_bytes(&set)?;
        self.blocks[slot.data_block].data = texture.data.clone();
        self.align_blocks();
        Ok(())
    }

    /// Replaces embedded texture `index` with an image file: PNG (encoded in
    /// the texture's format) or DDS. Name, hash, parameters and the number
    /// of mip levels are kept (the game crashes at start when an embedded
    /// texture gains mip levels). Returns the new texture.
    pub fn import_texture(&mut self, index: usize, image: &[u8], is_png: bool) -> Result<Texture> {
        let original = self.textures()?.into_iter().nth(index).ok_or_else(|| {
            Error::InvalidArgument(format!("the apx file has no embedded texture {index}"))
        })?;
        let mips = original.header.mip_count();
        let texture = if is_png {
            crate::resource::texture::from_png_with_mips(&original.header, image, Some(mips))?
        } else {
            crate::resource::texture::from_dds(&original.header, image)?
        };
        if texture.header.mip_count() != mips {
            return Err(Error::InvalidArgument(format!(
                "embedded texture {} has {mips} mip level(s), the {} {}; \
                 the game crashes when they differ",
                original.header.name,
                if is_png {
                    "image is too small for them and has"
                } else {
                    "DDS file"
                },
                texture.header.mip_count()
            )));
        }
        self.set_texture(index, &texture)?;
        Ok(texture)
    }

    /// Replaces the data of the first block with `tag`, which must exist.
    pub fn set_block(&mut self, tag: [u8; 4], data: Vec<u8>) -> Result<()> {
        let block = self
            .blocks
            .iter_mut()
            .find(|b| b.tag == tag)
            .ok_or_else(|| {
                Error::InvalidArgument(format!(
                    "the apx file has no `{}` block",
                    String::from_utf8_lossy(&tag)
                ))
            })?;
        block.data = data;
        self.align_blocks();
        Ok(())
    }

    /// Replaces the Apt movie (`apti`) with the contents of an `.apt` file.
    pub fn set_apt(&mut self, data: Vec<u8>) -> Result<()> {
        if !data.starts_with(APT_SIGNATURE) {
            return Err(Error::InvalidArgument(
                "not an .apt file: it does not start with `Apt Data:`".into(),
            ));
        }
        self.set_block(TAG_APT, data)
    }

    /// Replaces the constant table (`cons`) with the contents of a `.const` file.
    pub fn set_constants(&mut self, data: Vec<u8>) -> Result<()> {
        if !data.starts_with(CONSTANTS_SIGNATURE) {
            return Err(Error::InvalidArgument(
                "not a .const file: it does not start with `Apt constant file`".into(),
            ));
        }
        self.set_block(TAG_CONSTANTS, data)
    }

    /// Text references of the movie: `$name` strings (NUL-terminated, after a
    /// NUL) in the `apti` and `cons` blocks, which the game shows as the
    /// string table entry with hash [`crate::hash::string_id`]`(name)`.
    /// Returns `(block index, offset in the block, name)`.
    pub fn text_references(&self) -> Vec<(usize, usize, String)> {
        let mut out = Vec::new();
        for (b, block) in self.blocks.iter().enumerate() {
            if block.tag != TAG_APT && block.tag != TAG_CONSTANTS {
                continue;
            }
            let data = &block.data;
            let mut at = 0;
            while let Some(found) = data[at..].iter().position(|&c| c == b'$') {
                let start = at + found;
                at = start + 1;
                if start > 0 && data[start - 1] != 0 {
                    continue;
                }
                let Some(len) = data[start + 1..].iter().position(|&c| c == 0) else {
                    break;
                };
                let name = &data[start + 1..start + 1 + len];
                let identifier = name.first().is_some_and(|c| c.is_ascii_alphabetic())
                    && name.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_');
                if identifier {
                    out.push((b, start, String::from_utf8_lossy(name).into_owned()));
                }
            }
        }
        out
    }

    /// Points every `$old` text reference at `$new` instead (names compare
    /// without case, like their hashes). The string is rewritten in place, so
    /// `new` may not be longer than `old`. Returns the number of references
    /// changed.
    pub fn retarget_text(&mut self, old: &str, new: &str) -> Result<usize> {
        let valid = |n: &str| {
            n.as_bytes()
                .first()
                .is_some_and(|c| c.is_ascii_alphabetic())
                && n.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
        };
        if !valid(new) {
            return Err(Error::InvalidArgument(format!(
                "`{new}` is not a string name (letters, digits and _)"
            )));
        }
        if new.len() > old.len() {
            return Err(Error::InvalidArgument(format!(
                "`{new}` is longer than `{old}`: references are rewritten in place"
            )));
        }
        let mut changed = 0;
        for (b, at, name) in self.text_references() {
            if !name.eq_ignore_ascii_case(old) {
                continue;
            }
            let data = &mut self.blocks[b].data;
            let field = &mut data[at + 1..at + 1 + old.len()];
            field.fill(0);
            field[..new.len()].copy_from_slice(new.as_bytes());
            changed += 1;
        }
        Ok(changed)
    }

    /// Inserts an `alig` block before every block (other than `titl` and
    /// `alig`) whose data would not start on a 16-byte boundary.
    ///
    /// In the game files the data of every such block is aligned; the
    /// original tools leave `alig` out where a block happens to be aligned
    /// already, so a block that grows or shrinks can misalign the next one.
    /// Existing `alig` blocks are kept; [`AptFile::to_bytes`] sizes them.
    pub fn align_blocks(&mut self) {
        let mut pos = BLOCK_HEADER;
        let mut i = 0;
        while i < self.blocks.len() {
            let tag = self.blocks[i].tag;
            let size = if tag == TAG_ALIGN {
                let next_data = pos + 2 * BLOCK_HEADER;
                BLOCK_HEADER + io::align_up(next_data, DATA_ALIGNMENT) - next_data
            } else {
                if tag != TAG_TITLE && !(pos + BLOCK_HEADER).is_multiple_of(DATA_ALIGNMENT) {
                    let alig = Block {
                        tag: TAG_ALIGN,
                        data: Vec::new(),
                    };
                    self.blocks.insert(i, alig);
                    continue;
                }
                BLOCK_HEADER + self.blocks[i].data.len()
            };
            pos += size;
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alignment_is_restored_after_edits() {
        // As in the game: no `alig` between blocks that happen to be aligned.
        let mut file = AptFile {
            blocks: vec![
                block(b"titl", b"0123456\0"),
                block(b"alig", &[]),
                block(b"apti", b"Apt Data:\x1a\0\0\0\0\0\0\0\0\0\0\0\0\0\0"),
                block(b"cons", b"Apt constant file\x1a\0"),
            ],
        };
        let check = |file: &AptFile| {
            let bytes = file.to_bytes().unwrap();
            let parsed = AptFile::parse(&bytes).unwrap();
            let mut pos = 8;
            for b in &parsed.blocks {
                if b.tag != TAG_TITLE && b.tag != TAG_ALIGN {
                    assert_eq!((pos + 8) % 16, 0, "{:?}", String::from_utf8_lossy(&b.tag));
                }
                pos += 8 + b.data.len();
            }
            parsed
        };
        let before = file.to_bytes().unwrap();
        file.align_blocks();
        assert_eq!(
            file.to_bytes().unwrap(),
            before,
            "aligned files stay as they are"
        );
        check(&file);
        file.set_apt(b"Apt Data:\x1a\0longer".to_vec()).unwrap();
        let parsed = check(&file);
        assert_eq!(parsed.blocks.len(), 5, "an alig block was added");
        assert_eq!(
            parsed.block(TAG_APT).unwrap().data,
            b"Apt Data:\x1a\0longer"
        );
        assert!(file.set_apt(b"FWS".to_vec()).is_err());
        assert!(file.set_constants(b"Apt Data:".to_vec()).is_err());
        file.set_constants(b"Apt constant file\x1a\0\x01".to_vec())
            .unwrap();
        check(&file);
        assert!(file.set_block(*b"rubi", Vec::new()).is_err());
    }

    fn block(tag: &[u8; 4], data: &[u8]) -> Block {
        Block {
            tag: *tag,
            data: data.to_vec(),
        }
    }

    #[test]
    fn roundtrip_and_alignment() {
        let file = AptFile {
            blocks: vec![
                block(b"titl", b"shared\0"),
                block(b"alig", &[]),
                block(b"apti", b"Apt Data:\x1a\0"),
                block(b"alig", &[]),
                block(b"cons", b"Apt constant file\x1a\0"),
            ],
        };
        let bytes = file.to_bytes().unwrap();
        assert_eq!(&bytes[..4], b"fxpa");
        assert_eq!(
            io::read_u32(&bytes, 4, Endian::Little).unwrap() as usize,
            bytes.len()
        );
        let parsed = AptFile::parse(&bytes).unwrap();
        assert_eq!(parsed.title().as_deref(), Some("shared"));
        // Every block that follows an alig block has 16-byte aligned data.
        let mut pos = 8;
        let mut previous = [0u8; 4];
        for b in &parsed.blocks {
            if previous == TAG_ALIGN {
                assert_eq!((pos + 8) % 16, 0, "{:?}", std::str::from_utf8(&b.tag));
            }
            previous = b.tag;
            pos += 8 + b.data.len();
        }
        assert_eq!(parsed.to_bytes().unwrap(), bytes);
        assert!(parsed.textures().unwrap().is_empty());
    }

    #[test]
    fn text_references_are_found_and_retargeted() {
        let mut file = AptFile {
            blocks: vec![
                block(b"titl", b"menu\0"),
                block(
                    b"cons",
                    b"Apt constant file\x1a\0Impact\0\0$fe_press_start\0\0a$no\0$ui_x\0",
                ),
            ],
        };
        let refs = file.text_references();
        let names: Vec<&str> = refs.iter().map(|r| r.2.as_str()).collect();
        assert_eq!(names, ["fe_press_start", "ui_x"]);
        assert_eq!(
            file.retarget_text("FE_PRESS_START", "doctor_new").unwrap(),
            1
        );
        let names: Vec<String> = file.text_references().into_iter().map(|r| r.2).collect();
        assert_eq!(names, ["doctor_new", "ui_x"]);
        assert!(file.retarget_text("ui_x", "longer_name").is_err());
        assert!(file.retarget_text("ui_x", "a b").is_err());
        assert_eq!(file.retarget_text("missing", "x").unwrap(), 0);
        let bytes = file.to_bytes().unwrap();
        assert_eq!(AptFile::parse(&bytes).unwrap(), file);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(AptFile::parse(b"").is_err());
        assert!(AptFile::parse(b"fxpa\x10\0\0\0titl\x04\0\0\0").is_err());
        assert!(AptFile::parse(b"fxpa\x09\0\0\0x").is_err());
        let bytes = AptFile {
            blocks: vec![block(b"titl", b"a\0")],
        }
        .to_bytes()
        .unwrap();
        for len in 0..bytes.len() {
            let _ = AptFile::parse(&bytes[..len]);
        }
    }
}

//! `ffn` resources: bitmap fonts. The glyph images are in a texture (`tg4h`/`tg4d`)
//! of the same stream whose name equals the font name.
//!
//! ```text
//! 0x000  "FONT" (stored as the little-endian u32 0x464F4E54, bytes "TNOF")
//! 0x004  u32  file size
//! 0x008  u16  version, 7
//! 0x00A  u16  glyph count N
//! 0x00C  u32  offset of the glyph table, 0x454
//! 0x014  3 × u32  font metrics (unknown meaning; e.g. 43, 50, 41 for impact_lt_std)
//! 0x020  char[32]  name = texture name
//! 0x040  u32  offset of the page area (end of the glyph table)
//! 0x044  256 × u32  page directory: for every high byte of a UTF-16 code that has
//!        glyphs, the offset of a 0x400-byte page (from the page area); 0 otherwise
//! 0x448  u32  kerning entry count
//! 0x44C  u32  kerning offset (3 bytes per entry, layout not decoded)
//! 0x454  N × glyph, 24 bytes:
//!        u16 code (UTF-16), u16 width, u16 height, u16 advance,
//!        i16 x offset, i16 y offset, 4 × u16 atlas rectangle (u0, v0, u1, v1) in
//!        1/16 pixel, u32 0
//! page area: zero-filled pages of 256 × u32, filled with glyph pointers at run time
//! ```
//!
//! Adding glyphs for a new script (Cyrillic, U+04xx) means adding glyph records,
//! a page for the high byte 0x04, atlas space in the texture, and fixing the offsets
//! after the glyph table.

use serde::Serialize;

use crate::io::{self, Endian};
use crate::{Error, Result};

/// Type id of `ffn` resources.
pub const TYPE_FFN: u32 = crate::hash::sdbm(b"ffn");

const MAGIC: &[u8; 4] = b"TNOF";
const GLYPH_SIZE: usize = 24;
const PAGE_DIRECTORY: usize = 0x44;
const PAGE_SIZE: usize = 0x400;

/// One glyph.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Glyph {
    /// UTF-16 code unit.
    pub code: u16,
    pub width: u16,
    pub height: u16,
    pub advance: u16,
    pub x_offset: i16,
    pub y_offset: i16,
    /// Atlas rectangle in pixels: left, top, right, bottom.
    pub rect: [f32; 4],
}

impl Glyph {
    /// The character, when the code is a valid scalar value.
    pub fn character(&self) -> Option<char> {
        char::from_u32(self.code as u32)
    }
}

/// A parsed font.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Font {
    pub version: u16,
    pub name: String,
    pub metrics: [u32; 3],
    pub glyphs: Vec<Glyph>,
    /// High bytes of the UTF-16 codes that have a page.
    pub pages: Vec<u8>,
    pub kerning_count: u32,
    /// Raw kerning table.
    #[serde(skip)]
    pub kerning: Vec<u8>,
}

impl Font {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let le = Endian::Little;
        if io::slice(data, 0, 4)? != MAGIC {
            return Err(Error::BadMagic {
                what: "ffn",
                found: io::read_u32(data, 0, le)?,
            });
        }
        let version = io::read_u16(data, 8, le)?;
        if version != 7 {
            return Err(Error::UnsupportedVersion {
                what: "ffn",
                version: version as u32,
            });
        }
        let count = io::read_u16(data, 10, le)? as usize;
        let table = io::read_u32(data, 0x0C, le)? as usize;
        let metrics = [0x14, 0x18, 0x1C].map(|o| io::read_u32(data, o, le).unwrap_or(0));
        let name_bytes = io::slice(data, 0x20, 0x20)?;
        let end = name_bytes.iter().position(|&b| b == 0).unwrap_or(32);
        let name = String::from_utf8_lossy(&name_bytes[..end]).into_owned();
        io::slice(data, table, count * GLYPH_SIZE)?;
        let glyphs = (0..count)
            .map(|i| {
                let at = table + GLYPH_SIZE * i;
                let h = |k: usize| io::read_u16(data, at + 2 * k, le);
                Ok(Glyph {
                    code: h(0)?,
                    width: h(1)?,
                    height: h(2)?,
                    advance: h(3)?,
                    x_offset: h(4)? as i16,
                    y_offset: h(5)? as i16,
                    rect: [h(6)?, h(7)?, h(8)?, h(9)?].map(|v| v as f32 / 16.0),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let page_area = io::read_u32(data, 0x40, le)? as usize;
        let mut pages = Vec::new();
        for high in 0..256 {
            let offset = io::read_u32(data, PAGE_DIRECTORY + 4 * high, le)? as usize;
            if offset != 0 {
                io::slice(data, page_area + offset, PAGE_SIZE)?;
                pages.push(high as u8);
            }
        }
        if let Some(g) = glyphs
            .iter()
            .find(|g| !pages.contains(&((g.code >> 8) as u8)))
        {
            return Err(Error::malformed(
                "ffn",
                table,
                format!("glyph {:#06x} has no page", g.code),
            ));
        }
        let kerning_count = io::read_u32(data, 0x448, le)?;
        let kerning_at = io::read_u32(data, 0x44C, le)? as usize;
        let kerning = if kerning_count == 0 {
            Vec::new()
        } else {
            io::slice(data, kerning_at, 3 * kerning_count as usize)?.to_vec()
        };
        Ok(Self {
            version,
            name,
            metrics,
            glyphs,
            pages,
            kerning_count,
            kerning,
        })
    }

    /// The atlas texture: the texture of the stream named like the font.
    pub fn find_atlas(
        &self,
        file: &[u8],
        layout: &crate::container::ustream::StreamLayout,
    ) -> Option<crate::resource::texture::Texture> {
        use crate::resource::{texture, tg4::Tg4Header};
        texture::pairs(layout).into_iter().find_map(|pair| {
            let header = Tg4Header::parse(&layout.resource_data(file, pair.header).ok()?).ok()?;
            if header.name != self.name {
                return None;
            }
            texture::Texture::load(file, layout, pair).ok()
        })
    }

    /// Whether the font has a glyph for `c`.
    pub fn covers(&self, c: char) -> bool {
        self.glyphs.iter().any(|g| g.code as u32 == c as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn sample() -> Vec<u8> {
        let mut d = vec![0u8; 0x454 + 2 * GLYPH_SIZE + 4 + PAGE_SIZE];
        d[..4].copy_from_slice(MAGIC);
        let len = d.len() as u32;
        d[4..8].copy_from_slice(&len.to_le_bytes());
        d[8..10].copy_from_slice(&7u16.to_le_bytes());
        d[10..12].copy_from_slice(&2u16.to_le_bytes());
        d[12..16].copy_from_slice(&0x454u32.to_le_bytes());
        d[0x20..0x24].copy_from_slice(b"test");
        let page_area = 0x454 + 2 * GLYPH_SIZE;
        d[0x40..0x44].copy_from_slice(&(page_area as u32).to_le_bytes());
        d[0x44..0x48].copy_from_slice(&4u32.to_le_bytes()); // page for 0x00xx
        for (i, code) in [b'A' as u16, b'B' as u16].iter().enumerate() {
            let at = 0x454 + GLYPH_SIZE * i;
            let fields = [
                *code,
                10,
                12,
                11,
                1,
                (-2i16) as u16,
                160 * i as u16,
                0,
                160,
                192,
            ];
            for (k, v) in fields.iter().enumerate() {
                d[at + 2 * k..at + 2 * k + 2].copy_from_slice(&v.to_le_bytes());
            }
        }
        d
    }

    #[test]
    fn parses_glyphs() {
        let font = Font::parse(&sample()).unwrap();
        assert_eq!(font.name, "test");
        assert_eq!(font.glyphs.len(), 2);
        assert_eq!(font.pages, vec![0]);
        let a = font.glyphs[0];
        assert_eq!(a.character(), Some('A'));
        assert_eq!((a.width, a.height, a.advance), (10, 12, 11));
        assert_eq!(a.y_offset, -2);
        assert_eq!(a.rect, [0.0, 0.0, 10.0, 12.0]);
        assert!(font.covers('B'));
        assert!(!font.covers('Ж'));
    }

    #[test]
    fn rejects_bad_input() {
        assert!(Font::parse(b"TNOF").is_err());
        let mut d = sample();
        d[0x44] = 0; // glyphs without a page
        assert!(Font::parse(&d).is_err());
        let mut d = sample();
        d[10] = 200; // more glyphs than the file holds
        assert!(Font::parse(&d).is_err());
    }
}

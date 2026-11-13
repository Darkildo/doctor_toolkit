//! `mobface`: the player's crew editor ("Mob face") — colour palettes, face
//! sliders and the apparel menus (one resource in `globals_global`).
//!
//! The menus pick parts of the `player` parts database by name (`jacket_classic_op01`),
//! set culling flags (`untuckedShirtCulled`) and colour parameters
//! (`g_tintcolor0`, `g_skintintcolor`) from the palettes. Sliders drive the
//! facial morph targets the `player` blenders name (`headnawd`, `eyesize`).
//!
//! ```text
//! 0x00  "FboM"; u16 version 9, u16 1
//! 0x08  u32 17; u32 0; u32 size
//! 0x14  ptr, 0x18 u32  menus {*name, *group, *items, u32 count} (16 bytes)
//! 0x1C  ptr, 0x20 u32  sliders {*name, *axes, u32 count} (12 bytes)
//!       axis {u32 axis, *ranges, u32 count}; range {f32 min, f32 max,
//!       *targets, u32 count}; target {*morph name, f32 scale, f32 −1}
//! 0x24  ptr, 0x28 u32  slider links, 32 bytes each (not decoded)
//! 0x2C  ptr, 0x30 u32  palettes {*name, *entries, u32 count}; entry = 12 f32:
//!       four RGB colours 0..255 (tint 0, 1, 2 and a fourth)
//! ```
//! Menu items (the apparel tree) are not decoded beyond their count.

use serde::Serialize;

use crate::io::{self, Endian};
use crate::{Error, Result};

/// Type id of `mobface` resources.
pub const TYPE_MOBFACE: u32 = crate::hash::sdbm(b"mobface");
const MAGIC: u32 = 0x4D6F_6246;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Palette {
    pub name: String,
    /// Four RGB colours (0..255) per entry.
    pub entries: Vec<[[f32; 3]; 4]>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MorphTarget {
    pub name: String,
    pub scale: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SliderRange {
    pub min: f32,
    pub max: f32,
    pub targets: Vec<MorphTarget>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SliderAxis {
    pub axis: u32,
    pub ranges: Vec<SliderRange>,
}

/// A face slider (two axes on the stick).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Slider {
    pub name: String,
    pub axes: Vec<SliderAxis>,
}

/// A top-level menu of the editor.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Menu {
    pub name: String,
    pub group: String,
    pub items: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MobFace {
    pub version: u16,
    pub menus: Vec<Menu>,
    pub sliders: Vec<Slider>,
    pub palettes: Vec<Palette>,
}

struct R<'a>(&'a [u8]);

impl R<'_> {
    fn u32(&self, at: usize) -> Result<u32> {
        io::read_u32(self.0, at, Endian::Little)
    }
    fn f32(&self, at: usize) -> Result<f32> {
        self.u32(at).map(f32::from_bits)
    }
    fn string(&self, field: usize) -> Result<String> {
        let at = self.u32(field)? as usize;
        Ok(String::from_utf8_lossy(io::cstr(self.0, at)?).into_owned())
    }
    /// `(pointer, count)` of an array of `size`-byte entries, bounds-checked.
    fn array(&self, ptr: usize, count: usize, size: usize) -> Result<(usize, usize)> {
        let at = self.u32(ptr)? as usize;
        let n = self.u32(count)? as usize;
        io::slice(self.0, at, n.saturating_mul(size))?;
        Ok((at, n))
    }
}

impl MobFace {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let r = R(data);
        let magic = r.u32(0)?;
        if magic != MAGIC {
            return Err(Error::BadMagic {
                what: "mobface",
                found: magic,
            });
        }
        let version = io::read_u16(data, 4, Endian::Little)?;
        let (at, n) = r.array(0x14, 0x18, 16)?;
        let menus = (0..n)
            .map(|k| {
                let e = at + 16 * k;
                Ok(Menu {
                    name: r.string(e)?,
                    group: r.string(e + 4)?,
                    items: r.u32(e + 12)?,
                })
            })
            .collect::<Result<_>>()?;
        let (at, n) = r.array(0x1C, 0x20, 12)?;
        let sliders = (0..n)
            .map(|k| {
                let e = at + 12 * k;
                let (axes_at, axes) = r.array(e + 4, e + 8, 12)?;
                let axes = (0..axes)
                    .map(|a| {
                        let x = axes_at + 12 * a;
                        let (ranges_at, ranges) = r.array(x + 4, x + 8, 16)?;
                        let ranges = (0..ranges)
                            .map(|q| {
                                let g = ranges_at + 16 * q;
                                let (t_at, t) = r.array(g + 8, g + 12, 12)?;
                                Ok(SliderRange {
                                    min: r.f32(g)?,
                                    max: r.f32(g + 4)?,
                                    targets: (0..t)
                                        .map(|i| {
                                            Ok(MorphTarget {
                                                name: r.string(t_at + 12 * i)?,
                                                scale: r.f32(t_at + 12 * i + 4)?,
                                            })
                                        })
                                        .collect::<Result<_>>()?,
                                })
                            })
                            .collect::<Result<_>>()?;
                        Ok(SliderAxis {
                            axis: r.u32(x)?,
                            ranges,
                        })
                    })
                    .collect::<Result<_>>()?;
                Ok(Slider {
                    name: r.string(e)?,
                    axes,
                })
            })
            .collect::<Result<_>>()?;
        let (at, n) = r.array(0x2C, 0x30, 12)?;
        let palettes = (0..n)
            .map(|k| {
                let e = at + 12 * k;
                let (p, count) = r.array(e + 4, e + 8, 48)?;
                let entries = (0..count)
                    .map(|i| {
                        let mut c = [[0f32; 3]; 4];
                        for (j, colour) in c.iter_mut().enumerate() {
                            for (k, v) in colour.iter_mut().enumerate() {
                                *v = r.f32(p + 48 * i + 12 * j + 4 * k)?;
                            }
                        }
                        Ok(c)
                    })
                    .collect::<Result<_>>()?;
                Ok(Palette {
                    name: r.string(e)?,
                    entries,
                })
            })
            .collect::<Result<_>>()?;
        Ok(Self {
            version,
            menus,
            sliders,
            palettes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_small_image_and_rejects_garbage() {
        let mut d = vec![0u8; 0x100];
        let put =
            |d: &mut Vec<u8>, at: usize, v: u32| d[at..at + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut d, 0, MAGIC);
        d[4] = 9;
        // One palette "skin" with one entry at 0x60; strings at 0xC0.
        d[0xC0..0xC5].copy_from_slice(b"skin\0");
        put(&mut d, 0x2C, 0x50);
        put(&mut d, 0x30, 1);
        put(&mut d, 0x50, 0xC0);
        put(&mut d, 0x54, 0x60);
        put(&mut d, 0x58, 1);
        for i in 0..12 {
            put(&mut d, 0x60 + 4 * i, (i as f32).to_bits());
        }
        let m = MobFace::parse(&d).unwrap();
        assert_eq!(m.version, 9);
        assert_eq!(m.palettes[0].name, "skin");
        assert_eq!(m.palettes[0].entries[0][3], [9.0, 10.0, 11.0]);
        assert!(MobFace::parse(&d[..8]).is_err());
        for i in 0..d.len() {
            let mut c = d.clone();
            c[i] ^= 0x5A;
            let _ = MobFace::parse(&c);
        }
    }
}

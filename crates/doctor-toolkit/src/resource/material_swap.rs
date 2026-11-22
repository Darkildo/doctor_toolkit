//! `materialswaplistresource` (`0xFFB235E6`): lists of material swaps, the
//! variants of a model (`docs/analysis/resource-placement.md` §3).
//!
//! Its ids are a NeoGuid, and SimGroup objects name the resource by that
//! GUID. Every material is named by the CRC-32 of its lower-case name, the
//! hash a mesh draw record uses.
//!
//! ```text
//! 0x00  u32  0x1008
//! 0x04  u32  0
//! 0x08  u32  0
//! 0x0C  u32  list count N
//! 0x10  N × u32  offset of each list
//!       lists, one after another:
//!         u32  swap count M
//!         M × { u32 material, u32 replacement, u32 third hash }
//!       zeros up to the resource's alignment
//! ```
//!
//! All 188 resources of the game parse (corpus test
//! `game_index_covers_every_resource_and_resolves_references`: no record of
//! the index has an error), and each lies in the stream of the object with
//! its GUID (`placement::placement_census`).

use crate::io::{self, Endian};
use crate::{Error, Result};

/// Type id of `materialswaplistresource` resources.
pub const TYPE_MATERIAL_SWAP_LIST: u32 = crate::hash::sdbm(b"materialswaplistresource");

const MAGIC: u32 = 0x1008;
const WHAT: &str = "materialswaplistresource";

/// One swap: draw records with material `from` are drawn with `to`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Swap {
    /// CRC-32 of the lower-case name of the material to replace.
    pub from: u32,
    /// CRC-32 of the lower-case name of the replacement.
    pub to: u32,
    /// Third word; its meaning is not known.
    pub other: u32,
}

/// A parsed swap list resource.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MaterialSwapList {
    pub lists: Vec<Vec<Swap>>,
}

impl MaterialSwapList {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let u32_at = |o: usize| io::read_u32(data, o, Endian::Little);
        let magic = u32_at(0)?;
        if magic != MAGIC {
            return Err(Error::BadMagic {
                what: WHAT,
                found: magic,
            });
        }
        let count = u32_at(0xC)? as usize;
        if count > data.len() / 4 {
            return Err(Error::malformed(WHAT, 0xC, format!("{count} lists")));
        }
        let mut lists = Vec::with_capacity(count);
        for i in 0..count {
            let at = u32_at(0x10 + 4 * i)? as usize;
            let swaps = u32_at(at)? as usize;
            if swaps > data.len() / 12 {
                return Err(Error::malformed(WHAT, at, format!("{swaps} swaps")));
            }
            let mut list = Vec::with_capacity(swaps);
            for k in 0..swaps {
                let o = at + 4 + 12 * k;
                list.push(Swap {
                    from: u32_at(o)?,
                    to: u32_at(o + 4)?,
                    other: u32_at(o + 8)?,
                });
            }
            lists.push(list);
        }
        Ok(Self { lists })
    }

    /// Every material hash of the lists, `from` and `to`, in order, once.
    pub fn materials(&self) -> Vec<u32> {
        let mut out = Vec::new();
        for swap in self.lists.iter().flatten() {
            for hash in [swap.from, swap.to] {
                if !out.contains(&hash) {
                    out.push(hash);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        let words: [u32; 14] = [
            0x1008, 0, 0, 2, 0x18, 0x28, // header, two lists
            1, 0xA, 0xA, 0x77, // list 0: a → a
            2, 0xA, 0xB, 0x77, // list 1: a → b, c → d
        ];
        let mut data: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        data.extend([0xC, 0, 0, 0, 0xD, 0, 0, 0, 0x77, 0, 0, 0]);
        data
    }

    #[test]
    fn parses_lists_and_materials() {
        let list = MaterialSwapList::parse(&sample()).unwrap();
        assert_eq!(list.lists.len(), 2);
        assert_eq!(
            list.lists[1][1],
            Swap {
                from: 0xC,
                to: 0xD,
                other: 0x77
            }
        );
        assert_eq!(list.materials(), [0xA, 0xB, 0xC, 0xD]);
    }

    #[test]
    fn rejects_other_data() {
        let mut data = sample();
        data[0] = 0;
        assert!(MaterialSwapList::parse(&data).is_err());
        assert!(MaterialSwapList::parse(&sample()[..0x30]).is_err());
    }
}

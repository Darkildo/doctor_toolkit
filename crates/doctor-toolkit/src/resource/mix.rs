//! `mix` resources: snapshots of the audio mixer. A mix lists mix categories
//! (by hash, see `auc`) with the levels they take while the mix is applied
//! (`ApplyMix("occlusion.mix")` of the scripts, the ducks of `auc`).
//!
//! ```text
//! 0x00  u32  0 (the game puts a method table here), u32 0
//! 0x08  u32  0x1C, offset of the body
//! 0x0C  3 × u32 0 (run-time fields)
//! 0x18  u32  0x7C90EE18 in every mix of the game
//! body (offsets below are from the body):
//! 0x00  name[32], cut to 31 characters (`combine_chatter_followup_duck.m`)
//! 0x20  u32  15, version
//! 0x24  u32  flags; bit 0 is set by the game once the offsets are fixed up
//! 0x28  u32  number of channels N
//! 0x2C  u32  offset of the first channel = 0x38
//! 0x30  u32  number of records of a second list (0 in the game)
//! 0x34  u32  offset of the second list = end of the channels
//! 0x38  N × 44 bytes, each linked to the next:
//!       +0x00 u32 category: sdbm of a category name of `auc`
//!       +0x04 8 × f32 levels
//!       +0x24 u32 frequency, Hz (96000 in most channels)
//!       +0x28 u32 offset of the next channel from this one (44; 0 in the last)
//! ```
//!
//! `godfather2.exe` 1.0.789.0: handler `0x004565E0`, version check and
//! offset fix-up `0x005DDE90`; the mix is registered by the `sdbm` of its
//! name (`0x00456530`).
//!
//! The meaning of the levels is read off the data, not off the code: level 3
//! is the volume (0..1), levels 6 and 7 are equal times in seconds, the
//! frequency is a low-pass cut-off (`occlusion.mix`: 6000 Hz). The rest is
//! kept as numbers.
//!
//! [`Mix::to_bytes`] reproduces every `mix` of the game byte for byte.

use serde::{Deserialize, Serialize};

use crate::hash::sdbm;
use crate::io::{self, Endian};
use crate::{Error, Result};

/// Type id of `mix` resources.
pub const TYPE_MIX: u32 = sdbm(b"mix");

const BODY: usize = 0x1C;
const NAME: usize = 32;
/// Version of the mixes of the game.
pub const VERSION: u32 = 15;
const FIRST: u32 = 0x38;
const CHANNEL_SIZE: usize = 44;
/// Index of the volume among the levels of a channel.
pub const LEVEL_VOLUME: usize = 3;
/// Indices of the two times (seconds) among the levels of a channel.
pub const LEVEL_TIMES: [usize; 2] = [6, 7];

/// The levels of one mix category.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MixChannel {
    /// `sdbm` of the lower-case category name.
    pub category: u32,
    pub levels: [f32; 8],
    /// Hz; 96000 leaves the sound as it is.
    pub frequency: u32,
}

impl MixChannel {
    /// The volume of the category in the mix, 0..1.
    pub fn volume(&self) -> f32 {
        self.levels[LEVEL_VOLUME]
    }
}

/// A parsed `mix` resource.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mix {
    /// The word at 0x18; the same in every mix of the game.
    pub stamp: u32,
    pub name: String,
    pub version: u32,
    pub flags: u32,
    pub channels: Vec<MixChannel>,
}

impl Mix {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let le = Endian::Little;
        let word = |at: usize| io::read_u32(data, at, le);
        if word(0)? != 0 || word(4)? != 0 || word(8)? as usize != BODY {
            return Err(Error::malformed("mix", 0, "unexpected header"));
        }
        if word(0x0C)? != 0 || word(0x10)? != 0 || word(0x14)? != 0 {
            return Err(Error::malformed("mix", 0x0C, "run-time fields are set"));
        }
        let stamp = word(0x18)?;
        let field = io::slice(data, BODY, NAME)?;
        let end = field.iter().position(|&b| b == 0).unwrap_or(NAME);
        if field[end..].iter().any(|&b| b != 0) {
            return Err(Error::malformed("mix", BODY, "bytes after the name"));
        }
        let name = String::from_utf8(field[..end].to_vec())
            .map_err(|_| Error::malformed("mix", BODY, "name is not UTF-8"))?;
        let version = word(BODY + 0x20)?;
        if version != VERSION {
            return Err(Error::malformed(
                "mix",
                BODY + 0x20,
                format!("version {version}"),
            ));
        }
        let flags = word(BODY + 0x24)?;
        let n = word(BODY + 0x28)? as usize;
        let first = word(BODY + 0x2C)?;
        let second = word(BODY + 0x30)?;
        let end = word(BODY + 0x34)? as usize;
        if second != 0 {
            return Err(Error::malformed(
                "mix",
                BODY + 0x30,
                "a second list is not supported",
            ));
        }
        if (n != 0 && first != FIRST) || end != FIRST as usize + n * CHANNEL_SIZE {
            return Err(Error::malformed("mix", BODY + 0x2C, "unexpected offsets"));
        }
        let mut channels = Vec::with_capacity(n);
        let mut at = BODY + FIRST as usize;
        for i in 0..n {
            let c = io::slice(data, at, CHANNEL_SIZE)?;
            let w = |k: usize| io::read_u32(c, k, le);
            let mut levels = [0f32; 8];
            for (k, level) in levels.iter_mut().enumerate() {
                *level = f32::from_bits(w(4 + 4 * k)?);
            }
            let next = w(0x28)? as usize;
            let expected = if i + 1 == n { 0 } else { CHANNEL_SIZE };
            if next != expected {
                return Err(Error::malformed(
                    "mix",
                    at + 0x28,
                    format!("next channel at +{next}"),
                ));
            }
            channels.push(MixChannel {
                category: w(0)?,
                levels,
                frequency: w(0x24)?,
            });
            at += CHANNEL_SIZE;
        }
        if at != data.len() {
            return Err(Error::malformed("mix", at, "data after the channels"));
        }
        Ok(Self {
            stamp,
            name,
            version,
            flags,
            channels,
        })
    }

    /// Serialises the mix in the game's layout.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let le = Endian::Little;
        if self.name.len() >= NAME || self.name.contains('\0') {
            return Err(Error::InvalidArgument(format!(
                "mix name `{}` does not fit {} bytes",
                self.name,
                NAME - 1
            )));
        }
        let n = self.channels.len();
        let mut out = Vec::with_capacity(BODY + FIRST as usize + n * CHANNEL_SIZE);
        for word in [0, 0, BODY as u32, 0, 0, 0, self.stamp] {
            io::put_u32(&mut out, word, le);
        }
        out.extend_from_slice(self.name.as_bytes());
        out.resize(BODY + NAME, 0);
        for word in [
            self.version,
            self.flags,
            n as u32,
            FIRST,
            0,
            FIRST + (n * CHANNEL_SIZE) as u32,
        ] {
            io::put_u32(&mut out, word, le);
        }
        for (i, c) in self.channels.iter().enumerate() {
            io::put_u32(&mut out, c.category, le);
            for level in c.levels {
                io::put_u32(&mut out, level.to_bits(), le);
            }
            io::put_u32(&mut out, c.frequency, le);
            let next = if i + 1 == n { 0 } else { CHANNEL_SIZE as u32 };
            io::put_u32(&mut out, next, le);
        }
        Ok(out)
    }

    /// The channel of a category hash.
    pub fn channel(&self, category: u32) -> Option<&MixChannel> {
        self.channels.iter().find(|c| c.category == category)
    }

    /// Mutable access to the channel of a category hash.
    pub fn channel_mut(&mut self, category: u32) -> Option<&mut MixChannel> {
        self.channels.iter_mut().find(|c| c.category == category)
    }
}

/// The name a mix is found by: the stored name is cut to 31 characters, and
/// the game hashes what is stored.
pub fn stored_name(name: &str) -> &str {
    let mut end = name.len().min(NAME - 1);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    &name[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::string_id;

    #[test]
    fn round_trips() {
        let channel = |name: &str, volume: f32| MixChannel {
            category: string_id(name),
            levels: [-1.0, -1.0, 1.0, volume, 0.0, -1.0, 2.0, 2.0],
            frequency: 6000,
        };
        let mut mix = Mix {
            stamp: 0x7C90_EE18,
            name: "occlusion.mix".into(),
            version: VERSION,
            flags: 0,
            channels: vec![channel("CHATTER", 0.6), channel("DIALOG", 0.6)],
        };
        let bytes = mix.to_bytes().unwrap();
        assert_eq!(bytes.len(), 0x1C + 0x38 + 2 * 44);
        assert_eq!(Mix::parse(&bytes).unwrap(), mix);
        assert_eq!(mix.channel(string_id("dialog")).unwrap().volume(), 0.6);
        mix.channel_mut(string_id("chatter")).unwrap().levels[LEVEL_VOLUME] = 0.25;
        mix.channels.push(channel("DOCTOR_VOICE", 1.0));
        let back = Mix::parse(&mix.to_bytes().unwrap()).unwrap();
        assert_eq!(back, mix);
        assert!(Mix::parse(&bytes[..bytes.len() - 4]).is_err());
        let mut bad = bytes.clone();
        bad[0x1C + 0x20] = 14;
        assert!(Mix::parse(&bad).is_err());
        let empty = Mix {
            channels: Vec::new(),
            ..mix.clone()
        };
        assert_eq!(Mix::parse(&empty.to_bytes().unwrap()).unwrap(), empty);
        assert_eq!(
            stored_name("combine_chatter_followup_duck.mix"),
            "combine_chatter_followup_duck.m"
        );
        mix.name = "x".repeat(32);
        assert!(mix.to_bytes().is_err());
    }
}

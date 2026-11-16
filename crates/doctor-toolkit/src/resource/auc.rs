//! `auc` resources: the audio configuration. The tree of mix categories
//! (`DIALOG` → `CHATTER`, `SFX` → `FOOTSTEPS`), the pools of sound banks and
//! the ducks (a mix applied while sounds of a kind play).
//!
//! ```text
//! 0x00  u32  0 (the game puts a method table here), u32 0
//! 0x08  u32  12, offset of the body
//! 0x0C  u32  11, version
//! 0x10  u32  number of categories C
//! 0x14  u32  number of bank pools P
//! 0x18  u32  number of ducks D
//! 0x1C  u32  1 when plug-in chains follow the ducks (0 in the game)
//! 0x20  C × 100 bytes: name[32], second name[32] (empty in the game),
//!                      parent name[32], u32 (0); `DIALOG` and `MUSIC` name
//!                      themselves, `SFX` is a parent without a record
//!       P × 40 bytes:  name[32], u32 size, u32 kind
//!       D × 176 bytes: u32, u32, 8 bytes not initialised by the build tool,
//!                      name[32], mix name[128]
//! ```
//!
//! `godfather2.exe` 1.0.789.0 reads it in `0x005E35E0` (from the resource
//! handler `0x00456460`): a category is registered by the `sdbm` of its name
//! with its parent (`0x005DC510`), a duck by the hashes of its name and of
//! its mix (`0x004C8CE0`). Sound banks (`sbk`) are refused until an `auc`
//! has been loaded (`0x004562F0`).
//!
//! [`AudioConfig::to_bytes`] reproduces both `auc` of the game byte for byte.

use serde::{Deserialize, Serialize};

use crate::hash::{sdbm, string_id};
use crate::io::{self, Endian};
use crate::{Error, Result};

/// Type id of `auc` resources.
pub const TYPE_AUC: u32 = sdbm(b"auc");

const BODY_AT: u32 = 12;
/// Version of the resources of the game.
pub const VERSION: u32 = 11;
const HEADER: usize = 0x20;
const NAME: usize = 32;
const CATEGORY_SIZE: usize = 100;
const POOL_SIZE: usize = 40;
const DUCK_SIZE: usize = 176;
const DUCK_MIX: usize = 128;

/// A mix category: sounds name it by hash, mixes set its volume.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Category {
    pub name: String,
    /// Empty in the game.
    pub second: String,
    /// The parent category; the roots (`DIALOG`, `MUSIC`, `SFX`) name
    /// themselves.
    pub parent: String,
    pub value: u32,
}

impl Category {
    /// The hash sounds and mixes store.
    pub fn hash(&self) -> u32 {
        string_id(&self.name)
    }
}

/// A pool of sound banks (`audio_bank_00`, `Mission`, `Venue Large`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BankPool {
    pub name: String,
    pub size: u32,
    pub kind: u32,
}

/// A duck: the mix `mix` is applied while sounds that name the duck play.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Duck {
    /// Two numbers (95 and 50, 100 and 20, 90 and 20 in the game).
    pub levels: [u32; 2],
    /// Bytes the build tool did not initialise; kept as they are.
    pub filler: [u8; 8],
    pub name: String,
    /// Name of the `mix` resource (`combine_weapon_duck.mix`).
    pub mix: String,
}

/// A parsed `auc` resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioConfig {
    pub version: u32,
    pub categories: Vec<Category>,
    pub pools: Vec<BankPool>,
    pub ducks: Vec<Duck>,
    /// The word at 0x1C and whatever follows the ducks.
    pub chains: u32,
    pub tail: Vec<u8>,
}

fn name(data: &[u8], at: usize, len: usize) -> Result<String> {
    let field = io::slice(data, at, len)?;
    let end = field.iter().position(|&b| b == 0).unwrap_or(len);
    if field[end..].iter().any(|&b| b != 0) {
        return Err(Error::malformed("auc", at, "bytes after the name"));
    }
    String::from_utf8(field[..end].to_vec())
        .map_err(|_| Error::malformed("auc", at, "name is not UTF-8"))
}

fn put_name(out: &mut Vec<u8>, name: &str, len: usize) -> Result<()> {
    if name.len() >= len || name.contains('\0') {
        return Err(Error::InvalidArgument(format!(
            "name `{name}` does not fit {} bytes",
            len - 1
        )));
    }
    let start = out.len();
    out.extend_from_slice(name.as_bytes());
    out.resize(start + len, 0);
    Ok(())
}

impl AudioConfig {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let le = Endian::Little;
        let word = |at: usize| io::read_u32(data, at, le);
        if word(0)? != 0 || word(4)? != 0 || word(8)? != BODY_AT {
            return Err(Error::malformed("auc", 0, "unexpected header"));
        }
        let version = word(0x0C)?;
        if version != VERSION {
            return Err(Error::malformed("auc", 0x0C, format!("version {version}")));
        }
        let (c, p, d) = (
            word(0x10)? as usize,
            word(0x14)? as usize,
            word(0x18)? as usize,
        );
        let chains = word(0x1C)?;
        let total = c
            .saturating_mul(CATEGORY_SIZE)
            .saturating_add(p.saturating_mul(POOL_SIZE))
            .saturating_add(d.saturating_mul(DUCK_SIZE));
        io::slice(data, HEADER, total)?;
        let mut at = HEADER;
        let mut categories = Vec::with_capacity(c);
        for _ in 0..c {
            categories.push(Category {
                name: name(data, at, NAME)?,
                second: name(data, at + NAME, NAME)?,
                parent: name(data, at + 2 * NAME, NAME)?,
                value: word(at + 3 * NAME)?,
            });
            at += CATEGORY_SIZE;
        }
        let mut pools = Vec::with_capacity(p);
        for _ in 0..p {
            pools.push(BankPool {
                name: name(data, at, NAME)?,
                size: word(at + NAME)?,
                kind: word(at + NAME + 4)?,
            });
            at += POOL_SIZE;
        }
        let mut ducks = Vec::with_capacity(d);
        for _ in 0..d {
            let mut filler = [0u8; 8];
            filler.copy_from_slice(io::slice(data, at + 8, 8)?);
            ducks.push(Duck {
                levels: [word(at)?, word(at + 4)?],
                filler,
                name: name(data, at + 16, NAME)?,
                mix: name(data, at + 16 + NAME, DUCK_MIX)?,
            });
            at += DUCK_SIZE;
        }
        Ok(Self {
            version,
            categories,
            pools,
            ducks,
            chains,
            tail: data[at..].to_vec(),
        })
    }

    /// Serialises the configuration in the game's layout.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let le = Endian::Little;
        let mut out = Vec::new();
        for word in [
            0,
            0,
            BODY_AT,
            self.version,
            self.categories.len() as u32,
            self.pools.len() as u32,
            self.ducks.len() as u32,
            self.chains,
        ] {
            io::put_u32(&mut out, word, le);
        }
        for c in &self.categories {
            put_name(&mut out, &c.name, NAME)?;
            put_name(&mut out, &c.second, NAME)?;
            put_name(&mut out, &c.parent, NAME)?;
            io::put_u32(&mut out, c.value, le);
        }
        for p in &self.pools {
            put_name(&mut out, &p.name, NAME)?;
            io::put_u32(&mut out, p.size, le);
            io::put_u32(&mut out, p.kind, le);
        }
        for d in &self.ducks {
            io::put_u32(&mut out, d.levels[0], le);
            io::put_u32(&mut out, d.levels[1], le);
            out.extend_from_slice(&d.filler);
            put_name(&mut out, &d.name, NAME)?;
            put_name(&mut out, &d.mix, DUCK_MIX)?;
        }
        out.extend_from_slice(&self.tail);
        Ok(out)
    }

    /// The category with this name, case ignored (the game compares hashes
    /// of lower-case names).
    pub fn category(&self, name: &str) -> Option<&Category> {
        self.categories
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(name))
    }

    /// The name of a category or duck hash, as sounds and mixes store them.
    pub fn name_of(&self, hash: u32) -> Option<&str> {
        self.categories
            .iter()
            .map(|c| c.name.as_str())
            .chain(self.ducks.iter().map(|d| d.name.as_str()))
            .find(|n| string_id(n) == hash)
    }

    /// Adds a category under `parent`: a category of the table or a parent
    /// other categories name (`SFX` has no record of its own in the game).
    pub fn add_category(&mut self, name: &str, parent: &str) -> Result<()> {
        if self.category(name).is_some() {
            return Err(Error::InvalidArgument(format!(
                "category `{name}` already exists"
            )));
        }
        // `SFX` is a parent without a record of its own.
        let Some(parent) = self
            .categories
            .iter()
            .flat_map(|c| [&c.name, &c.parent])
            .find(|n| n.eq_ignore_ascii_case(parent))
            .cloned()
        else {
            return Err(Error::InvalidArgument(format!("no category `{parent}`")));
        };
        if name.is_empty() || name.len() >= NAME {
            return Err(Error::InvalidArgument(format!(
                "category name `{name}` must be 1..{} bytes",
                NAME - 1
            )));
        }
        self.categories.push(Category {
            name: name.to_owned(),
            second: String::new(),
            parent,
            value: 0,
        });
        Ok(())
    }

    /// Problems: repeated names (the game keys categories by hash), empty names.
    pub fn validate(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let mut seen = std::collections::HashMap::new();
        for c in &self.categories {
            if let Some(other) = seen.insert(c.hash(), &c.name) {
                problems.push(format!(
                    "categories `{other}` and `{}` share a hash",
                    c.name
                ));
            }
            if c.name.is_empty() || c.parent.is_empty() {
                problems.push(format!("`{}`: empty name or parent", c.name));
            }
        }
        problems
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> AudioConfig {
        let cat = |name: &str, parent: &str| Category {
            name: name.into(),
            second: String::new(),
            parent: parent.into(),
            value: 0,
        };
        AudioConfig {
            version: VERSION,
            categories: vec![cat("DIALOG", "DIALOG"), cat("CHATTER", "DIALOG")],
            pools: vec![BankPool {
                name: "audio_bank_00".into(),
                size: 1536,
                kind: 1,
            }],
            ducks: vec![Duck {
                levels: [95, 50],
                filler: *b"rsion='1",
                name: "WEAPON_DUCK".into(),
                mix: "combine_weapon_duck.mix".into(),
            }],
            chains: 0,
            tail: Vec::new(),
        }
    }

    #[test]
    fn round_trips_and_adds_categories() {
        let mut config = sample();
        let bytes = config.to_bytes().unwrap();
        assert_eq!(bytes.len(), 0x20 + 2 * 100 + 40 + 176);
        assert_eq!(AudioConfig::parse(&bytes).unwrap(), config);
        assert!(config.validate().is_empty());
        config.add_category("DOCTOR_VOICE", "dialog").unwrap();
        assert_eq!(config.categories[2].parent, "DIALOG");
        assert!(config.add_category("doctor_voice", "DIALOG").is_err());
        assert!(config.add_category("X", "NOPE").is_err());
        assert_eq!(config.name_of(string_id("chatter")), Some("CHATTER"));
        assert_eq!(
            config.name_of(string_id("weapon_duck")),
            Some("WEAPON_DUCK")
        );
        let back = AudioConfig::parse(&config.to_bytes().unwrap()).unwrap();
        assert_eq!(back, config);
        assert!(AudioConfig::parse(&bytes[..0x60]).is_err());
        config.categories[1].parent = String::new();
        assert_eq!(config.validate().len(), 1);
    }
}

//! `variabledictionary` resources: named global script variables of a level.
//!
//! # Layout
//!
//! All values are little-endian; offsets are absolute.
//!
//! ```text
//! 0x00  u32  version, 4
//! 0x04  u32  entry count N
//! 0x08  N × 16-byte entries, sorted by hash:
//!       u32  hash of the name (the function is not known; it is not SDBM,
//!            FNV-1a or CRC-32 of the name in any letter case)
//!       u32  kind: 0 or 1 (1 when the value is used as a flag)
//!       u32  initial value: 0 for kind 0, 0 or 1 for kind 1
//!       u32  offset of the NUL-terminated name
//!       string region: one NUL, then the names, each with a NUL, in an
//!       order unrelated to the entries (probably the order of the source);
//!       no padding at the end
//! ```

use serde::Serialize;

use crate::io::{self, Endian};
use crate::resource::simgroup::Hash32;
use crate::{Error, Result};

pub const VERSION: u32 = 4;
const ENTRY_SIZE: usize = 16;

/// One variable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Variable {
    pub name: String,
    pub hash: Hash32,
    pub kind: u32,
    pub value: u32,
    /// Offset of the name as stored; [`VariableDictionary::encode`] keeps
    /// the order of the names. 0 for new variables.
    #[serde(skip)]
    pub name_offset: u32,
}

/// A parsed variable dictionary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VariableDictionary {
    pub version: u32,
    pub variables: Vec<Variable>,
}

impl VariableDictionary {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let le = Endian::Little;
        let version = io::read_u32(data, 0, le)?;
        if version != VERSION {
            return Err(Error::UnsupportedVersion {
                what: "variabledictionary",
                version,
            });
        }
        let count = io::read_u32(data, 4, le)? as usize;
        let strings = count
            .checked_mul(ENTRY_SIZE)
            .and_then(|n| n.checked_add(8))
            .ok_or_else(|| Error::malformed("variabledictionary", 4, "count overflows"))?;
        io::slice(data, 8, strings - 8)?;
        let variables = (0..count)
            .map(|k| {
                let at = 8 + ENTRY_SIZE * k;
                let name_at = io::read_u32(data, at + 12, le)? as usize;
                if name_at <= strings {
                    return Err(Error::malformed(
                        "variabledictionary",
                        at + 12,
                        "name outside the string region",
                    ));
                }
                Ok(Variable {
                    name_offset: name_at as u32,
                    name: String::from_utf8_lossy(io::cstr(data, name_at)?).into_owned(),
                    hash: Hash32(io::read_u32(data, at, le)?),
                    kind: io::read_u32(data, at + 4, le)?,
                    value: io::read_u32(data, at + 8, le)?,
                })
            })
            .collect::<Result<_>>()?;
        Ok(Self { version, variables })
    }

    /// Writes the dictionary: entries in the given order, names in the
    /// order of [`Variable::name_offset`], new names last.
    pub fn encode(&self) -> Vec<u8> {
        let le = Endian::Little;
        let strings = 8 + ENTRY_SIZE * self.variables.len();
        let mut order: Vec<&Variable> = self.variables.iter().collect();
        order.sort_by_key(|v| (v.name_offset == 0, v.name_offset));
        let mut region = vec![0u8];
        let mut offsets = std::collections::HashMap::new();
        for v in order {
            offsets.entry(v.name.as_str()).or_insert_with(|| {
                let at = strings + region.len();
                region.extend_from_slice(v.name.as_bytes());
                region.push(0);
                at
            });
        }
        let mut out = Vec::with_capacity(strings + region.len() + 3);
        io::put_u32(&mut out, self.version, le);
        io::put_u32(&mut out, self.variables.len() as u32, le);
        for v in &self.variables {
            io::put_u32(&mut out, v.hash.0, le);
            io::put_u32(&mut out, v.kind, le);
            io::put_u32(&mut out, v.value, le);
            io::put_u32(&mut out, offsets[v.name.as_str()] as u32, le);
        }
        out.extend_from_slice(&region);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> VariableDictionary {
        let var = |name: &str, hash, kind, value| Variable {
            name: name.into(),
            hash: Hash32(hash),
            kind,
            value,
            name_offset: 0,
        };
        VariableDictionary {
            version: VERSION,
            variables: vec![
                var("timer", 0x0100_0000, 0, 0),
                var("Bridge_Var", 0x0200_0000, 1, 1),
            ],
        }
    }

    #[test]
    fn round_trips() {
        let data = sample().encode();
        assert_eq!(&data[0x29..0x2E], b"timer");
        let parsed = VariableDictionary::parse(&data).unwrap();
        assert_eq!(parsed.variables[1].name, "Bridge_Var");
        assert_eq!(parsed.variables[1].name_offset, 0x2F);
        assert_eq!(parsed.encode(), data);
        // Stored name order wins over entry order.
        let mut swapped = parsed.clone();
        swapped.variables.swap(0, 1);
        assert_eq!(&swapped.encode()[0x29..0x2E], b"timer");
    }

    #[test]
    fn rejects_bad_input() {
        let data = sample().encode();
        for len in 0..0x2E {
            assert!(VariableDictionary::parse(&data[..len]).is_err());
        }
        let mut huge = data.clone();
        huge[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(VariableDictionary::parse(&huge).is_err());
        let mut v = data.clone();
        v[0] = 5;
        assert!(VariableDictionary::parse(&v).is_err());
    }
}

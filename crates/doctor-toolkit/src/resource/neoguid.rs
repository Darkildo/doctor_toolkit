//! NeoGuids: the GUIDs of SimGroup objects, `graph` resources and
//! metamodel states and parts, and how this tool makes new ones.
//!
//! A NeoGuid is the name of the machine that created the object and two
//! 28-bit keys. The machine name has 10 characters, upper-case ASCII letters
//! and digits, and always starts with `X`, which is not stored
//! (`XCHRIST427`, `XDUMONT770`, `XMMOPTIMIZ`). As little-endian words
//! `w0..w3` of the 16 bytes:
//!
//! ```text
//! w0 = name[9] << 24 | key1 >> 4
//! w1 = (key1 & 0xF) << 28 | key2
//! bytes 8..16 = name[1..9]
//! ```
//!
//! so byte 3 is the last character of the name. The text form is
//! `<name>_<key1>_<key2>_v3` with the keys as 7 upper-case hexadecimal digits
//! (`XCHAVEZ197_C8A0137_6EDD352_v3`). Every object GUID of the game (111 518)
//! and every `graph` GUID (2 178) has this form; see
//! `docs/formats/simgroup.md` §5.
//!
//! # New GUIDs
//!
//! [`neo_guid`] builds the GUID of a machine name and two keys.
//! [`guid_for_name`] derives the keys from a name (an object or stream name,
//! a seed) with FNV-1a, so that the same name always gives the same GUID;
//! `attempt` picks the next candidate when one is taken. [`new_guid`] does
//! that loop. New GUIDs of this tool use the machine name
//! [`MachineName::TOOLKIT`], `XDOCTORTKX` (stored as `DOCTORTK` in bytes
//! 8..16 and `X` in byte 3); any other valid name may be passed.
//!
//! GUIDs made by earlier versions of the tool (`DOCTORTK` in bytes 8..16
//! with an FNV byte 3) are left as they are: they are only compared for
//! equality, never rebuilt.

use std::fmt;

use crate::hash::fnv1a;
use crate::resource::simgroup::Guid;
use crate::{Error, Result};

/// The keys of a NeoGuid have 28 bits.
pub const KEY_MASK: u32 = 0x0FFF_FFFF;

/// A NeoGuid machine name: 10 upper-case ASCII letters and digits, the first
/// one `X`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MachineName([u8; 10]);

impl MachineName {
    /// The machine name of the GUIDs this tool creates: `XDOCTORTKX`.
    pub const TOOLKIT: Self = Self(*b"XDOCTORTKX");

    /// Checks and wraps a machine name. Lower-case letters are accepted and
    /// stored in upper case, as in the game data.
    pub fn new(name: &str) -> Result<Self> {
        let bytes = name.as_bytes();
        let valid = bytes.len() == 10
            && bytes[0].eq_ignore_ascii_case(&b'X')
            && bytes.iter().all(u8::is_ascii_alphanumeric);
        if !valid {
            return Err(Error::InvalidArgument(format!(
                "`{name}` is not a machine name: 10 letters and digits starting with X expected"
            )));
        }
        let mut out = [0u8; 10];
        for (o, b) in out.iter_mut().zip(bytes) {
            *o = b.to_ascii_uppercase();
        }
        Ok(Self(out))
    }

    /// The name as text.
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0).expect("ASCII letters and digits")
    }
}

impl fmt::Display for MachineName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The GUID of machine `machine` with keys `key1` and `key2` (28 bits each;
/// higher bits are ignored).
pub fn neo_guid(machine: &MachineName, key1: u32, key2: u32) -> Guid {
    let (key1, key2) = (key1 & KEY_MASK, key2 & KEY_MASK);
    let name = &machine.0;
    let w0 = (name[9] as u32) << 24 | key1 >> 4;
    let w1 = (key1 & 0xF) << 28 | key2;
    let mut bytes = [0u8; 16];
    bytes[0..4].copy_from_slice(&w0.to_le_bytes());
    bytes[4..8].copy_from_slice(&w1.to_le_bytes());
    bytes[8..16].copy_from_slice(&name[1..9]);
    Guid(bytes)
}

/// The machine name and keys of a GUID in the canonical NeoGuid form (byte 3
/// and bytes 8..16 upper-case letters or digits); `None` otherwise.
pub fn parts(guid: &Guid) -> Option<(MachineName, u32, u32)> {
    let b = &guid.0;
    let ok = |c: u8| c.is_ascii_uppercase() || c.is_ascii_digit();
    if !ok(b[3]) || !b[8..16].iter().copied().all(ok) {
        return None;
    }
    let mut name = [b'X'; 10];
    name[1..9].copy_from_slice(&b[8..16]);
    name[9] = b[3];
    let w0 = u32::from_le_bytes(b[0..4].try_into().expect("4 bytes"));
    let w1 = u32::from_le_bytes(b[4..8].try_into().expect("4 bytes"));
    let key1 = (w0 & 0x00FF_FFFF) << 4 | w1 >> 28;
    Some((MachineName(name), key1, w1 & KEY_MASK))
}

/// The text form `<name>_<key1>_<key2>_v3` of a canonical NeoGuid.
pub fn text(guid: &Guid) -> Option<String> {
    let (name, key1, key2) = parts(guid)?;
    Some(format!("{name}_{key1:07X}_{key2:07X}_v3"))
}

/// A GUID derived from `name`: machine `machine`, keys from FNV-1a of the name
/// (`key1` of `name`, `key2` of `name/b`; with `attempt` n > 0 of `name#n`
/// and `name#n/b`). The same arguments always give the same GUID, so a
/// project that names its objects and streams gets the same GUIDs on every
/// build. Use `attempt` (or [`new_guid`]) to step past a GUID that is taken.
pub fn guid_for_name(machine: &MachineName, name: &str, attempt: u32) -> Guid {
    let text = if attempt == 0 {
        name.to_owned()
    } else {
        format!("{name}#{attempt}")
    };
    let key1 = fnv1a(text.as_bytes());
    let key2 = fnv1a(format!("{text}/b").as_bytes());
    neo_guid(machine, key1, key2)
}

/// The first GUID of [`guid_for_name`] (attempts 0, 1, ...) for which `taken`
/// is false.
pub fn new_guid(machine: &MachineName, name: &str, taken: impl Fn(&Guid) -> bool) -> Guid {
    (0u32..)
        .map(|attempt| guid_for_name(machine, name, attempt))
        .find(|guid| !taken(guid))
        .expect("2^32 attempts")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_matches_a_game_guid() {
        // XCHAVEZ197_C8A0137_6EDD352_v3 from a message name of the game.
        let machine = MachineName::new("XCHAVEZ197").unwrap();
        let guid = neo_guid(&machine, 0xC8A0137, 0x6EDD352);
        assert_eq!(guid.0[3], b'7');
        assert_eq!(&guid.0[8..], b"CHAVEZ19");
        assert_eq!(
            u32::from_le_bytes(guid.0[0..4].try_into().unwrap()),
            (b'7' as u32) << 24 | 0xC8A0137 >> 4
        );
        assert_eq!(
            text(&guid).as_deref(),
            Some("XCHAVEZ197_C8A0137_6EDD352_v3")
        );
        assert_eq!(parts(&guid), Some((machine, 0xC8A0137, 0x6EDD352)));
        assert_eq!(guid.resource_id(), None);
    }

    #[test]
    fn toolkit_guids_are_canonical_and_reproducible() {
        let guid = guid_for_name(&MachineName::TOOLKIT, "gf2/doctor/x", 0);
        assert_eq!(guid.0[3], b'X');
        assert_eq!(&guid.0[8..], b"DOCTORTK");
        let (machine, key1, key2) = parts(&guid).unwrap();
        assert_eq!(machine.as_str(), "XDOCTORTKX");
        assert_eq!(key1, fnv1a(b"gf2/doctor/x") & KEY_MASK);
        assert_eq!(key2, fnv1a(b"gf2/doctor/x/b") & KEY_MASK);
        assert_eq!(
            guid,
            guid_for_name(&MachineName::TOOLKIT, "gf2/doctor/x", 0)
        );
        assert_ne!(
            guid,
            guid_for_name(&MachineName::TOOLKIT, "gf2/doctor/x", 1)
        );
        let next = new_guid(&MachineName::TOOLKIT, "gf2/doctor/x", |g| *g == guid);
        assert_eq!(
            next,
            guid_for_name(&MachineName::TOOLKIT, "gf2/doctor/x", 1)
        );
    }

    #[test]
    fn machine_names_are_checked() {
        assert_eq!(
            MachineName::new("xdoctortkx").unwrap(),
            MachineName::TOOLKIT
        );
        for bad in ["DOCTORTKXX", "XDOCTORTK", "XDOCTOR-KX", "XDOCTORTKXX"] {
            assert!(MachineName::new(bad).is_err(), "{bad}");
        }
        // An old GUID of the tool: byte 3 from FNV, not a letter or digit.
        let mut old = [0u8; 16];
        old[3] = 0x9C;
        old[8..].copy_from_slice(b"DOCTORTK");
        assert_eq!(parts(&Guid(old)), None);
    }
}

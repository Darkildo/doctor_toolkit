//! Live link with the running game (stage 34).
//!
//! The game runs a small module of ours (`doctor_live.asi`, crate
//! `doctor-toolkit-live-asi`), loaded by the ASI loader the player already
//! has (`dinput8.dll`). The module listens on `127.0.0.1` and answers
//! requests of the editor and the command line:
//!
//! - [`proto`]: frames, requests and replies, with a version;
//! - [`addr`]: addresses of `godfather2.exe` PC 1.0.789.0 the module uses,
//!   each with the place in the code that proves it
//!   (`docs/analysis/live-link.md`);
//! - [`edition`]: the bytes the module checks before it touches anything —
//!   on another build it only answers that the edition is wrong;
//! - [`config`]: the file the installer writes next to the module (port,
//!   token, whether writes are allowed).
//!
//! The crate has no dependencies: the module builds it for
//! `i686-pc-windows-gnu`, the editor for the host.

pub mod addr;
pub mod config;
pub mod edition;
pub mod proto;
mod wire;

pub use proto::*;

/// File name of the module in the game folder (under `scripts/`).
pub const MODULE_FILE: &str = "doctor_live.asi";
/// File name of its configuration, next to it.
pub const CONFIG_FILE: &str = "doctor_live.cfg";
/// File name of its log, next to it.
pub const LOG_FILE: &str = "doctor_live.log";
/// Port when the configuration names none.
pub const DEFAULT_PORT: u16 = 47_801;

/// The engine's hash of names (messages, classes): SDBM of the name with
/// ASCII letters in lower case (`godfather2.exe` `0x004DAFD0`).
pub fn name_hash(name: &str) -> u32 {
    name.bytes().fold(0u32, |hash, b| {
        u32::from(b.to_ascii_lowercase()).wrapping_add(hash.wrapping_mul(65_599))
    })
}

/// A GUID as the game prints it: `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}`,
/// first three groups little-endian (`docs/analysis/launch-flags.md` §3.2).
pub fn braced_guid(g: &Guid) -> String {
    format!(
        "{{{:02X}{:02X}{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        g[3],
        g[2],
        g[1],
        g[0],
        g[5],
        g[4],
        g[7],
        g[6],
        g[8],
        g[9],
        g[10],
        g[11],
        g[12],
        g[13],
        g[14],
        g[15]
    )
}

/// The GUID bytes of `{…}` text (see [`braced_guid`]) or of 32 hex digits in
/// memory order (as the toolkit prints SimGroup GUIDs).
pub fn parse_guid(text: &str) -> Option<Guid> {
    let t = text.trim();
    let hex: String = t
        .chars()
        .filter(|c| !matches!(c, '{' | '}' | '-'))
        .collect();
    if hex.len() != 32 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut g = [0u8; 16];
    for (i, b) in g.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    if t.starts_with('{') {
        g[0..4].reverse();
        g[4..6].reverse();
        g[6..8].reverse();
    }
    Some(g)
}

/// 32 hex digits of a GUID in memory order.
pub fn guid_hex(g: &Guid) -> String {
    g.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_hash_is_lower_case_sdbm() {
        // `sdbm(b"tg4h")` of hash.rs, and case does not matter.
        assert_eq!(name_hash("tg4h"), 0x4802_E0A7);
        assert_eq!(name_hash("TG4H"), 0x4802_E0A7);
        // Class names registered by 0x004833A0 with these hashes.
        assert_eq!(name_hash("Entity"), 0x3852_3FC3);
        assert_eq!(name_hash("PlayerTeleportRelay"), 0x9667_6985);
    }

    #[test]
    fn guid_text_round_trips() {
        let g = parse_guid("26d7c8323dfa11aa464f574c45523130").unwrap();
        assert_eq!(braced_guid(&g), "{32C8D726-FA3D-AA11-464F-574C45523130}");
        assert_eq!(
            parse_guid("{32C8D726-FA3D-AA11-464F-574C45523130}"),
            Some(g)
        );
        assert_eq!(guid_hex(&g), "26d7c8323dfa11aa464f574c45523130");
        assert_eq!(parse_guid("xyz"), None);
        assert_eq!(parse_guid("26d7c8323dfa11aa464f574c4552313"), None);
    }
}

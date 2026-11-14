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
//!   token, whether writes are allowed);
//! - [`journal`]: events of the game's journal (stage 49) — what it prints,
//!   Lua errors, messages of the bus — and the ring the module keeps them in;
//! - [`world`]: every SimGroup object, logic included (stage 49);
//! - [`debug`]: breakpoints, pause and step, the free camera and the
//!   replacement of a texture (stage 35);
//! - [`state`]: the world and the state of the game — time of day,
//!   weather, streams one at a time, money, family, missions, variables
//!   (stage 48);
//! - [`files`]: files the game opens from the toolkit's folder instead of
//!   its own — a mod or a workspace without writing into the game (stage
//!   46);
//! - [`edit`]: moving, turning, hiding and showing objects while the game
//!   runs, and the journal of those edits for undo (stage 47);
//! - [`profile`]: the pace of the game measured from inside — the main
//!   loop, the presents, reads of files, CPU time, textures, memory and a
//!   sampler of its threads (stage 58).
//!
//! The crate has no dependencies: the module builds it for
//! `i686-pc-windows-gnu`, the editor for the host.

pub mod addr;
pub mod config;
pub mod debug;
pub mod edit;
pub mod edition;
pub mod files;
pub mod journal;
pub mod profile;
pub mod proto;
pub mod state;
mod wire;
pub mod world;

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

/// The port the player should read his input from, when it is not the one
/// he reads: `Some(port)` when the controller manager has a device
/// (`manager_port` below `0x10`) and the player listens to another device
/// (`player_port` below `0x10`, so not network play's `0x12`). That is a
/// start with `-stream`: the player is made before the manager has its port
/// and keeps port 0 (`addr::PLAYER_INPUT_PORT`). Loading a save makes him
/// after the menus, with the manager's port — then there is nothing to do.
pub fn input_port_fix(manager_port: u8, player_port: u32) -> Option<u32> {
    (manager_port < 0x10 && player_port < 0x10 && player_port != u32::from(manager_port))
        .then_some(u32::from(manager_port))
}

/// Whether 16 floats are a world matrix: orthogonal rows, last column 0, 0,
/// 0, 1, a finite translation (as `tools/bench/gamemem.py` finds them).
/// With `scaled` the rows may have any length from 0.001 to 1000: objects
/// scaled in their SimGroup (`simgroup-set --scale`, patches of a project)
/// keep the scale in their matrix (stage 45); cameras and players have
/// unit rows.
pub fn is_world_matrix(f: &[f32; 16], scaled: bool) -> bool {
    if f[3] != 0.0 || f[7] != 0.0 || f[11] != 0.0 || f[15] != 1.0 {
        return false;
    }
    let row = |i: usize| [f[i * 4], f[i * 4 + 1], f[i * 4 + 2]];
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let (r, u, w) = (row(0), row(1), row(2));
    let lengths = [dot(r, r).sqrt(), dot(u, u).sqrt(), dot(w, w).sqrt()];
    let length_ok = |l: f32| {
        if scaled {
            (1.0e-3..1.0e3).contains(&l)
        } else {
            (l - 1.0).abs() < 5.0e-4
        }
    };
    lengths.iter().all(|&l| length_ok(l))
        && [
            dot(r, u) / (lengths[0] * lengths[1]),
            dot(r, w) / (lengths[0] * lengths[2]),
            dot(u, w) / (lengths[1] * lengths[2]),
        ]
        .iter()
        .all(|d| d.abs() < 1.0e-3)
        && f[12..15].iter().all(|v| v.is_finite() && v.abs() < 1.0e5)
}

/// Whether a GUID is that of a SimGroup object (a NeoGuid): bytes 8 to 15
/// are the author's machine name, capital letters and digits
/// (`docs/formats/simgroup.md` §5; all 111 518 objects of the game). Other
/// listeners of the bus (Lua receivers, systems of the engine) have
/// something else there.
pub fn is_object_guid(g: &Guid) -> bool {
    g[8..16]
        .iter()
        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
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
    fn the_player_gets_the_port_of_the_controller_manager() {
        // -stream: keyboard on port 15, the player made with port 0.
        assert_eq!(input_port_fix(0x0F, 0), Some(0x0F));
        // Loaded from a save: already the same port.
        assert_eq!(input_port_fix(0x0F, 0x0F), None);
        // No controller yet (0x12, 0x13), or network play (0x12 at the player).
        assert_eq!(input_port_fix(0x13, 0), None);
        assert_eq!(input_port_fix(0x12, 0), None);
        assert_eq!(input_port_fix(0x0F, 0x12), None);
        // A pad on port 1 while the player was made with port 0.
        assert_eq!(input_port_fix(1, 0), Some(1));
    }

    #[test]
    fn world_matrices_with_and_without_scale() {
        let unit = [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 283.9, 0.0, 371.8, 1.0,
        ];
        assert!(is_world_matrix(&unit, false));
        assert!(is_world_matrix(&unit, true));
        // A lamp scaled twice by a project patch (stage 45), turned 90°.
        let scaled = [
            0.0, 0.0, -2.0, 0.0, 0.0, 2.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 299.97, 3.0, 368.42, 1.0,
        ];
        assert!(!is_world_matrix(&scaled, false));
        assert!(is_world_matrix(&scaled, true));
        // Not orthogonal, or not a matrix at all.
        let mut skew = unit;
        skew[4] = 0.5;
        assert!(!is_world_matrix(&skew, true));
        let mut last = unit;
        last[15] = 0.0;
        assert!(!is_world_matrix(&last, true));
        let mut flat = scaled;
        flat[5] = 0.0;
        assert!(!is_world_matrix(&flat, true));
    }

    #[test]
    fn object_guids_carry_a_machine_name() {
        // `XBOYD1734` of the bus on the stand, and the toolkit's own.
        let g = parse_guid("a9dbc739217b64fe424f594431373334").unwrap();
        assert!(is_object_guid(&g));
        let ours = parse_guid("db057c5800000000444f43544f52544b").unwrap();
        assert!(is_object_guid(&ours));
        assert!(!is_object_guid(
            &parse_guid("0000000000000000e803000001000000").unwrap()
        ));
        assert!(!is_object_guid(
            &parse_guid("000c1a1a8c81d500ac10e30008221e19").unwrap()
        ));
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

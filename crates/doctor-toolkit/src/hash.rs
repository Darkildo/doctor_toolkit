//! Name hashing functions used by the engine.

/// SDBM hash (`hash = byte + hash * 65599`).
///
/// The engine uses it for resource type tags and memory policy names:
/// `sdbm(b"tg4h") == 0x4802_E0A7`.
pub const fn sdbm(bytes: &[u8]) -> u32 {
    let mut hash = 0u32;
    let mut i = 0;
    while i < bytes.len() {
        hash = (bytes[i] as u32).wrapping_add(hash.wrapping_mul(65599));
        i += 1;
    }
    hash
}

/// Hash of a string table (`lh2`) entry name: SDBM of the name with ASCII
/// letters in lower case (`godfather2.exe` `0x004DB060`).
///
/// Interface files refer to strings as `$name` (`$fe_press_start` →
/// `0xCBD0A3C6`, "Press any key"); SimGroup attributes of type hash store the
/// hash itself.
pub fn string_id(name: &str) -> u32 {
    sdbm(name.to_ascii_lowercase().as_bytes())
}

/// 32-bit FNV-1a hash.
pub const fn fnv1a(bytes: &[u8]) -> u32 {
    let mut hash = 0x811C_9DC5u32;
    let mut i = 0;
    while i < bytes.len() {
        hash = (hash ^ bytes[i] as u32).wrapping_mul(0x0100_0193);
        i += 1;
    }
    hash
}

/// CRC-32 (IEEE 802.3, the zlib variant).
///
/// Materials and shaders are identified by the CRC-32 of their lower-case name.
pub fn crc32(bytes: &[u8]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut table = [0u32; 256];
        let mut i = 0;
        while i < 256 {
            let mut c = i as u32;
            let mut k = 0;
            while k < 8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
                k += 1;
            }
            table[i] = c;
            i += 1;
        }
        table
    };
    !bytes.iter().fold(!0u32, |c, &b| {
        TABLE[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdbm_matches_type_ids_found_in_game_files() {
        assert_eq!(sdbm(b""), 0);
        assert_eq!(sdbm(b"tg4h"), 0x4802_E0A7);
        assert_eq!(sdbm(b"tg4d"), 0x4802_E0A3);
        assert_eq!(sdbm(b"apx"), 0x3033_FBE9);
        assert_eq!(sdbm(b"simgroup"), 0xCCB5_1828);
        assert_eq!(sdbm(b"default"), 0x5393_AC01);
    }

    #[test]
    fn string_ids_match_the_english_table() {
        // `$fe_press_start` in gf2/frontend #20 is "Press any key".
        assert_eq!(string_id("fe_press_start"), 0xCBD0_A3C6);
        assert_eq!(string_id("FE_Press_Start"), 0xCBD0_A3C6);
        assert_eq!(string_id("ui_pro_select"), 0x486F_5259);
        assert_eq!(string_id("ui_pro_create"), 0x1113_2259);
    }

    #[test]
    fn crc32_reference_vectors() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn fnv1a_reference_vectors() {
        assert_eq!(fnv1a(b""), 0x811C_9DC5);
        assert_eq!(fnv1a(b"a"), 0xE40C_292C);
        assert_eq!(fnv1a(b"foobar"), 0xBF9C_F968);
    }
}

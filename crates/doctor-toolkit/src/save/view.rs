//! A save as text: the summary and the content of a chunk, with names from
//! the game's files where they are known. Shared by the command line, the
//! MCP server and the editor.

use std::fmt::Write as _;

use super::{Building, CORLEONE, Chunk, Content, MONEY_ELSEWHERE, SaveFile, chunk_info};
use crate::hash::sdbm;
use crate::live::names::LiveNames;
use crate::resource::neoguid;
use crate::resource::simgroup::{Guid, Hash32};

/// Names of the 28 families (attribute 1 of the `Family` objects of
/// `gf2/globals_global`) and of the cities: the hashes in saves are `sdbm`
/// of these names.
const HASHED_NAMES: &[&str] = &[
    "corleone",
    "rosato_carmine",
    "rosato_tony",
    "granados",
    "mangano",
    "almeida",
    "almeida_m4",
    "batista",
    "cuban_fake_police",
    "cuban_law",
    "cuban_rebel",
    "cuban_rioter",
    "innocent",
    "miami_law",
    "neutral",
    "queens_law",
    "recruit",
    "roth",
    "security",
    "security_roth",
    "striker",
    "worker",
    "worker_food",
    "worker_hospitality",
    "worker_industrial",
    "worker_racket",
    "worker_sex",
    "worker_whitecollar",
    "cuba",
    "florida",
    "newyork",
];

/// The name of a family or a city by its hash.
pub fn hashed_name(hash: u32) -> Option<&'static str> {
    HASHED_NAMES
        .iter()
        .copied()
        .find(|name| sdbm(name.as_bytes()) == hash)
}

fn family(hash: Hash32) -> String {
    match hashed_name(hash.0) {
        Some(name) => name.to_owned(),
        None if hash.0 == 0 => "-".to_owned(),
        None => hash.to_string(),
    }
}

/// A GUID with the name of its object, when the game's files have it.
fn object(guid: &Guid, names: Option<&LiveNames>) -> String {
    let id = neoguid::text(guid).unwrap_or_else(|| guid.to_string());
    match names.and_then(|n| n.object(&guid.0)) {
        Some(o) if o.name.is_empty() => format!("{id} ({})", o.class),
        Some(o) => format!("{id} ({} `{}`)", o.class, o.name),
        None => id,
    }
}

fn variable(hash: Hash32, names: Option<&LiveNames>) -> String {
    match names.and_then(|n| n.variable(hash.0)) {
        Some(v) => v.name.clone(),
        None => hash.to_string(),
    }
}

/// The header of the save and the list of its chunks.
pub fn summary(save: &SaveFile) -> String {
    let mut out = String::new();
    let check = |ok: bool| if ok { "ok" } else { "MISMATCH" };
    let _ = writeln!(out, "name:        {}", save.description);
    let _ = writeln!(out, "profile:     {}", save.profile);
    let _ = writeln!(out, "city:        {}", save.city);
    let _ = writeln!(
        out,
        "play time:   {} ({} s)",
        save.play_time, save.play_seconds
    );
    let _ = writeln!(out, "version:     {}", save.version);
    let _ = writeln!(
        out,
        "data:        {:#x} of {:#x} bytes used, mask {:#x}",
        save.used, save.capacity, save.mask
    );
    let _ = writeln!(
        out,
        "checksums:   header {}, block 1 {}, block 2 {}",
        check(save.checksums.header.ok()),
        check(save.checksums.info.ok()),
        check(save.checksums.data.ok())
    );
    if let Ok(Some(money)) = save.money() {
        let _ = writeln!(out, "money:       {money:.0}");
    }
    let _ = writeln!(out, "chunks:      {}", save.chunks.len());
    for chunk in &save.chunks {
        let about = chunk_info(chunk.id).map_or("unknown id", |c| c.about);
        let _ = writeln!(
            out,
            "  {:08x} {:<14} {:>6}  {about}",
            chunk.id,
            chunk.name(),
            chunk.data.len()
        );
    }
    out
}

/// The content of a chunk; a hex dump when its layout is not known.
pub fn chunk_text(chunk: &Chunk, names: Option<&LiveNames>) -> String {
    let mut out = String::new();
    let content = match chunk.content() {
        Ok(content) => content,
        Err(e) => {
            let _ = writeln!(out, "the chunk does not decode: {e}");
            Content::Raw
        }
    };
    match content {
        Content::Variables(list) => {
            let _ = writeln!(out, "{} variable(s)", list.len());
            for v in list {
                let _ = writeln!(out, "  {} = {}", variable(v.hash, names), v.value);
            }
        }
        Content::PlayerFamily(f) => {
            let _ = writeln!(
                out,
                "version {}, money {:.0}, flag {}",
                f.version, f.money, f.flag
            );
            let _ = writeln!(out, "{} member(s)", f.members.len());
            for m in &f.members {
                let _ = writeln!(out, "  {}  value {}", object(&m.guid, names), m.value);
            }
        }
        Content::Families(f) => {
            let _ = writeln!(
                out,
                "version {}, crew value {}, {} family(ies)",
                f.version,
                f.crew_value,
                f.families.len()
            );
            for fam in &f.families {
                let money = if fam.name.0 == CORLEONE && fam.money == MONEY_ELSEWHERE {
                    "in player-family".to_owned()
                } else {
                    format!("{:.0}", fam.money)
                };
                let _ = writeln!(
                    out,
                    "  {}: money {money}, flags {:#x}, timers {:?}",
                    family(fam.name),
                    fam.flags,
                    &fam.timers[..4]
                );
                if !fam.buildings.is_empty() || !fam.buildings_2.is_empty() {
                    let _ = writeln!(
                        out,
                        "    buildings {:?}, second list {:?}",
                        fam.buildings, fam.buildings_2
                    );
                }
                let relations: Vec<String> = fam
                    .relations
                    .iter()
                    .map(|(h, v)| format!("{} {v}", family(*h)))
                    .collect();
                let _ = writeln!(out, "    relations: {}", relations.join(", "));
                let _ = writeln!(
                    out,
                    "    {} member(s), {} record(s)",
                    fam.members.len(),
                    fam.records
                );
                for guid in &fam.members {
                    let _ = writeln!(out, "      {}", object(guid, names));
                }
                if let Some(books) = &fam.books {
                    for (i, list) in books.iter().enumerate() {
                        let _ = writeln!(out, "    books {i}: {list:?}");
                    }
                }
            }
        }
        Content::Buildings(b) => {
            let _ = writeln!(
                out,
                "version {}, {} building(s)",
                b.version,
                b.buildings.len()
            );
            let _ = writeln!(
                out,
                "  id   owner            first owner      state flags      guards"
            );
            for building in &b.buildings {
                let _ = writeln!(out, "{}", building_line(building));
            }
        }
        Content::Cities(list) => {
            for city in list {
                let _ = writeln!(out, "  {}: state {}", family(city.name), city.state);
            }
        }
        Content::History(list) => {
            let _ = writeln!(out, "{} entry(ies)", list.len());
            for line in list {
                let _ = writeln!(out, "  {line}");
            }
        }
        Content::Inventory(inv) => {
            let _ = writeln!(out, "flags {:#010x} {:#010x}", inv.flags[0], inv.flags[1]);
            for slot in &inv.slots {
                let _ = write!(
                    out,
                    "  slot {}: {} x {}",
                    slot.slot,
                    slot.count,
                    object(&slot.weapon, names)
                );
                if let (Some(level), Some(ammo)) = (slot.level, slot.ammo) {
                    let _ = write!(out, ", level {level}, ammunition {ammo}");
                }
                let _ = writeln!(out);
            }
        }
        Content::Player(p) => {
            let _ = writeln!(out, "values {} and {}", p.values[0], p.values[1]);
            let items: Vec<String> = p.items.iter().map(Hash32::to_string).collect();
            let _ = writeln!(out, "{} item(s): {}", items.len(), items.join(" "));
        }
        Content::Checkpoint(c) => {
            let _ = writeln!(out, "checkpoint {}", c.checkpoint);
            if c.has_transform {
                let t = &c.transform;
                let _ = writeln!(out, "player at ({:.3}; {:.3}; {:.3})", t[12], t[13], t[14]);
            } else {
                let _ = writeln!(out, "no transform of the player");
            }
        }
        Content::Safehouses(states) => {
            let _ = writeln!(out, "states: {states:?}");
        }
        Content::Raw => out.push_str(&hex_dump(&chunk.data)),
    }
    out
}

fn building_line(b: &Building) -> String {
    format!(
        "  {:<4} {:<16} {:<16} {:<5} {:#010x} {}+{}",
        b.id,
        family(b.owner),
        family(b.first_owner),
        b.state,
        b.flags,
        b.guards.len(),
        b.guards_2.len()
    )
}

/// A hex dump of 16 bytes a line; runs of zero lines are folded.
pub fn hex_dump(data: &[u8]) -> String {
    let mut out = String::new();
    let mut zeros = 0usize;
    for (i, line) in data.chunks(16).enumerate() {
        if line.iter().all(|&b| b == 0) && data.len() > 64 {
            zeros += 1;
            continue;
        }
        if zeros > 0 {
            let _ = writeln!(out, "  ... {zeros} zero line(s)");
            zeros = 0;
        }
        let hex: Vec<String> = line.iter().map(|b| format!("{b:02x}")).collect();
        let text: String = line
            .iter()
            .map(|&b| {
                if (0x20..0x7F).contains(&b) {
                    b as char
                } else {
                    '.'
                }
            })
            .collect();
        let _ = writeln!(out, "  {:05x}  {:<47}  {text}", i * 16, hex.join(" "));
    }
    if zeros > 0 {
        let _ = writeln!(out, "  ... {zeros} zero line(s)");
    }
    out
}

/// The whole save: the summary and every chunk.
pub fn full_text(save: &SaveFile, names: Option<&LiveNames>) -> String {
    let mut out = summary(save);
    for chunk in &save.chunks {
        let _ = writeln!(out, "\n[{}]", chunk.name());
        out.push_str(&chunk_text(chunk, names));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::save::build;

    #[test]
    fn names_of_families_and_cities() {
        assert_eq!(hashed_name(0x0637_B907), Some("corleone"));
        assert_eq!(hashed_name(0xAF19_3075), Some("mangano"));
        assert_eq!(hashed_name(0x8DA8_826F), Some("newyork"));
        assert_eq!(hashed_name(1), None);
    }

    #[test]
    fn text_of_a_save() {
        let vars: Vec<u8> = [1u32, 0xAABB_CCDD, 7]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let data = build(
            "P",
            "Cuba",
            61,
            0xB,
            &[(0xE60B_027D, vars), (0x99, vec![0xFF; 3])],
        )
        .unwrap();
        let save = SaveFile::parse(&data).unwrap();
        let text = full_text(&save, None);
        assert!(text.contains("name:        P - Cuba - 0:01:01"), "{text}");
        assert!(
            text.contains("checksums:   header ok, block 1 ok, block 2 ok"),
            "{text}"
        );
        assert!(text.contains("e60b027d variables"), "{text}");
        assert!(text.contains("  aabbccdd = 7"), "{text}");
        assert!(text.contains("[c00000099]\n  00000  ff ff ff"), "{text}");
    }

    #[test]
    fn zero_lines_fold() {
        let mut data = vec![0u8; 96];
        data[80] = 1;
        let dump = hex_dump(&data);
        assert!(
            dump.starts_with("  ... 5 zero line(s)\n  00050  01"),
            "{dump}"
        );
    }
}

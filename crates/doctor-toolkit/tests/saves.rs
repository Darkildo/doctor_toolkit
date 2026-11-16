//! Tests against real save files of The Godfather II (PC 1.0.789.0).
//!
//! Saves are the player's files and are not in the repository. Set
//! `EARS_GF2_SAVES` to a folder with copies of saves (`GF2-EN-autosave`,
//! `GF2-EN-<digits>` under any name; folders are searched) to run the test;
//! without it the test passes without doing anything.
//!
//! ```sh
//! EARS_GF2_SAVES=/path/to/copies cargo test -p doctor-toolkit --test saves
//! ```

use std::path::{Path, PathBuf};

use doctor_toolkit::save::{self, CHUNKS, Content, SaveFile, view};

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect(&path, out);
        } else if std::fs::metadata(&path).unwrap().len() == save::FILE_SIZE as u64 {
            out.push(path);
        }
    }
}

#[test]
fn every_save_parses_and_every_known_chunk_decodes() {
    let Some(dir) = std::env::var_os("EARS_GF2_SAVES") else {
        return;
    };
    let mut files = Vec::new();
    collect(Path::new(&dir), &mut files);
    assert!(
        !files.is_empty(),
        "EARS_GF2_SAVES has no file of {} bytes",
        save::FILE_SIZE
    );
    for path in &files {
        let data = std::fs::read(path).unwrap();
        let name = path.display();
        let file = SaveFile::parse(&data).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(file.checksums.ok(), "{name}: checksums");
        assert_eq!(
            (file.version, file.capacity, file.mask),
            (6, 0x40000, 0xB),
            "{name}"
        );
        // The play time of block 1 is the one of the header.
        let s = file.play_seconds;
        assert_eq!(
            file.play_time,
            format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60),
            "{name}"
        );
        // An autosave has `Auto:` before the name.
        assert!(
            file.description.ends_with(&format!(
                "{} - {} - {}",
                file.profile, file.city, file.play_time
            )),
            "{name}: {}",
            file.description
        );
        // The chunks fill the used size exactly, and the rest is zero.
        let last = file.chunks.last().unwrap();
        let end = last.offset + 12 + last.data.len().next_multiple_of(4);
        assert_eq!(
            end,
            data.len() - 0x40000 + file.used as usize,
            "{name}: used size"
        );
        assert!(
            data[end..].iter().all(|&b| b == 0),
            "{name}: bytes after the chunks"
        );
        // Every save has the same 25 chunks, each known.
        assert_eq!(file.chunks.len(), CHUNKS.len(), "{name}");
        for chunk in &file.chunks {
            assert!(
                save::chunk_info(chunk.id).is_some(),
                "{name}: chunk {:08x}",
                chunk.id
            );
            let content = chunk
                .content()
                .unwrap_or_else(|e| panic!("{name}: chunk {}: {e}", chunk.name()));
            match content {
                Content::Families(f) => {
                    assert_eq!(f.version, 9, "{name}");
                    assert_eq!(f.families.len(), 6, "{name}");
                    assert_eq!(f.families[0].name.0, save::CORLEONE, "{name}");
                    assert_eq!(f.families[0].money, save::MONEY_ELSEWHERE, "{name}");
                    assert!(f.families[0].books.is_some(), "{name}");
                    for family in &f.families {
                        assert!(
                            view::hashed_name(family.name.0).is_some(),
                            "{name}: {}",
                            family.name
                        );
                    }
                }
                Content::Buildings(b) => {
                    assert_eq!(b.version, 7, "{name}");
                    for building in &b.buildings {
                        assert!(
                            building.owner.0 == 0 || view::hashed_name(building.owner.0).is_some(),
                            "{name}: owner {} of building {}",
                            building.owner,
                            building.id
                        );
                    }
                }
                Content::Cities(c) => {
                    // Three cities; a game started with `-stream` saves
                    // only the loaded one.
                    assert!(!c.is_empty(), "{name}");
                    for city in &c {
                        let known = view::hashed_name(city.name.0);
                        assert!(
                            matches!(known, Some("cuba" | "florida" | "newyork")),
                            "{name}: city {}",
                            city.name
                        );
                    }
                }
                Content::PlayerFamily(f) => assert_eq!(f.version, 4, "{name}"),
                Content::Safehouses(s) => assert_eq!(s.len(), 25, "{name}"),
                _ => {}
            }
        }
        // A save built from the chunks has the same chunks at the same
        // places. The bytes differ in the strings of the header (the
        // localised title, `Auto:` of an autosave) and in the padding after
        // a chunk of odd size, which the game does not clear.
        let chunks: Vec<_> = file.chunks.iter().map(|c| (c.id, c.data.clone())).collect();
        let built = save::build(
            &file.profile,
            &file.city,
            file.play_seconds,
            file.mask,
            &chunks,
        )
        .unwrap();
        let again = SaveFile::parse(&built).unwrap();
        assert!(again.checksums.ok(), "{name}: rebuilt checksums");
        assert!(again.chunks == file.chunks, "{name}: rebuilt chunks");
        assert_eq!(again.used, file.used, "{name}: rebuilt used size");
        assert!(!view::full_text(&file, None).is_empty());
    }
    println!("{} save(s)", files.len());
}

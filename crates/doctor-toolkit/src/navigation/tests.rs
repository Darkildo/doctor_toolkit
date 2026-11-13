//! Tests of the index, search and unpack/pack on a miniature game directory.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use crate::container::ustream::{FileHeader, ResourceHeader, Storage, StreamWriter};
use crate::game::GameInstance;
use crate::hash::sdbm;
use crate::io::Endian;
use crate::resource::lh2::StringTable;
use crate::resource::simgroup::Hash32;
use crate::resource::variabledictionary::{Variable, VariableDictionary};
use crate::workspace::Workspace;

use super::unpack::{self, FileKind, Manifest, PackOptions, UnpackOptions};
use super::{Dictionary, GameIndex, NameKind, Origin, Query, search};

fn stream_bytes(resources: &[(&[u8], u32, Vec<u8>)]) -> Vec<u8> {
    let mut writer = StreamWriter::new(&FileHeader::GF2_PC, Endian::Little);
    for (tag, id, data) in resources {
        let header = ResourceHeader {
            mem_policy: sdbm(b"default"),
            alignment: 0x10,
            type_id: sdbm(tag),
            ids: [*id; 4],
            data_size: 0,
        };
        writer.push_resource(&header, data, Storage::Raw).unwrap();
    }
    writer.finish()
}

fn strings() -> StringTable {
    StringTable {
        version: 1,
        flags: 0x4000,
        languages: vec![],
        hashes: vec![1, 2],
        strings: vec![vec!["Hello Don".into(), "Goodbye".into()]],
    }
}

fn variables() -> Vec<u8> {
    VariableDictionary {
        version: 4,
        variables: vec![Variable {
            name: "poi_active".into(),
            hash: Hash32(0xAAAA),
            kind: 1,
            value: 0,
            name_offset: 0,
        }],
    }
    .encode()
}

/// `gf2/a.str`: a string table, a variable dictionary and an effect;
/// `gf2/b.str`: another effect.
fn sample_game() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("gf2")).unwrap();
    let a = stream_bytes(&[
        (b"lh2", 0x10, strings().to_bytes().unwrap()),
        (b"variabledictionary", 0x20, variables()),
        (b"vfx", 0x30, b"fire_big\0\x01\x02ember1\0".to_vec()),
    ]);
    std::fs::write(dir.path().join("gf2/a.str"), a).unwrap();
    write_b(dir.path(), 0x31);
    dir
}

fn write_b(root: &Path, id: u32) {
    let b = stream_bytes(&[(b"vfx", id, b"smoke_puff\0".to_vec())]);
    // A new file (new inode), as `workspace apply` writes it.
    let path = root.join("gf2/b.str");
    let _ = std::fs::remove_file(&path);
    std::fs::write(path, b).unwrap();
}

fn find(index: &GameIndex, line: &str) -> Vec<String> {
    let query = Query::parse(line).unwrap();
    let (results, total) = search::search(index, &query, 0);
    assert_eq!(results.len(), total);
    results
        .iter()
        .map(|r| index.key(r.record).to_string())
        .collect()
}

#[test]
fn index_names_texts_and_search() {
    let dir = sample_game();
    let game = GameInstance::open(dir.path()).unwrap();
    let index = GameIndex::build(&game, &|_, _| {});
    assert_eq!(index.len(), 4);
    assert_eq!(index.record_errors(), 0);
    let fire = index
        .find_key(&"gf2/a:vfx:00000030".parse().unwrap())
        .unwrap();
    assert_eq!(index.record(fire).name(), Some("fire_big"));
    assert!(
        index
            .record(fire)
            .names
            .contains(&(NameKind::Text, "ember1".into()))
    );

    assert_eq!(find(&index, "ember"), ["gf2/a:vfx:00000030"]);
    assert_eq!(find(&index, "SMOKE"), ["gf2/b:vfx:00000031"]);
    assert_eq!(find(&index, "*_puff"), ["gf2/b:vfx:00000031"]);
    assert_eq!(
        find(&index, "type:vfx stream:gf2/a"),
        ["gf2/a:vfx:00000030"]
    );
    assert_eq!(find(&index, "text:goodbye"), ["gf2/a:lh2:00000010"]);
    assert_eq!(
        find(&index, "0x0000aaaa"),
        ["gf2/a:variabledictionary:00000020"]
    );
    assert_eq!(find(&index, "0x00000002"), ["gf2/a:lh2:00000010"]);
    assert_eq!(find(&index, "gf2/b:vfx:00000031"), ["gf2/b:vfx:00000031"]);
    assert!(find(&index, "nothing like this").is_empty());

    let dict = Dictionary::for_game(&index);
    assert_eq!(
        dict.lookup(0xAAAA),
        [("poi_active", Origin::Stored(sdbm(b"variabledictionary")))]
    );
    assert!(dict.knows(sdbm(b"smoke_puff")));
    let unknown = super::dictionary::unknown_hashes(&index, &dict);
    assert!(
        unknown
            .iter()
            .any(|u| u.kind == "string entry" && u.hash == 2)
    );
}

#[test]
fn dictionary_names_entries_by_texts_and_build_paths() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("gf2/cuba")).unwrap();
    let entry = sdbm(b"menu_title");
    // A table whose texts are entry names, as `gf2/text/d70e6b6c`.
    let names = StringTable {
        version: 1,
        flags: 0x4000,
        languages: vec![],
        hashes: vec![entry, 7],
        strings: vec![vec!["MENU_Title".into(), "not a name".into()]],
    };
    let path_id = sdbm(b"levels\\gf2\\cuba\\streams\\cuba\\local.smb");
    let stream = stream_bytes(&[
        (b"lh2", 0x10, names.to_bytes().unwrap()),
        (b"smb", path_id, vec![0; 4]),
        (b"sbk", 0x1234, vec![0; 4]),
        (b"newtype", 0x40, vec![0; 4]),
    ]);
    std::fs::write(dir.path().join("gf2/cuba/local.str"), stream).unwrap();
    let game = GameInstance::open(dir.path()).unwrap();
    let index = GameIndex::build(&game, &|_, _| {});
    let dict = Dictionary::for_game(&index);
    assert_eq!(
        dict.lookup(entry),
        [("MENU_Title", Origin::Computed(super::HashFn::SdbmLower))]
    );
    assert!(dict.lookup(sdbm(b"not a name")).is_empty());
    assert_eq!(
        dict.lookup(path_id),
        [(
            "levels\\gf2\\cuba\\streams\\cuba\\local.smb",
            Origin::Computed(super::HashFn::Sdbm)
        )]
    );
    let unknown = super::dictionary::unknown_hashes(&index, &dict);
    let kinds: Vec<(&str, u32)> = unknown.iter().map(|u| (u.kind.as_str(), u.hash)).collect();
    assert!(kinds.contains(&("sbk id", 0x1234)), "{kinds:?}");
    assert!(kinds.contains(&("type tag", sdbm(b"newtype"))), "{kinds:?}");
    assert!(kinds.contains(&("string entry", 7)), "{kinds:?}");
    assert!(!kinds.iter().any(|k| k.1 == entry || k.1 == path_id));
}

#[test]
fn cache_is_reused_and_refreshed_per_stream() {
    let dir = sample_game();
    let cache_dir = tempfile::tempdir().unwrap();
    let cache = cache_dir.path().join("index.bin");
    let game = GameInstance::open(dir.path()).unwrap();

    let (_, report) = GameIndex::open(&game, Some(&cache), &|_, _| {});
    assert_eq!((report.reused, report.rebuilt), (0, 2));
    assert_eq!(report.saved.as_deref(), Some(cache.as_path()));

    let (mut index, report) = GameIndex::open(&game, Some(&cache), &|_, _| {});
    assert_eq!((report.reused, report.rebuilt), (2, 0));
    assert!(report.saved.is_none());

    write_b(dir.path(), 0x32);
    let report = index.refresh(&game, Some(&cache), &|_, _| {});
    assert_eq!((report.reused, report.rebuilt), (1, 1));
    assert_eq!(find(&index, "smoke"), ["gf2/b:vfx:00000032"]);
    let (reopened, report) = GameIndex::open(&game, Some(&cache), &|_, _| {});
    assert_eq!((report.reused, report.rebuilt), (2, 0));
    assert_eq!(find(&reopened, "smoke"), ["gf2/b:vfx:00000032"]);

    // A cache of another game directory is ignored, not trusted.
    let other = sample_game();
    let other_game = GameInstance::open(other.path()).unwrap();
    let (_, report) = GameIndex::open(&other_game, Some(&cache), &|_, _| {});
    assert!(report.cache_error.is_some());
    assert_eq!(report.rebuilt, 2);
}

#[test]
fn unpack_and_pack_raw_and_strings() {
    let dir = sample_game();
    let game = GameInstance::open(dir.path()).unwrap();
    let index = GameIndex::build(&game, &|_, _| {});
    let out = tempfile::tempdir().unwrap();
    let selection = unpack::select(&game, &[], &[]).unwrap();
    let report = unpack::unpack(
        &game,
        Some(&index),
        &selection,
        out.path(),
        UnpackOptions::default(),
        &|_, _| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(report.resources, 4);
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    let manifest = Manifest::read(out.path()).unwrap();
    let paths: Vec<&str> = manifest.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "gf2/a/lh2/00000010.json",
            "gf2/a/variabledictionary/00000020.bin",
            "gf2/a/variabledictionary/00000020.json",
            "gf2/a/vfx/fire_big.bin",
            "gf2/b/vfx/smoke_puff.bin",
        ]
    );
    let options = PackOptions::default();
    assert!(
        unpack::pack(&game, out.path(), &options)
            .unwrap()
            .changed
            .is_empty()
    );

    std::fs::write(out.path().join("gf2/a/vfx/fire_big.bin"), b"fire_small\0").unwrap();
    let json = out.path().join("gf2/a/lh2/00000010.json");
    let text = std::fs::read_to_string(&json).unwrap();
    std::fs::write(&json, text.replace("Goodbye", "Farewell")).unwrap();
    let dry = PackOptions {
        dry_run: true,
        ..PackOptions::default()
    };
    assert_eq!(
        unpack::pack(&game, out.path(), &dry).unwrap().changed.len(),
        2
    );
    assert!(!Workspace::exists(&Workspace::default_dir(game.root())));

    let report = unpack::pack(&game, out.path(), &options).unwrap();
    assert!(report.changed.iter().all(|f| f.recorded));
    let ws = Workspace::open(report.workspace.unwrap()).unwrap();
    let keys: Vec<String> = ws.changes().map(|c| c.key.to_string()).collect();
    assert_eq!(keys, ["gf2/a:lh2:00000010", "gf2/a:vfx:00000030"]);
    let table = StringTable::parse(&ws.data(&keys[0].parse().unwrap()).unwrap().unwrap()).unwrap();
    assert_eq!(table.strings[0], ["Hello Don", "Farewell"]);

    // Editing the JSON of the variable dictionary is not supported.
    let view = out.path().join("gf2/a/variabledictionary/00000020.json");
    std::fs::write(&view, "{}").unwrap();
    let error = unpack::pack(&game, out.path(), &options).unwrap_err();
    assert!(error.to_string().contains("00000020.json"), "{error}");
    assert_eq!(manifest.files[2].kind, FileKind::View);
}

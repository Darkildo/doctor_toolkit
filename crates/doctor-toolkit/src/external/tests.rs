//! External editing on a synthetic game: export and import are symmetric,
//! saves are imported once, rapid saves make one operation, and the file
//! watcher sees in-place writes, renames over the file and writes in pieces.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::*;
use crate::container::ustream::{FileHeader, Storage, StreamLayout, StreamWriter};
use crate::hash::sdbm;
use crate::io::Endian;
use crate::resource::pixels;
use crate::resource::tg4::{PARAM_DATA_SIZE, PARAM_FORMAT, ParamValue};
use crate::toc::Toc;
use crate::workspace::tests::{header, sample_table, toc_bytes};

const LEVEL: &str = "gf2/level";
const TEX_HEADER: usize = 0;
const STRINGS: usize = 2;
const SIMGROUP: usize = 3;
const SCRIPT: usize = 4;

/// `gf2/level.str` with a texture, a string table, a SimGroup and a script.
fn game() -> (tempfile::TempDir, GameInstance) {
    let mut tex = Tg4Header {
        version: 3,
        budget_hash: 0,
        package: "pkg".into(),
        name_hash: 0x55,
        name: "wall".into(),
        data_size: 8,
        width: 4,
        height: 4,
        padding: 0,
        surface_count: 1,
        params: Vec::new(),
    };
    tex.set_param(PARAM_FORMAT, ParamValue::Str("DXT1c".into()));
    tex.set_param(PARAM_DATA_SIZE, ParamValue::Int(8));
    let mut table = sample_table(["Hello", "World"]);
    table.strings[0][1] = "with, \"quotes\"\nand lines".into();
    let script = lua::compile_for_game(b"x = 1\nprint(\"hi\")\n", b"@x:\\scripts\\test.lua")
        .expect("compiles");

    let mut level = StreamWriter::new(&FileHeader::GF2_PC, Endian::Little);
    let resources: [(&[u8], [u32; 4], Vec<u8>); 5] = [
        (
            b"tg4h",
            [0, sdbm(b"tg4h"), 7, 0x55],
            tex.to_bytes().unwrap(),
        ),
        (b"tg4d", [0, sdbm(b"tg4d"), 7, 0x55], vec![0u8; 8]),
        (b"lh2", [9; 4], table.to_bytes().unwrap()),
        (
            b"simgroup",
            [4; 4],
            crate::resource::simgroup::tests::sample(),
        ),
        (b"lua", [5; 4], script),
    ];
    for (tag, ids, data) in resources {
        level
            .push_resource(&header(tag, ids), &data, Storage::Raw)
            .unwrap();
    }
    let level = level.finish();
    let size = StreamLayout::parse(&level).unwrap().total_data_size();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("gf2")).unwrap();
    fs::write(root.join("godfather2.exe"), b"MZ fake").unwrap();
    fs::write(root.join("gf2/level.str"), &level).unwrap();
    let toc = Toc::parse(&toc_bytes(&[(LEVEL, size)])).unwrap();
    fs::write(root.join("gf2/level.toc"), toc.to_bytes().unwrap()).unwrap();
    let game = GameInstance::open(root).unwrap();
    (dir, game)
}

fn target(game: &GameInstance, index: usize, format: Format) -> Target {
    Target::at(game, game.find_stream(LEVEL).unwrap(), index, Some(format)).unwrap()
}

#[test]
fn formats_follow_the_type() {
    assert_eq!(formats_for(TYPE_TG4D), [Format::Png, Format::Dds]);
    assert_eq!(formats_for(TYPE_LH2), [Format::Csv, Format::Json]);
    assert_eq!(formats_for(TYPE_METAMODEL), [Format::Glb, Format::Json]);
    assert!(formats_for(sdbm(b"rcb")).is_empty());
    assert_eq!("GLTF".parse::<Format>().unwrap(), Format::Glb);
    assert!("bmp".parse::<Format>().is_err());

    let (_dir, game) = game();
    // The data half of a texture links its header.
    let t = target(&game, 1, Format::Png);
    assert_eq!((t.resource, t.texture_data), (TEX_HEADER, Some(1)));
    let s = game.find_stream(LEVEL).unwrap();
    assert!(Target::at(&game, s, STRINGS, Some(Format::Png)).is_err());
    assert_eq!(
        Target::at(&game, s, STRINGS, None).unwrap().format,
        Format::Csv
    );
    assert_eq!(file_name(&game, None, &t), "wall_00000055.png");
    let script = target(&game, SCRIPT, Format::Lua);
    assert_eq!(file_name(&game, None, &script), "test_00000005.lua");
}

#[test]
fn exported_files_import_back_without_changes() {
    let (_dir, game) = game();
    let mut ws = Workspace::for_game(&game, None).unwrap();
    // PNG is encoded again with a new mip chain, so it is left out: the
    // link never imports a file that was not saved with new content.
    for (index, format) in [
        (TEX_HEADER, Format::Dds),
        (STRINGS, Format::Csv),
        (STRINGS, Format::Json),
        (SIMGROUP, Format::Json),
        (SCRIPT, Format::Lua),
    ] {
        let t = target(&game, index, format);
        let bytes = export(&game, Some(&ws), &t, None).unwrap();
        let imported = import(&game, &mut ws, &t, &bytes, None, &ModelOptions::default())
            .unwrap_or_else(|e| panic!("{format}: {e}"));
        assert_eq!(imported.seq, None, "{format}: {imported:?}");
    }
    assert!(ws.operations().is_empty());
}

#[test]
fn edited_files_change_the_resources() {
    let (_dir, game) = game();
    let mut ws = Workspace::for_game(&game, None).unwrap();
    let none = ModelOptions::default();

    let t = target(&game, STRINGS, Format::Csv);
    let csv = String::from_utf8(export(&game, None, &t, None).unwrap()).unwrap();
    let edited = csv.replace("Hello", "Привет");
    let imported = import(&game, &mut ws, &t, edited.as_bytes(), None, &none).unwrap();
    assert!(imported.seq.is_some());
    let table = StringTable::parse(&ws.data(&t.key).unwrap().unwrap()).unwrap();
    assert_eq!(table.strings[0][0], "Привет");
    assert_eq!(table.strings[0][1], "with, \"quotes\"\nand lines");
    // The JSON export shows the workspace version.
    let tj = target(&game, STRINGS, Format::Json);
    let json = String::from_utf8(export(&game, Some(&ws), &tj, None).unwrap()).unwrap();
    assert!(json.contains("Привет"));

    let t = target(&game, SIMGROUP, Format::Json);
    let mut doc: Value = serde_json::from_slice(&export(&game, None, &t, None).unwrap()).unwrap();
    assert_eq!(doc["stream"], LEVEL);
    doc["objects"][0]["packets"][0]["attrs"][0]["value"] = "renamed".into();
    let imported = import(&game, &mut ws, &t, doc.to_string().as_bytes(), None, &none).unwrap();
    assert!(imported.seq.is_some(), "{imported:?}");
    // A document of another resource is refused.
    doc["id"] = "12345678".into();
    assert!(import(&game, &mut ws, &t, doc.to_string().as_bytes(), None, &none).is_err());

    let t = target(&game, SCRIPT, Format::Lua);
    let error = import(&game, &mut ws, &t, b"x = = 2\n", None, &none).unwrap_err();
    assert!(error.to_string().contains("line 1"), "{error}");
    let imported = import(&game, &mut ws, &t, b"x = 2\n", None, &none).unwrap();
    assert!(imported.seq.is_some());
    let chunk = Chunk::parse(&ws.data(&t.key).unwrap().unwrap()).unwrap();
    assert_eq!(chunk.source(), "@x:\\scripts\\test.lua");

    let t = target(&game, TEX_HEADER, Format::Png);
    let png = pixels::encode_png(8, 8, &[200u8; 8 * 8 * 4]).unwrap();
    let imported = import(&game, &mut ws, &t, &png, None, &none).unwrap();
    assert_eq!(imported.label, "Import texture wall");
    let header = Tg4Header::parse(&ws.data(&t.key).unwrap().unwrap()).unwrap();
    assert_eq!((header.width, header.height), (8, 8));
    assert_eq!(ws.operations().len(), 4);
}

/// Writes a PNG of one grey level.
fn grey(level: u8) -> Vec<u8> {
    pixels::encode_png(4, 4, &[level; 4 * 4 * 4]).unwrap()
}

/// Polls `link` until it reports something, for at most five seconds.
fn wait(link: &mut EditLink, game: &GameInstance, ws: &mut Workspace) -> Option<LinkEvent> {
    let end = Instant::now() + Duration::from_secs(5);
    while Instant::now() < end {
        if let Some(event) = link.check(game, ws, None, Instant::now()) {
            return Some(event);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    None
}

fn linked(game: &GameInstance, dir: &Path) -> EditLink {
    let t = target(game, TEX_HEADER, Format::Png);
    let mut link = EditLink::create(game, None, t, dir, None, None).unwrap();
    link.set_quiet(Duration::from_millis(60));
    link
}

fn imported(event: Option<LinkEvent>) -> (Imported, bool) {
    match event {
        Some(LinkEvent::Imported { imported, merged }) => (imported, merged),
        other => panic!("expected an import, got {other:?}"),
    }
}

#[test]
fn rapid_saves_make_one_operation() {
    let (_dir, game) = game();
    let files = tempfile::tempdir().unwrap();
    let mut ws = Workspace::for_game(&game, None).unwrap();
    let mut link = linked(&game, files.path());
    assert_eq!(link.path(), files.path().join("wall_00000055.png"));
    assert_eq!(
        pixels::decode_png(&fs::read(link.path()).unwrap())
            .unwrap()
            .0,
        4
    );

    fs::write(link.path(), grey(10)).unwrap();
    let (first, merged) = imported(wait(&mut link, &game, &mut ws));
    assert!(!merged && first.seq.is_some());
    fs::write(link.path(), grey(20)).unwrap();
    let (second, merged) = imported(wait(&mut link, &game, &mut ws));
    assert!(merged);
    assert_eq!(
        ws.operations().len(),
        1,
        "the second save replaced the first"
    );
    assert_ne!(first.seq, second.seq);

    // Another edit in between: the next save is an operation of its own.
    let s = game.find_stream(LEVEL).unwrap();
    let table = sample_table(["A", "B"]);
    ws.set_string_table(&game, s, STRINGS, &table).unwrap();
    fs::write(link.path(), grey(30)).unwrap();
    let (_, merged) = imported(wait(&mut link, &game, &mut ws));
    assert!(!merged);
    assert_eq!(ws.operations().len(), 3);

    // After the coalescing time, too.
    link.coalesce = Duration::ZERO;
    std::thread::sleep(Duration::from_millis(5));
    fs::write(link.path(), grey(40)).unwrap();
    let (_, merged) = imported(wait(&mut link, &game, &mut ws));
    assert!(!merged);
    assert_eq!(ws.operations().len(), 4);
    // Undo takes the workspace back one save.
    ws.undo().unwrap();
    let data = ws.data(&link.target.key).unwrap().unwrap();
    let header = Tg4Header::parse(&data).unwrap();
    assert_eq!(header.name, "wall");
    assert_eq!(link.imports, 4);
}

#[test]
fn unchanged_saves_are_not_imported() {
    let (_dir, game) = game();
    let files = tempfile::tempdir().unwrap();
    let mut ws = Workspace::for_game(&game, None).unwrap();
    let mut link = linked(&game, files.path());
    let exported = fs::read(link.path()).unwrap();
    // Saved again as it was, and touched by a rename of the same bytes.
    fs::write(link.path(), &exported).unwrap();
    assert_eq!(wait(&mut link, &game, &mut ws), Some(LinkEvent::Unchanged));
    let other = files.path().join("tmp.png");
    fs::write(&other, &exported).unwrap();
    fs::rename(&other, link.path()).unwrap();
    assert_eq!(wait(&mut link, &game, &mut ws), Some(LinkEvent::Unchanged));
    assert!(ws.operations().is_empty());
    assert_eq!(link.imports, 0);
}

#[test]
fn saves_by_rename_are_seen() {
    let (_dir, game) = game();
    let files = tempfile::tempdir().unwrap();
    let mut ws = Workspace::for_game(&game, None).unwrap();
    let mut link = linked(&game, files.path());
    // GIMP and Krita: write a new file, then rename it over the old one.
    let temp = files.path().join(".wall.png.part");
    fs::write(&temp, grey(99)).unwrap();
    fs::rename(&temp, link.path()).unwrap();
    let (first, _) = imported(wait(&mut link, &game, &mut ws));
    assert!(first.seq.is_some());
    // Delete, then write the file again a little later.
    fs::remove_file(link.path()).unwrap();
    std::thread::sleep(Duration::from_millis(150));
    fs::write(link.path(), grey(120)).unwrap();
    let (_, merged) = imported(wait(&mut link, &game, &mut ws));
    assert!(merged);
    assert!(link.event_error().is_none(), "{:?}", link.event_error());
}

#[test]
fn a_file_written_in_pieces_is_imported_when_complete() {
    let (_dir, game) = game();
    let files = tempfile::tempdir().unwrap();
    let mut ws = Workspace::for_game(&game, None).unwrap();
    let mut link = linked(&game, files.path());
    let png = grey(77);
    // The first half, then a pause longer than the quiet time.
    fs::write(link.path(), &png[..png.len() / 2]).unwrap();
    match wait(&mut link, &game, &mut ws) {
        Some(LinkEvent::Failed(_)) => {}
        other => panic!("a half-written PNG must fail: {other:?}"),
    }
    assert!(ws.operations().is_empty());
    // The rest arrives.
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(link.path())
        .unwrap();
    std::io::Write::write_all(&mut file, &png[png.len() / 2..]).unwrap();
    drop(file);
    let (done, _) = imported(wait(&mut link, &game, &mut ws));
    assert!(done.seq.is_some());
    assert_eq!(ws.operations().len(), 1);
}

#[test]
fn a_failed_merge_keeps_the_previous_import() {
    let (_dir, game) = game();
    let files = tempfile::tempdir().unwrap();
    let mut ws = Workspace::for_game(&game, None).unwrap();
    let t = target(&game, SCRIPT, Format::Lua);
    let mut link = EditLink::create(&game, None, t, files.path(), None, None).unwrap();
    link.set_quiet(Duration::from_millis(60));
    let source = fs::read_to_string(link.path()).unwrap();
    fs::write(link.path(), source.replace("1", "3")).unwrap();
    let (_, merged) = imported(wait(&mut link, &game, &mut ws));
    assert!(!merged);
    let after_first = ws.data(&link.target.key).unwrap();
    // A syntax error: reported, and the first import stays in effect.
    fs::write(link.path(), "x = = 1\n").unwrap();
    match wait(&mut link, &game, &mut ws) {
        Some(LinkEvent::Failed(error)) => assert!(error.contains("line 1"), "{error}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(ws.data(&link.target.key).unwrap(), after_first);
    assert_eq!(ws.head(), 1);
    // Writing the workspace version back into the file.
    link.refresh(&game, Some(&ws), None).unwrap();
    assert!(fs::read_to_string(link.path()).unwrap().contains('3'));
    assert_eq!(wait_quietly(&mut link), None);
}

/// Polls for a short while; the link must report nothing.
fn wait_quietly(link: &mut EditLink) -> Option<Poll> {
    let end = Instant::now() + Duration::from_millis(300);
    while Instant::now() < end {
        match link.poll(Instant::now()).unwrap() {
            Poll::Idle | Poll::Waiting(_) => {}
            other => return Some(other),
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    None
}

#[test]
fn working_folders_differ_per_resource() {
    let (_dir, game) = game();
    let a: PathBuf = EditLink::default_dir(&target(&game, TEX_HEADER, Format::Png));
    let b = EditLink::default_dir(&target(&game, STRINGS, Format::Csv));
    assert_ne!(a, b);
    assert!(a.ends_with("gf2_level_tg4h_00000055"), "{}", a.display());
}

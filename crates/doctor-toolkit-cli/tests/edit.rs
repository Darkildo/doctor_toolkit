//! `doctor-toolkit edit` with a fake editor: a shell script that changes the
//! working file like an editor saves it, then exits. No real editor (or any
//! window) is started.
#![cfg(unix)]

use std::fs;
use std::path::Path;
use std::process::Command;

use doctor_toolkit::container::ustream::{
    FileHeader, ResourceHeader, Storage, StreamLayout, StreamWriter,
};
use doctor_toolkit::game::GameInstance;
use doctor_toolkit::hash::sdbm;
use doctor_toolkit::io::Endian;
use doctor_toolkit::resource::lh2::StringTable;
use doctor_toolkit::resource::lua::{self, Chunk};
use doctor_toolkit::toc::{self, Toc};
use doctor_toolkit::workspace::{ResourceKey, Workspace};

const STREAM: &str = "gf2/level";

fn header(tag: &[u8], id: u32) -> ResourceHeader {
    ResourceHeader {
        mem_policy: sdbm(b"default"),
        alignment: 0x10,
        type_id: sdbm(tag),
        ids: [id; 4],
        data_size: 0,
    }
}

/// A table of contents listing one stream.
fn toc_bytes(name: &str, size: u64) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&toc::MAGIC.to_le_bytes());
    out.extend_from_slice(&10u32.to_le_bytes());
    for value in [1u16, 1, 0, 0] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    let names_start = 0x10 + 20;
    out.extend_from_slice(&(names_start as u32).to_le_bytes());
    out.extend_from_slice(&(size as u32).to_le_bytes());
    out.extend_from_slice(&[0u8; 12]);
    out.extend_from_slice(&0x100u32.to_le_bytes());
    out.extend_from_slice(&((names_start + 8 + 1) as u32).to_le_bytes());
    out.push(0);
    out.extend_from_slice(name.replace('/', "\\").as_bytes());
    out.push(0);
    out.resize(toc::PADDING, 0);
    out
}

/// A game with a string table (resource 0) and a script (resource 1).
fn game(root: &Path) {
    let table = StringTable {
        version: 1,
        flags: 0x4000,
        languages: Vec::new(),
        hashes: vec![1, 2],
        strings: vec![vec!["Hello".into(), "World".into()]],
    };
    let script = lua::compile_for_game(b"x = 1\n", b"@x:\\scripts\\door.lua").unwrap();
    let mut writer = StreamWriter::new(&FileHeader::GF2_PC, Endian::Little);
    writer
        .push_resource(&header(b"lh2", 9), &table.to_bytes().unwrap(), Storage::Raw)
        .unwrap();
    writer
        .push_resource(&header(b"lua", 5), &script, Storage::Raw)
        .unwrap();
    let stream = writer.finish();
    let size = StreamLayout::parse(&stream).unwrap().total_data_size();
    fs::create_dir_all(root.join("gf2")).unwrap();
    fs::write(root.join("godfather2.exe"), b"MZ fake").unwrap();
    fs::write(root.join("gf2/level.str"), &stream).unwrap();
    let toc = Toc::parse(&toc_bytes(STREAM, size)).unwrap();
    fs::write(root.join("gf2/level.toc"), toc.to_bytes().unwrap()).unwrap();
}

/// Runs `edit` with a fake editor script; returns the output.
fn edit(root: &Path, key: &str, script: &str) -> String {
    let editor = root.join("fake-editor.sh");
    fs::write(&editor, script).unwrap();
    let files = root.join("files");
    let output = Command::new(env!("CARGO_BIN_EXE_doctor-toolkit"))
        .arg("edit")
        .arg(root)
        .arg(key)
        .arg("--with")
        .arg(format!("sh {} {{file}}", editor.display()))
        .arg("--wait")
        .arg("--file-dir")
        .arg(&files)
        .env("DOCTOR_TOOLKIT_CONFIG_DIR", root.join("config"))
        .env("XDG_STATE_HOME", root.join("state"))
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{text}");
    text
}

fn key(root: &Path, index: usize) -> ResourceKey {
    let game = GameInstance::open(root).unwrap();
    let file = game.open_stream(game.find_stream(STREAM).unwrap()).unwrap();
    ResourceKey::new(STREAM, &file.layout().resources[index].header)
}

#[test]
fn saves_of_a_fake_editor_are_imported_as_one_operation() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    game(root);
    let before = fs::read(root.join("gf2/level.str")).unwrap();
    // Save 1: a new file renamed over the old one (GIMP, LibreOffice).
    // Save 2: written in place. Save 3: the same content again.
    let script = r#"
f="$1"
sleep 0.3
sed 's/Hello/Hi/' "$f" > "$f.tmp" && mv "$f.tmp" "$f"
sleep 1
sed 's/World/Earth/' "$f" > "$f.new" && cat "$f.new" > "$f" && rm "$f.new"
sleep 1
cp "$f" "$f.copy" && mv "$f.copy" "$f"
sleep 1
"#;
    let text = edit(root, &format!("{STREAM}:0"), script);
    assert!(
        text.contains("imported: Edit strings in gf2/level"),
        "{text}"
    );
    assert!(text.contains("(replaces the previous save)"), "{text}");
    assert!(text.contains("saved without changes"), "{text}");
    assert!(text.contains("2 save(s) imported"), "{text}");

    let ws = Workspace::open(Workspace::default_dir(root)).unwrap();
    assert_eq!(ws.operations().len(), 1, "two saves make one operation");
    let table = StringTable::parse(&ws.data(&key(root, 0)).unwrap().unwrap()).unwrap();
    assert_eq!(table.strings[0], ["Hi", "Earth"]);
    // The game is not touched without --direct.
    assert_eq!(fs::read(root.join("gf2/level.str")).unwrap(), before);
}

#[test]
fn script_errors_are_reported_and_the_next_save_is_imported() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    game(root);
    let script = r#"
f="$1"
sleep 0.3
printf 'x = = 2\n' > "$f"
sleep 1
printf 'x = 2\n' > "$f"
sleep 1
"#;
    let key = key(root, 1);
    let text = edit(root, &key.to_string(), script);
    assert!(text.contains("cannot import"), "{text}");
    assert!(text.contains("line 1"), "{text}");
    assert!(text.contains("imported: Edit script door.lua"), "{text}");
    let ws = Workspace::open(Workspace::default_dir(root)).unwrap();
    let chunk = Chunk::parse(&ws.data(&key).unwrap().unwrap()).unwrap();
    let expected = lua::compile_for_game(b"x = 2\n", b"@x:\\scripts\\door.lua").unwrap();
    assert_eq!(chunk.to_bytes(), expected);
}

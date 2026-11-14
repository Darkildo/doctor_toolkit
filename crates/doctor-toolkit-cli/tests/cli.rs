//! The command line program on a small synthetic game.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use doctor_toolkit::container::ustream::{FileHeader, ResourceHeader, Storage, StreamWriter};
use doctor_toolkit::hash::sdbm;
use doctor_toolkit::io::Endian;
use doctor_toolkit::resource::lh2::StringTable;
use doctor_toolkit::toc;

fn run(args: &[&std::ffi::OsStr], log_dir: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_doctor-toolkit"))
        .args(args)
        .env("DOCTOR_TOOLKIT_LOG_DIR", log_dir)
        // A game running elsewhere on the machine (a test bench) does not count.
        .env("DOCTOR_TOOLKIT_GAME_PROCESS", "no-such-game.exe")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap()
}

/// A table of contents listing `streams` (name, data size).
fn toc_bytes(streams: &[(&str, u64)]) -> Vec<u8> {
    let count = streams.len();
    let names_start = 0x10 + count * 20;
    let pool_start = names_start + count * 8;
    let mut out = Vec::new();
    out.extend_from_slice(&toc::MAGIC.to_le_bytes());
    out.extend_from_slice(&10u32.to_le_bytes());
    for value in [count as u16, count as u16, 0, 0] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    for (i, (_, size)) in streams.iter().enumerate() {
        out.extend_from_slice(&((names_start + i * 8) as u32).to_le_bytes());
        out.extend_from_slice(&(*size as u32).to_le_bytes());
        out.extend_from_slice(&[0u8; 12]);
    }
    let mut string_offset = pool_start + 1;
    for (i, (name, _)) in streams.iter().enumerate() {
        out.extend_from_slice(&(0x100 + i as u32).to_le_bytes());
        out.extend_from_slice(&(string_offset as u32).to_le_bytes());
        string_offset += name.len() + 1;
    }
    out.push(0);
    for (name, _) in streams {
        out.extend_from_slice(name.replace('/', "\\").as_bytes());
        out.push(0);
    }
    out.resize(toc::PADDING, 0);
    out
}

/// A game with one stream holding one string table.
fn game(root: &Path) {
    let table = StringTable {
        version: 1,
        flags: 0x4000,
        languages: Vec::new(),
        hashes: vec![1, 2],
        strings: vec![vec!["Hello".into(), "World".into()]],
    };
    let header = ResourceHeader {
        mem_policy: sdbm(b"default"),
        alignment: 0x10,
        type_id: sdbm(b"lh2"),
        ids: [9; 4],
        data_size: 0,
    };
    let mut writer = StreamWriter::new(&FileHeader::GF2_PC, Endian::Little);
    writer
        .push_resource(&header, &table.to_bytes().unwrap(), Storage::Raw)
        .unwrap();
    let stream = writer.finish();
    let size = doctor_toolkit::container::ustream::StreamLayout::parse(&stream)
        .unwrap()
        .total_data_size();
    std::fs::create_dir_all(root.join("gf2")).unwrap();
    std::fs::write(root.join("godfather2.exe"), b"MZ").unwrap();
    std::fs::write(root.join("gf2/zone.str"), &stream).unwrap();
    let toc = toc::Toc::parse(&toc_bytes(&[("gf2/zone", size)])).unwrap();
    std::fs::write(root.join("gf2/zone.toc"), toc.to_bytes().unwrap()).unwrap();
}

fn json_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.extend(json_files(&path));
        } else if path.extension().is_some_and(|e| e == "json") {
            out.push(path);
        }
    }
    out
}

#[test]
fn editing_commands_create_the_workspace_only_when_they_record() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("game");
    let logs = dir.path().join("logs");
    game(&root);
    let ws = root.join("_doctor_toolkit/workspace");
    let os = |s: &Path| s.as_os_str().to_owned();

    let out = dir.path().join("text");
    let export = run(
        &[
            "text-export".as_ref(),
            root.as_ref(),
            "-o".as_ref(),
            out.as_ref(),
        ],
        &logs,
    );
    assert!(export.status.success(), "{export:?}");
    let files = json_files(&out);
    assert_eq!(files.len(), 1);

    // A failing command.
    let missing = dir.path().join("missing.json");
    let failed = run(
        &["text-import".as_ref(), root.as_ref(), missing.as_ref()],
        &logs,
    );
    assert!(!failed.status.success());
    assert!(!ws.exists(), "a failed command leaves no workspace");
    // A command that records nothing.
    let unchanged = run(
        &["text-import".as_ref(), root.as_ref(), files[0].as_ref()],
        &logs,
    );
    assert!(unchanged.status.success(), "{unchanged:?}");
    assert!(!ws.exists(), "an unchanged import leaves no workspace");
    // A command that refuses its arguments after opening the game.
    let bad = run(
        &[
            "simgroup-set".as_ref(),
            root.as_ref(),
            "gf2/zone".as_ref(),
            "0".as_ref(),
            "0".as_ref(),
            "--position".as_ref(),
            "1,2,3".as_ref(),
        ],
        &logs,
    );
    assert!(!bad.status.success());
    assert!(!ws.exists());

    // A real edit records it.
    let mut document: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&files[0]).unwrap()).unwrap();
    let entry = document["entries"][0].as_object_mut().unwrap();
    let column = entry.keys().find(|k| *k != "hash").unwrap().clone();
    entry.insert(column, "Ciao".into());
    std::fs::write(&files[0], document.to_string()).unwrap();
    let edited = run(
        &[
            "text-import".as_ref(),
            os(&root).as_ref(),
            files[0].as_ref(),
        ],
        &logs,
    );
    assert!(edited.status.success(), "{edited:?}");
    assert!(ws.join("manifest.json").is_file());
}

#[test]
fn detect_and_play_dry_run() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("game");
    let logs = dir.path().join("logs");
    game(&root);
    let detect = run(&["detect".as_ref(), root.as_ref()], &logs);
    assert!(detect.status.success(), "{detect:?}");
    let text = String::from_utf8_lossy(&detect.stdout);
    assert!(text.contains("The Godfather II (PC)"), "{text}");
    assert!(text.contains("found by folder"), "{text}");
    let json = run(
        &["detect".as_ref(), "--json".as_ref(), root.as_ref()],
        &logs,
    );
    let list: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1);

    let play = run(
        &[
            "play".as_ref(),
            root.as_ref(),
            "--method".as_ref(),
            "wine".as_ref(),
            "--prefix".as_ref(),
            "/prefixes/gf2".as_ref(),
            "--apply".as_ref(),
            "--dry-run".as_ref(),
        ],
        &logs,
    );
    assert!(play.status.success(), "{play:?}");
    let text = String::from_utf8_lossy(&play.stdout);
    assert!(text.contains("WINEPREFIX=/prefixes/gf2"), "{text}");
    assert!(text.contains("would apply the workspace first"), "{text}");
    // Without an ASI loader the live module cannot give the game the
    // workspace (stage 46): written into the game, with a word why; and
    // refused where nothing may be written.
    assert!(text.contains("note: the game folder is written"), "{text}");
    for flag in ["--no-game-writes", "--mods"] {
        let mut args: Vec<&std::ffi::OsStr> = vec![
            "play".as_ref(),
            root.as_ref(),
            "--method".as_ref(),
            "wine".as_ref(),
            flag.as_ref(),
            "--dry-run".as_ref(),
        ];
        if flag != "--mods" {
            args.push("--apply".as_ref());
        }
        let refused = run(&args, &logs);
        assert!(!refused.status.success(), "{flag}");
        let err = String::from_utf8_lossy(&refused.stderr);
        assert!(
            err.contains("cannot play without writing into the game"),
            "{flag}: {err}"
        );
    }
}

/// Edits the first string of the exported table and records it.
fn record_an_edit(root: &Path, dir: &Path, logs: &Path) {
    let out = dir.join("text");
    let export = run(
        &[
            "text-export".as_ref(),
            root.as_ref(),
            "-o".as_ref(),
            out.as_ref(),
        ],
        logs,
    );
    assert!(export.status.success(), "{export:?}");
    let file = json_files(&out).remove(0);
    let mut document: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    let entry = document["entries"][0].as_object_mut().unwrap();
    let column = entry.keys().find(|k| *k != "hash").unwrap().clone();
    entry.insert(column, "Ciao".into());
    std::fs::write(&file, document.to_string()).unwrap();
    let edited = run(
        &["text-import".as_ref(), root.as_ref(), file.as_ref()],
        logs,
    );
    assert!(edited.status.success(), "{edited:?}");
}

#[cfg(unix)]
#[test]
fn play_applies_runs_a_fake_game_and_reverts() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("game");
    let logs = dir.path().join("logs");
    game(&root);
    let original = std::fs::read(root.join("gf2/zone.str")).unwrap();
    record_an_edit(&root, dir.path(), &logs);
    // The fake game copies the stream it sees and exits.
    let script = root.join("fake-game.sh");
    std::fs::write(&script, "#!/bin/sh\ncp gf2/zone.str seen.str\nsleep 0.2\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let play = run(
        &[
            "play".as_ref(),
            root.as_ref(),
            "--method".as_ref(),
            "command".as_ref(),
            "--command".as_ref(),
            "{dir}/fake-game.sh".as_ref(),
            "--apply".as_ref(),
            "--revert-after".as_ref(),
        ],
        &logs,
    );
    let text = String::from_utf8_lossy(&play.stdout);
    assert!(play.status.success(), "{play:?}");
    assert!(text.contains("applied: 1 stream(s)"), "{text}");
    assert!(text.contains("the game has exited"), "{text}");
    assert!(text.contains("reverted: 1 stream(s)"), "{text}");
    let seen = std::fs::read(root.join("seen.str")).unwrap();
    assert_ne!(seen, original, "the game saw the applied workspace");
    assert_eq!(
        std::fs::read(root.join("gf2/zone.str")).unwrap(),
        original,
        "and the original came back after it exited"
    );
}

/// `play --stream`: the game reads its options from cmd.txt, which is there
/// while it runs and gone after it exits.
#[cfg(unix)]
#[test]
fn play_writes_cmd_txt_for_the_launch_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("game");
    let logs = dir.path().join("logs");
    game(&root);
    let script = root.join("fake-game.sh");
    std::fs::write(&script, "#!/bin/sh\ncp cmd.txt seen.txt\nsleep 0.2\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let args = |extra: &[&str]| {
        let mut list: Vec<std::ffi::OsString> = vec![
            "play".into(),
            root.clone().into(),
            "--method".into(),
            "command".into(),
            "--command".into(),
            "{dir}/fake-game.sh".into(),
            "--stream".into(),
            "gf2/cuba".into(),
            "--spawn".into(),
            "26d7c8323dfa11aa464f574c45523130".into(),
            "--windowed".into(),
            "--resolution".into(),
            "800x600".into(),
        ];
        list.extend(extra.iter().map(Into::into));
        list
    };
    let expected = "-windowed -resolution 800 600 \
        -playerspawn {32C8D726-FA3D-AA11-464F-574C45523130} -stream gf2 cuba";

    let dry = args(&["--dry-run"]);
    let dry = run(
        &dry.iter().map(|a| a.as_os_str()).collect::<Vec<_>>(),
        &logs,
    );
    assert!(dry.status.success(), "{dry:?}");
    let text = String::from_utf8_lossy(&dry.stdout);
    assert!(text.contains(&format!("cmd.txt: {expected}")), "{text}");
    assert!(!root.join("cmd.txt").exists(), "a dry run writes nothing");

    let play = args(&[]);
    let play = run(
        &play.iter().map(|a| a.as_os_str()).collect::<Vec<_>>(),
        &logs,
    );
    let text = String::from_utf8_lossy(&play.stdout);
    assert!(play.status.success(), "{play:?}");
    assert!(text.contains("the game has exited"), "{text}");
    assert!(text.contains("took away cmd.txt"), "{text}");
    assert_eq!(
        std::fs::read_to_string(root.join("seen.txt")).unwrap(),
        format!("{expected}\r\n"),
        "the game saw its options"
    );
    assert!(
        !root.join("cmd.txt").exists(),
        "and they are gone after it exited"
    );

    // Streams of different folders cannot start together.
    let bad = run(
        &[
            "play".as_ref(),
            root.as_ref(),
            "--stream".as_ref(),
            "gf2/cuba".as_ref(),
            "--stream".as_ref(),
            "mp_mall/mp_mall".as_ref(),
            "--dry-run".as_ref(),
        ],
        &logs,
    );
    assert!(!bad.status.success(), "{bad:?}");
}

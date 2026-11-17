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

/// `doctor-toolkit ARGS…`: whether it succeeded, and stdout with stderr.
fn mods_run(args: &[&str], logs: &Path) -> (bool, String) {
    let args: Vec<&std::ffi::OsStr> = args.iter().map(|a| a.as_ref()).collect();
    let out = run(&args, logs);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

#[test]
fn mods_of_format_3_build_install_order_and_switch() {
    use doctor_toolkit::testing::{self, MESH, STRINGS};
    use doctor_toolkit::workspace::{ResourceEdit, StorageChoice, Workspace};

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("game");
    let logs = dir.path().join("logs");
    testing::write_game(&root).unwrap();
    let game = doctor_toolkit::game::GameInstance::open(&root).unwrap();
    let level = game.find_stream(testing::LEVEL).unwrap();
    let edit = |ws: &mut Workspace, index: usize, data: Vec<u8>| {
        let edit = ResourceEdit {
            index,
            data,
            storage: StorageChoice::Keep,
        };
        ws.set_resources(&game, "edit", level, &[edit]).unwrap();
    };
    let ws_dir = Workspace::default_dir(&root);
    let mut ws = Workspace::create(&ws_dir, &game).unwrap();
    edit(&mut ws, MESH, vec![1; 300]);
    let r = root.to_str().unwrap();
    let base = dir.path().join("base.zip");
    let b = base.to_str().unwrap();
    let (ok, text) = mods_run(
        &[
            "mods",
            "build",
            r,
            "-o",
            b,
            "--name",
            "Base",
            "--mod-version",
            "1.1",
        ],
        &logs,
    );
    assert!(ok && text.contains("format 3"), "{text}");
    // No edition by default: the mod fits any edition whose resources match.
    let (ok, text) = mods_run(&["mods", "info", b], &logs);
    assert!(ok && !text.contains("editions"), "{text}");
    // `--edition this` needs a game of a known edition.
    let other = dir.path().join("other.zip");
    let (ok, text) = mods_run(
        &[
            "mods",
            "build",
            r,
            "-o",
            other.to_str().unwrap(),
            "--name",
            "Other",
            "--edition",
            "this",
        ],
        &logs,
    );
    assert!(!ok && text.contains("is not known"), "{text}");
    // A version that is no version number is refused in format 3.
    let (ok, text) = mods_run(
        &[
            "mods",
            "build",
            r,
            "-o",
            b,
            "--name",
            "Base",
            "--mod-version",
            "beta",
        ],
        &logs,
    );
    assert!(!ok && text.contains("is not a version"), "{text}");

    std::fs::remove_dir_all(&ws_dir).unwrap();
    let mut ws = Workspace::create(&ws_dir, &game).unwrap();
    edit(
        &mut ws,
        STRINGS,
        testing::sample_table(["Patch", "Text"]).to_bytes().unwrap(),
    );
    let patch = dir.path().join("patch.zip");
    let p = patch.to_str().unwrap();
    let (ok, text) = mods_run(
        &[
            "mods",
            "build",
            r,
            "-o",
            p,
            "--name",
            "Patch",
            "--depends",
            "base=>=1.1 <2",
            "--incompatible",
            "old=<3:changes the same lamps",
            "--load-after",
            "hd",
            "--edition",
            "1.0.789.0",
            "--requires-manager",
            "none",
        ],
        &logs,
    );
    assert!(ok, "{text}");
    let (ok, text) = mods_run(&["mods", "info", p], &logs);
    assert!(ok, "{text}");
    for line in [
        "format   3",
        "editions 1.0.789.0",
        "depends on   base >=1.1 <2",
        "incompatible with old <3: changes the same lamps",
        "load after   hd",
    ] {
        assert!(text.contains(line), "{line}: {text}");
    }
    assert!(!text.contains("needs mod manager"), "{text}");

    // Installed: the patch is of another edition than this (unknown) game.
    let (ok, text) = mods_run(&["mods", "install", r, p, b], &logs);
    assert!(ok, "{text}");
    assert!(text.contains("tested on the editions 1.0.789.0"), "{text}");
    let (_, list) = mods_run(&["mods", "list", r], &logs);
    let base_at = list.find("base").unwrap();
    assert!(base_at < list.find("patch").unwrap(), "{list}");

    // The base cannot go below the patch, nor be switched off while it is on.
    let (ok, text) = mods_run(&["mods", "move", r, "base", "2"], &logs);
    assert!(!ok && text.contains("`patch` needs `base`"), "{text}");
    let (ok, text) = mods_run(&["mods", "disable", r, "base"], &logs);
    assert!(
        !ok && text.contains("needed by the enabled mod(s) patch"),
        "{text}"
    );
    let (ok, text) = mods_run(&["mods", "uninstall", r, "base"], &logs);
    assert!(!ok && text.contains("--force"), "{text}");
    let (ok, text) = mods_run(&["mods", "disable", r, "base", "patch"], &logs);
    assert!(ok, "{text}");
    let (ok, text) = mods_run(&["mods", "enable", r, "patch"], &logs);
    assert!(ok && text.contains("--with-deps"), "{text}");
    let (ok, text) = mods_run(&["mods", "check", r], &logs);
    assert!(ok && text.contains("switched off"), "{text}");
    let (ok, text) = mods_run(&["mods", "enable", r, "patch", "--with-deps"], &logs);
    assert!(
        ok && text.contains("enabled base (needed by patch)"),
        "{text}"
    );
    let (ok, text) = mods_run(&["mods", "sort", r, "--dry-run"], &logs);
    assert!(ok && text.contains("already follows"), "{text}");
    let (ok, text) = mods_run(&["mods", "disable", r, "base", "--force"], &logs);
    assert!(ok, "{text}");
}

/// `dialogue-import --raw` writes the `dlg` resource of a tree without a game:
/// the data of a dialogue that gets a stream of its own.
#[test]
fn dialogue_import_raw_writes_the_resource() {
    use doctor_toolkit::resource::dlg::{
        Animation, Dialogue, KIND_ASSIGN, KIND_CHOICE, KIND_END, Node,
    };
    const NONE: u32 = u32::MAX;
    let mut menu = vec![0x4416_0000, 0, 0, 0, 0, 0x40102, 0, 0, 0x1080, 8, 8];
    menu.extend([0, 1, NONE, NONE, NONE]);
    let tree = Dialogue {
        nodes: vec![
            Node {
                kind: KIND_ASSIGN,
                children: vec![Some(1)],
                payload: Some(vec![0, 0, 0x49, 1]),
            },
            Node {
                kind: KIND_CHOICE,
                children: vec![Some(2), Some(3), None, Some(3), None],
                payload: Some(menu),
            },
            Node {
                kind: KIND_ASSIGN,
                children: vec![Some(1)],
                payload: Some(vec![0, 0, 0x23, 1]),
            },
            Node {
                kind: KIND_END,
                children: vec![],
                payload: None,
            },
        ],
        groups: vec![vec![0]],
        animations: vec![Animation {
            name: "f2f_player_talk_neutral_1".into(),
            flags: 0x40044,
        }],
        labels: vec![
            "doctor_buy".into(),
            "doctor_leave".into(),
            String::new(),
            String::new(),
            String::new(),
        ],
    };
    let dir = tempfile::tempdir().unwrap();
    let json = dir.path().join("tree.json");
    let raw = dir.path().join("tree.dlg");
    let document = serde_json::json!({ "stream": "gf2/globalfolder/dlg/new", "data": tree });
    std::fs::write(&json, serde_json::to_string(&document).unwrap()).unwrap();
    let out = run(
        &[
            "dialogue-import".as_ref(),
            dir.path().as_os_str(),
            json.as_os_str(),
            "--raw".as_ref(),
            raw.as_os_str(),
        ],
        dir.path(),
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("4 node(s)"));
    assert_eq!(
        Dialogue::parse(&std::fs::read(&raw).unwrap()).unwrap(),
        tree
    );
}

/// Stage 81: a mod with executable code (format version 5) is built,
/// shown, installed — and its code waits for the answer; on the miniature
/// game, which is not the edition 1.0.789.0, the code is never installed.
#[test]
fn mods_with_code_wait_for_the_answer() {
    use doctor_toolkit::testing::{self, MESH};
    use doctor_toolkit::workspace::{ResourceEdit, StorageChoice, Workspace};

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("game");
    let logs = dir.path().join("logs");
    testing::write_game(&root).unwrap();
    let game = doctor_toolkit::game::GameInstance::open(&root).unwrap();
    let level = game.find_stream(testing::LEVEL).unwrap();
    let mut ws = Workspace::create(Workspace::default_dir(&root), &game).unwrap();
    let edit = ResourceEdit {
        index: MESH,
        data: vec![0x5A; 300],
        storage: StorageChoice::Keep,
    };
    ws.set_resources(&game, "edit", level, &[edit]).unwrap();
    drop(ws);
    // The smallest file that passes for a 32-bit DLL, and one that does not.
    let mut dll = vec![0u8; 0x100];
    dll[..2].copy_from_slice(b"MZ");
    dll[0x3C] = 0x40;
    dll[0x40..0x44].copy_from_slice(b"PE\0\0");
    dll[0x44..0x46].copy_from_slice(&0x014Cu16.to_le_bytes());
    dll[0x56..0x58].copy_from_slice(&0x2102u16.to_le_bytes());
    let dll_path = dir.path().join("My_Mod.dll");
    std::fs::write(&dll_path, &dll).unwrap();
    let text_path = dir.path().join("notes.dll");
    std::fs::write(&text_path, "not a DLL").unwrap();
    let r = root.to_str().unwrap();
    let zip = dir.path().join("code.zip");
    let z = zip.to_str().unwrap();
    let build = |native: &str, extra: &[&str]| {
        let mut args = vec![
            "mods", "build", r, "-o", z, "--name", "Code", "--native", native,
        ];
        args.extend_from_slice(extra);
        mods_run(&args, &logs)
    };

    let (ok, text) = build(text_path.to_str().unwrap(), &[]);
    assert!(!ok && text.contains("not a Windows DLL"), "{text}");
    let (ok, text) = build(&format!("../evil.asi={}", dll_path.display()), &[]);
    assert!(!ok && text.contains("bad module name"), "{text}");
    let (ok, text) = build(&format!("doctor_live.asi={}", dll_path.display()), &[]);
    assert!(!ok && text.contains("bad module name"), "{text}");

    // The name of the module is made from the file name.
    let (ok, text) = build(dll_path.to_str().unwrap(), &[]);
    assert!(ok && text.contains("format 5"), "{text}");
    assert!(text.contains("1 module(s) of executable code"), "{text}");
    assert!(text.contains("does not export doctor_mod_init"), "{text}");
    let (ok, text) = mods_run(&["mods", "info", z], &logs);
    assert!(ok, "{text}");
    assert!(
        text.contains("EXECUTABLE CODE for the edition 1.0.789.0"),
        "{text}"
    );
    assert!(
        text.contains("code scripts/my_mod.asi  256 byte(s)  sha256 "),
        "{text}"
    );
    assert!(!text.contains("not used by any resource"), "{text}");

    // Installed: the question is printed, nothing is asked of the keyboard.
    let (ok, text) = mods_run(&["mods", "install", r, z], &logs);
    assert!(ok && text.contains("contains EXECUTABLE CODE"), "{text}");
    assert!(
        text.contains("mods native") && text.contains("--allow"),
        "{text}"
    );
    let (ok, text) = mods_run(&["mods", "native", r], &logs);
    assert!(ok && text.contains("waits for your consent"), "{text}");
    // Without an answer the data is written and nothing executable is.
    let (ok, text) = mods_run(&["mods", "apply", r], &logs);
    assert!(ok && text.contains("waits for your consent"), "{text}");
    assert!(!root.join("scripts").exists());
    assert!(!root.join("dinput8.dll").exists());
    // The answer is recorded by the hash of the file …
    let (ok, text) = mods_run(&["mods", "native", r, "code", "--allow"], &logs);
    assert!(
        ok && text.contains("agreed: scripts/my_mod.asi  sha256 "),
        "{text}"
    );
    let record = std::fs::read_to_string(root.join("_doctor_toolkit/mods/native.json")).unwrap();
    assert!(record.contains("\"code\""), "{record}");
    // … and still the miniature game is not the edition the code is for.
    let (ok, text) = mods_run(&["mods", "apply", r], &logs);
    assert!(
        ok && text.contains("only for the edition 1.0.789.0"),
        "{text}"
    );
    assert!(!root.join("scripts").exists());
    let (ok, text) = mods_run(&["mods", "native", r, "code", "--forget"], &logs);
    assert!(ok && text.contains("forgotten"), "{text}");
    assert!(!root.join("_doctor_toolkit/mods/native.json").exists());

    // A mod that needs its code is not written at all without it.
    let (ok, text) = build(dll_path.to_str().unwrap(), &["--native-required"]);
    assert!(ok, "{text}");
    let (ok, text) = mods_run(&["mods", "install", r, z], &logs);
    assert!(
        ok && text.contains("does not work without its code"),
        "{text}"
    );
    let (ok, text) = mods_run(&["mods", "native", r, "code", "--data-only"], &logs);
    assert!(
        !ok && text.contains("does not work without its code"),
        "{text}"
    );
    let (ok, text) = mods_run(&["mods", "unapply", r], &logs);
    assert!(ok, "{text}");
    let (ok, text) = mods_run(&["mods", "apply", r], &logs);
    assert!(!ok && text.contains("waits for your consent"), "{text}");
    let (ok, text) = mods_run(&["mods", "apply", r, "--ignore-compatibility"], &logs);
    assert!(!ok, "{text}");
}

/// Stage 103: `check-edits` is the check of the workspace's edits the MCP
/// tool `validate` and the editor make (`doctor_toolkit::validate`).
#[test]
fn check_edits_reports_what_is_wrong_and_fails_on_errors() {
    use doctor_toolkit::testing::{self, SIMGROUP, STRINGS};
    use doctor_toolkit::workspace::{ResourceEdit, StorageChoice, Workspace};

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("game");
    let logs = dir.path().join("logs");
    testing::write_game(&root).unwrap();
    let check = |extra: &[&str]| {
        let mut args: Vec<&std::ffi::OsStr> = vec!["check-edits".as_ref(), root.as_os_str()];
        args.extend(extra.iter().map(|a| std::ffi::OsStr::new(*a)));
        let out = run(&args, &logs);
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
        )
    };
    // No workspace: nothing to check.
    let (ok, text) = check(&[]);
    assert!(ok && text.contains("nothing to check"), "{text}");
    let game = doctor_toolkit::game::GameInstance::open(&root).unwrap();
    let level = game.find_stream(testing::LEVEL).unwrap();
    let mut ws = Workspace::create(Workspace::default_dir(&root), &game).unwrap();
    let mut edit = |index: usize, data: Vec<u8>| {
        let edit = ResourceEdit {
            index,
            data,
            storage: StorageChoice::Keep,
        };
        ws.set_resources(&game, "edit", level, &[edit]).unwrap();
    };
    // A good edit of the string table.
    edit(
        STRINGS,
        testing::sample_table(["Patch", "Text"]).to_bytes().unwrap(),
    );
    let (ok, text) = check(&[]);
    assert!(ok, "{text}");
    assert!(
        text.contains("1 change(s) checked") && text.contains("0 error(s)"),
        "{text}"
    );
    // A SimGroup that cannot be read: an error, a failure, the same in JSON.
    edit(SIMGROUP, vec![1, 2, 3]);
    let (ok, text) = check(&[]);
    assert!(!ok, "{text}");
    assert!(
        text.contains("error: gf2/level:") && text.contains("cannot be read"),
        "{text}"
    );
    assert!(text.contains("2 change(s) checked"), "{text}");
    let (ok, text) = check(&["--json"]);
    assert!(!ok);
    let report: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(report["ok"], false);
    assert_eq!(report["checked_changes"], 2);
    assert_eq!(report["errors"].as_array().unwrap().len(), 1);
    assert!(
        report["errors"][0]["key"]
            .as_str()
            .unwrap()
            .starts_with("gf2/level:")
    );
    // Only the changes whose key has the text.
    let (ok, text) = check(&["--filter", "lh2"]);
    assert!(ok && text.contains("1 change(s) checked"), "{text}");
}

/// Stage 103: `material` lists the parameters of a material and sets them
/// (`doctor_toolkit::resource::material::set_params`, as the editor does).
#[test]
fn material_shows_and_sets_parameters() {
    use doctor_toolkit::resource::material::{Material, ParamValue};
    use doctor_toolkit::testing::{self, MATERIAL, MATERIAL_NAME};
    use doctor_toolkit::workspace::Workspace;

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("game");
    let logs = dir.path().join("logs");
    testing::write_game(&root).unwrap();
    let material = |extra: &[&str]| {
        let mut args: Vec<&std::ffi::OsStr> = vec![
            "material".as_ref(),
            root.as_os_str(),
            testing::LEVEL.as_ref(),
        ];
        args.extend(extra.iter().map(|a| std::ffi::OsStr::new(*a)));
        let out = run(&args, &logs);
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned()
                + &String::from_utf8_lossy(&out.stderr),
        )
    };
    // By name: the parameters as `--set` takes them.
    let (ok, text) = material(&[MATERIAL_NAME]);
    assert!(ok, "{text}");
    assert!(
        text.contains("g_BaseSampler") && text.contains("tex:"),
        "{text}"
    );
    assert!(text.contains("g_Color"), "{text}");
    assert!(
        !Workspace::exists(&Workspace::default_dir(&root)),
        "nothing recorded"
    );
    let game = doctor_toolkit::game::GameInstance::open(&root).unwrap();
    let level = game.find_stream(testing::LEVEL).unwrap();
    let before = game
        .open_stream(level)
        .unwrap()
        .resource_data(MATERIAL)
        .unwrap();
    let ParamValue::Floats(old) = Material::parse(&before).unwrap().params[1].value.clone() else {
        panic!("g_Color is a number");
    };
    let new: Vec<String> = (0..old.len()).map(|i| format!("0.{}", i + 1)).collect();
    // A wrong count of numbers and an unknown parameter are refused.
    let (ok, text) = material(&[MATERIAL_NAME, "--set", "g_Color=1,2,3,4,5,6,7"]);
    assert!(!ok && text.contains("holds"), "{text}");
    let (ok, text) = material(&["2", "--set", "g_Nothing=1"]);
    assert!(!ok && text.contains("no parameter"), "{text}");
    // By index, two parameters in one operation.
    let set = format!("g_Color={}", new.join(","));
    let (ok, text) = material(&["2", "--set", &set, "--set", "g_BaseSampler=tex:0x000000aa"]);
    assert!(ok, "{text}");
    let ws = Workspace::open(Workspace::default_dir(&root)).unwrap();
    assert_eq!(ws.head(), 1, "one operation");
    let m = Material::parse(&ws.current_data(&game, level, MATERIAL).unwrap()).unwrap();
    assert_eq!(m.params[0].value, ParamValue::Texture(0xaa));
    assert_eq!(
        m.floats("g_Color").unwrap(),
        new.iter()
            .map(|v| v.parse::<f32>().unwrap())
            .collect::<Vec<_>>()
    );
    // The same values again: nothing to record.
    let (ok, text) = material(&["2", "--set", &set]);
    assert!(ok && text.contains("nothing changed"), "{text}");
    // Not a material.
    let (ok, text) = material(&["0"]);
    assert!(!ok && text.contains("not a material"), "{text}");
}

/// Stage 103: a zone made with the command line is seen by `level …` and
/// the commands that address resources by stream and index before
/// `workspace apply`, and a model is cloned into it before and after
/// (`level add --clone-model` into a stream the workspace created).
#[test]
fn a_new_zone_is_seen_by_the_level_commands_before_apply() {
    use doctor_toolkit::testing as mini;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("game");
    let logs = dir.path().join("logs");
    std::fs::create_dir_all(&root).unwrap();
    mini::write_game(&root).unwrap();
    mini::write_town(&root).unwrap();
    let zone = "gf2/town/doctor";
    let run_ok = |args: &[&str]| -> String {
        let mut all: Vec<&std::ffi::OsStr> = vec![args[0].as_ref()];
        // The game directory follows the command words.
        let words = if ["level", "world-grid", "workspace"].contains(&args[0]) {
            2
        } else {
            1
        };
        all.extend(args[1..words].iter().map(|a| std::ffi::OsStr::new(*a)));
        all.push(root.as_ref());
        all.extend(args[words..].iter().map(|a| std::ffi::OsStr::new(*a)));
        let out = run(&all, &logs);
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(
            out.status.success(),
            "{args:?}: {text}{}",
            String::from_utf8_lossy(&out.stderr)
        );
        text
    };
    run_ok(&["add-stream", zone, "--parent", mini::TOWN]);
    let made = run_ok(&["add-simgroup", zone]);
    assert!(made.contains("new SimGroup"), "{made}");
    assert!(made.contains(&format!("resource 0 of {zone}")), "{made}");
    run_ok(&[
        "world-grid",
        "add",
        mini::TOWN,
        zone,
        "--min",
        "30,0,30",
        "--max",
        "50,10,50",
    ]);
    // The zone is listed and read before anything is written.
    assert!(!root.join("gf2/town/doctor.str").exists());
    let list = run_ok(&["level", "list"]);
    assert!(list.contains(zone), "{list}");
    assert!(list.contains(mini::YARD), "{list}");
    let empty = run_ok(&["level", "objects", zone]);
    assert!(!empty.contains("Entity"), "{empty}");

    // The crate of the yard as the template, its model cloned with
    // collision of its own into the new zone.
    let yard_model = format!("{}:{}", mini::YARD, mini::YARD_MODEL);
    let yard_group = mini::YARD_SIMGROUP.to_string();
    let add = |group: &str, position: &str| {
        run_ok(&[
            "level",
            "add",
            zone,
            group,
            "0",
            "--template-stream",
            mini::YARD,
            "--template-resource",
            &yard_group,
            "--clone-model",
            &yard_model,
            "--collision",
            "box",
            "--position",
            position,
        ])
    };
    let added = add("0", "40,0,40");
    assert!(added.contains("added object 0"), "{added}");
    assert!(added.contains(&format!("cloned {zone}:hkx:")), "{added}");
    // The clones went before the SimGroup: it is resource 3 now (mesh, its
    // collision, metamodel).
    let info = run_ok(&["level", "info", zone]);
    assert!(info.contains("objects 1:"), "{info}");
    let objects = run_ok(&["level", "objects", zone]);
    assert!(objects.contains("40.0"), "{objects}");
    let copy = run_ok(&["level", "duplicate", zone, "3", "0", "--offset", "2,0,0"]);
    assert!(copy.contains("duplicated as 1"), "{copy}");
    // Resources cloned from the yard, and inside the new stream.
    let cloned = run_ok(&["clone-resource", mini::YARD, "--index", "0", "--to", zone]);
    assert!(cloned.contains(&format!("-> {zone}:mesh:")), "{cloned}");
    run_ok(&["clone-resource", zone, "--index", "0"]);
    // A second SimGroup with a copy of an object of the first.
    let second = run_ok(&[
        "add-simgroup",
        zone,
        "--copy-of",
        &format!("{zone}#5"),
        "--object",
        "1",
    ]);
    assert!(second.contains("with 1 object"), "{second}");
    let refs = run_ok(&["check-refs", zone]);
    assert!(refs.contains("unresolved reference"), "{refs}");
    assert!(!root.join("gf2/town/doctor.str").exists());

    // Written into the game: the same zone, and the same commands go on.
    run_ok(&["workspace", "apply"]);
    assert!(root.join("gf2/town/doctor.str").is_file());
    let objects = run_ok(&["level", "objects", zone]);
    assert!(objects.contains("40.0"), "{objects}");
    let group = {
        let data = std::fs::read(root.join("gf2/town/doctor.str")).unwrap();
        let layout = doctor_toolkit::container::ustream::StreamLayout::parse(&data).unwrap();
        let simgroup = sdbm(b"simgroup");
        layout
            .resources
            .iter()
            .position(|r| r.header.type_id == simgroup)
            .unwrap()
            .to_string()
    };
    let again = add(&group, "44,0,40");
    assert!(again.contains("added object 2"), "{again}");
    run_ok(&["workspace", "apply"]);
}

/// Writes the miniature game with its town into the folder named by
/// `DOCTOR_TOOLKIT_MINI_GAME`, for manual runs and screenshots of the editor
/// on a game that is not the real one:
/// `DOCTOR_TOOLKIT_MINI_GAME=/tmp/mini cargo test -p doctor-toolkit-cli --test cli -- --ignored write_the_miniature_game`.
#[test]
#[ignore = "writes the miniature game into $DOCTOR_TOOLKIT_MINI_GAME"]
fn write_the_miniature_game() {
    use doctor_toolkit::testing as mini;
    let Some(dir) = std::env::var_os("DOCTOR_TOOLKIT_MINI_GAME") else {
        return;
    };
    let dir = PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    mini::write_game(&dir).unwrap();
    mini::write_town(&dir).unwrap();
    std::fs::write(
        dir.join("box.glb"),
        mini::box_glb([2.0, 2.0, 2.0], mini::MATERIAL_NAME),
    )
    .unwrap();
}

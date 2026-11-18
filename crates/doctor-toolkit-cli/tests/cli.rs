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
/// Stage 110: `mods build` stores a changed SimGroup as a delta (format 6)
/// and says so; `--no-deltas` keeps it whole in the older format. Two mods
/// that set one attribute merge: `mods conflicts` names the object and the
/// attribute, `mods apply` writes the lower mod's value.
#[test]
fn mods_with_deltas_build_merge_and_tell_their_conflicts() {
    use doctor_toolkit::resource::simgroup::{CLASS_ENTITY, SimGroup, Value};
    use doctor_toolkit::testing::{self, YARD, YARD_SIMGROUP};
    use doctor_toolkit::workspace::{ResourceEdit, StorageChoice, Workspace};

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("game");
    let logs = dir.path().join("logs");
    std::fs::create_dir_all(&root).unwrap();
    testing::write_game(&root).unwrap();
    testing::write_town(&root).unwrap();
    let game = doctor_toolkit::game::GameInstance::open(&root).unwrap();
    let yard = game.find_stream(YARD).unwrap();
    let group = || {
        let stream = game.open_stream(yard).unwrap();
        SimGroup::parse(&stream.resource_data(YARD_SIMGROUP).unwrap()).unwrap()
    };
    let r = root.to_str().unwrap();
    // Each mod lights the crate of the yard its own way, in its own workspace.
    let build = |id: &str, value: i32, extra: &[&str]| {
        let ws_dir = dir.path().join(format!("ws-{id}"));
        let mut ws = Workspace::create(&ws_dir, &game).unwrap();
        let mut group = group();
        group
            .set_class_attr(0, CLASS_ENTITY, 3, Some(Value::Int(value)))
            .unwrap();
        let edit = ResourceEdit {
            index: YARD_SIMGROUP,
            data: group.encode(),
            storage: StorageChoice::Keep,
        };
        ws.set_resources(&game, "light", yard, &[edit]).unwrap();
        let out = dir.path().join(format!("{id}.zip"));
        let mut args = vec![
            "mods",
            "build",
            r,
            "-o",
            out.to_str().unwrap(),
            "--name",
            id,
            "--dir",
            ws_dir.to_str().unwrap(),
        ];
        args.extend_from_slice(extra);
        let (ok, text) = mods_run(&args, &logs);
        assert!(ok, "{text}");
        (out, text)
    };
    let (a, text) = build("a", 5, &[]);
    assert!(
        text.contains("format 6") && text.contains("1 of 1 resource(s) that have deltas"),
        "{text}"
    );
    let (b, _) = build("b", 7, &[]);
    let (whole, text) = build("whole", 9, &["--no-deltas"]);
    assert!(
        text.contains("format 3") && !text.contains("stored as deltas"),
        "{text}"
    );
    let (ok, text) = mods_run(&["mods", "info", a.to_str().unwrap(), "--resources"], &logs);
    assert!(
        ok && text.contains("format   6")
            && text.contains("1 resource(s) are deltas, 1 operation(s)")
            && text.contains("simgroup delta (format 1), 1 operation(s)"),
        "{text}"
    );
    let (ok, text) = mods_run(&["mods", "info", whole.to_str().unwrap()], &logs);
    assert!(ok && !text.contains("deltas"), "{text}");

    let (ok, text) = mods_run(
        &[
            "mods",
            "install",
            r,
            a.to_str().unwrap(),
            b.to_str().unwrap(),
        ],
        &logs,
    );
    assert!(ok, "{text}");
    let (ok, text) = mods_run(&["mods", "conflicts", r], &logs);
    assert!(
        ok && text.contains("a, b  -> b wins (the mods merge; set by both:)")
            && text.contains("Entity.CMD_SetInteriorLit: the value was 5"),
        "{text}"
    );
    let (ok, text) = mods_run(&["mods", "check", r], &logs);
    assert!(
        ok && text.contains("every enabled mod fits the game"),
        "{text}"
    );
    let (ok, text) = mods_run(&["mods", "apply", r], &logs);
    assert!(ok, "{text}");
    let written = group();
    assert_eq!(
        written.attr(&written.objects[0], CLASS_ENTITY, 3),
        Some(&Value::Int(7))
    );
    let (ok, text) = mods_run(&["mods", "status", r], &logs);
    assert!(ok && text.contains("the game is up to date"), "{text}");
}

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
    // The presets (stage 113): the town at near detail is its yard and the
    // new zone, and a preset is a word of the level commands.
    let presets = run_ok(&["level", "presets"]);
    assert!(
        presets.contains("gf2/town: whole level, near detail"),
        "{presets}"
    );
    let town = run_ok(&["level", "presets", "town", "--streams"]);
    assert!(town.contains(zone) && town.contains(mini::YARD), "{town}");
    assert!(!town.contains("gf2/level"), "{town}");
    let info = run_ok(&["level", "info", "preset:town"]);
    assert!(info.contains("2 zone(s)"), "{info}");

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

/// Stage 107: the own workspace of a project is recorded by the editing
/// commands as any workspace, but `workspace apply` (and `project build
/// --apply`) refuse to write it into the game and say how it gets there:
/// `project install` puts it into the mod manager as the project's mod.
#[test]
fn a_projects_workspace_is_installed_as_a_mod_not_applied() {
    use doctor_toolkit::game::GameInstance;
    use doctor_toolkit::project::{NewDocument, create_document};
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("game");
    let logs = dir.path().join("logs");
    game(&root);
    let project = dir.path().join("Yard Mod");
    create_document(
        &GameInstance::open(&root).unwrap(),
        &project,
        &NewDocument {
            name: "Yard Mod".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let ws = project.join("workspace");
    let (root_s, project_s, ws_s) = (
        root.to_str().unwrap(),
        project.to_str().unwrap(),
        ws.to_str().unwrap(),
    );

    // An editing command given the folder records into the project.
    let out = dir.path().join("text");
    let (ok, text) = mods_run(&["text-export", root_s, "-o", out.to_str().unwrap()], &logs);
    assert!(ok, "{text}");
    let file = json_files(&out).remove(0);
    let mut document: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    let entry = document["entries"][0].as_object_mut().unwrap();
    let column = entry.keys().find(|k| *k != "hash").unwrap().clone();
    entry.insert(column, "Ciao".into());
    std::fs::write(&file, document.to_string()).unwrap();
    let (ok, text) = mods_run(
        &[
            "text-import",
            root_s,
            file.to_str().unwrap(),
            "--workspace-dir",
            ws_s,
        ],
        &logs,
    );
    assert!(ok, "{text}");
    assert!(ws.join("manifest.json").is_file());
    assert!(!root.join("_doctor_toolkit").exists());

    // It is not written into the game by itself.
    let pristine = std::fs::read(root.join("gf2/zone.str")).unwrap();
    let (ok, text) = mods_run(&["workspace", "apply", root_s, "--dir", ws_s], &logs);
    assert!(!ok, "{text}");
    assert!(
        text.contains("a project's own") && text.contains("project install"),
        "{text}"
    );
    let (ok, text) = mods_run(&["project", "build", project_s, "--apply"], &logs);
    assert!(!ok && text.contains("its own workspace"), "{text}");
    assert_eq!(std::fs::read(root.join("gf2/zone.str")).unwrap(), pristine);
    assert!(!root.join("_doctor_toolkit").exists());
    // Its state is still told.
    let (ok, text) = mods_run(&["workspace", "status", root_s, "--dir", ws_s], &logs);
    assert!(ok, "{text}");

    // As a mod: the project's id and name, last and enabled; then written.
    let (ok, text) = mods_run(&["project", "install", project_s], &logs);
    assert!(
        ok && text.contains("installed mod yard-mod (Yard Mod)"),
        "{text}"
    );
    let (ok, text) = mods_run(&["mods", "list", root_s], &logs);
    assert!(ok && text.contains("yard-mod"), "{text}");
    assert_eq!(std::fs::read(root.join("gf2/zone.str")).unwrap(), pristine);
    let (ok, text) = mods_run(&["mods", "apply", root_s], &logs);
    assert!(ok, "{text}");
    assert_ne!(std::fs::read(root.join("gf2/zone.str")).unwrap(), pristine);
    // Installed again, it is the same mod.
    let (ok, text) = mods_run(&["project", "install", project_s], &logs);
    assert!(ok && text.contains("updated mod yard-mod"), "{text}");
}

/// Stage 108: `project base` prints the base of a project, takes it from
/// the mods written into the game, tells what differs in the game (and
/// fails on it when asked) and brings the mods to the base; the mod of the
/// project depends on the base.
#[test]
fn project_base_records_checks_and_brings_the_mods() {
    use doctor_toolkit::game::GameInstance;
    use doctor_toolkit::mods::{ModArchive, ModInfo, ModManager, build_from_workspace};
    use doctor_toolkit::project::{NewDocument, ProjectConfig, create_document};
    use doctor_toolkit::resource::lh2::StringTable;
    use doctor_toolkit::testing::{self, LEVEL, STRINGS, TEXT};
    use doctor_toolkit::workspace::Workspace;

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("game");
    let logs = dir.path().join("logs");
    testing::write_game(&root).unwrap();
    let game = GameInstance::open(&root).unwrap();
    let set_word = |ws: &mut Workspace, place: (&str, usize), word: &str| {
        let stream = game.find_stream(place.0).unwrap();
        let mut table =
            StringTable::parse(&ws.current_data(&game, stream, place.1).unwrap()).unwrap();
        table.strings[0][0] = word.into();
        ws.set_string_table(&game, stream, place.1, &table).unwrap();
    };
    let install = |id: &str, version: &str, place: (&str, usize)| {
        let mut ws =
            Workspace::for_game(&game, Some(&dir.path().join(format!("ws-{id}")))).unwrap();
        set_word(&mut ws, place, id);
        let package = dir.path().join(format!("{id}.zip"));
        let info = ModInfo {
            id: id.into(),
            name: id.into(),
            version: version.into(),
            ..Default::default()
        };
        build_from_workspace(&ws, &info, None, None, &package).unwrap();
        ModManager::open(&game)
            .unwrap()
            .install_file(&game, &package, true)
            .unwrap();
    };
    let project = dir.path().join("Yard Mod");
    let new = NewDocument {
        name: "Yard Mod".into(),
        ..Default::default()
    };
    create_document(&game, &project, &new).unwrap();
    let (r, p) = (root.to_str().unwrap(), project.to_str().unwrap());

    // No base, no mods.
    let (ok, text) = mods_run(&["project", "base", p], &logs);
    assert!(ok && text.contains("yard-mod names no base"), "{text}");
    let (ok, text) = mods_run(&["project", "base", p, "--apply"], &logs);
    assert!(!ok && text.contains("--record"), "{text}");

    // Taken from the game: the mods written into it.
    install("city", "1.1.0", (TEXT, 0));
    install("lights", "1.0", (LEVEL, STRINGS));
    let (ok, text) = mods_run(&["mods", "apply", r], &logs);
    assert!(ok, "{text}");
    let (ok, text) = mods_run(&["project", "base", p, "--record", "--check"], &logs);
    assert!(ok, "{text}");
    assert!(
        text.contains("recorded the base of yard-mod from the game: 2 mod(s)")
            && text.contains("city ^1.1.0, made on 1.1.0 (")
            && text.contains("lights ^1.0, made on 1.0 (")
            && text.contains("exclusive")
            && text.contains("the game holds the base"),
        "{text}"
    );
    let config = ProjectConfig::read(&project).unwrap();
    assert_eq!(config.format, 2);
    assert_eq!(config.base.as_ref().unwrap().mods.len(), 2);

    // The game with other mods: told, and an error when checked.
    install("extra", "1.0", (TEXT, 0));
    let (ok, text) = mods_run(&["mods", "disable", r, "lights"], &logs);
    assert!(ok, "{text}");
    let (ok, text) = mods_run(&["mods", "apply", r], &logs);
    assert!(ok, "{text}");
    let (ok, text) = mods_run(&["project", "base", p], &logs);
    assert!(ok, "{text}");
    assert!(
        text.contains("differs: the base mod `lights` is switched off")
            && text.contains("differs: the mod `extra` is in the game but not in the base")
            && text.contains("--apply"),
        "{text}"
    );
    let (ok, text) = mods_run(&["project", "status", p], &logs);
    assert!(
        ok && text.contains("warning: the base mod `lights` is switched off"),
        "{text}"
    );
    let (ok, text) = mods_run(&["project", "base", p, "--check"], &logs);
    assert!(!ok && text.contains("differ from the base"), "{text}");

    // Brought: the set to go back to is named, and the game holds the base.
    let (ok, text) = mods_run(&["project", "base", p, "--apply", "--check"], &logs);
    assert!(ok, "{text}");
    assert!(
        text.contains("saved as the set `Before Yard Mod`")
            && text.contains("the mods are brought to the base")
            && text.contains("the game holds the base"),
        "{text}"
    );
    let mods = ModManager::open(&game).unwrap();
    let order: Vec<(&str, bool)> = mods
        .mods()
        .iter()
        .map(|m| (m.id.as_str(), m.enabled))
        .collect();
    assert_eq!(order, [("city", true), ("lights", true), ("extra", false)]);
    assert_eq!(mods.sets().unwrap()[0].name, "Before Yard Mod");

    // The mod of the project depends on the base, with its conditions.
    let mut ws = Workspace::for_game(&game, Some(&project.join("workspace"))).unwrap();
    set_word(&mut ws, (TEXT, 0), "Mine");
    drop(ws);
    let out = dir.path().join("yard.zip");
    let (ok, text) = mods_run(&["project", "mod", p, "-o", out.to_str().unwrap()], &logs);
    assert!(ok, "{text}");
    let manifest = ModArchive::open(&out).unwrap().manifest().clone();
    let depends: Vec<(&str, &str)> = manifest
        .depends
        .iter()
        .map(|(id, condition)| (id.as_str(), condition.as_str()))
        .collect();
    assert_eq!(depends, [("city", "^1.1.0"), ("lights", "^1.0")]);

    // A mod of the base that is not installed cannot be brought.
    let (ok, text) = mods_run(&["mods", "uninstall", r, "lights"], &logs);
    assert!(ok, "{text}");
    let (ok, text) = mods_run(&["project", "base", p, "--apply"], &logs);
    assert!(
        !ok && text.contains("`lights` 1.0 is not installed"),
        "{text}"
    );
}

/// Stage 111: the base mod of a project is updated under its edits.
/// `project status` and `project mod` warn, `project rebase --check` lists
/// the stale edits and fails, `project rebase` merges the string table and
/// leaves the mesh for a decision, `--keep` settles it, and `workspace
/// rebase` does the same on the folder of the workspace.
#[test]
fn project_rebase_moves_stale_edits_onto_the_updated_base() {
    use doctor_toolkit::game::GameInstance;
    use doctor_toolkit::mods::{ModInfo, ModManager, build_from_workspace};
    use doctor_toolkit::project::{NewDocument, create_document};
    use doctor_toolkit::resource::lh2::StringTable;
    use doctor_toolkit::testing::{self, LEVEL, MESH, STRINGS};
    use doctor_toolkit::workspace::Workspace;

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("game");
    let logs = dir.path().join("logs");
    testing::write_game(&root).unwrap();
    let game = GameInstance::open(&root).unwrap();
    let level = game.find_stream(LEVEL).unwrap();
    let words = |ws: &Workspace| {
        let data = ws.current_data(&game, level, STRINGS).unwrap();
        StringTable::parse(&data).unwrap()
    };
    // Two builds of the base mod, both made on the unmodified game: the
    // first word of the table of the level, and its mesh.
    let package = |version: &str, word: &str, mesh: u8| {
        let at = dir.path().join(format!("ws-city-{version}"));
        let mut ws = Workspace::for_game(&game, Some(&at)).unwrap();
        let mut table = words(&ws);
        table.strings[0][0] = word.into();
        ws.set_string_table(&game, level, STRINGS, &table).unwrap();
        ws.set_resource(&game, level, MESH, vec![mesh; 40], "mesh")
            .unwrap();
        let file = dir.path().join(format!("city-{version}.zip"));
        let info = ModInfo {
            id: "city".into(),
            name: "City".into(),
            version: version.into(),
            ..Default::default()
        };
        build_from_workspace(&ws, &info, None, None, &file).unwrap();
        file
    };
    let (city_1, city_2) = (package("1.0.0", "City", 2), package("1.1.0", "Town", 5));
    let r = root.to_str().unwrap();
    let install = |file: &Path| {
        ModManager::open(&game)
            .unwrap()
            .install_file(&game, file, true)
            .unwrap();
        let (ok, text) = mods_run(&["mods", "apply", r], &logs);
        assert!(ok, "{text}");
    };
    install(&city_1);

    // The project is made on the game with the base mod and edits both
    // resources the mod changes: the second word, and the mesh.
    let project = dir.path().join("Yard Mod");
    let new = NewDocument {
        name: "Yard Mod".into(),
        ..Default::default()
    };
    create_document(&game, &project, &new).unwrap();
    let p = project.to_str().unwrap();
    let (ok, text) = mods_run(&["project", "base", p, "--record"], &logs);
    assert!(ok, "{text}");
    let ws_dir = project.join("workspace");
    let mut ws = Workspace::for_game(&game, Some(&ws_dir)).unwrap();
    let mut table = words(&ws);
    assert_eq!(table.strings[0], ["City", "World"]);
    table.strings[0][1] = "Mine".into();
    ws.set_string_table(&game, level, STRINGS, &table).unwrap();
    ws.set_resource(&game, level, MESH, vec![3; 30], "my mesh")
        .unwrap();
    drop(ws);
    let (ok, text) = mods_run(&["project", "status", p], &logs);
    assert!(ok && !text.contains("other data"), "{text}");
    let (ok, text) = mods_run(&["project", "rebase", p], &logs);
    assert!(ok && text.contains("nothing to rebase"), "{text}");

    // The base mod is updated.
    install(&city_2);
    let (ok, text) = mods_run(&["project", "status", p], &logs);
    assert!(
        ok && text.contains("warning: 2 edit(s) were made on other data than the game gives now"),
        "{text}"
    );
    let (ok, text) = mods_run(&["project", "rebase", p, "--check"], &logs);
    assert!(!ok, "{text}");
    assert!(
        text.contains("2 change(s) were made on other data than the game gives now")
            && text.contains("lh2: rebased by its operations")
            && text.contains("not merged: keep yours (--keep) or take the game's (--take-base)")
            && text.contains("2 change(s) are stale"),
        "{text}"
    );
    // A mod of the stale edits is built, and says what it does.
    let stale = dir.path().join("stale.zip");
    let (ok, text) = mods_run(&["project", "mod", p, "-o", stale.to_str().unwrap()], &logs);
    assert!(
        ok && text
            .contains("warning: 2 change(s) were made on other data than the mod is built on"),
        "{text}"
    );

    // Rebased: the table is merged, the mesh waits for a decision.
    let (ok, text) = mods_run(&["project", "rebase", p], &logs);
    assert!(!ok, "{text}");
    assert!(
        text.contains("rebased, 1 operation(s) applied")
            && text.contains("it needs your decision")
            && text.contains("recorded as one operation")
            && text.contains("1 change(s) need your decision"),
        "{text}"
    );
    let ws = Workspace::open_read_only(&ws_dir).unwrap();
    assert_eq!(words(&ws).strings[0], ["Town", "Mine"]);
    drop(ws);
    let (ok, text) = mods_run(&["project", "rebase", p, "--keep", "mesh"], &logs);
    assert!(
        ok && text.contains("kept as it is") && text.contains("--record"),
        "{text}"
    );
    let (ok, text) = mods_run(&["project", "rebase", p, "--check"], &logs);
    assert!(ok && text.contains("nothing to rebase"), "{text}");
    let out = dir.path().join("yard.zip");
    let (ok, text) = mods_run(&["project", "mod", p, "-o", out.to_str().unwrap()], &logs);
    assert!(ok && !text.contains("warning"), "{text}");

    // The same on the folder of the workspace: the kept mesh is stale again
    // after an undo, and is given up for the base's.
    let w = ws_dir.to_str().unwrap();
    let (ok, text) = mods_run(&["workspace", "undo", r, "--dir", w], &logs);
    assert!(ok && text.contains("undone: Rebase"), "{text}");
    let (ok, text) = mods_run(&["workspace", "rebase", r, "--dir", w, "--check"], &logs);
    assert!(!ok && text.contains("1 change(s) are stale"), "{text}");
    let (ok, text) = mods_run(
        &["workspace", "rebase", r, "--dir", w, "--take-base", "mesh"],
        &logs,
    );
    assert!(ok && text.contains("the change is removed"), "{text}");
    let ws = Workspace::open_read_only(&ws_dir).unwrap();
    assert_eq!(ws.len(), 1, "the table stays, the mesh is the base's");
    assert_eq!(ws.current_data(&game, level, MESH).unwrap(), [5; 40]);
}

/// Stage 112: a project is made empty with its own workspace, or of the
/// workspace of the game moved into it with its sources; the sources hold
/// the resources the workspace adds and the stream it creates, so a build
/// into an empty workspace makes them again; `project sources` writes what
/// was added since.
#[test]
fn the_sources_of_a_project_hold_what_the_workspace_adds() {
    use doctor_toolkit::game::GameInstance;
    use doctor_toolkit::project::Project;
    use doctor_toolkit::testing as mini;
    use doctor_toolkit::workspace::Workspace;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("game");
    let logs = dir.path().join("logs");
    std::fs::create_dir_all(&root).unwrap();
    mini::write_game(&root).unwrap();
    mini::write_town(&root).unwrap();
    let root_s = root.to_str().unwrap();
    let zone = "gf2/town/doctor";

    // An empty project: its own workspace, no sources, nothing in the game.
    let empty = dir.path().join("Empty Mod");
    let empty_s = empty.to_str().unwrap();
    let (ok, text) = mods_run(
        &["project", "new", root_s, empty_s, "--author", "me"],
        &logs,
    );
    assert!(ok && text.contains("an empty project empty-mod"), "{text}");
    let made = Project::open(&empty).unwrap();
    assert!(made.has_own_workspace() && made.entries().count() == 0);
    assert_eq!(made.config.project.author, "me");
    assert!(!root.join("_doctor_toolkit").exists());
    let (ok, text) = mods_run(&["project", "status", empty_s], &logs);
    assert!(ok && text.contains("0 entries"), "{text}");
    // Types choose among resources; a workspace elsewhere is not its own.
    let other = dir.path().join("other");
    let other_s = other.to_str().unwrap();
    let (ok, text) = mods_run(&["project", "new", root_s, other_s, "--type", "lh2"], &logs);
    assert!(
        !ok && text.contains("an empty project takes none"),
        "{text}"
    );
    let (ok, text) = mods_run(
        &[
            "project",
            "new",
            root_s,
            other_s,
            "--workspace-dir",
            other_s,
        ],
        &logs,
    );
    assert!(!ok && text.contains("its own workspace"), "{text}");
    assert!(!other.exists());
    // Nothing to move yet.
    let (ok, text) = mods_run(
        &["project", "new", root_s, other_s, "--move-workspace"],
        &logs,
    );
    assert!(!ok && text.contains("there is no workspace"), "{text}");

    // Work in the workspace of the game: a new stream with a SimGroup, a
    // resource added to a stream of the game at a place.
    let raw = dir.path().join("sparks.bin");
    std::fs::write(&raw, b"sparks").unwrap();
    for args in [
        vec!["add-stream", root_s, zone, "--parent", mini::TOWN],
        vec!["add-simgroup", root_s, zone],
        vec![
            "add-resource",
            root_s,
            mini::LEVEL,
            "--type",
            "vfx",
            "--data",
            raw.to_str().unwrap(),
            "--ids",
            "00005501",
            "--before",
            "3",
        ],
    ] {
        let (ok, text) = mods_run(&args, &logs);
        assert!(ok, "{args:?}: {text}");
    }
    let game_ws = Workspace::default_dir(&root);
    assert_eq!(Workspace::open(&game_ws).unwrap().changes().count(), 2);

    // Not while its changes are in the files of the game.
    let project = dir.path().join("Yard Mod");
    let project_s = project.to_str().unwrap();
    let (ok, text) = mods_run(&["workspace", "apply", root_s], &logs);
    assert!(ok, "{text}");
    let moved = [
        "project",
        "new",
        root_s,
        project_s,
        "--move-workspace",
        "--no-index",
    ];
    let (ok, text) = mods_run(&moved, &logs);
    assert!(!ok && text.contains("workspace revert"), "{text}");
    assert!(!project.exists() && game_ws.join("manifest.json").is_file());
    let (ok, text) = mods_run(&["workspace", "revert", root_s, "--game"], &logs);
    assert!(ok, "{text}");

    // Moved: the project has the edits and their history, and sources of
    // all of it.
    let (ok, text) = mods_run(&moved, &logs);
    assert!(ok, "{text}");
    assert!(text.contains("2 change(s), 3 operation(s)"), "{text}");
    assert!(
        text.contains(&format!("stream {zone}: described")),
        "{text}"
    );
    assert!(text.contains("2 entries written"), "{text}");
    assert!(!game_ws.exists(), "the workspace of the game is gone");
    let ws_dir = project.join("workspace");
    assert!(ws_dir.join("manifest.json").is_file());
    let manifest =
        std::fs::read_to_string(project.join("src/gf2/town/doctor.str/stream.toml")).unwrap();
    assert!(
        manifest.contains("[new]") && manifest.contains("parent = \"gf2/town\""),
        "{manifest}"
    );
    let level = std::fs::read_to_string(project.join("src/gf2/level.str/stream.toml")).unwrap();
    assert!(
        level.contains("key = \"gf2/level:vfx:00005501\"")
            && level.contains("before = \"gf2/level:mesh:00003333\""),
        "{level}"
    );
    let (ok, text) = mods_run(&["project", "status", project_s], &logs);
    assert!(
        ok && text.contains("2 unchanged, 0 built, 0 modified"),
        "{text}"
    );

    // A checkout of the sources: no workspace, no build cache. The build
    // makes the stream and the resources again, under the same keys.
    let game = GameInstance::open(&root).unwrap();
    let before: Vec<String> = Workspace::open(&ws_dir)
        .unwrap()
        .changes()
        .map(|c| format!("{} {} {:?}", c.key, c.payload.sha256, c.added))
        .collect();
    std::fs::remove_dir_all(&ws_dir).unwrap();
    std::fs::remove_file(project.join(".doctor/build.json")).unwrap();
    let (ok, text) = mods_run(&["project", "status", project_s], &logs);
    assert!(ok && text.contains("`project build` builds 3"), "{text}");
    assert!(text.contains("stream.toml (gf2/town/doctor)"), "{text}");
    let (ok, text) = mods_run(&["project", "build", project_s], &logs);
    assert!(
        ok && text.contains("3 built, 0 up to date, 0 failed"),
        "{text}"
    );
    let ws = Workspace::open(&ws_dir).unwrap();
    let after: Vec<String> = ws
        .changes()
        .map(|c| format!("{} {} {:?}", c.key, c.payload.sha256, c.added))
        .collect();
    assert_eq!(after, before);
    assert!(ws.new_stream(zone).is_some());
    assert_eq!(ws.head(), 1, "one undoable operation");
    drop(ws);
    let (ok, text) = mods_run(&["project", "build", project_s], &logs);
    assert!(ok && text.contains("0 built, 2 up to date"), "{text}");

    // More is added to the project's workspace: `project sources` writes
    // it, and only it.
    let smoke = dir.path().join("smoke.bin");
    std::fs::write(&smoke, b"smoke").unwrap();
    let (ok, text) = mods_run(
        &[
            "add-resource",
            root_s,
            zone,
            "--type",
            "vfx",
            "--data",
            smoke.to_str().unwrap(),
            "--ids",
            "00005502",
            "--workspace-dir",
            ws_dir.to_str().unwrap(),
        ],
        &logs,
    );
    assert!(ok, "{text}");
    let (ok, text) = mods_run(&["project", "sources", project_s, "--no-index"], &logs);
    assert!(ok, "{text}");
    assert!(
        text.contains("src/gf2/town/doctor.str/vfx/00005502.bin: written")
            && text.contains("1 entry written"),
        "{text}"
    );
    assert_eq!(
        std::fs::read(project.join("src/gf2/town/doctor.str/vfx/00005502.bin")).unwrap(),
        b"smoke"
    );
    let (ok, text) = mods_run(&["project", "sources", project_s, "--no-index"], &logs);
    assert!(ok && text.contains("0 entries written"), "{text}");
    // Its file removed: the next build takes the resource out.
    std::fs::remove_file(project.join("src/gf2/town/doctor.str/vfx/00005502.bin")).unwrap();
    let (ok, text) = mods_run(&["project", "status", project_s], &logs);
    assert!(ok && text.contains("removed"), "{text}");
    let (ok, text) = mods_run(&["project", "build", project_s], &logs);
    assert!(ok, "{text}");
    assert_eq!(Workspace::open(&ws_dir).unwrap().changes().count(), 2);
    // A project that builds into the workspace of the game has no
    // workspace of its own to write the sources of.
    let streams = dir.path().join("streams");
    let streams_s = streams.to_str().unwrap();
    let (ok, text) = mods_run(
        &[
            "project",
            "new",
            root_s,
            streams_s,
            "--stream",
            mini::TEXT,
            "--no-index",
        ],
        &logs,
    );
    assert!(ok, "{text}");
    let (ok, text) = mods_run(&["project", "sources", streams_s, "--no-index"], &logs);
    assert!(!ok && text.contains("its own workspace"), "{text}");
    drop(game);
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

//! Commands whose action is a function of the library shared with the
//! editor and the MCP server (stage 104), on the miniature game: the new
//! `tunables`, and the commands that were rewired to the library
//! (`partsdb add-part` and `allow`, `sbk-edit`, `speech-events`,
//! `audio-config`, `mix`, `workspace revert`, `script-run`, `logic-run`,
//! `save`).

use std::path::Path;
use std::process::Command;

use doctor_toolkit::testing::{self, LEVEL, SOUND};

fn run(args: &[&str], logs: &Path) -> (bool, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_doctor-toolkit"))
        .args(args)
        .env("DOCTOR_TOOLKIT_LOG_DIR", logs)
        // A game running elsewhere on the machine (a test bench) does not count.
        .env("DOCTOR_TOOLKIT_GAME_PROCESS", "no-such-game.exe")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    (output.status.success(), text)
}

#[test]
fn commands_over_the_shared_operations() {
    let dir = tempfile::tempdir().unwrap();
    let game = dir.path().join("game");
    std::fs::create_dir_all(&game).unwrap();
    testing::write_game(&game).unwrap();
    testing::write_extra_streams(&game).unwrap();
    testing::write_tunable_globals(&game).unwrap();
    testing::write_sound_stream(&game).unwrap();
    let logs = dir.path().join("logs");
    let ws = dir.path().join("ws");
    let (g, w) = (game.to_str().unwrap(), ws.to_str().unwrap());
    let ok = |args: &[&str]| {
        let (ok, text) = run(args, &logs);
        assert!(ok, "{args:?}: {text}");
        text
    };
    let refused = |args: &[&str]| {
        let (ok, text) = run(args, &logs);
        assert!(!ok, "{args:?}: {text}");
        text
    };

    // Game parameters: the list, one of them, a dry run, a change, bounds.
    let list = ok(&["tunables", g, "--category", "weapons", "--workspace-dir", w]);
    assert!(
        list.contains("weapons.pistol.damage") && list.contains("25 / 26 / 27 / 28"),
        "{list}"
    );
    let dry = ok(&[
        "tunables",
        g,
        "--multiply",
        "weapons.pistol.damage=2",
        "--dry-run",
        "--workspace-dir",
        w,
    ]);
    assert!(
        dry.contains("25 -> 50") && dry.contains("4 value(s) change"),
        "{dry}"
    );
    assert!(!ws.exists(), "a dry run records nothing");
    let set = ok(&[
        "tunables",
        g,
        "--multiply",
        "weapons.pistol.damage=2",
        "--set",
        "weapons.pistol.clip_size=12",
        "--workspace-dir",
        w,
    ]);
    assert!(
        set.contains("8 value(s) change") && set.contains("recorded in workspace"),
        "{set}"
    );
    let one = ok(&["tunables", g, "weapons.pistol.damage", "--workspace-dir", w]);
    assert!(
        one.contains("weapon_pistol_level_04") && one.contains("56"),
        "{one}"
    );
    refused(&[
        "tunables",
        g,
        "--set",
        "weapons.pistol.clip_size=0",
        "--workspace-dir",
        w,
    ]);
    refused(&[
        "tunables",
        g,
        "--set",
        "no.such.thing=1",
        "--workspace-dir",
        w,
    ]);

    // Clothes.
    let added = ok(&[
        "partsdb",
        "add-part",
        g,
        LEVEL,
        "db",
        "--name",
        "mod_cap",
        "--slot",
        "a_hat",
        "--mesh",
        "3333",
        "--set",
        "db",
        "--only",
        "--workspace-dir",
        w,
    ]);
    assert!(
        added.contains("part 3 `mod_cap` in slot a_hat") && added.contains("1 set(s) changed"),
        "{added}"
    );
    let allowed = ok(&[
        "partsdb",
        "allow",
        g,
        LEVEL,
        "db",
        "hat",
        "--workspace-dir",
        w,
    ]);
    assert!(allowed.contains("1 set(s) changed"), "{allowed}");
    assert!(
        ok(&[
            "partsdb",
            "allow",
            g,
            LEVEL,
            "db",
            "hat",
            "--workspace-dir",
            w
        ])
        .contains("no change")
    );
    let shown = ok(&["partsdb", "show", g, LEVEL, "db", "--workspace-dir", w]);
    assert!(shown.contains("mod_cap"), "{shown}");

    // Sound: a bank, the speech events, the mixer.
    let bank = format!("{SOUND}:sbk:00000055");
    let edited = ok(&[
        "sbk-edit",
        g,
        &bank,
        "--emitter",
        "a.emx",
        "--volume",
        "0.5",
        "--workspace-dir",
        w,
    ]);
    assert!(
        edited.contains("Edit sound") && edited.contains("volume"),
        "{edited}"
    );
    refused(&[
        "sbk-edit",
        g,
        &bank,
        "--emitter",
        "a.emx",
        "--workspace-dir",
        w,
    ]);
    let events = ok(&[
        "speech-events",
        g,
        "DOC_NEW",
        "F2F_HELLO",
        "--set",
        "--cooldown",
        "9",
        "--workspace-dir",
        w,
    ]);
    assert!(
        events.contains("DOC_NEW: new record") && events.contains("F2F_HELLO: changed"),
        "{events}"
    );
    let speakers = ok(&["voice-add", g, "--speakers", "--workspace-dir", w]);
    assert!(speakers.contains("1 chatter set(s)"), "{speakers}");
    let category = ok(&[
        "audio-config",
        g,
        "--add-category",
        "DOCTOR_VOICE",
        "--parent",
        "DIALOG",
        "--workspace-dir",
        w,
    ]);
    assert!(category.contains("1 configuration(s)"), "{category}");
    let mixed = ok(&[
        "mix",
        g,
        "occlusion.mix",
        "--category",
        "DOCTOR_VOICE",
        "--volume",
        "0.4",
        "--workspace-dir",
        w,
    ]);
    assert!(mixed.contains("changed in 1 cop(ies)"), "{mixed}");
    refused(&[
        "mix",
        g,
        "occlusion.mix",
        "--category",
        "NO_SUCH",
        "--volume",
        "1",
        "--workspace-dir",
        w,
    ]);

    // One change out again.
    let reverted = ok(&["workspace", "revert", g, "mix", "--dir", w]);
    assert!(reverted.contains("revert gf2/sound:mix"), "{reverted}");
    refused(&["workspace", "revert", g, "nothing-like-this", "--dir", w]);

    // Runs without the game.
    let script = dir.path().join("hello.lua");
    std::fs::write(&script, "print(\"hello\")\nSendMessage(\"Go\")\n").unwrap();
    let tried = ok(&["script-run", script.to_str().unwrap(), "wait:1"]);
    assert!(
        tried.contains("print hello") && tried.contains("send Go"),
        "{tried}"
    );
    let broken = dir.path().join("broken.lua");
    std::fs::write(&broken, "error(\"boom\")\n").unwrap();
    assert!(refused(&["script-run", broken.to_str().unwrap()]).contains("boom"));
    let nodes = ok(&["script-nodes", "--json"]);
    assert!(
        nodes.contains("\"kind\": \"event.load\""),
        "{}",
        &nodes[..200.min(nodes.len())]
    );

    // A save.
    let save = dir.path().join("GF2-EN-autosave");
    std::fs::write(
        &save,
        doctor_toolkit::save::build("Don", "New York", 60, 0, &[]).unwrap(),
    )
    .unwrap();
    let info = ok(&["save", save.to_str().unwrap(), "--json"]);
    assert!(info.contains("\"profile\": \"Don\""), "{info}");
    let list = ok(&["save", dir.path().to_str().unwrap()]);
    assert!(list.contains("GF2-EN-autosave"), "{list}");
}

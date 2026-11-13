//! `doctor-toolkit live` against a fake module (no game needed).

use std::path::Path;
use std::process::{Command, Output};

use doctor_toolkit::live::Config;
use doctor_toolkit::live::fake::FakeGame;

fn run(args: &[&str], log_dir: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_doctor-toolkit"))
        .args(args)
        .env("DOCTOR_TOOLKIT_LOG_DIR", log_dir)
        .env("DOCTOR_TOOLKIT_GAME_PROCESS", "no-such-game.exe")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn fake(writes: bool) -> FakeGame {
    FakeGame::start(Config {
        port: 0,
        token: "tok".into(),
        writes,
        fix_input: true,
        journal: true,
    })
    .unwrap()
}

#[test]
fn reads() {
    let game = fake(false);
    let logs = tempfile::tempdir().unwrap();
    let port = game.port.to_string();
    let link = ["--port", port.as_str(), "--token", "tok"];
    let cmd = |sub: &[&str]| {
        let mut args = vec!["live"];
        args.extend_from_slice(sub);
        args.extend_from_slice(&link);
        run(&args, logs.path())
    };

    let o = cmd(&["status"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let text = stdout(&o);
    assert!(text.contains("main loop: GameMainLoop"), "{text}");
    assert!(text.contains("writes: off"), "{text}");
    assert!(text.contains("time of day: 12:00:00"), "{text}");

    let o = cmd(&["player"]);
    assert!(
        stdout(&o).starts_with("283.900 -0.070 371.800"),
        "{}",
        stdout(&o)
    );

    let o = cmd(&["player", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["position"][0].as_f64().unwrap() as f32, 283.9);

    let o = cmd(&["camera"]);
    assert!(stdout(&o).contains("fov 55.0"), "{}", stdout(&o));

    let o = cmd(&["streams"]);
    assert_eq!(stdout(&o).lines().count(), 2);

    let o = cmd(&["objects", "--radius", "5", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    let list = v.as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["class"], "PlayerSpawn");
    assert!(list[0]["braced"].as_str().unwrap().starts_with('{'));

    let o = cmd(&[
        "objects",
        "--class",
        "VariableSwitch",
        "--at",
        "300,0,400",
        "--radius",
        "100",
    ]);
    assert_eq!(stdout(&o).lines().count(), 1, "{}", stdout(&o));

    let o = cmd(&["peek", "0x00400000", "4"]);
    assert!(
        stdout(&o).starts_with("00400000: 00 01 02 03"),
        "{}",
        stdout(&o)
    );
}

#[test]
fn writes_need_yes_and_permission() {
    let logs = tempfile::tempdir().unwrap();
    // Without --yes: refused before connecting.
    let game = fake(true);
    let port = game.port.to_string();
    let o = run(
        &[
            "live", "teleport", "1,2,3", "--port", &port, "--token", "tok",
        ],
        logs.path(),
    );
    assert!(!o.status.success());
    assert!(stderr(&o).contains("--yes"), "{}", stderr(&o));
    assert_eq!(
        game.world.lock().unwrap().player.unwrap().position,
        [283.9, -0.07, 371.8]
    );

    let o = run(
        &[
            "live", "teleport", "1,-2,3", "--yaw", "90", "--yes", "--port", &port, "--token", "tok",
        ],
        logs.path(),
    );
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(stdout(&o).contains("to (1.00, -2.00, 3.00)"));
    let p = game.world.lock().unwrap().player.unwrap();
    assert_eq!(p.position, [1.0, -2.0, 3.0]);
    assert!((p.forward[0] - 1.0).abs() < 1e-6);

    let o = run(
        &[
            "live",
            "send",
            "doctor_ping",
            "--yes",
            "--port",
            &port,
            "--token",
            "tok",
        ],
        logs.path(),
    );
    assert!(stdout(&o).contains("(3)"), "{}", stdout(&o));
    let o = run(
        &[
            "live",
            "send",
            "doctor_ping",
            "--to",
            "01010101010101010101010101010101",
            "--yes",
            "--port",
            &port,
            "--token",
            "tok",
        ],
        logs.path(),
    );
    assert!(stdout(&o).contains("(1)"), "{}", stdout(&o));

    let o = run(
        &[
            "live",
            "lua",
            "return GetSimTime()",
            "--yes",
            "--port",
            &port,
            "--token",
            "tok",
        ],
        logs.path(),
    );
    assert!(o.status.success());
    let o = run(
        &[
            "live", "lua", "broken", "--yes", "--port", &port, "--token", "tok",
        ],
        logs.path(),
    );
    assert!(!o.status.success());

    // Writes switched off in the module's configuration.
    let read_only = fake(false);
    let port = read_only.port.to_string();
    let o = run(
        &[
            "live", "teleport", "1,2,3", "--yes", "--port", &port, "--token", "tok",
        ],
        logs.path(),
    );
    assert!(!o.status.success());
    assert!(stderr(&o).contains("switched off"), "{}", stderr(&o));
}

#[test]
fn errors_without_the_game() {
    let logs = tempfile::tempdir().unwrap();
    let game = fake(false);
    let port = game.port.to_string();
    // Wrong token.
    let o = run(
        &["live", "status", "--port", &port, "--token", "x"],
        logs.path(),
    );
    assert!(!o.status.success());
    assert!(stderr(&o).contains("wrong token"), "{}", stderr(&o));
    // Another build of the game.
    game.world.lock().unwrap().wrong_edition = true;
    let o = run(
        &["live", "status", "--port", &port, "--token", "tok"],
        logs.path(),
    );
    assert!(!o.status.success());
    assert!(stderr(&o).contains("another build"), "{}", stderr(&o));
    // Nobody listening.
    drop(game);
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = free.local_addr().unwrap().port().to_string();
    drop(free);
    let o = run(
        &["live", "status", "--port", &port, "--timeout", "1"],
        logs.path(),
    );
    assert!(!o.status.success());
    assert!(stderr(&o).contains("no live link"), "{}", stderr(&o));
}

#[test]
fn install_and_uninstall() {
    let logs = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let module = dir.path().join("module.asi");
    std::fs::write(&module, b"MZ fake module").unwrap();
    let game = dir.path().join("game");
    std::fs::create_dir(&game).unwrap();
    std::fs::write(game.join("dinput8.dll"), b"Ultimate ASI Loader").unwrap();
    let g = game.to_str().unwrap();
    let o = run(
        &[
            "live",
            "install",
            g,
            "--module",
            module.to_str().unwrap(),
            "--writes",
            "--port",
            "47999",
        ],
        logs.path(),
    );
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(stdout(&o).contains("Ultimate ASI Loader"), "{}", stdout(&o));
    let cfg = std::fs::read_to_string(game.join("scripts/doctor_live.cfg")).unwrap();
    assert!(
        cfg.contains("port=47999") && cfg.contains("writes=1"),
        "{cfg}"
    );
    // `--game` reads the port and token from it.
    let o = run(
        &["live", "status", "--game", g, "--timeout", "1"],
        logs.path(),
    );
    assert!(stderr(&o).contains("127.0.0.1:47999"), "{}", stderr(&o));

    let o = run(&["live", "uninstall", g], logs.path());
    assert!(stdout(&o).contains("took the module away"));
    assert!(!game.join("scripts").exists());
    assert_eq!(
        std::fs::read_dir(&game).unwrap().count(),
        2,
        "dinput8.dll and _doctor_toolkit"
    );
}

#[test]
fn journal_every_object_and_find() {
    let game = fake(true);
    let logs = tempfile::tempdir().unwrap();
    let port = game.port.to_string();
    let link = ["--port", port.as_str(), "--token", "tok"];
    let cmd = |sub: &[&str]| {
        let mut args = vec!["live"];
        args.extend_from_slice(sub);
        args.extend_from_slice(&link);
        run(&args, logs.path())
    };

    // Something in the journal: a print, an error, a message.
    assert!(
        cmd(&["lua", "print(\"hello journal\")", "--yes"])
            .status
            .success()
    );
    assert!(!cmd(&["lua", "this is bad", "--yes"]).status.success());
    assert!(cmd(&["send", "doctor_ping", "--yes"]).status.success());

    let o = cmd(&["log"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let text = stdout(&o);
    assert!(
        text.contains("print doctor_live:1: hello journal"),
        "{text}"
    );
    assert!(text.contains("lua error: doctor_live:1:"), "{text}");
    assert!(text.contains("    [C]: ?"), "{text}");
    // Messages only when asked for.
    assert!(!text.contains("message"), "{text}");
    let o = cmd(&["log", "--messages"]);
    let text = stdout(&o);
    assert!(
        text.contains("message 0x")
            && text.contains("3 listener(s)")
            && text.contains("(live link)"),
        "{text}"
    );
    let o = cmd(&["log", "--errors"]);
    assert_eq!(stdout(&o).lines().count(), 2, "{}", stdout(&o));
    let o = cmd(&["log", "--filter", "HELLO"]);
    assert_eq!(stdout(&o).lines().count(), 1, "{}", stdout(&o));
    let o = cmd(&["log", "--json", "--messages"]);
    let kinds: Vec<String> = stdout(&o)
        .lines()
        .map(|l| {
            serde_json::from_str::<serde_json::Value>(l).unwrap()["kind"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(kinds, ["output", "lua_error", "message"]);

    let o = cmd(&["objects", "--all"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(stdout(&o).lines().count(), 5, "{}", stdout(&o));
    let o = cmd(&["objects", "--all", "--logic", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    let list = v["objects"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert!(list.iter().all(|o| o["position"].is_null()));
    assert_eq!(list[0]["stream"], "gf2\\cuba.str");
    assert_eq!(list[0]["class"], "VariableSwitch");
    let o = cmd(&["objects", "--all", "--stream", "nowhere"]);
    assert_eq!(stdout(&o).lines().count(), 0);

    let guid = "05".repeat(16);
    let o = cmd(&["find", &guid]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(stdout(&o).contains("MessageRelay"), "{}", stdout(&o));
    let o = cmd(&["find", &"0e".repeat(16)]);
    assert!(!o.status.success());
    // A name needs the SimGroups of a game folder.
    let o = cmd(&["find", "relay_gate"]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("--game"), "{}", stderr(&o));
}

#[test]
fn a_module_without_the_journal() {
    let game = fake(false);
    game.world.lock().unwrap().protocol = 1;
    let logs = tempfile::tempdir().unwrap();
    let port = game.port.to_string();
    let o = run(
        &["live", "log", "--port", &port, "--token", "tok"],
        logs.path(),
    );
    assert!(!o.status.success());
    assert!(stderr(&o).contains("protocol 1"), "{}", stderr(&o));
    // The reads of stage 34 still work.
    let o = run(
        &["live", "status", "--port", &port, "--token", "tok"],
        logs.path(),
    );
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(stdout(&o).contains("(protocol 1)"), "{}", stdout(&o));
}

#[test]
fn breakpoints_pause_freecam_and_textures() {
    let game = fake(true);
    let logs = tempfile::tempdir().unwrap();
    let port = game.port.to_string();
    let link = ["--port", port.as_str(), "--token", "tok"];
    let cmd = |sub: &[&str]| {
        let mut args = vec!["live"];
        args.extend_from_slice(sub);
        args.extend_from_slice(&link);
        run(&args, logs.path())
    };

    let o = cmd(&["break"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(stdout(&o).contains("no breakpoints"), "{}", stdout(&o));
    let o = cmd(&[
        "break",
        "--message",
        "doctor_ping",
        "--to",
        "04040404040404040404040404040404",
        "--hold",
        "30",
    ]);
    assert!(o.status.success(), "{}", stderr(&o));
    let text = stdout(&o);
    assert!(text.contains("break on message"), "{text}");
    assert!(text.contains("at most 30 s"), "{text}");
    // --add keeps the rules set before.
    let o = cmd(&["break", "--lua-error", "--add"]);
    assert!(stdout(&o).contains("break on Lua errors"), "{}", stdout(&o));
    assert_eq!(game.world.lock().unwrap().debug.rules.len(), 2);
    // The game's traffic stops it in the bus.
    game.world.lock().unwrap().activity(0);
    let o = cmd(&["break", "--wait", "--json"]);
    let v: serde_json::Value = serde_json::from_str(stdout(&o).trim()).unwrap();
    assert_eq!(v["stopped"], true);
    assert_eq!(v["place"], "message bus");
    let o = cmd(&["continue"]);
    assert!(stdout(&o).contains("goes on"), "{}", stdout(&o));
    let o = cmd(&["pause"]);
    assert!(stdout(&o).contains("paused"), "{}", stdout(&o));
    let o = cmd(&["continue", "--step"]);
    assert!(stdout(&o).contains("one frame"), "{}", stdout(&o));
    let o = cmd(&["break"]);
    assert!(
        stdout(&o).contains("STOPPED in the frame"),
        "{}",
        stdout(&o)
    );
    cmd(&["continue"]);
    let o = cmd(&["break", "--clear"]);
    assert!(stdout(&o).contains("no breakpoints"), "{}", stdout(&o));
    // The journal's counts of messages.
    let o = cmd(&["log", "--messages"]);
    assert!(
        stdout(&o).contains("messages without own events"),
        "{}",
        stdout(&o)
    );

    // The free camera: a write, needs --yes.
    let o = cmd(&["freecam", "1,2,3", "--yaw", "90"]);
    assert!(!o.status.success());
    let o = cmd(&["freecam", "1,2,3", "--yaw", "90", "--yes"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let o = cmd(&["camera"]);
    assert!(
        stdout(&o).starts_with("1.000 2.000 3.000 looking 1.0000"),
        "{}",
        stdout(&o)
    );
    let o = cmd(&["freecam", "--release", "--yes"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(game.world.lock().unwrap().debug.camera.is_none());

    // Textures come from the workspace of --game.
    let o = cmd(&["texture", "--yes"]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("--game"), "{}", stderr(&o));
}

#[test]
fn a_module_without_debugging() {
    let game = fake(true);
    game.world.lock().unwrap().protocol = 2;
    let logs = tempfile::tempdir().unwrap();
    let port = game.port.to_string();
    let o = run(
        &["live", "pause", "--port", &port, "--token", "tok"],
        logs.path(),
    );
    assert!(!o.status.success());
    assert!(stderr(&o).contains("protocol 2"), "{}", stderr(&o));
    // The rest still answers.
    let o = run(
        &["live", "status", "--port", &port, "--token", "tok"],
        logs.path(),
    );
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(stdout(&o).contains("(protocol 2)"), "{}", stdout(&o));
}

#[test]
fn world_and_state() {
    let game = fake(true);
    let logs = tempfile::tempdir().unwrap();
    let port = game.port.to_string();
    let link = ["--port", port.as_str(), "--token", "tok"];
    let cmd = |sub: &[&str]| {
        let mut args = vec!["live"];
        args.extend_from_slice(sub);
        args.extend_from_slice(&link);
        run(&args, logs.path())
    };

    let o = cmd(&["time"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(
        stdout(&o).starts_with("12:00:00, day 0 of 1950"),
        "{}",
        stdout(&o)
    );
    // A write without --yes is refused before it reaches the game.
    let o = cmd(&["time", "23:30"]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("--yes"), "{}", stderr(&o));
    let o = cmd(&["time", "23:30", "--rate", "0", "--yes"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(
        stdout(&o).contains("23:30:00, day 0 of 1950, 0 game seconds"),
        "{}",
        stdout(&o)
    );
    let o = cmd(&["time", "--default-rate", "--yes", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["rate"], 60.0);
    let o = cmd(&["time", "25:00", "--yes"]);
    assert!(!o.status.success());

    let o = cmd(&["weather"]);
    assert!(stdout(&o).contains("no WeatherManager"), "{}", stdout(&o));

    let o = cmd(&["money", "--add", "12345", "--yes"]);
    assert!(stdout(&o).contains("money: 12345"), "{}", stdout(&o));
    let o = cmd(&["money", "--set", "500", "--yes", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["cash"], 500.0);

    let o = cmd(&["family"]);
    let text = stdout(&o);
    assert!(text.contains("1 (soldier  ) 04040404"), "{text}");
    assert!(text.contains("soldier, medic"), "{text}");
    assert!(text.contains("7 (underboss) locked"), "{text}");
    assert!(text.contains("corleone"), "{text}");

    let o = cmd(&["mission", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert!(v["current"].is_null());
    assert_eq!(v["checkpoints"][0]["number"], 737);

    let o = cmd(&["var", "get", "doctor_fake_flag"]);
    assert!(stdout(&o).contains("0  saved"), "{}", stdout(&o));
    let o = cmd(&["var", "set", "doctor_fake_flag", "-3", "--yes"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(stdout(&o).contains("= -3 (was 0)"), "{}", stdout(&o));
    let o = cmd(&["var", "get", "doctor_fake_flag", "no_such_var"]);
    assert!(!o.status.success());
    assert!(
        stderr(&o).contains("not in the loaded streams"),
        "{}",
        stderr(&o)
    );

    let o = cmd(&[
        "stream",
        "load",
        "gf2/cuba/local/yuri/high/filler_056_01",
        "--wait",
        "--yes",
    ]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(stdout(&o).contains("— done"), "{}", stdout(&o));
    let o = cmd(&["stream", "list"]);
    assert!(
        stdout(&o).contains("gf2\\cuba\\local\\yuri\\high\\filler_056_01"),
        "{}",
        stdout(&o)
    );
    assert!(stdout(&o).contains("link: load"), "{}", stdout(&o));
    let o = cmd(&["stream", "unload", "gf2/cuba", "--yes"]);
    assert!(!o.status.success());
    assert!(
        stderr(&o).contains("not loaded by the link"),
        "{}",
        stderr(&o)
    );
}

#[test]
fn world_and_state_need_a_module_of_protocol_4() {
    let game = fake(true);
    game.world.lock().unwrap().protocol = 3;
    let logs = tempfile::tempdir().unwrap();
    let port = game.port.to_string();
    let o = run(
        &["live", "money", "--port", &port, "--token", "tok"],
        logs.path(),
    );
    assert!(!o.status.success());
    assert!(stderr(&o).contains("protocol 3"), "{}", stderr(&o));
}

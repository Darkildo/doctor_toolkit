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
    assert!(stdout(&o).contains("(2)"), "{}", stdout(&o));
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

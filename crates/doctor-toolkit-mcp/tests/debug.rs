//! Live debugging through the server (stage 104) against the fake game:
//! the journal is read without flags; with `--allow-game` the game is
//! paused, stepped and stopped at breakpoints, a stopped game refuses
//! writes outside a frame and says what to do, the free camera is set and
//! given back, edited textures are sent; crash reports and the copies of
//! the game are read.

mod common;

use std::path::PathBuf;

use serde_json::{Value, json};

use common::{Client, config};
use doctor_toolkit::game::GameInstance;
use doctor_toolkit::live::fake::FakeGame;
use doctor_toolkit::testing::{self, LEVEL, TEX_DATA, TEXTURE_HASH};
use doctor_toolkit::workspace::Workspace;
use doctor_toolkit_mcp::Config;

struct Fixture {
    _dirs: Vec<tempfile::TempDir>,
    game: PathBuf,
    workspace: PathBuf,
    fake: FakeGame,
    token: &'static str,
}

fn fixture() -> Fixture {
    let game_dir = tempfile::tempdir().unwrap();
    let ws_dir = tempfile::tempdir().unwrap();
    testing::write_game(game_dir.path()).unwrap();
    let token = "debug-token";
    let fake = FakeGame::start(doctor_toolkit::live::Config {
        port: 0,
        token: token.into(),
        writes: true,
        fix_input: true,
        journal: true,
        ..Default::default()
    })
    .unwrap();
    Fixture {
        game: game_dir.path().to_path_buf(),
        workspace: ws_dir.path().join("ws"),
        fake,
        token,
        _dirs: vec![game_dir, ws_dir],
    }
}

fn configured(f: &Fixture, edits: bool, game: bool) -> Config {
    let mut c = config(&f.game);
    c.workspace = Some(f.workspace.clone());
    c.allow_edits = edits;
    c.allow_game = game;
    c.live = Some(f.fake.endpoint(f.token));
    c
}

fn start(c: &Config) -> Client {
    Client::start(doctor_toolkit_mcp::open(c).unwrap())
}

fn text(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_journal_reports_and_copies_of_the_game_are_read_without_flags() {
    let f = fixture();
    let mut c = start(&configured(&f, false, false));
    c.initialize().await;
    let tools = c.request("tools/list", json!({})).await;
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for tool in ["game_log", "crash_reports", "detect_games"] {
        assert!(names.contains(&tool), "{tool}");
    }
    for tool in ["pause", "resume", "breakpoints", "camera", "push_textures"] {
        assert!(!names.contains(&tool), "{tool} without --allow-game");
    }

    {
        let mut w = f.fake.world.lock().unwrap();
        w.print("mission started", "missions/intro.lua", 12);
        w.print("dt:intro:3; node traced", "", 0);
    }
    let log = c.json("game_log", json!({})).await;
    let events = log["events"].as_array().unwrap();
    assert_eq!(events.len(), 2, "{log}");
    assert_eq!(events[0]["kind"], "output");
    assert_eq!(events[0]["text"], "mission started");
    assert_eq!(events[0]["script"], "missions/intro.lua");
    let next = log["next"].clone();
    let later = c.json("game_log", json!({ "after": next })).await;
    assert_eq!(later["events"], json!([]), "{later}");
    f.fake.world.lock().unwrap().print("again", "", 0);
    let later = c.json("game_log", json!({ "after": next })).await;
    assert_eq!(later["events"][0]["text"], "again", "{later}");
    let traced = c.json("game_log", json!({ "filter": "DT:INTRO" })).await;
    assert_eq!(traced["events"].as_array().unwrap().len(), 1, "{traced}");
    let errors = c.json("game_log", json!({ "errors": true })).await;
    assert_eq!(errors["events"], json!([]), "{errors}");
    let (error_text, error) = c
        .call("game_log", json!({ "errors": true, "messages": true }))
        .await;
    assert!(error, "{error_text}");

    // No crash yet; the list says where reports would be.
    let reports = c.json("crash_reports", json!({})).await;
    assert_eq!(reports["total"], 0, "{reports}");
    assert_eq!(reports["module"], "not installed");
    let (error_text, error) = c.call("crash_reports", json!({ "report": "1" })).await;
    assert!(error && error_text.contains("no report"), "{error_text}");

    let found = c.json("detect_games", json!({})).await;
    assert!(found["games"].is_array(), "{found}");
    assert!(
        text(&found["this_game"]["path"]).ends_with(f.game.file_name().unwrap().to_str().unwrap()),
        "{found}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_game_is_paused_stepped_and_stopped_at_breakpoints() {
    let f = fixture();
    let mut c = start(&configured(&f, false, true));
    c.initialize().await;

    // Pause: stopped in a frame, where writes are still taken.
    let paused = c.json("pause", json!({})).await;
    assert_eq!(paused["state"]["stopped"], true, "{paused}");
    assert_eq!(paused["state"]["place"], "frame");
    assert!(text(&paused["state"]["note"]).contains("resume"));
    assert!(f.fake.world.lock().unwrap().stopped());
    c.json("teleport", json!({ "position": [1.0, 2.0, 3.0] }))
        .await;
    let frame = paused["state"]["frame"].as_u64().unwrap();
    let stepped = c.json("resume", json!({ "step": true })).await;
    assert_eq!(stepped["state"]["stopped"], true, "{stepped}");
    assert_eq!(stepped["state"]["frame"], frame + 1, "one frame on");
    let running = c.json("resume", json!({})).await;
    assert_eq!(running["state"]["stopped"], false, "{running}");
    assert!(!f.fake.world.lock().unwrap().stopped());

    // A breakpoint on printed text: the game stops in print.
    let set = c
        .json(
            "breakpoints",
            json!({ "prints": ["dt:intro:3;"], "lua_error": true, "hold_s": 30 }),
        )
        .await;
    assert_eq!(set["changed"], true, "{set}");
    assert_eq!(set["state"]["rules"], 2);
    assert_eq!(set["state"]["hold_limit_s"], 30);
    assert_eq!(set["state"]["stopped"], false);
    f.fake
        .world
        .lock()
        .unwrap()
        .print("dt:intro:3; node traced", "", 0);
    let state = c.json("breakpoints", json!({ "wait_s": 5 })).await;
    assert_eq!(state["stopped_while_waiting"], true, "{state}");
    assert_eq!(state["changed"], false);
    assert_eq!(state["state"]["place"], "print", "{state}");
    assert!(state["state"]["lines"].to_string().contains("dt:intro:3"));

    // Stopped outside a frame: reads answer, writes say what to do.
    let player = c.json("player", json!({})).await;
    assert!(player["player"]["position"].is_array(), "{player}");
    for (tool, arguments) in [
        ("teleport", json!({ "position": [0.0, 0.0, 0.0] })),
        ("lua", json!({ "code": "return 1" })),
        ("send_message", json!({ "message": "doctor_ping" })),
        ("set_money", json!({ "set": 5 })),
    ] {
        let (error_text, error) = c.call(tool, arguments).await;
        assert!(
            error && error_text.contains("breakpoint") && error_text.contains("resume"),
            "{tool}: {error_text}"
        );
    }
    c.json("resume", json!({})).await;
    c.json("teleport", json!({ "position": [0.0, 0.0, 0.0] }))
        .await;

    // More rules are added, then all are cleared.
    let added = c
        .json(
            "breakpoints",
            json!({ "messages": ["doctor_ping"], "add": true, "once": true }),
        )
        .await;
    assert_eq!(added["state"]["rules"], 3, "{added}");
    let (error_text, error) = c
        .call(
            "breakpoints",
            json!({ "to": "00112233445566778899aabbccddeeff" }),
        )
        .await;
    assert!(error && error_text.contains("messages"), "{error_text}");
    let (error_text, error) = c
        .call("breakpoints", json!({ "clear": true, "lua_error": true }))
        .await;
    assert!(error, "{error_text}");
    let cleared = c.json("breakpoints", json!({ "clear": true })).await;
    assert_eq!(cleared["state"]["rules"], 0, "{cleared}");
    assert!(f.fake.world.lock().unwrap().debug.rules.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_free_camera_is_set_and_released_and_textures_are_sent() {
    let f = fixture();
    // A workspace that edits the pixels of the level's texture.
    let game = GameInstance::open(&f.game).unwrap();
    let level = game.find_stream(LEVEL).unwrap();
    let mut ws = Workspace::create(&f.workspace, &game).unwrap();
    let mut pixels = ws.current_data(&game, level, TEX_DATA).unwrap();
    for b in &mut pixels {
        *b ^= 0x55;
    }
    let size = pixels.len() as u64;
    ws.set_resource(&game, level, TEX_DATA, pixels, "Repaint")
        .unwrap();
    drop(ws);
    let mut c = start(&configured(&f, false, true));
    c.initialize().await;

    // The camera: from a point, looking at another.
    let set = c
        .json(
            "camera",
            json!({ "position": [10.0, 5.0, 0.0], "look_at": [10.0, 5.0, 20.0] }),
        )
        .await;
    assert_eq!(set["free_camera"], true, "{set}");
    assert_eq!(set["position"], json!([10.0, 5.0, 0.0]));
    assert!(
        (set["forward"][2].as_f64().unwrap() - 1.0).abs() < 1e-4,
        "{set}"
    );
    let seen = c.json("player", json!({})).await;
    assert_eq!(
        seen["camera"]["pose"]["position"],
        json!([10.0, 5.0, 0.0]),
        "{seen}"
    );
    let turned = c
        .json(
            "camera",
            json!({ "yaw_degrees": 90.0, "pitch_degrees": 0.0 }),
        )
        .await;
    assert_eq!(
        turned["position"],
        json!([10.0, 5.0, 0.0]),
        "kept: {turned}"
    );
    assert!((turned["forward"][0].as_f64().unwrap() - 1.0).abs() < 1e-4);
    let (error_text, error) = c
        .call(
            "camera",
            json!({ "look_at": [0.0, 0.0, 0.0], "yaw_degrees": 1.0 }),
        )
        .await;
    assert!(error, "{error_text}");
    let state = c.json("breakpoints", json!({})).await;
    assert_eq!(state["state"]["free_camera"], true, "{state}");
    let released = c.json("camera", json!({ "release": true })).await;
    assert_eq!(released["free_camera"], false, "{released}");
    assert!(f.fake.world.lock().unwrap().debug.camera.is_none());

    // Textures: the game has not loaded it, then it has.
    let sent = c.json("push_textures", json!({})).await;
    assert_eq!(sent["failed"], 1, "{sent}");
    assert!(text(&sent["textures"][0]["error"]).contains("no loaded texture"));
    f.fake
        .world
        .lock()
        .unwrap()
        .textures
        .push((TEXTURE_HASH, size));
    let sent = c
        .json(
            "push_textures",
            json!({ "name": format!("{TEXTURE_HASH:#010x}") }),
        )
        .await;
    assert_eq!(sent["failed"], 0, "{sent}");
    assert_eq!(
        f.fake.world.lock().unwrap().replaced,
        vec![(TEXTURE_HASH, size as usize)]
    );
    let (error_text, error) = c
        .call("push_textures", json!({ "name": "0x12345678" }))
        .await;
    assert!(error && error_text.contains("no texture"), "{error_text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_spawn_point_at_a_position_needs_a_stream_and_the_edits_flag() {
    let f = fixture();
    let mut c = start(&configured(&f, false, true));
    c.initialize().await;
    let (error_text, error) = c.call("play", json!({ "position": [1.0, 2.0, 3.0] })).await;
    assert!(error && error_text.contains("stream"), "{error_text}");
    let (error_text, error) = c
        .call(
            "play",
            json!({ "stream": "gf2/level", "position": [1.0, 2.0, 3.0], "spawn": "x" }),
        )
        .await;
    assert!(error, "{error_text}");
    let (error_text, error) = c
        .call(
            "play",
            json!({ "stream": "gf2/level", "position": [1.0, 2.0, 3.0] }),
        )
        .await;
    assert!(
        error && error_text.contains("--allow-edits"),
        "{error_text}"
    );
    let (error_text, error) = c
        .call(
            "play",
            json!({ "stream": "gf2/level", "position": [1.0, 2.0, 3.0], "apply": false }),
        )
        .await;
    assert!(error && error_text.contains("workspace"), "{error_text}");
    // With the flag: the miniature level has no PlayerSpawn to copy.
    let mut c = start(&configured(&f, true, true));
    c.initialize().await;
    let (error_text, error) = c
        .call(
            "play",
            json!({ "stream": "gf2/level", "position": [1.0, 2.0, 3.0] }),
        )
        .await;
    assert!(error && error_text.contains("PlayerSpawn"), "{error_text}");
    assert!(!f.workspace.exists(), "nothing was recorded");
}

//! The writing tools on the miniature game (stage 51): each edit is one
//! undoable operation of the workspace with its diff, undo takes it back,
//! the game files stay as they were; the flags turn the tools on; the lock
//! of the workspace keeps the editor's workspace safe; concurrent calls are
//! serialized; the game is written, run, photographed and stopped with
//! stand-ins for the game, its X server and its live module.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};

use common::{Client, config, logic_group, snapshot};
use doctor_toolkit::game::GameInstance;
use doctor_toolkit::live::fake::FakeGame;
use doctor_toolkit::play::LaunchPlan;
use doctor_toolkit::testing::{self, LEVEL, NAV, SIMGROUP};
use doctor_toolkit::workspace::{Workspace, WorkspaceLock};
use doctor_toolkit_mcp::Config;
use doctor_toolkit_mcp::game::XServer;

const EDIT_TOOLS: [&str; 16] = [
    "set_attribute",
    "move_object",
    "duplicate_object",
    "add_object",
    "remove_object",
    "edit_string",
    "add_string",
    "edit_script",
    "scripts",
    "graph_edit",
    "undo",
    "redo",
    "project_build",
    "mod_build",
    "validate",
    // Not an edit tool: listed to check the count below.
    "workspace_diff",
];

/// Tools of a server without flags (reading, the live link, the editor).
const READ_TOOLS: usize = 20;

const GAME_TOOLS: [&str; 13] = [
    "apply",
    "unapply",
    "play",
    "stop",
    "screenshot",
    "teleport",
    "send_message",
    "lua",
    "set_time",
    "set_money",
    "set_variable",
    "stream_load",
    "stream_unload",
];

struct Fixture {
    _dirs: Vec<tempfile::TempDir>,
    game: PathBuf,
    workspace: PathBuf,
}

/// The miniature game with a navigation graph, and a workspace whose level
/// SimGroup has logic (a trigger at (10, 2, -5), a manager, a spawner).
fn fixture() -> Fixture {
    let game_dir = tempfile::tempdir().unwrap();
    let ws_dir = tempfile::tempdir().unwrap();
    testing::write_game(game_dir.path()).unwrap();
    testing::write_extra_streams(game_dir.path()).unwrap();
    let game = GameInstance::open(game_dir.path()).unwrap();
    let workspace = ws_dir.path().join("ws");
    let mut ws = Workspace::create(&workspace, &game).unwrap();
    let level = game.find_stream(LEVEL).unwrap();
    ws.set_resource(&game, level, SIMGROUP, logic_group().encode(), "New logic")
        .unwrap();
    Fixture {
        game: game_dir.path().to_path_buf(),
        workspace,
        _dirs: vec![game_dir, ws_dir],
    }
}

fn configured(f: &Fixture, edits: bool, game: bool) -> Config {
    let mut c = config(&f.game);
    c.workspace = Some(f.workspace.clone());
    c.allow_edits = edits;
    c.allow_game = game;
    c
}

fn start(config: &Config) -> Client {
    Client::start(doctor_toolkit_mcp::open(config).unwrap())
}

/// The game's own files (streams, tables of contents, the executable).
fn game_files(dir: &Path) -> Vec<(String, (u64, std::time::SystemTime, u64))> {
    snapshot(dir)
        .into_iter()
        .filter(|(path, _)| !path.starts_with("_doctor_toolkit"))
        .collect()
}

async fn tool_names(c: &mut Client) -> Vec<String> {
    let tools = c.request("tools/list", json!({})).await;
    tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_flags_turn_the_writing_tools_on() {
    let f = fixture();
    // Read-only: no writing tool, the live link is read.
    let mut c = start(&configured(&f, false, false));
    let init = c.initialize().await;
    assert!(
        init["instructions"]
            .as_str()
            .unwrap()
            .contains("only reads")
    );
    let names = tool_names(&mut c).await;
    assert_eq!(names.len(), READ_TOOLS, "{names:?}");
    for t in EDIT_TOOLS.iter().chain(&GAME_TOOLS) {
        assert!(
            *t == "workspace_diff" || !names.iter().any(|n| n == t),
            "{t}"
        );
    }
    let r = c
        .request(
            "tools/call",
            json!({ "name": "set_attribute", "arguments": { "object": "gf2/level#4:1", "values": {} } }),
        )
        .await;
    assert!(
        r["error"].is_object() || r["result"]["isError"] == json!(true),
        "{r}"
    );

    // Edits only.
    let mut c = start(&configured(&f, true, false));
    let init = c.initialize().await;
    assert!(
        init["instructions"]
            .as_str()
            .unwrap()
            .contains("--allow-edits")
    );
    let tools = c.request("tools/list", json!({})).await;
    let tools = tools["result"]["tools"].as_array().unwrap().clone();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    for t in EDIT_TOOLS {
        assert!(names.contains(&t), "{t} missing");
    }
    for t in GAME_TOOLS {
        assert!(!names.contains(&t), "{t} without --allow-game");
    }
    assert_eq!(names.len(), READ_TOOLS + EDIT_TOOLS.len() - 1);
    for t in &tools {
        let name = t["name"].as_str().unwrap();
        let read_only = ["scripts", "validate"].contains(&name)
            || !EDIT_TOOLS.contains(&name)
                && !["editor_open", "editor_show_object"].contains(&name)
            || name == "workspace_diff";
        assert_eq!(t["annotations"]["readOnlyHint"], read_only, "{t}");
        assert_eq!(t["annotations"]["openWorldHint"], false, "{t}");
    }

    // Game only.
    let mut c = start(&configured(&f, false, true));
    c.initialize().await;
    let names = tool_names(&mut c).await;
    for t in GAME_TOOLS {
        assert!(names.iter().any(|n| n == t), "{t} missing");
    }
    assert!(!names.iter().any(|n| n == "set_attribute"));
    let destructive: Vec<String> = c.request("tools/list", json!({})).await["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["annotations"]["destructiveHint"] == json!(true))
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect();
    for t in ["apply", "unapply", "stop", "send_message", "lua"] {
        assert!(destructive.iter().any(|d| d == t), "{t}: {destructive:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn every_edit_is_one_undoable_operation_and_the_game_is_untouched() {
    let f = fixture();
    let before = game_files(&f.game);
    let mut c = start(&configured(&f, true, false));
    c.initialize().await;
    let ops = |dir: &Path| Workspace::open_read_only(dir).unwrap().head();
    let mut head = ops(&f.workspace);
    assert_eq!(head, 1);

    // Each tool: the answer has its operation and diff; undo takes it back
    // to the diff before; redo brings it again.
    let calls: Vec<(&str, Value, &str)> = vec![
        (
            "set_attribute",
            json!({ "object": "gf2/level#4:1", "values": { "MultiManager.3": "Spawn_All" } }),
            "Spawn_All",
        ),
        (
            "move_object",
            json!({ "object": "gf2/level#4:0", "position": [1.0, 2.0, 3.0] }),
            "gf2/level:simgroup",
        ),
        (
            "duplicate_object",
            json!({ "object": "gf2/level#4:0", "offset": [1.0, 0.0, 0.0] }),
            "4 object(s) instead of 1",
        ),
        (
            "add_object",
            json!({ "group": "gf2/level#4", "template": "gf2/level#4:2", "position": [5.0, 0.0, 5.0] }),
            "object(s) instead of",
        ),
        (
            "remove_object",
            json!({ "object": "gf2/level#4:2" }),
            "object(s) instead of",
        ),
        (
            "edit_string",
            json!({ "entry": "0x00000001", "text": "Oui" }),
            "Oui",
        ),
        (
            "add_string",
            json!({ "name": "doctor_greeting", "text": "Hi", "tables": ["type:lh2 stream:gf2/text"] }),
            "added string",
        ),
        (
            "graph_edit",
            json!({ "graph": format!("{NAV}:graph:6e617601.67726170.68313233.34353637"), "attach": [[5.0, 0.0, 1.0]] }),
            "byte(s) differ",
        ),
        (
            "edit_script",
            json!({
                "stream": LEVEL,
                "graph": {
                    "format": "doctor-toolkit-script", "version": 1, "name": "hello",
                    "start": { "on_load": true },
                    "nodes": [
                        { "id": 1, "kind": "event.message", "pos": [0, 0], "params": { "message": "doctor_ping" } },
                        { "id": 2, "kind": "action.send_message", "pos": [260, 0], "inputs": { "message": "doctor_pong" } }
                    ],
                    "links": [{ "from": [1, "then"], "to": [2, "exec"] }]
                }
            }),
            "lua",
        ),
    ];
    for (tool, args, expect) in calls {
        let diff_before = c.json("workspace_diff", json!({})).await;
        let answer = c.json(tool, args.clone()).await;
        assert_eq!(answer["changed"], true, "{tool}: {answer}");
        assert!(
            answer["game_files"].as_str().unwrap().contains("unchanged"),
            "{tool}"
        );
        // The diff is against the game files (the fixture changed the group already).
        assert!(
            !answer["diff"].as_array().unwrap().is_empty(),
            "{tool}: {answer}"
        );
        assert!(
            answer.to_string().contains(expect),
            "{tool}: `{expect}` not in {answer}"
        );
        assert_eq!(ops(&f.workspace), head + 1, "{tool}: one operation");
        head += 1;
        let undone = c.json("undo", json!({})).await;
        assert_eq!(undone["changed"], true, "{tool}: {undone}");
        let diff_after = c.json("workspace_diff", json!({})).await;
        assert_eq!(
            diff_before["changes"], diff_after["changes"],
            "{tool}: undo"
        );
        let redone = c.json("redo", json!({})).await;
        assert!(redone["done"].as_str().unwrap().starts_with("redone"));
        assert_eq!(ops(&f.workspace), head, "{tool}: redo");
    }
    // Edits build on each other; the workspace knows each.
    let status = c.json("workspace_status", json!({})).await;
    assert_eq!(status["recent_operations"].as_array().unwrap().len(), 10);
    let scripts = c.json("scripts", json!({ "stream": LEVEL })).await;
    assert!(scripts["graphs"]["hello"].is_object(), "{scripts}");
    // Removing the script and a string edit that changes nothing.
    let removed = c
        .json("edit_script", json!({ "stream": LEVEL, "remove": "hello" }))
        .await;
    assert_eq!(removed["changed"], true, "{removed}");
    let same = c
        .json(
            "edit_string",
            json!({ "entry": "0x00000001", "text": "Oui" }),
        )
        .await;
    assert_eq!(same["changed"], false);
    // Errors say what to do and record nothing.
    for (tool, args, want) in [
        (
            "set_attribute",
            json!({ "object": "gf2/level#4:1", "values": { "Nope.1": "x" } }),
            "no packet of class",
        ),
        (
            "move_object",
            json!({ "object": "gf2/level#4:1", "position": [0.0, 0.0, 0.0] }),
            "no transform",
        ),
        (
            "remove_object",
            json!({ "object": "gf2/level#4" }),
            "<stream>#<resource>:<object>",
        ),
        (
            "edit_string",
            json!({ "entry": "nothing_like_this", "text": "x" }),
            "no string table",
        ),
        (
            "graph_edit",
            json!({ "graph": format!("{NAV}:graph:6e617601.67726170.68313233.34353637") }),
            "nothing to change",
        ),
        (
            "edit_script",
            json!({ "stream": LEVEL, "graph": { "name": "bad", "nodes": [{ "id": 1, "kind": "no.such.node" }] } }),
            "graph",
        ),
        (
            "move_object",
            json!({ "object": "../../etc#0:0", "position": [0.0, 0.0, 0.0] }),
            "not a stream name",
        ),
    ] {
        let head = ops(&f.workspace);
        let (text, error) = c.call(tool, args).await;
        assert!(error, "{tool}: {text}");
        assert!(text.contains(want), "{tool}: `{want}` not in {text}");
        assert_eq!(ops(&f.workspace), head, "{tool} recorded on error");
    }
    // Checks and builds.
    let checked = c.json("validate", json!({})).await;
    assert_eq!(checked["ok"], true, "{checked}");
    let built = c.json("mod_build", json!({ "name": "Test edits" })).await;
    let file = PathBuf::from(built["file"].as_str().unwrap());
    assert!(file.starts_with(&f.workspace), "{file:?}");
    assert!(file.is_file());
    assert!(built["resources"].as_u64().unwrap() >= 3, "{built}");
    let (text, error) = c.call("project_build", json!({})).await;
    assert!(error && text.contains("--project"), "{text}");

    assert_eq!(before, game_files(&f.game), "the game files changed");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_workspace_open_in_the_editor_is_not_written() {
    let f = fixture();
    let mut c = start(&configured(&f, true, false));
    c.initialize().await;
    let editor = WorkspaceLock::acquire(&f.workspace, "the editor").unwrap();
    let manifest = std::fs::read(f.workspace.join("manifest.json")).unwrap();
    let (text, error) = c
        .call(
            "move_object",
            json!({ "object": "gf2/level#4:0", "offset": [1.0, 0.0, 0.0] }),
        )
        .await;
    assert!(error, "{text}");
    assert!(
        text.contains("open for writing in the editor (process"),
        "{text}"
    );
    assert!(text.contains("one program at a time"), "{text}");
    let (text, error) = c.call("undo", json!({})).await;
    assert!(error && text.contains("the editor"), "{text}");
    assert_eq!(
        manifest,
        std::fs::read(f.workspace.join("manifest.json")).unwrap()
    );
    // Reading goes on.
    let status = c.json("workspace_status", json!({})).await;
    assert_eq!(status["counts"]["changes"], 1);
    // The editor closes the game: the edit goes through.
    drop(editor);
    let moved = c
        .json(
            "move_object",
            json!({ "object": "gf2/level#4:0", "offset": [1.0, 0.0, 0.0] }),
        )
        .await;
    assert_eq!(moved["changed"], true);
    // While the server writes, it holds the lock; between calls it does not.
    assert_eq!(WorkspaceLock::holder(&f.workspace), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_calls_are_written_one_after_another() {
    let f = fixture();
    let kit = doctor_toolkit_mcp::open(&configured(&f, true, false)).unwrap();
    let mut clients = Vec::new();
    for _ in 0..3 {
        let mut c = Client::start(kit.clone());
        c.initialize().await;
        clients.push(c);
    }
    let mut tasks = Vec::new();
    for (i, mut c) in clients.into_iter().enumerate() {
        tasks.push(tokio::spawn(async move {
            for k in 0..4 {
                let answer = c
                    .json(
                        "move_object",
                        json!({ "object": "gf2/level#4:0", "offset": [1.0, 0.0, 0.0] }),
                    )
                    .await;
                assert_eq!(answer["changed"], true, "client {i} call {k}: {answer}");
                c.json("workspace_status", json!({})).await;
            }
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    let ws = Workspace::open_read_only(&f.workspace).unwrap();
    assert_eq!(ws.head(), 1 + 12, "every move is one operation");
    let game = GameInstance::open(&f.game).unwrap();
    let level = game.find_stream(LEVEL).unwrap();
    let group = doctor_toolkit::level::edit::current_group(
        &game,
        &ws,
        doctor_toolkit::level::Location {
            stream: level,
            resource: SIMGROUP,
        },
    )
    .unwrap();
    let m = group.transform(&group.objects[0]).unwrap();
    assert!((m[3][0] - 22.0).abs() < 1e-4, "10 + 12 moves: {m:?}");
}

/// A stand-in for the game: runs until stopped.
fn stand_in_game(f: &Fixture) -> LaunchPlan {
    LaunchPlan {
        program: "sh".into(),
        args: vec!["-c".into(), "exec sleep 60".into()],
        env: Vec::new(),
        dir: f.game.clone(),
        detached: false,
    }
}

/// A stand-in X server that prints a display number and waits.
fn stand_in_server() -> XServer {
    XServer::Custom(
        "sh".into(),
        vec!["-c".into(), "echo 99; exec sleep 60".into()],
    )
}

fn stream_files(dir: &Path) -> Vec<(String, (u64, std::time::SystemTime, u64))> {
    game_files(dir)
        .into_iter()
        .filter(|(p, _)| p.ends_with(".str") || p.ends_with(".toc"))
        .map(|(p, (size, _, hash))| (p, (size, std::time::UNIX_EPOCH, hash)))
        .collect()
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn the_game_is_written_run_and_stopped_and_the_live_link_acts_in_it() {
    let f = fixture();
    let original = stream_files(&f.game);
    let token = "test-token";
    let fake = FakeGame::start(doctor_toolkit::live::Config {
        port: 0,
        token: token.into(),
        writes: true,
        fix_input: true,
        journal: true,
    })
    .unwrap();
    let mut config = configured(&f, true, true);
    config.launch = Some(stand_in_game(&f));
    config.x_server = Some(stand_in_server());
    config.live = Some(fake.endpoint(token));
    let kit = doctor_toolkit_mcp::open(&config).unwrap();
    let mut c = Client::start(kit.clone());
    c.initialize().await;

    // apply and unapply.
    let applied = c.json("apply", json!({})).await;
    assert!(
        applied["streams"].to_string().contains("gf2/level"),
        "{applied}"
    );
    assert_ne!(original, stream_files(&f.game), "apply wrote the stream");
    let reverted = c.json("unapply", json!({})).await;
    assert!(
        reverted["streams"].to_string().contains("restored"),
        "{reverted}"
    );
    assert_eq!(original, stream_files(&f.game), "unapply put it back");

    // play: the workspace in, the game on its X server, loaded per the live link.
    let started = c
        .json("play", json!({ "stream": "gf2/level", "timeout_s": 20 }))
        .await;
    assert_eq!(started["started"], true, "{started}");
    assert_eq!(started["wait"]["loaded"], true, "{started}");
    assert_eq!(started["game"]["display"], ":99", "{started}");
    assert_ne!(
        original,
        stream_files(&f.game),
        "play applied the workspace"
    );
    assert!(f.game.join("cmd.txt").is_file(), "cmd.txt for -stream");
    let (text, error) = c.call("play", json!({})).await;
    assert!(error && text.contains("stop"), "{text}");
    let (text, error) = c.call("apply", json!({})).await;
    assert!(error && text.contains("running"), "{text}");

    // The live link reads and writes.
    let status = c.json("live_status", json!({})).await;
    assert_eq!(status["link"], true, "{status}");
    assert_eq!(status["game"]["state"], "running", "{status}");
    let player = c.json("player", json!({})).await;
    assert!(player["player"]["position"].is_array(), "{player}");
    let near = c.json("objects_near", json!({ "radius": 500.0 })).await;
    let guid = near["objects"][0]["guid"].as_str().unwrap().to_owned();
    let moved = c
        .json(
            "teleport",
            json!({ "position": [1.0, 2.0, 3.0], "yaw_degrees": 90.0 }),
        )
        .await;
    assert_eq!(
        moved["after"]["position"],
        json!([1.0, 2.0, 3.0]),
        "{moved}"
    );
    let sent = c
        .json("send_message", json!({ "message": "doctor_ping" }))
        .await;
    assert!(sent["received_by"].as_u64().is_some(), "{sent}");
    let sent = c
        .json(
            "send_message",
            json!({ "message": "doctor_ping", "target": guid }),
        )
        .await;
    assert!(sent["received_by"].is_number(), "{sent}");
    let (text, error) = c
        .call(
            "send_message",
            json!({ "message": "x", "target": "not-a-guid" }),
        )
        .await;
    assert!(error && text.contains("GUID"), "{text}");
    let lua = c.json("lua", json!({ "code": "return 1 + 1" })).await;
    assert!(lua["ok"].is_boolean(), "{lua}");

    // The stand-in X server cannot be photographed: the error says why.
    let (text, error) = c.call("screenshot", json!({})).await;
    assert!(error && text.contains(":99"), "{text}");

    // stop: the game ends, cmd.txt and the workspace go out of the game.
    let stopped = c.json("stop", json!({})).await;
    assert_eq!(stopped["stopped"], true, "{stopped}");
    assert!(!f.game.join("cmd.txt").exists());
    assert_eq!(
        original,
        stream_files(&f.game),
        "stop reverted the workspace"
    );
    let (text, error) = c.call("stop", json!({})).await;
    assert!(error && text.contains("no game"), "{text}");
    let (text, error) = c.call("screenshot", json!({})).await;
    assert!(error && text.contains("play"), "{text}");

    // A client that goes away takes its game along.
    c.json("play", json!({ "wait_loaded": false, "apply": false }))
        .await;
    assert!(kit.session.lock().unwrap().is_some());
    kit.shutdown();
    assert!(kit.session.lock().unwrap().is_none());
    assert_eq!(original, stream_files(&f.game));
}

#[tokio::test(flavor = "multi_thread")]
async fn writes_into_the_live_game_need_the_flag_and_the_module() {
    let f = fixture();
    let token = "t";
    let read_only_module = FakeGame::start(doctor_toolkit::live::Config {
        port: 0,
        token: token.into(),
        writes: false,
        fix_input: true,
        journal: true,
    })
    .unwrap();
    // Without --allow-game: reading only.
    let mut config = configured(&f, false, false);
    config.live = Some(read_only_module.endpoint(token));
    let mut c = start(&config);
    c.initialize().await;
    let status = c.json("live_status", json!({})).await;
    assert_eq!(status["writes_allowed_by_server"], false);
    let r = c
        .request(
            "tools/call",
            json!({ "name": "teleport", "arguments": { "position": [0, 0, 0] } }),
        )
        .await;
    assert!(
        r["error"].is_object() || r["result"]["isError"] == json!(true),
        "{r}"
    );
    // With it, the module still has the last word.
    config.allow_game = true;
    let mut c = start(&config);
    c.initialize().await;
    let (text, error) = c
        .call("teleport", json!({ "position": [0.0, 0.0, 0.0] }))
        .await;
    assert!(error && text.contains("does not allow writes"), "{text}");
    // No link at all: a word on how to get one.
    let mut config = configured(&f, false, false);
    config.live = Some(doctor_toolkit::live::Endpoint {
        port: 1,
        token: String::new(),
    });
    let mut c = start(&config);
    c.initialize().await;
    let status = c.json("live_status", json!({})).await;
    assert_eq!(status["link"], false);
    assert!(
        status["why"].as_str().unwrap().contains("live module"),
        "{status}"
    );
}

/// With Xvfb installed: the game's X server is real and a frame comes back
/// as a PNG image (black: the stand-in draws nothing). Nothing is shown on
/// the desktop.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_screenshot_is_a_png_of_the_games_own_screen() {
    let Some(server) = XServer::find() else {
        eprintln!("skipped: no Xvfb");
        return;
    };
    let f = fixture();
    let mut config = configured(&f, false, true);
    config.launch = Some(stand_in_game(&f));
    config.x_server = Some(server);
    let kit: Arc<_> = doctor_toolkit_mcp::open(&config).unwrap();
    let mut c = Client::start(kit);
    c.initialize().await;
    let started = c
        .json(
            "play",
            json!({ "wait_loaded": false, "apply": false, "size": [640, 360] }),
        )
        .await;
    // Its own X server, never the desktop's: Xvfb takes the first free
    // number, which is `:0` only where no desktop runs (a CI runner).
    let display = started["game"]["display"].as_str().unwrap();
    assert!(display.starts_with(':'), "{started}");
    if let Ok(desktop) = std::env::var("DISPLAY") {
        assert_ne!(display, desktop, "{started}");
    }
    let response = c
        .request(
            "tools/call",
            json!({ "name": "screenshot", "arguments": { "max_width": 320 } }),
        )
        .await;
    let content = response["result"]["content"].as_array().unwrap().clone();
    assert_eq!(content.len(), 2, "{response}");
    let text = content[0]["text"].as_str().unwrap();
    assert!(text.contains("\"width\": 320"), "{text}");
    assert_eq!(content[1]["type"], "image");
    assert_eq!(content[1]["mimeType"], "image/png");
    let data = content[1]["data"].as_str().unwrap();
    assert!(data.starts_with("iVBORw0KGgo"), "a PNG in base64");
    c.json("stop", json!({})).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_world_and_state_of_the_live_game() {
    let f = fixture();
    let token = "t";
    let module = FakeGame::start(doctor_toolkit::live::Config {
        port: 0,
        token: token.into(),
        writes: true,
        fix_input: true,
        journal: true,
    })
    .unwrap();
    // Reading needs no flag.
    let mut config = configured(&f, false, false);
    config.live = Some(module.endpoint(token));
    let mut c = start(&config);
    c.initialize().await;
    let state = c.json("game_state", json!({})).await;
    assert_eq!(state["time_of_day"]["time"], "12:00:00", "{state}");
    assert_eq!(state["money"], 0.0, "{state}");
    assert_eq!(state["crew"][0]["specialties"], json!(["medic"]), "{state}");
    assert!(state["rain"].is_null(), "{state}");
    let vars = c
        .json(
            "variables",
            json!({ "names": ["doctor_fake_flag", "no_such"] }),
        )
        .await;
    assert_eq!(vars["count"], 1, "{vars}");
    assert_eq!(vars["missing"], json!(["no_such"]), "{vars}");
    let r = c
        .request(
            "tools/call",
            json!({ "name": "set_money", "arguments": { "set": 5.0 } }),
        )
        .await;
    assert!(
        r["error"].is_object() || r["result"]["isError"] == json!(true),
        "{r}"
    );

    // With --allow-game: the writes.
    config.allow_game = true;
    let mut c = start(&config);
    c.initialize().await;
    let t = c
        .json("set_time", json!({ "time": "23:30", "rate": 0.0 }))
        .await;
    assert_eq!(t["time_of_day"]["time"], "23:30:00", "{t}");
    assert_eq!(t["time_of_day"]["rate"], 0.0, "{t}");
    let (text, error) = c.call("set_time", json!({ "time": "25:00" })).await;
    assert!(error && text.contains("time of day"), "{text}");
    let m = c.json("set_money", json!({ "add": 1500.0 })).await;
    assert_eq!(m["money"], 1500.0, "{m}");
    let (text, error) = c.call("set_money", json!({})).await;
    assert!(error && text.contains("set"), "{text}");
    let v = c
        .json(
            "set_variable",
            json!({ "name": "doctor_fake_flag", "value": 4 }),
        )
        .await;
    assert_eq!(v["value"], 4, "{v}");
    let s = c
        .json(
            "stream_load",
            json!({ "name": "gf2/cuba/local/yuri/high/filler_056_01" }),
        )
        .await;
    assert_eq!(s["request"]["done"], true, "{s}");
    let (text, error) = c.call("stream_unload", json!({ "name": "gf2/cuba" })).await;
    assert!(error && text.contains("not loaded by the link"), "{text}");
    let s = c
        .json(
            "stream_unload",
            json!({ "name": "gf2/cuba/local/yuri/high/filler_056_01" }),
        )
        .await;
    assert_eq!(s["request"]["done"], true, "{s}");
    assert_eq!(module.world.lock().unwrap().money.cash, 1500.0);
}

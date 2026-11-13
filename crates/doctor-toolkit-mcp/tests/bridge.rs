//! The bridge with the open editor (stage 51), with a stand-in editor on
//! the socket: selection and showing things, and a write the "user"
//! allows or refuses while the editor holds the workspace.
#![cfg(unix)]

mod common;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;

use common::{Client, config, logic_group};
use doctor_toolkit::bridge::{EDITOR, Reply, Request, Server};
use doctor_toolkit::game::GameInstance;
use doctor_toolkit::testing::{self, LEVEL, SIMGROUP};
use doctor_toolkit::workspace::{Workspace, WorkspaceLock};

/// What the stand-in editor saw and does.
struct Editor {
    lock: Mutex<Option<WorkspaceLock>>,
    asked: Mutex<Vec<String>>,
    allow: bool,
    returned: Mutex<usize>,
}

/// Answers on `server` as the editor would, until the test ends.
fn run_editor(server: Server, editor: Arc<Editor>, ws: PathBuf) {
    std::thread::spawn(move || {
        loop {
            for i in server.incoming() {
                let reply = match i.request {
                    Request::Selection => Reply::ok(json!({ "level_object": "gf2/level#4:1" })),
                    Request::Open { key } => Reply::ok(json!({ "shown": key })),
                    Request::ShowObject { object } => {
                        Reply::ok(json!({ "shown": object, "view": "level" }))
                    }
                    Request::ShowLogic { object } => {
                        Reply::ok(json!({ "shown": object, "view": "logic" }))
                    }
                    Request::Lend { summary, .. } => {
                        editor.asked.lock().unwrap().push(summary);
                        if editor.allow {
                            editor.lock.lock().unwrap().take();
                            Reply::ok(json!({ "allowed": true }))
                        } else {
                            Reply::error("refused in the editor")
                        }
                    }
                    Request::Return => {
                        *editor.returned.lock().unwrap() += 1;
                        *editor.lock.lock().unwrap() =
                            Some(WorkspaceLock::acquire(&ws, EDITOR).unwrap());
                        Reply::ok(json!({ "taken_back": true }))
                    }
                    Request::Hello => Reply::ok(json!({})),
                };
                i.reply.send(reply).unwrap();
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    });
}

struct Fixture {
    _dirs: Vec<tempfile::TempDir>,
    game: PathBuf,
    workspace: PathBuf,
    socket: PathBuf,
}

fn fixture() -> Fixture {
    let game_dir = tempfile::tempdir().unwrap();
    let ws_dir = tempfile::tempdir().unwrap();
    testing::write_game(game_dir.path()).unwrap();
    let game = GameInstance::open(game_dir.path()).unwrap();
    let workspace = ws_dir.path().join("ws");
    let mut ws = Workspace::create(&workspace, &game).unwrap();
    let level = game.find_stream(LEVEL).unwrap();
    ws.set_resource(&game, level, SIMGROUP, logic_group().encode(), "New logic")
        .unwrap();
    Fixture {
        game: game_dir.path().to_path_buf(),
        socket: ws_dir.path().join("editor.sock"),
        workspace,
        _dirs: vec![game_dir, ws_dir],
    }
}

fn start(f: &Fixture, allow: bool) -> (Client, Arc<Editor>) {
    let editor = Arc::new(Editor {
        lock: Mutex::new(Some(WorkspaceLock::acquire(&f.workspace, EDITOR).unwrap())),
        asked: Mutex::new(Vec::new()),
        allow,
        returned: Mutex::new(0),
    });
    let server = Server::start(&f.socket, || {}).unwrap();
    run_editor(server, editor.clone(), f.workspace.clone());
    let mut c = config(&f.game);
    c.workspace = Some(f.workspace.clone());
    c.allow_edits = true;
    c.bridge = Some(f.socket.clone());
    (Client::start(doctor_toolkit_mcp::open(&c).unwrap()), editor)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_editor_shows_and_lends_its_workspace_when_allowed() {
    let f = fixture();
    let (mut c, editor) = start(&f, true);
    c.initialize().await;
    let selection = c.json("editor_selection", json!({})).await;
    assert_eq!(selection["level_object"], "gf2/level#4:1");
    let shown = c
        .json("editor_open", json!({ "resource": "type:partsdb" }))
        .await;
    assert!(
        shown["shown"].as_str().unwrap().contains(":partsdb:"),
        "{shown}"
    );
    let shown = c
        .json(
            "editor_show_object",
            json!({ "object": "gf2/level#4:1", "view": "logic" }),
        )
        .await;
    assert_eq!(shown["view"], "logic");
    let (text, error) = c
        .call("editor_show_object", json!({ "object": "gf2/level#4" }))
        .await;
    assert!(
        error && text.contains("<stream>#<resource>:<object>"),
        "{text}"
    );

    // The editor holds the workspace: the write is asked for there, allowed,
    // recorded, and the workspace goes back to the editor.
    let moved = c
        .json(
            "move_object",
            json!({ "object": "gf2/level#4:0", "offset": [1.0, 0.0, 0.0] }),
        )
        .await;
    assert_eq!(moved["changed"], true, "{moved}");
    assert_eq!(
        *editor.asked.lock().unwrap(),
        ["move_object: gf2/level#4:0"]
    );
    assert_eq!(*editor.returned.lock().unwrap(), 1);
    assert_eq!(
        WorkspaceLock::holder(&f.workspace).unwrap().program,
        EDITOR,
        "the editor has it again"
    );
    assert_eq!(Workspace::open_read_only(&f.workspace).unwrap().head(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_write_refused_in_the_editor_records_nothing() {
    let f = fixture();
    let (mut c, editor) = start(&f, false);
    c.initialize().await;
    let (text, error) = c
        .call(
            "set_attribute",
            json!({ "object": "gf2/level#4:1", "values": { "MultiManager.3": "x" } }),
        )
        .await;
    assert!(error, "{text}");
    assert!(text.contains("not allowed there"), "{text}");
    assert_eq!(editor.asked.lock().unwrap().len(), 1);
    assert!(editor.asked.lock().unwrap()[0].starts_with("set_attribute: MultiManager.3"));
    assert_eq!(Workspace::open_read_only(&f.workspace).unwrap().head(), 1);
    assert_eq!(*editor.returned.lock().unwrap(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn without_an_editor_the_bridge_tools_say_so() {
    let f = fixture();
    let mut c = config(&f.game);
    c.workspace = Some(f.workspace.clone());
    c.bridge = Some(f.socket.clone());
    let mut c = Client::start(doctor_toolkit_mcp::open(&c).unwrap());
    c.initialize().await;
    let (text, error) = c.call("editor_selection", json!({})).await;
    assert!(error && text.contains("does not answer"), "{text}");
}

//! The server on the real game.
//!
//! Set `EARS_GF2_DIR` to the game directory to run it; without it the test
//! passes without doing anything:
//!
//! ```sh
//! EARS_GF2_DIR="/path/to/The Godfather II" cargo test -p doctor-toolkit-mcp --test corpus
//! ```
//!
//! The game index comes from its usual cache (`~/.cache/doctor-toolkit/`),
//! as with the command line. The game folder is only read: the test checks
//! that no file of it changed.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::{Value, json};

use common::Client;

fn game_dir() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("EARS_GF2_DIR")?);
    assert!(dir.is_dir(), "EARS_GF2_DIR does not point to a directory");
    Some(dir)
}

/// Size and modification time of every file under `dir` (hashing the whole
/// game would take minutes).
fn stat_all(dir: &Path) -> BTreeMap<PathBuf, (u64, std::time::SystemTime)> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let meta = e.metadata().unwrap();
            if meta.is_dir() {
                stack.push(e.path());
            } else {
                out.insert(e.path(), (meta.len(), meta.modified().unwrap()));
            }
        }
    }
    out
}

/// The JSON head and the body of a `resource_json` result.
fn split(data: &str) -> (Value, &str) {
    let (head, body) = data.split_once("\n-----").expect("head and body");
    let body = body.split_once("-----\n").expect("body").1;
    (serde_json::from_str(head).unwrap(), body)
}

#[tokio::test(flavor = "multi_thread")]
async fn search_simgroup_json_and_logic_graph_on_the_game() {
    let Some(dir) = game_dir() else {
        return;
    };
    let before = stat_all(&dir);
    let started = Instant::now();
    let mut config = doctor_toolkit_mcp::Config::new(&dir);
    config.bridge = None;
    let kit = doctor_toolkit_mcp::open(&config).unwrap();
    let mut c = Client::start(kit);
    c.initialize().await;

    let info = c.json("game_info", json!({})).await;
    assert_eq!(info["game"]["streams"], 2300, "{info}");
    assert!(info["index"]["resources"].as_u64().unwrap() > 200_000);
    eprintln!("game_info after {:.1} s", started.elapsed().as_secs_f64());

    // Search: SimGroups of a district, strings.
    let groups = c
        .json(
            "search",
            json!({ "query": "type:simgroup stream:miami/local/bonita", "limit": 5 }),
        )
        .await;
    assert!(groups["total"].as_u64().unwrap() >= 5, "{groups}");
    let key = groups["results"][0]["key"].as_str().unwrap().to_owned();
    let text = c
        .json("search", json!({ "query": "text:Corleone", "limit": 3 }))
        .await;
    assert!(text["total"].as_u64().unwrap() >= 1, "{text}");
    let strings = c
        .json("strings", json!({ "text": "Corleone", "limit": 5 }))
        .await;
    assert!(strings["total"].as_u64().unwrap() >= 5, "{strings}");

    // A SimGroup as JSON, in pages.
    let first = c
        .data(
            "resource_json",
            json!({ "resource": key, "max_chars": 4000 }),
        )
        .await;
    let (head, _) = split(&first);
    assert_eq!(head["type"], "simgroup");
    let total = head["total_chars"].as_u64().unwrap();
    let objects = c
        .data(
            "resource_json",
            json!({ "resource": key, "pointer": "/objects/0" }),
        )
        .await;
    let (_, body) = split(&objects);
    let object: Value = serde_json::from_str(body).unwrap();
    assert!(object.is_object(), "{object}");
    eprintln!("{key}: {total} chars of JSON");

    // resource_info of that SimGroup: what it uses.
    let info = c.json("resource_info", json!({ "resource": key })).await;
    assert_eq!(info["type"], "simgroup");

    // Other views: a dialogue, a Lua script, a material.
    for (query, want) in [
        ("type:dlg", "nodes"),
        ("type:lua", "----- lua -----"),
        ("type:material", "shaders"),
    ] {
        let found = c
            .json("search", json!({ "query": query, "limit": 1 }))
            .await;
        let k = found["results"][0]["key"].as_str().unwrap().to_owned();
        let data = c
            .data("resource_json", json!({ "resource": k, "max_chars": 3000 }))
            .await;
        assert!(data.contains(want), "{query}: {data}");
    }

    // The logic graph: who hears a message of the engine, and from one of
    // them two hops in both directions.
    let t = Instant::now();
    let msg = c
        .json(
            "logic_graph",
            json!({ "message": "iMsgNewCheckpointLoaded", "limit": 5 }),
        )
        .await;
    eprintln!("message index in {:.1} s", t.elapsed().as_secs_f64());
    assert_eq!(msg["found"], true);
    assert_eq!(msg["named_by_engine"], true);
    assert!(msg["listeners"]["total"].as_u64().unwrap() >= 1, "{msg}");
    let start = msg["listeners"]["items"][0]["object"]
        .as_str()
        .unwrap()
        .to_owned();
    let graph = c
        .json(
            "logic_graph",
            json!({ "object": start, "depth": 2, "limit": 150 }),
        )
        .await;
    assert_eq!(graph["start"]["object"], start);
    assert!(
        !graph["start"]["pins"].as_array().unwrap().is_empty(),
        "{graph}"
    );
    eprintln!(
        "{start}: {} object(s), {} edge(s)",
        graph["objects"].as_array().unwrap().len(),
        graph["edges"].as_array().unwrap().len()
    );
    let mission = info_missions(&mut c).await;
    let scope = c
        .json(
            "logic_graph",
            json!({ "scope": format!("mission:{mission}"), "limit": 20 }),
        )
        .await;
    assert!(scope["overview"]["nodes"].as_u64().unwrap() > 0, "{scope}");

    // Objects of a level stream with positions.
    let objects = c
        .json(
            "level_objects",
            json!({ "stream": "gf2/miami/local/bonita", "limit": 20 }),
        )
        .await;
    assert!(objects["total"].as_u64().unwrap() > 20, "{objects}");
    let placed = objects["objects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["position"].is_array())
        .expect("a placed object")
        .clone();
    let near = c
        .json(
            "level_objects",
            json!({ "stream": "gf2/miami/local/bonita", "near": placed["position"], "radius": 5.0 }),
        )
        .await;
    assert!(near["total"].as_u64().unwrap() >= 1);

    // Hashes, the workspace (if the game has one), resources.
    let hash = c.json("lookup_hash", json!({ "hash": "0x38523fc3" })).await;
    assert!(
        hash["hash"]["names"].to_string().contains("Entity"),
        "{hash}"
    );
    c.json("workspace_status", json!({})).await;
    c.json("workspace_diff", json!({ "limit": 5 })).await;
    let r = c
        .request(
            "resources/read",
            json!({ "uri": "doctor-toolkit://game/missions" }),
        )
        .await;
    assert!(
        r["result"]["contents"][0]["text"]
            .as_str()
            .unwrap()
            .contains(&mission)
    );
    eprintln!("all calls in {:.1} s", started.elapsed().as_secs_f64());

    assert_eq!(before, stat_all(&dir), "the game folder changed");
}

/// The name of a mission with logic.
async fn info_missions(c: &mut Client) -> String {
    let r = c
        .request(
            "resources/read",
            json!({ "uri": "doctor-toolkit://game/missions" }),
        )
        .await;
    let text = r["result"]["contents"][0]["text"].as_str().unwrap();
    let data = common::unfence(text, "game/missions");
    let missions: Vec<Value> = serde_json::from_str(&data).unwrap();
    assert!(missions.len() > 10);
    missions[0]["name"].as_str().unwrap().to_owned()
}

/// Copies the workspace in `from` (the owner's, only read) to `to`.
fn copy_workspace(from: &Path, to: &Path) {
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        std::fs::create_dir_all(to.join(&rel)).unwrap();
        for e in std::fs::read_dir(from.join(&rel)).unwrap().flatten() {
            let name = e.file_name();
            if name == "lock" || name == "lock.holder" {
                continue;
            }
            let rel = rel.join(&name);
            if e.metadata().unwrap().is_dir() {
                stack.push(rel);
            } else {
                std::fs::copy(from.join(&rel), to.join(&rel)).unwrap();
            }
        }
    }
}

/// Stage 51 on the real game: every writing tool on a copy of the
/// workspace in a temporary folder (never the one in the game folder),
/// one operation each, undone at the end; the game folder is not touched.
#[tokio::test(flavor = "multi_thread")]
async fn edits_on_the_game_go_into_a_temporary_workspace() {
    let Some(dir) = game_dir() else {
        return;
    };
    let before = stat_all(&dir);
    let started = Instant::now();
    let temp = tempfile::tempdir().unwrap();
    let ws_dir = temp.path().join("workspace");
    let owners = doctor_toolkit::workspace::Workspace::default_dir(&dir);
    if doctor_toolkit::workspace::Workspace::exists(&owners) {
        copy_workspace(&owners, &ws_dir);
    }
    let head = |d: &Path| {
        if doctor_toolkit::workspace::Workspace::exists(d) {
            doctor_toolkit::workspace::Workspace::open_read_only(d)
                .unwrap()
                .head()
        } else {
            0
        }
    };
    let start_head = head(&ws_dir);
    let mut config = doctor_toolkit_mcp::Config::new(&dir);
    config.bridge = None;
    config.workspace = Some(ws_dir.clone());
    config.allow_edits = true;
    let kit = doctor_toolkit_mcp::open(&config).unwrap();
    let mut c = Client::start(kit);
    c.initialize().await;
    let mut expected = start_head;
    let step = async |c: &mut Client, tool: &str, args: Value| -> Value {
        let answer = c.json(tool, args.clone()).await;
        assert_eq!(answer["changed"], true, "{tool} {args}: {answer}");
        assert!(!answer["diff"].as_array().unwrap().is_empty(), "{answer}");
        answer
    };

    // Objects of a district: move, copy, remove the copy.
    let objects = c
        .json(
            "level_objects",
            json!({ "stream": "gf2/miami/local/bonita/high/terrain010", "limit": 50 }),
        )
        .await;
    let placed = objects["objects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["position"].is_array())
        .expect("a placed object")
        .clone();
    let id = placed["object"].as_str().unwrap().to_owned();
    step(
        &mut c,
        "move_object",
        json!({ "object": id, "offset": [0.0, 1.0, 0.0] }),
    )
    .await;
    let copy = step(
        &mut c,
        "duplicate_object",
        json!({ "object": id, "offset": [2.0, 0.0, 0.0] }),
    )
    .await;
    let copy = copy["done"]["copy"].as_str().unwrap().to_owned();
    step(&mut c, "remove_object", json!({ "object": copy })).await;
    expected += 3;

    // An attribute of a MultiManager of the always-loaded stream.
    let managers = c
        .json(
            "level_objects",
            json!({ "stream": "gf2/globals_global", "filter": "MultiManager", "limit": 1 }),
        )
        .await;
    let manager = managers["objects"][0]["object"]
        .as_str()
        .unwrap()
        .to_owned();
    let set = step(
        &mut c,
        "set_attribute",
        json!({ "object": manager, "values": { "MultiManager.3": "doctor_mcp_corpus" } }),
    )
    .await;
    assert!(set.to_string().contains("doctor_mcp_corpus"), "{set}");
    expected += 1;

    // Strings: change one in one table and language, add one everywhere.
    let fredo = c
        .json(
            "strings",
            json!({ "text": "Fredo", "language": "text", "limit": 1 }),
        )
        .await;
    let s = &fredo["strings"][0];
    let edited = step(
        &mut c,
        "edit_string",
        json!({
            "entry": format!("0x{}", s["hash"].as_str().unwrap()),
            "table": s["table"],
            "language": s["language"],
            "text": "Fredo (edited by the MCP corpus test)",
        }),
    )
    .await;
    assert_eq!(edited["done"]["changes"].as_array().unwrap().len(), 1);
    let added = step(
        &mut c,
        "add_string",
        json!({ "name": "doctor_mcp_corpus", "text": "Hello from MCP" }),
    )
    .await;
    assert!(
        added["done"]["tables"].as_array().unwrap().len() >= 2,
        "{added}"
    );
    expected += 2;

    // A navigation graph of the district: a node attached near its first.
    let graphs = c
        .json(
            "search",
            json!({ "query": "type:graph stream:miami/local/bonita", "limit": 1 }),
        )
        .await;
    let graph = graphs["results"][0]["key"].as_str().unwrap().to_owned();
    let node = c
        .data(
            "resource_json",
            json!({ "resource": graph, "pointer": "/nodes/0/position" }),
        )
        .await;
    let (_, body) = split(&node);
    let p: Vec<f32> = serde_json::from_str(body.trim()).unwrap();
    let edited = step(
        &mut c,
        "graph_edit",
        json!({ "graph": graph, "attach": [[p[0] + 1.0, p[1], p[2] + 1.0]] }),
    )
    .await;
    assert!(
        edited["done"]["problems"].as_array().unwrap().is_empty(),
        "{edited}"
    );
    expected += 1;

    // A script made from a graph, in the always-loaded stream.
    let script = step(
        &mut c,
        "edit_script",
        json!({
            "stream": "gf2/globals_global",
            "graph": {
                "format": "doctor-toolkit-script", "version": 1, "name": "doctor_mcp_corpus",
                "start": { "on_load": false, "message": "doctor_mcp_ping" },
                "nodes": [
                    { "id": 1, "kind": "event.message", "pos": [0, 0], "params": { "message": "doctor_mcp_ping" } },
                    { "id": 2, "kind": "action.send_message", "pos": [260, 0], "inputs": { "message": "doctor_mcp_pong" } }
                ],
                "links": [{ "from": [1, "then"], "to": [2, "exec"] }]
            }
        }),
    )
    .await;
    assert!(
        script["done"]["lua"]
            .as_str()
            .unwrap()
            .contains("doctor_mcp_pong")
    );
    expected += 1;
    assert_eq!(head(&ws_dir), expected, "one operation per call");

    // Checks and a mod of the edits.
    let checked = c.json("validate", json!({})).await;
    assert!(
        checked["checked_changes"].as_u64().unwrap() >= 6,
        "{checked}"
    );
    eprintln!("validate: {}", checked["errors"]);
    let built = c
        .json("mod_build", json!({ "name": "MCP corpus test" }))
        .await;
    assert!(
        Path::new(built["file"].as_str().unwrap()).starts_with(&ws_dir),
        "{built}"
    );

    // Everything undone: the workspace is the copy again.
    while head(&ws_dir) > start_head {
        let undone = c.json("undo", json!({})).await;
        assert_eq!(undone["changed"], true, "{undone}");
    }
    eprintln!(
        "edits, checks, mod and undo on the game in {:.1} s",
        started.elapsed().as_secs_f64()
    );
    assert_eq!(before, stat_all(&dir), "the game folder changed");
}

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
    let kit = doctor_toolkit_mcp::open(&doctor_toolkit_mcp::Config::new(&dir)).unwrap();
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

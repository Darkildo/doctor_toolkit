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

/// Stage 57 on the real game: a new level area made only through the
/// server, in an empty temporary workspace — a stream next to the airport
/// gates of Havana, a SimGroup in it, a wall imported from glTF with box
/// collision on a template of the zone, an object showing it; `validate`
/// finds no error and the mod holds the new stream. The game folder is not
/// touched.
#[tokio::test(flavor = "multi_thread")]
async fn a_new_area_is_made_through_the_server() {
    let Some(dir) = game_dir() else {
        return;
    };
    const ZONE: &str = "gf2/cuba/local/yuri/medium/zone4";
    let before = stat_all(&dir);
    let started = Instant::now();
    let temp = tempfile::tempdir().unwrap();
    let wall = temp.path().join("wall.glb");
    std::fs::write(
        &wall,
        doctor_toolkit::testing::box_glb([6.0, 3.0, 0.4], "doctor_wall"),
    )
    .unwrap();
    let out = temp.path().join("out");
    let mut config = doctor_toolkit_mcp::Config::new(&dir);
    config.bridge = None;
    config.workspace = Some(temp.path().join("workspace"));
    config.output_dir = Some(out.clone());
    config.allow_edits = true;
    let kit = doctor_toolkit_mcp::open(&config).unwrap();
    let mut c = Client::start(kit);
    c.initialize().await;

    let stream = c
        .json(
            "add_stream",
            json!({ "name": "gf2/doctor/area57", "parent": ZONE }),
        )
        .await;
    assert!(
        !stream["done"]["tocs"].as_array().unwrap().is_empty(),
        "{stream}"
    );
    let group = c
        .json("add_simgroup", json!({ "stream": "gf2/doctor/area57" }))
        .await;
    let key = group["done"]["simgroup"].as_str().unwrap().to_owned();
    let model = c
        .json(
            "import_model",
            json!({
                "file": wall,
                "template": format!("{ZONE}:metamodel:a6379114"),
                "name": "doctor_wall",
                "stream": "gf2/doctor/area57",
                "collision": "box",
            }),
        )
        .await;
    let summary = &model["done"]["summary"];
    assert!(summary["collision"].is_object(), "{model}");
    assert!(
        summary["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|w| !w.as_str().unwrap().contains("not loaded before")),
        "{model}"
    );
    let id = model["done"]["model"].as_str().unwrap().to_owned();
    // A Destructible of the zone as the template, 6 m from the gates.
    let placed = c
        .json(
            "add_object",
            json!({
                "group": key,
                "template": format!("{ZONE}#118:4"),
                "model": id,
                "position": [283.9, 0.0, 377.8],
            }),
        )
        .await;
    assert_eq!(placed["done"]["model"], id, "{placed}");
    let listed = c
        .json("level_objects", json!({ "stream": "gf2/doctor/area57" }))
        .await;
    assert_eq!(listed["total"], 1, "{listed}");
    assert_eq!(listed["objects"][0]["model"], id, "{listed}");

    let checked = c.json("validate", json!({})).await;
    assert_eq!(checked["ok"], true, "{checked}");
    assert!(
        checked["checked_changes"].as_u64().unwrap() >= 6,
        "{checked}"
    );
    let built = c.json("mod_build", json!({ "name": "Doctor area" })).await;
    assert_eq!(built["new_streams"], 1, "{built}");
    assert!(built["added_resources"].as_u64().unwrap() >= 6, "{built}");
    // The new model exports as a model file.
    let exported = c
        .json(
            "export",
            json!({ "resource": format!("gf2/doctor/area57:metamodel:{id}") }),
        )
        .await;
    assert!(
        !exported["files"].as_array().unwrap().is_empty(),
        "{exported}"
    );
    eprintln!(
        "a new area through the server in {:.1} s: model {id}, mod {}",
        started.elapsed().as_secs_f64(),
        built["file"]
    );
    assert_eq!(before, stat_all(&dir), "the game folder changed");
}

/// The tools of stage 104 (packages B, D, E, G) on the real game: game
/// parameters, the speech tables and the mixer, the world grid, the ground
/// and a walk over the collision of a district, objects put on the ground.
#[tokio::test(flavor = "multi_thread")]
async fn authoring_tools_on_the_game() {
    let Some(dir) = game_dir() else {
        return;
    };
    let before = stat_all(&dir);
    let temp = tempfile::tempdir().unwrap();
    let mut config = doctor_toolkit_mcp::Config::new(&dir);
    config.bridge = None;
    config.workspace = Some(temp.path().join("workspace"));
    config.output_dir = Some(temp.path().join("out"));
    config.allow_edits = true;
    let mut c = Client::start(doctor_toolkit_mcp::open(&config).unwrap());
    c.initialize().await;

    // Every curated parameter is found in this edition of the game.
    let list = c.json("tunables", json!({ "limit": 300 })).await;
    let rows = list["tunables"].as_array().unwrap();
    assert!(rows.len() >= 30, "{}", rows.len());
    let missing: Vec<&Value> = rows
        .iter()
        .filter(|r| r["now"]["unavailable"].is_string())
        .collect();
    assert!(missing.is_empty(), "{missing:?}");
    let set = c
        .json(
            "set_tunables",
            json!({ "changes": [{ "id": "weapons.pistol.damage", "multiply": 2 }] }),
        )
        .await;
    assert_eq!(set["changed"], true, "{set}");
    assert_eq!(set["done"]["changed_values"].as_array().unwrap().len(), 4);

    // Speech and the mixer.
    let speakers = c
        .json("speech", json!({ "show": "speakers", "limit": 5 }))
        .await;
    assert!(speakers["total"].as_u64().unwrap() > 10, "{speakers}");
    let events = c.json("speech", json!({ "limit": 3 })).await;
    assert!(events["records"].as_u64().unwrap() > 100, "{events}");
    let mixer = c.json("mix", json!({})).await;
    assert!(mixer["categories"].as_array().unwrap().len() > 5, "{mixer}");
    let name = mixer["mixes"][0]["name"].as_str().unwrap().to_owned();
    let one = c.json("mix", json!({ "name": name })).await;
    assert!(one["channels"].is_array(), "{one}");

    // The world grid of a city.
    let grids = c.json("zone_cells", json!({ "level": "gf2/miami" })).await;
    assert!(!grids["grids"].as_array().unwrap().is_empty(), "{grids}");
    let cells = c
        .json(
            "zone_cells",
            json!({ "level": "gf2/miami", "cells": true, "filter": "bonita", "limit": 5 }),
        )
        .await;
    assert!(cells["total"].as_u64().unwrap() > 0, "{cells}");

    // The ground of a district: a prop of a street zone stands on it.
    let zone = "gf2/miami/local/bonita/high/terrain06";
    let started = Instant::now();
    let objects = c
        .json("level_objects", json!({ "stream": zone, "limit": 500 }))
        .await;
    let placed: Vec<&Value> = objects["objects"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|o| o["position"].is_array() && o["model"].is_string())
        .collect();
    assert!(!placed.is_empty(), "{objects}");
    let prop = placed[placed.len() / 2];
    let at: Vec<f64> = prop["position"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect();
    let ground = c
        .json(
            "ground",
            json!({ "streams": [zone], "point": [at[0], at[1] + 30.0, at[2]] }),
        )
        .await;
    assert!(
        ground["world"]["colliders"].as_u64().unwrap() > 0,
        "{ground}"
    );
    let y = ground["ground"][1]
        .as_f64()
        .unwrap_or_else(|| panic!("{ground}"));
    assert!(y < at[1] + 30.0 && y > at[1] - 60.0, "{ground}");
    eprintln!(
        "ground under {}: {y:.2} in {:?}",
        prop["object"],
        started.elapsed()
    );
    // A ray straight down gives the same ground.
    let ray = c
        .json(
            "ground",
            json!({
                "streams": [zone],
                "ray_origin": [at[0], at[1] + 30.0, at[2]],
                "ray_direction": [0, -2, 0],
            }),
        )
        .await;
    let ray_y = ray["ground"][1].as_f64().unwrap_or_else(|| panic!("{ray}"));
    assert!((ray_y - y).abs() < 0.05, "{ray} against {ground}");

    // Lifted by 25 m and put down: the object comes back onto what is under it.
    let object = prop["object"].as_str().unwrap();
    let dropped = c
        .json(
            "move_object",
            json!({ "object": object, "offset": [0.0, 25.0, 0.0], "on_ground": true }),
        )
        .await;
    assert_eq!(dropped["changed"], true, "{dropped}");
    let new_y = dropped["done"]["moved"][0]["new_position"][1]
        .as_f64()
        .unwrap();
    assert!(new_y < at[1] + 20.0, "it came down: {dropped}");
    // A new object on the ground at that point.
    let added = c
        .json(
            "add_object",
            json!({
                "group": object.rsplit_once(':').unwrap().0,
                "template": object,
                "position": [at[0] + 2.0, at[1] + 40.0, at[2]],
                "on_ground": true,
            }),
        )
        .await;
    assert_eq!(added["changed"], true, "{added}");

    // A walk of a few metres over the collision: an answer either way, with the path.
    let walk = c
        .json(
            "walk_route",
            json!({
                "streams": [zone],
                "from": [at[0], y + 0.5, at[2]],
                "to": [at[0] + 4.0, y + 0.5, at[2] + 3.0],
            }),
        )
        .await;
    assert!(
        ["reached", "stuck", "fell", "timed_out"].contains(&walk["outcome"].as_str().unwrap()),
        "{walk}"
    );
    assert!(walk["path"].as_array().unwrap().len() >= 2, "{walk}");
    eprintln!(
        "walk: {} after {} m",
        walk["outcome"], walk["walked_metres"]
    );

    drop(c);
    assert_eq!(stat_all(&dir), before, "the game folder is only read");
}

/// Stage 104: the cut-scene `igc16` is read and edited with the tools alone
/// in a temporary workspace — the camera of two shots, an actor and the
/// time of a subtitle — and a mod of it says that it carries the whole
/// sound; the other shots and the game folder stay as they were.
#[tokio::test(flavor = "multi_thread")]
async fn a_cut_scene_is_edited_through_the_server() {
    let Some(dir) = game_dir() else {
        return;
    };
    let before = stat_all(&dir);
    let temp = tempfile::tempdir().unwrap();
    let mut config = doctor_toolkit_mcp::Config::new(&dir);
    config.bridge = None;
    config.workspace = Some(temp.path().join("workspace"));
    config.allow_edits = true;
    let kit = doctor_toolkit_mcp::open(&config).unwrap();
    let mut c = Client::start(kit);
    c.initialize().await;
    let stream =
        "gf2/cuba/global/objectives/chapter60-mission08-killingcastro/cp_734/_734_igc_content";
    let near = |v: &Value, expected: f64| (v.as_f64().unwrap() - expected).abs() < 2e-3;

    let all = c.json("cutscenes", json!({})).await;
    assert!(all["total"].as_u64().unwrap() >= 20, "{}", all["total"]);
    assert!(all["shots"].as_u64().unwrap() >= 250, "{}", all["shots"]);
    let scene = c.json("cutscenes", json!({ "stream": stream })).await;
    assert_eq!(scene["cutscene"], "igc16", "{scene}");
    assert_eq!(scene["shots"].as_array().unwrap().len(), 17);
    let track = scene["audio"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| !a["subtitles"].as_array().unwrap().is_empty())
        .expect("a sound track with subtitles")
        .clone();
    assert_eq!(track["subtitles"][0]["start_ms"], 5064, "{track}");
    let at = |shot: &str| json!({ "stream": stream, "shot": shot, "time": 0.5 });
    let s02 = c.json("cutscenes", at("igc16_s02")).await;
    let camera = &s02["moment"]["camera"];
    assert!(near(&camera["position"][1], 6.668), "{camera}");
    assert!(near(&camera["focal_length"], 50.0), "{camera}");
    let s03 = c.json("cutscenes", at("igc16_s03")).await;

    c.json(
        "cutscene_camera",
        json!({ "stream": stream, "shot": "igc16_s01", "offset": [0.0, 1.5, 0.0], "focal_length": 35.0 }),
    )
    .await;
    c.json(
        "cutscene_camera",
        json!({
            "stream": stream, "shot": "igc16_s02", "offset": [0.0, 0.8, 1.5],
            "turn": [-8.0, 0.0, 0.0], "focal_length": 28.0,
        }),
    )
    .await;
    c.json(
        "cutscene_actor",
        json!({
            "stream": stream, "shot": "igc16_s02", "actor": "igc16_player_herc",
            "offset": [-1.2, 0.0, 0.0],
        }),
    )
    .await;
    let retimed = c
        .json(
            "edit_subtitle",
            json!({ "entry": track["entry"], "line": 0, "start_ms": 1500, "duration_ms": 6000 }),
        )
        .await;
    assert!(
        retimed["done"]["file_bytes"].as_u64().unwrap() > 10_000_000,
        "the whole sound is the change: {retimed}"
    );

    let after = c.json("cutscenes", at("igc16_s02")).await;
    let camera = &after["moment"]["camera"];
    assert!(near(&camera["position"][1], 7.468), "{camera}");
    assert!(near(&camera["position"][2], 7.307), "{camera}");
    assert!(near(&camera["rotation"][0], -8.786), "{camera}");
    assert!(near(&camera["focal_length"], 28.0), "{camera}");
    let player = after["moment"]["actors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "igc16_player_herc")
        .unwrap();
    assert!(near(&player["position"][0], -269.733), "{player}");
    assert_eq!(
        c.json("cutscenes", at("igc16_s03")).await["moment"],
        s03["moment"],
        "the third shot is untouched"
    );
    let scene = c.json("cutscenes", json!({ "stream": stream })).await;
    let line = &scene["audio"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["entry"] == track["entry"])
        .unwrap()["subtitles"][0];
    assert_eq!(line["start_ms"], 1500, "{line}");
    assert_eq!(line["duration_ms"], 6000, "{line}");

    let checked = c.json("validate", json!({})).await;
    assert_eq!(checked["ok"], true, "{checked}");
    let built = c.json("mod_build", json!({ "name": "Doctor igc16" })).await;
    assert_eq!(built["resources"], 2, "{built}");
    let notes = built["notes"].as_array().unwrap();
    assert_eq!(notes.len(), 1, "{built}");
    assert!(notes[0].as_str().unwrap().contains("whole"), "{built}");
    assert_eq!(before, stat_all(&dir), "the game folder changed");
}

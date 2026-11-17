//! The cut-scene tools (stage 104) on the miniature game: a shot is read,
//! its camera, an actor and a command are edited and a subtitle is retimed,
//! each as one undoable operation of the workspace; the game files stay as
//! they were.

mod common;

use std::path::Path;

use serde_json::{Value, json};

use common::{Client, config, snapshot};
use doctor_toolkit::testing::{self, CUTSCENE, CUTSCENE_SOUND};

fn game() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    testing::write_game(dir.path()).unwrap();
    testing::write_cutscene(dir.path()).unwrap();
    testing::write_cutscene_sound(dir.path()).unwrap();
    dir
}

fn start(game: &Path, ws: &Path, edits: bool, out: Option<&Path>) -> Client {
    let mut c = config(game);
    c.workspace = Some(ws.to_path_buf());
    c.allow_edits = edits;
    c.output_dir = out.map(Path::to_path_buf);
    Client::start(doctor_toolkit_mcp::open(&c).unwrap())
}

fn near(value: &Value, expected: f64) -> bool {
    (value.as_f64().unwrap() - expected).abs() < 1e-4
}

#[tokio::test(flavor = "multi_thread")]
async fn cutscenes_are_listed_and_a_shot_is_shown_without_flags() {
    let game = game();
    let ws = tempfile::tempdir().unwrap();
    let mut c = start(game.path(), &ws.path().join("ws"), false, None);
    c.initialize().await;
    let tools = c.request("tools/list", json!({})).await;
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"cutscenes"));
    assert!(!names.contains(&"cutscene_camera"), "an edit tool");

    let all = c.json("cutscenes", json!({})).await;
    assert_eq!(all["total"], 1, "{all}");
    assert_eq!(all["streams"][0]["stream"], CUTSCENE);
    assert_eq!(all["streams"][0]["shots"], "igc99_s01");
    let none = c.json("cutscenes", json!({ "filter": "miami" })).await;
    assert_eq!(none["total"], 0);

    let scene = c.json("cutscenes", json!({ "stream": CUTSCENE })).await;
    assert_eq!(scene["cutscene"], "igc99");
    assert_eq!(scene["shots"][0]["actors"], json!(["igc99_hero"]));
    assert_eq!(scene["audio"][0]["entry"], CUTSCENE_SOUND);
    assert_eq!(scene["audio"][0]["subtitles"][1]["start_ms"], 1000);

    let shot = c
        .json(
            "cutscenes",
            json!({ "stream": CUTSCENE, "shot": "igc99_s01", "time": 1.0 }),
        )
        .await;
    assert_eq!(shot["shot"]["camera_keys"], json!([0.0, 2.0]), "{shot}");
    let commands = shot["shot"]["commands"].as_array().unwrap();
    assert!(commands.iter().any(|c| c["op"] == "camera_curves"));
    assert!(commands.iter().all(|c| c["op"] != "wait"));
    assert!(
        near(&shot["moment"]["camera"]["position"][0], 5.0),
        "{shot}"
    );
    assert!(near(&shot["moment"]["camera"]["focal_length"], 50.0));
    assert_eq!(shot["moment"]["camera"]["on_key"], false);
    assert_eq!(shot["moment"]["actors"][0]["name"], "igc99_hero");

    let (text, error) = c
        .call("cutscenes", json!({ "stream": CUTSCENE, "shot": "nope" }))
        .await;
    assert!(error && text.contains("igc99_s01"), "{text}");
    let (text, error) = c.call("cutscenes", json!({ "shot": "0" })).await;
    assert!(error && text.contains("stream"), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_shot_is_edited_with_the_tools_alone_and_every_edit_is_undone() {
    let game = game();
    let before = snapshot(game.path());
    let ws = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let mut c = start(game.path(), &ws.path().join("ws"), true, Some(out.path()));
    c.initialize().await;
    let at = |time: f64| json!({ "stream": CUTSCENE, "shot": "0", "time": time });

    // A camera key between the keys, with a new lens.
    let done = c
        .json(
            "cutscene_camera",
            json!({
                "stream": CUTSCENE, "shot": "igc99_s01", "time": 1.0,
                "position": [5.0, 3.0, 8.0], "rotation": [-8.0, 0.0, 0.0], "focal_length": 35.0,
            }),
        )
        .await;
    assert_eq!(done["changed"], true, "{done}");
    assert!(
        done["operation"].as_str().unwrap().contains("igc99_s01"),
        "{done}"
    );
    assert_eq!(done["diff"].as_array().unwrap().len(), 1, "{done}");
    let shot = c.json("cutscenes", at(1.0)).await;
    assert_eq!(shot["shot"]["camera_keys"], json!([0.0, 1.0, 2.0]));
    let camera = &shot["moment"]["camera"];
    assert!(near(&camera["position"][1], 3.0), "{camera}");
    assert!(near(&camera["rotation"][0], -8.0));
    assert!(near(&camera["focal_length"], 35.0));
    assert_eq!(camera["on_key"], true);
    // The game files have the old shot.
    let original = c
        .json(
            "cutscenes",
            json!({ "stream": CUTSCENE, "shot": "0", "time": 1.0, "original": true }),
        )
        .await;
    assert_eq!(original["shot"]["camera_keys"], json!([0.0, 2.0]));

    // The whole path higher and a wider lens on every key.
    c.json(
        "cutscene_camera",
        json!({ "stream": CUTSCENE, "shot": "0", "offset": [0.0, 1.5, 0.0], "vertical_fov": 40.0 }),
    )
    .await;
    let shot = c.json("cutscenes", at(0.0)).await;
    assert!(near(&shot["moment"]["camera"]["position"][1], 3.2));
    assert!((shot["moment"]["camera"]["vertical_fov"].as_f64().unwrap() - 40.0).abs() < 1e-2);
    // The key goes again.
    c.json(
        "cutscene_camera",
        json!({ "stream": CUTSCENE, "shot": "0", "time": 1.0, "delete_key": true }),
    )
    .await;
    let shot = c.json("cutscenes", at(1.0)).await;
    assert_eq!(shot["shot"]["camera_keys"], json!([0.0, 2.0]));

    // The actor: moved for the shot, then keyed.
    c.json(
        "cutscene_actor",
        json!({ "stream": CUTSCENE, "shot": "0", "actor": "igc99_hero", "offset": [-1.2, 0.0, 0.0] }),
    )
    .await;
    let shot = c.json("cutscenes", at(2.0)).await;
    assert!(near(&shot["moment"]["actors"][0]["position"][0], 2.8));
    c.json(
        "cutscene_actor",
        json!({
            "stream": CUTSCENE, "shot": "0", "actor": "igc99_hero", "time": 1.5,
            "position": [9.0, 0.0, 0.0], "rotation": [0.0, 90.0, 0.0],
        }),
    )
    .await;
    let shot = c.json("cutscenes", at(1.5)).await;
    assert_eq!(shot["moment"]["actors"][0]["keys"], json!([1.0, 1.5, 2.0]));
    let (text, error) = c
        .call(
            "cutscene_actor",
            json!({ "stream": CUTSCENE, "shot": "0", "actor": "nobody", "offset": [1, 0, 0] }),
        )
        .await;
    assert!(error && text.contains("igc99_hero"), "{text}");

    // A command: the camera's aspect, half a second later.
    let index = shot["shot"]["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["op"] == "camera_create")
        .unwrap()["index"]
        .clone();
    let done = c
        .json(
            "cutscene_command",
            json!({
                "stream": CUTSCENE, "shot": "0", "command": index,
                "time": 0.5, "fields": { "aspect": 1.5 },
            }),
        )
        .await;
    assert_eq!(done["changed"], true, "{done}");
    let shot = c.json("cutscenes", at(0.0)).await;
    let moved = shot["shot"]["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["op"] == "camera_create")
        .unwrap()
        .clone();
    assert!(near(&moved["time"], 0.5), "{moved}");
    let (text, error) = c
        .call(
            "cutscene_command",
            json!({ "stream": CUTSCENE, "shot": "0", "command": moved["index"], "fields": { "colour": 1 } }),
        )
        .await;
    assert!(error && text.contains("aspect"), "{text}");

    // A subtitle is retimed in its sound file; the whole file is the change.
    let done = c
        .json(
            "edit_subtitle",
            json!({ "entry": CUTSCENE_SOUND, "line": 0, "start_ms": 1500, "duration_ms": 600 }),
        )
        .await;
    assert_eq!(done["changed"], true, "{done}");
    assert!(
        done["done"]["note"].as_str().unwrap().contains("whole"),
        "{done}"
    );
    let scene = c.json("cutscenes", json!({ "stream": CUTSCENE })).await;
    let lines = scene["audio"][0]["subtitles"].as_array().unwrap();
    assert_eq!(lines[0]["start_ms"], 1000, "sorted by start: {scene}");
    assert_eq!(lines[1]["start_ms"], 1500);
    assert_eq!(lines[1]["duration_ms"], 600);
    let (text, error) = c
        .call(
            "edit_subtitle",
            json!({ "entry": CUTSCENE_SOUND, "start_ms": 5 }),
        )
        .await;
    assert!(error && text.contains("line"), "{text}");
    let (text, error) = c
        .call("edit_subtitle", json!({ "entry": CUTSCENE_SOUND }))
        .await;
    assert!(error && text.contains("texts"), "{text}");

    // The shot goes out as JSON and as glTF, and comes back with import_file.
    let key = format!("{CUTSCENE}#0");
    let exported = c.json("export", json!({ "resource": key })).await;
    let file = exported["files"][0].as_str().unwrap().to_owned();
    let mut seq: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    seq["blocks"][0]["duration"] = json!(2.5);
    std::fs::write(&file, seq.to_string()).unwrap();
    let back = c
        .json("import_file", json!({ "resource": key, "file": file }))
        .await;
    assert_eq!(back["changed"], true, "{back}");
    let scene = c.json("cutscenes", json!({ "stream": CUTSCENE })).await;
    assert!(near(&scene["seconds"], 2.5), "{scene}");
    let glb = c
        .json("export", json!({ "resource": key, "format": "glb" }))
        .await;
    let glb = std::fs::read(glb["files"][0].as_str().unwrap()).unwrap();
    assert_eq!(&glb[..4], b"glTF");

    // Eight operations, one per call; undo takes each back.
    let status = c.json("workspace_status", json!({})).await;
    assert_eq!(
        status["recent_operations"].as_array().unwrap().len(),
        8,
        "{status}"
    );
    for _ in 0..8 {
        c.json("undo", json!({})).await;
    }
    let status = c.json("workspace_status", json!({})).await;
    assert_eq!(status["counts"]["changes"], 0, "{status}");
    assert_eq!(status["files"], json!([]), "{status}");
    let shot = c.json("cutscenes", at(1.0)).await;
    assert_eq!(shot["shot"]["camera_keys"], json!([0.0, 2.0]));
    assert!(near(&shot["moment"]["actors"][0]["position"][0], 0.0));
    let scene = c.json("cutscenes", json!({ "stream": CUTSCENE })).await;
    assert_eq!(scene["audio"][0]["subtitles"][0]["start_ms"], 500);
    assert_eq!(
        snapshot(game.path()),
        before,
        "the game files are untouched"
    );
}

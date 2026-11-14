//! `export` and `media` (stage 57) on the miniature game: files go into the
//! output folder the server was started with, and nowhere else.

mod common;

use serde_json::{Value, json};

use common::{Client, config};
use doctor_toolkit::testing::{self, LEVEL};

fn start(game: &std::path::Path, out: Option<&std::path::Path>) -> Client {
    let mut c = config(game);
    c.workspace = Some(game.join("no-workspace"));
    c.output_dir = out.map(std::path::Path::to_path_buf);
    Client::start(doctor_toolkit_mcp::open(&c).unwrap())
}

fn files(answer: &Value) -> Vec<std::path::PathBuf> {
    answer["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f.as_str().unwrap().into())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn resources_are_exported_into_the_output_folder() {
    let game = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    testing::write_game(game.path()).unwrap();
    testing::write_extra_streams(game.path()).unwrap();

    // Without an output folder: a clear refusal.
    let mut c = start(game.path(), None);
    c.initialize().await;
    let (text, error) = c
        .call("export", json!({ "resource": format!("{LEVEL}#0") }))
        .await;
    assert!(error && text.contains("--output-dir"), "{text}");

    let mut c = start(game.path(), Some(out.path()));
    c.initialize().await;
    // A texture as PNG.
    let png = c
        .json("export", json!({ "resource": format!("{LEVEL}#0") }))
        .await;
    let written = files(&png);
    assert_eq!(written.len(), 1, "{png}");
    assert!(written[0].starts_with(out.path()));
    assert!(std::fs::read(&written[0]).unwrap().starts_with(b"\x89PNG"));
    // A mesh as GLB, with its material and texture.
    let glb = c
        .json(
            "export",
            json!({ "resource": format!("{LEVEL}#3"), "name": "quad" }),
        )
        .await;
    let written = files(&glb);
    assert_eq!(written, [out.path().join("quad.glb")], "{glb}");
    assert!(std::fs::read(&written[0]).unwrap().starts_with(b"glTF"));
    // A string table in the form import_file reads back.
    let text = c
        .json("export", json!({ "resource": format!("{LEVEL}#5") }))
        .await;
    let doc: Value = serde_json::from_slice(&std::fs::read(&files(&text)[0]).unwrap()).unwrap();
    assert_eq!(doc["stream"], LEVEL);
    assert!(doc["entries"].as_array().unwrap().len() >= 2, "{doc}");
    // A SimGroup as logic JSON, a navigation graph, raw data.
    for (resource, format) in [
        (format!("{LEVEL}#4"), "json"),
        (format!("{}#0", testing::NAV), "json"),
        (format!("{LEVEL}#2"), "raw"),
    ] {
        let a = c
            .json("export", json!({ "resource": resource, "format": format }))
            .await;
        assert!(files(&a)[0].is_file(), "{a}");
    }
    // Names stay inside the folder; formats a type has not are refused.
    let (text, error) = c
        .call(
            "export",
            json!({ "resource": format!("{LEVEL}#0"), "name": "../escape" }),
        )
        .await;
    assert!(error && text.contains("without `..`"), "{text}");
    let (text, error) = c
        .call(
            "export",
            json!({ "resource": format!("{LEVEL}#5"), "format": "glb" }),
        )
        .await;
    assert!(error && text.contains("do not export"), "{text}");
    // No movies in the miniature game.
    let movies = c.json("media", json!({})).await;
    assert_eq!(movies["total"], 0, "{movies}");
    assert!(!game.path().join("_doctor_toolkit").exists());
}

//! The tools that bring the server level with the editor (stage 104) on the
//! miniature game: what the wizards do (game parameters, clothes, materials,
//! every copy of a texture), each as one undoable operation with its diff.

mod common;

use std::path::PathBuf;

use serde_json::{Value, json};

use common::{Client, config};
use doctor_toolkit::game::GameInstance;
use doctor_toolkit::resource::partsdb::PartsDb;
use doctor_toolkit::testing::{
    self, LEVEL, MATERIAL, MESH_ID, PARTSDB, SIMGROUP, SOUND, SOUND_APX, SOUND_BNK, SOUND_MIX,
    SOUND_SBK, TEXT,
};
use doctor_toolkit::workspace::Workspace;
use doctor_toolkit_mcp::Config;

struct Fixture {
    _dirs: Vec<tempfile::TempDir>,
    game: PathBuf,
    workspace: PathBuf,
    out: PathBuf,
}

fn fixture() -> Fixture {
    let game = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    testing::write_game(game.path()).unwrap();
    testing::write_extra_streams(game.path()).unwrap();
    testing::write_tunable_globals(game.path()).unwrap();
    Fixture {
        game: game.path().to_path_buf(),
        workspace: ws.path().join("ws"),
        out: out.path().to_path_buf(),
        _dirs: vec![game, ws, out],
    }
}

fn configured(f: &Fixture, edits: bool) -> Config {
    let mut c = config(&f.game);
    c.workspace = Some(f.workspace.clone());
    c.output_dir = Some(f.out.clone());
    c.allow_edits = edits;
    c
}

async fn start(f: &Fixture, edits: bool) -> Client {
    let mut c = Client::start(doctor_toolkit_mcp::open(&configured(f, edits)).unwrap());
    c.initialize().await;
    c
}

fn workspace(f: &Fixture) -> (GameInstance, Workspace) {
    let game = GameInstance::open(&f.game).unwrap();
    let ws = Workspace::open_read_only(&f.workspace).unwrap();
    (game, ws)
}

#[tokio::test(flavor = "multi_thread")]
async fn game_parameters_are_listed_and_changed_in_every_object() {
    let f = fixture();
    let mut c = start(&f, true).await;
    // The list reads the values; a parameter the game does not have says so.
    let list = c.json("tunables", json!({ "category": "weapons" })).await;
    let rows = list["tunables"].as_array().unwrap();
    let damage = rows
        .iter()
        .find(|r| r["id"] == "weapons.pistol.damage")
        .unwrap();
    assert_eq!(damage["now"]["objects"], 4, "{damage}");
    assert_eq!(damage["now"]["values"], json!(["25", "26", "27", "28"]));
    assert!(rows.iter().any(|r| r["now"]["unavailable"].is_string()));
    let one = c
        .json("tunables", json!({ "id": "weapons.pistol.clip_size" }))
        .await;
    assert_eq!(one["objects"].as_array().unwrap().len(), 4, "{one}");
    assert_eq!(one["objects"][0]["value"], 8.0);

    // A dry run changes nothing.
    let dry = c
        .json(
            "set_tunables",
            json!({ "changes": [{ "id": "weapons.pistol.damage", "multiply": 2 }], "dry_run": true }),
        )
        .await;
    assert_eq!(dry["would_change"].as_array().unwrap().len(), 4, "{dry}");
    assert!(!Workspace::exists(&f.workspace));

    // Two parameters in one operation: a factor keeps the differences.
    let done = c
        .json(
            "set_tunables",
            json!({ "changes": [
                { "id": "weapons.pistol.damage", "multiply": 2 },
                { "id": "weapons.pistol.clip_size", "value": 12 },
            ] }),
        )
        .await;
    assert_eq!(done["changed"], true, "{done}");
    assert_eq!(done["done"]["changed_values"].as_array().unwrap().len(), 8);
    let after = c
        .json("tunables", json!({ "id": "weapons.pistol.damage" }))
        .await;
    let values: Vec<f64> = after["objects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["value"].as_f64().unwrap())
        .collect();
    assert_eq!(values, [50.0, 52.0, 54.0, 56.0]);
    let (_, ws) = workspace(&f);
    assert_eq!(ws.head(), 1, "one operation");

    // Bounds and bad input are refused with nothing recorded.
    for bad in [
        json!({ "changes": [{ "id": "weapons.pistol.clip_size", "value": 0 }] }),
        json!({ "changes": [{ "id": "weapons.pistol.clip_size", "value": 2.5 }] }),
        json!({ "changes": [{ "id": "weapons.pistol.clip_size" }] }),
        json!({ "changes": [{ "id": "no.such.thing", "value": 1 }] }),
    ] {
        let (text, error) = c.call("set_tunables", bad.clone()).await;
        assert!(error, "{bad}: {text}");
    }
    c.json("undo", json!({})).await;
    let back = c
        .json("tunables", json!({ "id": "weapons.pistol.clip_size" }))
        .await;
    assert_eq!(back["objects"][0]["value"], 8.0);

    // The list is there without flags; the change is not.
    let mut reader = start(&f, false).await;
    reader.json("tunables", json!({})).await;
    let listed = reader.request("tools/list", json!({})).await;
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"tunables") && !names.contains(&"set_tunables"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_part_of_clothes_is_added_and_allowed_in_sets() {
    let f = fixture();
    let mut c = start(&f, true).await;
    let db = format!("{LEVEL}#{PARTSDB}");
    let done = c
        .json(
            "partsdb_add_part",
            json!({
                "database": db,
                "name": "mod_cap",
                "slot": "a_hat",
                "meshes": [format!("{MESH_ID:x}")],
                "tags": ["hatCulled"],
                "sets": ["db"],
                "only": true,
            }),
        )
        .await;
    assert_eq!(done["changed"], true, "{done}");
    assert_eq!(done["done"]["sets_changed"], 1);
    assert_eq!(done["done"]["variant_name"], "mod_cap");
    assert_eq!(done["done"]["meshes_not_in_stream"], json!([]));
    let (game, ws) = workspace(&f);
    let level = game.find_stream(LEVEL).unwrap();
    let read =
        |ws: &Workspace| PartsDb::parse(&ws.current_data(&game, level, PARTSDB).unwrap()).unwrap();
    let parts = read(&ws);
    let part = parts.part_index("mod_cap").unwrap();
    let variant = parts.variant_index("mod_cap").unwrap();
    assert_eq!(parts.parts[part].models[0].0, MESH_ID);
    assert_eq!(parts.parts[part].tags, ["hatCulled"]);
    assert_eq!(parts.sets[0].choices[1].variants, [variant]);

    // The same name again is refused; an existing variant goes back in.
    let (text, error) = c
        .call(
            "partsdb_add_part",
            json!({ "database": db, "name": "mod_cap", "slot": "a_hat", "meshes": ["3333"] }),
        )
        .await;
    assert!(error && text.contains("already has a part"), "{text}");
    let allowed = c
        .json("partsdb_allow", json!({ "database": db, "variant": "hat" }))
        .await;
    assert_eq!(allowed["done"]["sets_changed"], 1, "{allowed}");
    assert_eq!(allowed["done"]["slot"], "a_hat");
    let ws = Workspace::open_read_only(&f.workspace).unwrap();
    assert_eq!(read(&ws).sets[0].choices[1].variants.len(), 2);
    // Nothing to do: no operation.
    let again = c
        .json("partsdb_allow", json!({ "database": db, "variant": "hat" }))
        .await;
    assert_eq!(again["changed"], false, "{again}");
    let (text, error) = c
        .call(
            "partsdb_allow",
            json!({ "database": db, "variant": "nope" }),
        )
        .await;
    assert!(error && text.contains("no variant"), "{text}");

    // The set as one model: the head part shows the mesh of the level.
    let glb = c
        .json(
            "export",
            json!({ "resource": db, "format": "glb", "set": "db", "name": "character" }),
        )
        .await;
    let file = PathBuf::from(glb["files"][0].as_str().unwrap());
    assert!(std::fs::read(&file).unwrap().starts_with(b"glTF"), "{glb}");
    let (text, error) = c
        .call("export", json!({ "resource": db, "format": "glb" }))
        .await;
    assert!(error && text.contains("Sets: db"), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn material_parameters_are_set_by_name() {
    let f = fixture();
    let mut c = start(&f, true).await;
    let material = format!("{LEVEL}#{MATERIAL}");
    let done = c
        .json(
            "set_material",
            json!({ "material": material, "params": { "g_Color": [0.1, 0.2, 0.3, 1.0] } }),
        )
        .await;
    assert_eq!(done["changed"], true, "{done}");
    assert_eq!(
        done["done"]["changed"][0]["before"],
        json!([0.5, 0.25, 1.0, 1.0])
    );
    let shown = c
        .data(
            "resource_json",
            json!({ "resource": done["done"]["material"] }),
        )
        .await;
    assert!(shown.contains("0.3"), "{shown}");
    for bad in [
        json!({ "g_Color": [1.0] }),
        json!({ "g_BaseSampler": [1.0] }),
        json!({ "g_Nothing": [1.0] }),
    ] {
        let (text, error) = c
            .call(
                "set_material",
                json!({ "material": material, "params": bad }),
            )
            .await;
        assert!(error, "{text}");
    }
    let (text, error) = c
        .call(
            "set_material",
            json!({ "material": format!("{LEVEL}#0"), "params": { "g_Color": [1, 1, 1, 1] } }),
        )
        .await;
    assert!(error && text.contains("not a material"), "{text}");
    // The same values again: nothing changes.
    let again = c
        .json(
            "set_material",
            json!({ "material": material, "params": { "g_Color": [0.1, 0.2, 0.3, 1.0] } }),
        )
        .await;
    assert_eq!(again["changed"], false, "{again}");
}

#[tokio::test(flavor = "multi_thread")]
async fn every_copy_of_a_texture_is_replaced_in_one_operation() {
    let f = fixture();
    let mut c = start(&f, true).await;
    // The texture of the level as PNG, then back into every copy.
    let png = c
        .json(
            "export",
            json!({ "resource": format!("{LEVEL}#0"), "name": "wall" }),
        )
        .await;
    let file = png["files"][0].as_str().unwrap().to_owned();
    let done = c
        .json(
            "import_texture",
            json!({ "texture": format!("{LEVEL}#0"), "file": file, "all_copies": true }),
        )
        .await;
    assert_eq!(done["done"]["streams"], json!([LEVEL]), "{done}");
    assert_eq!(done["done"]["name"], "wall");
}

/// The tools of this file and the flag each needs.
#[tokio::test(flavor = "multi_thread")]
async fn the_flags_turn_the_tools_on() {
    let f = fixture();
    let mut c = start(&f, false).await;
    let listed = c.request("tools/list", json!({})).await;
    let names: Vec<String> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect();
    for tool in [
        "set_tunables",
        "voice_add",
        "partsdb_add_part",
        "partsdb_allow",
        "set_material",
    ] {
        assert!(!names.iter().any(|n| n == tool), "{tool} without flags");
    }
    let _: Value = c.json("tunables", json!({ "filter": "pistol" })).await;
}

// ------------------------------------------------------------ 104-E

/// The level SimGroup with logic in the workspace (a trigger, a manager, a spawner).
fn with_logic(f: &Fixture) {
    let game = GameInstance::open(&f.game).unwrap();
    let mut ws = Workspace::create(&f.workspace, &game).unwrap();
    let level = game.find_stream(LEVEL).unwrap();
    ws.set_resource(
        &game,
        level,
        SIMGROUP,
        common::logic_group().encode(),
        "New logic",
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn logic_and_scripts_run_without_the_game() {
    let f = fixture();
    with_logic(&f);
    let mut c = start(&f, false).await;
    let scope = format!("group:{LEVEL}#{SIMGROUP}");
    let actors = c
        .json("logic_run", json!({ "scope": scope, "actors": true }))
        .await;
    assert_eq!(actors["total"], 3, "{actors}");
    assert!(actors["actors"][0]["class"].is_string());
    let run = c
        .json(
            "logic_run",
            json!({ "scope": scope, "events": ["load", "enter:Trigger", "wait:5"] }),
        )
        .await;
    let total = run["journal_total"].as_u64().unwrap();
    assert!(total >= 2, "{run}");
    assert_eq!(run["objects"], 3);
    assert!(run["stopped"].is_string());
    // Pages: the same run, cut elsewhere.
    let page = c
        .json(
            "logic_run",
            json!({ "scope": scope, "events": ["load", "enter:Trigger", "wait:5"],
                    "offset": 1, "limit": 1, "json": true }),
        )
        .await;
    assert_eq!(page["journal"].as_array().unwrap().len(), 1, "{page}");
    assert_eq!(page["journal"][0]["seq"], 2);
    assert_eq!(page["journal_total"].as_u64().unwrap(), total);
    // The game files without the workspace have no logic in that group.
    let (text, error) = c
        .call("logic_run", json!({ "scope": "group:nowhere#0" }))
        .await;
    assert!(error, "{text}");

    // A script: Lua source, its prints and what it sends.
    let lua = "print(\"hello\")\nSendMessage(\"Go\")\n";
    let tried = c
        .json("script_run", json!({ "lua": lua, "steps": ["wait:1"] }))
        .await;
    let log = tried["log"].to_string();
    assert!(
        log.contains("print hello") && log.contains("send Go"),
        "{tried}"
    );
    assert_eq!(tried["error"], Value::Null);
    let broken = c
        .json("script_run", json!({ "lua": "error(\"boom\")" }))
        .await;
    assert!(
        broken["error"].as_str().unwrap().contains("boom"),
        "{broken}"
    );
    let (text, error) = c.call("script_run", json!({ "lua": "x = = 1" })).await;
    assert!(error && text.contains("script.lua:1"), "{text}");
    let (text, error) = c.call("script_run", json!({})).await;
    assert!(error && text.contains("one of"), "{text}");
    // The kinds of nodes graphs are made of, and the document of the format.
    let mut editor = start(&f, true).await;
    let kinds = editor.json("scripts", json!({ "node_kinds": true })).await;
    assert!(kinds["total"].as_u64().unwrap() > 10, "{kinds}");
    assert_eq!(kinds["node_kinds"][0]["kind"], "event.load");
    let one = editor
        .json(
            "scripts",
            json!({ "node_kinds": true, "filter": "event.load" }),
        )
        .await;
    assert!(one["node_kinds"][0]["outputs"].is_array(), "{one}");
    let listed = c.request("resources/list", json!({})).await;
    assert!(
        listed["result"]["resources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["uri"] == "doctor-toolkit://docs/scripts.md"),
        "scripts.md is a resource of the server"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_save_is_read_and_never_written() {
    let f = fixture();
    let mut c = start(&f, false).await;
    let dir = f.out.join("saves");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("GF2-EN-autosave");
    let bytes = doctor_toolkit::save::build("Don", "New York", 3723, 0, &[]).unwrap();
    std::fs::write(&file, &bytes).unwrap();
    let list = c.json("save_info", json!({ "path": dir })).await;
    assert_eq!(list["saves"].as_array().unwrap().len(), 1, "{list}");
    let info = c.json("save_info", json!({ "path": file })).await;
    assert_eq!(info["profile"], "Don", "{info}");
    assert_eq!(info["play_seconds"], 3723);
    assert_eq!(info["loads_in_game"], true);
    assert_eq!(std::fs::read(&file).unwrap(), bytes);
    let (text, error) = c
        .call("save_info", json!({ "path": file, "chunks": ["nope"] }))
        .await;
    assert!(error && text.contains("no chunk"), "{text}");
    let (text, error) = c
        .call(
            "save_info",
            json!({ "path": f.game.join("godfather2.exe") }),
        )
        .await;
    assert!(error && text.contains("not a save"), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn streams_are_unpacked_and_changed_files_packed_back() {
    let f = fixture();
    let mut c = start(&f, true).await;
    let done = c
        .json("unpack", json!({ "streams": [TEXT], "name": "text" }))
        .await;
    let folder = PathBuf::from(done["folder"].as_str().unwrap());
    assert!(
        folder.starts_with(&f.out) && done["files"].as_u64().unwrap() >= 1,
        "{done}"
    );
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(folder.join("manifest.json")).unwrap()).unwrap();
    let strings = manifest["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "strings")
        .unwrap_or_else(|| panic!("{manifest}"))["path"]
        .as_str()
        .unwrap()
        .to_owned();
    // Nothing changed: nothing to pack.
    let none = c.json("pack", json!({ "folder": "text" })).await;
    assert_eq!(none["changed"], false, "{none}");
    let path = folder.join(&strings);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("Yes"), "{text}");
    std::fs::write(&path, text.replace("Yes", "Sure")).unwrap();
    let dry = c
        .json("pack", json!({ "folder": "text", "dry_run": true }))
        .await;
    assert_eq!(dry["would_record"].as_array().unwrap().len(), 1, "{dry}");
    assert!(!Workspace::exists(&f.workspace));
    let packed = c.json("pack", json!({ "folder": folder })).await;
    assert_eq!(packed["changed"], true, "{packed}");
    assert_eq!(packed["done"]["files"][0]["recorded"], true);
    let changes = || Workspace::open_read_only(&f.workspace).unwrap().len();
    assert_eq!(changes(), 1);
    assert_eq!(Workspace::open_read_only(&f.workspace).unwrap().head(), 1);
    // One change out again by a part of its key; undo brings it back.
    let reverted = c.json("revert", json!({ "keys": ["gf2/text"] })).await;
    assert_eq!(
        reverted["done"]["reverted"].as_array().unwrap().len(),
        1,
        "{reverted}"
    );
    assert_eq!(changes(), 0);
    c.json("undo", json!({})).await;
    let all = c.json("revert", json!({ "all": true })).await;
    assert_eq!(all["done"]["reverted_changes"], 1, "{all}");
    for bad in [
        json!({}),
        json!({ "keys": ["x"], "all": true }),
        json!({ "keys": ["nothing-like-this"] }),
    ] {
        let (text, error) = c.call("revert", bad).await;
        assert!(error, "{text}");
    }
    // Not a folder of unpack; too many streams.
    let (text, error) = c.call("pack", json!({ "folder": "nope" })).await;
    assert!(error && text.contains("manifest.json"), "{text}");
    let (text, error) = c.call("unpack", json!({ "streams": [] })).await;
    assert!(error && text.contains("1 to 40"), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_project_is_made_and_pulled() {
    let f = fixture();
    let mut c = start(&f, true).await;
    let made = c
        .json(
            "project_new",
            json!({ "folder": "proj", "name": "Texts", "streams": [TEXT] }),
        )
        .await;
    let root = PathBuf::from(made["project"].as_str().unwrap());
    assert!(root.join("project.toml").is_file(), "{made}");
    assert!(made["entries"].as_u64().unwrap() >= 1);
    // The folder is taken now.
    let (text, error) = c
        .call(
            "project_new",
            json!({ "folder": "proj", "streams": [TEXT] }),
        )
        .await;
    assert!(error && text.contains("not empty"), "{text}");
    // Types choose among resources of streams or of the workspace.
    let (text, error) = c
        .call("project_new", json!({ "folder": "p2", "types": ["lh2"] }))
        .await;
    assert!(error && text.contains("empty"), "{text}");
    let (text, error) = c
        .call(
            "project_new",
            json!({ "folder": "p2", "streams": [TEXT], "from_workspace": true }),
        )
        .await;
    assert!(error && text.contains("not both"), "{text}");
    // Without --project there is nothing to pull into.
    let (text, error) = c.call("project_pull", json!({})).await;
    assert!(error && text.contains("--project"), "{text}");

    // A server of the project: an edit made elsewhere is pulled into the files.
    let mut config = configured(&f, true);
    config.project = Some(root.clone());
    let mut c = Client::start(doctor_toolkit_mcp::open(&config).unwrap());
    c.initialize().await;
    let none = c.json("project_pull", json!({})).await;
    assert_eq!(none["pulled"], json!([]), "{none}");
    c.json("edit_string", json!({ "entry": "0x1", "text": "Maybe" }))
        .await;
    let pulled = c.json("project_pull", json!({})).await;
    assert_eq!(pulled["pulled"].as_array().unwrap().len(), 1, "{pulled}");
    // The project names the workspace of this server as its own: sources
    // are written for what no entry has (the string of the other table).
    let written = c.json("project_pull", json!({ "sources": true })).await;
    assert_eq!(
        written["added"],
        json!(["src/gf2/level.str/lh2/00000009.json"]),
        "{written}"
    );
}

/// Stage 112: without streams and without the workspace `project_new` makes
/// an empty project with its own workspace; a server of that project edits
/// the workspace, and `project_pull` with `sources` writes what it holds —
/// a resource it adds too — as files of the project.
#[tokio::test(flavor = "multi_thread")]
async fn an_empty_project_gets_the_sources_of_its_workspace() {
    let f = fixture();
    let mut c = start(&f, true).await;
    let made = c
        .json(
            "project_new",
            json!({ "folder": "doc", "name": "Yard Mod" }),
        )
        .await;
    let root = PathBuf::from(made["project"].as_str().unwrap());
    assert_eq!(made["id"], "yard-mod", "{made}");
    assert_eq!(made["entries"], 0, "{made}");
    let workspace = PathBuf::from(made["workspace"].as_str().unwrap());
    assert_eq!(workspace, root.join("workspace"));
    let project = doctor_toolkit::project::Project::open(&root).unwrap();
    assert!(project.has_own_workspace() && project.entries().count() == 0);
    assert!(!root.join("src").exists() && !workspace.exists());

    // A server of the project: its edits go into the project's workspace.
    let mut config = configured(&f, true);
    config.project = Some(root.clone());
    config.workspace = None;
    let mut c = Client::start(doctor_toolkit_mcp::open(&config).unwrap());
    c.initialize().await;
    c.json("edit_string", json!({ "entry": "0x1", "text": "Maybe" }))
        .await;
    let raw = f.out.join("sparks.bin");
    std::fs::write(&raw, b"sparks").unwrap();
    let added = c
        .json(
            "add_resource",
            json!({ "stream": LEVEL, "type": "vfx", "file": raw, "ids": "00005601" }),
        )
        .await;
    assert_eq!(added["changed"], true, "{added}");
    assert!(workspace.join("manifest.json").is_file());
    let (text, error) = c
        .call("project_pull", json!({ "sources": true, "only": ["src"] }))
        .await;
    assert!(error && text.contains("only"), "{text}");
    let written = c.json("project_pull", json!({ "sources": true })).await;
    let ids: Vec<&str> = written["added"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        ids.contains(&"src/gf2/level.str/vfx/00005601.bin") && ids.len() == 3,
        "{written}"
    );
    assert_eq!(
        std::fs::read(root.join("src/gf2/level.str/vfx/00005601.bin")).unwrap(),
        b"sparks"
    );
    let manifest = std::fs::read_to_string(root.join("src/gf2/level.str/stream.toml")).unwrap();
    assert!(manifest.contains("[[resource.added]]"), "{manifest}");
    let again = c.json("project_pull", json!({ "sources": true })).await;
    assert_eq!(again["added"], json!([]), "{again}");
    let status = c.json("project_status", json!({})).await;
    assert_eq!(status["counts"]["to_build"], 0, "{status}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_object_of_a_class_is_added_with_empty_attributes() {
    let f = fixture();
    with_logic(&f);
    let mut c = start(&f, true).await;
    let group = format!("{LEVEL}#{SIMGROUP}");
    let added = c
        .json(
            "add_object",
            json!({ "group": group, "class": "TriggerAuto" }),
        )
        .await;
    assert_eq!(added["changed"], true, "{added}");
    assert_eq!(added["done"]["class"], "TriggerAuto");
    assert_eq!(added["done"]["object"], format!("{group}:3"));
    let objects = c.json("level_objects", json!({ "stream": LEVEL })).await;
    assert!(
        objects
            .to_string()
            .contains(added["done"]["guid"].as_str().unwrap()),
        "{objects}"
    );
    for bad in [
        json!({ "group": group, "class": "NoSuchClassAtAll" }),
        json!({ "group": group, "class": "TriggerAuto", "template": format!("{group}:0") }),
        json!({ "group": group }),
    ] {
        let (text, error) = c.call("add_object", bad).await;
        assert!(error, "{text}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_original_game_files_come_back() {
    let f = fixture();
    with_logic(&f);
    let mut config = configured(&f, true);
    config.allow_game = true;
    // A stand-in launch: the real game on the machine is not looked for.
    config.launch = Some(doctor_toolkit::play::LaunchPlan {
        program: "true".into(),
        args: Vec::new(),
        env: Vec::new(),
        dir: f.game.clone(),
        detached: false,
    });
    let before = common::snapshot(&f.game);
    let mut c = Client::start(doctor_toolkit_mcp::open(&config).unwrap());
    c.initialize().await;
    c.json("apply", json!({})).await;
    let level = format!("{LEVEL}.str");
    assert_ne!(common::snapshot(&f.game)[&level].2, before[&level].2);
    let restored = c.json("restore_originals", json!({})).await;
    assert_eq!(restored["files"], 2, "{restored}");
    assert_eq!(restored["changes_kept_in_workspace"], 1);
    assert_eq!(common::snapshot(&f.game)[&level].2, before[&level].2);
    // Not offered without --allow-game.
    let mut plain = start(&f, true).await;
    let listed = plain.request("tools/list", json!({})).await;
    assert!(
        !listed["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "restore_originals")
    );
}

// ------------------------------------------------------------ 104-D

/// A fixture with the stream of sound, animation and interface data.
/// Stage 111: something else — a mod — changes the game under the edits
/// of the workspace. `rebase` lists the stale changes, merges the string
/// table by its operations, leaves the mesh for a decision, and takes the
/// two decisions; every call is one operation `undo` takes back.
#[tokio::test(flavor = "multi_thread")]
async fn stale_changes_are_rebased_kept_or_given_up() {
    use doctor_toolkit::container::ustream::{Replacement, Storage};
    use doctor_toolkit::resource::lh2::StringTable;
    use doctor_toolkit::testing::{MESH, STRINGS};

    let f = fixture();
    let game = GameInstance::open(&f.game).unwrap();
    let level = game.find_stream(LEVEL).unwrap();
    let table_of = |data: &[u8]| StringTable::parse(data).unwrap();
    // A write that leaves a backup, as the mod manager makes one.
    let game_gets = |word: &str, mesh: u8| {
        let original = GameInstance::open(&f.game).unwrap();
        let file = original.open_stream(level).unwrap();
        let mut table = table_of(&file.resource_data(STRINGS).unwrap());
        drop(file);
        table.strings[0][0] = word.into();
        let (strings, mesh) = (table.to_bytes().unwrap(), vec![mesh; 40]);
        let replacements = [
            Replacement {
                index: STRINGS,
                data: &strings,
                storage: Storage::Raw,
            },
            Replacement {
                index: MESH,
                data: &mesh,
                storage: Storage::RefPack,
            },
        ];
        game.replace_resources(level, &replacements, Default::default())
            .unwrap();
    };
    game_gets("City", 2);
    // The edits, made on the game with the mod.
    let mut ws = Workspace::create(&f.workspace, &game).unwrap();
    let mut table = table_of(&ws.current_data(&game, level, STRINGS).unwrap());
    assert_eq!(table.strings[0][0], "City");
    table.strings[0][1] = "Mine".into();
    ws.set_string_table(&game, level, STRINGS, &table).unwrap();
    ws.set_resource(&game, level, MESH, vec![3; 30], "my mesh")
        .unwrap();
    drop(ws);

    let mut c = start(&f, true).await;
    let none = c.json("rebase", json!({ "check": true })).await;
    assert_eq!(none["count"], 0, "{none}");
    let nothing = c.json("rebase", json!({})).await;
    assert_eq!(nothing["changed"], false, "{nothing}");

    // The mod is updated.
    game_gets("Town", 5);
    let listed = c.json("rebase", json!({ "check": true })).await;
    assert_eq!(listed["count"], 2, "{listed}");
    let kinds: Vec<&Value> = listed["stale"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| &s["merged_by"])
        .collect();
    assert!(
        kinds.contains(&&json!("lh2")) && kinds.contains(&&Value::Null),
        "{listed}"
    );
    assert_eq!(Workspace::open_read_only(&f.workspace).unwrap().head(), 2);

    let rebased = c.json("rebase", json!({})).await;
    assert_eq!(rebased["changed"], true, "{rebased}");
    assert_eq!(rebased["done"]["needs_a_decision"], 1, "{rebased}");
    let outcome = |answer: &Value, part: &str| -> Value {
        answer["done"]["changes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["key"].as_str().unwrap().contains(part))
            .unwrap_or_else(|| panic!("{answer}"))
            .clone()
    };
    assert_eq!(outcome(&rebased, "lh2")["outcome"], "rebased", "{rebased}");
    assert_eq!(outcome(&rebased, "lh2")["applied"], 1);
    assert_eq!(outcome(&rebased, "mesh")["outcome"], "needs a decision");
    let (game, ws) = workspace(&f);
    let table = table_of(&ws.current_data(&game, level, STRINGS).unwrap());
    assert_eq!(table.strings[0], ["Town", "Mine"]);
    assert_eq!(ws.head(), 3, "one operation");
    drop(ws);

    // Keep mine, then the other way after an undo.
    let kept = c.json("rebase", json!({ "keep": ["mesh"] })).await;
    assert_eq!(outcome(&kept, "mesh")["outcome"], "kept", "{kept}");
    let clean = c.json("rebase", json!({ "check": true })).await;
    assert_eq!(clean["count"], 0, "{clean}");
    c.json("undo", json!({})).await;
    let taken = c.json("rebase", json!({ "take_base": ["mesh"] })).await;
    assert_eq!(
        outcome(&taken, "mesh")["outcome"],
        "taken from the game",
        "{taken}"
    );
    let (game, ws) = workspace(&f);
    assert_eq!(ws.len(), 1);
    assert_eq!(ws.current_data(&game, level, MESH).unwrap(), [5; 40]);

    for bad in [
        json!({ "check": true, "keep": ["mesh"] }),
        json!({ "keep": ["nothing-like-this"] }),
    ] {
        let (text, error) = c.call("rebase", bad).await;
        assert!(error, "{text}");
    }
}

fn sound_fixture() -> Fixture {
    let f = fixture();
    testing::write_sound_stream(&f.game).unwrap();
    f
}

/// A mono 16-bit WAV of `frames` samples at 22050 Hz.
fn wav(path: &std::path::Path, frames: usize) {
    let mut d = Vec::new();
    let data = (frames * 2) as u32;
    d.extend_from_slice(b"RIFF");
    d.extend_from_slice(&(36 + data).to_le_bytes());
    d.extend_from_slice(b"WAVEfmt ");
    d.extend_from_slice(&16u32.to_le_bytes());
    d.extend_from_slice(&1u16.to_le_bytes());
    d.extend_from_slice(&1u16.to_le_bytes());
    d.extend_from_slice(&22050u32.to_le_bytes());
    d.extend_from_slice(&44100u32.to_le_bytes());
    d.extend_from_slice(&2u16.to_le_bytes());
    d.extend_from_slice(&16u16.to_le_bytes());
    d.extend_from_slice(b"data");
    d.extend_from_slice(&data.to_le_bytes());
    for i in 0..frames {
        d.extend_from_slice(&(((i * 37) % 2000) as i16).to_le_bytes());
    }
    std::fs::write(path, d).unwrap();
}

fn sound_resource(index: usize) -> String {
    format!("{SOUND}#{index}")
}

#[tokio::test(flavor = "multi_thread")]
async fn sounds_of_a_bank_are_edited_and_added() {
    let f = sound_fixture();
    let mut c = start(&f, true).await;
    let bank = sound_resource(SOUND_SBK);
    let edited = c
        .json(
            "sbk_edit",
            json!({ "bank": bank, "emitter": "a.emx", "volume": 0.5, "falloff": 12.0 }),
        )
        .await;
    assert_eq!(edited["changed"], true, "{edited}");
    assert!(
        edited["operation"]
            .as_str()
            .unwrap()
            .contains("volume, falloff")
    );
    let key = edited["done"]["bank"].as_str().unwrap().to_owned();
    let shown = c.data("resource_json", json!({ "resource": key })).await;
    assert!(shown.contains("\"falloff\": 12.0"), "{shown}");
    for bad in [
        json!({ "bank": bank, "emitter": "a.emx" }),
        json!({ "bank": bank, "emitter": "nope.emx", "volume": 1.0 }),
        json!({ "bank": bank, "emitter": "a.emx", "falloff": -1.0 }),
        json!({ "bank": sound_resource(SOUND_MIX), "emitter": "a.emx", "volume": 1.0 }),
    ] {
        let (text, error) = c.call("sbk_edit", bad).await;
        assert!(error, "{text}");
    }

    let file = f.out.join("step.wav");
    wav(&file, 600);
    // A new variation of an emitter, then a new emitter by name.
    let more = c
        .json(
            "sound_add",
            json!({ "bank": bank, "files": [file], "emitter": "b.emx" }),
        )
        .await;
    assert_eq!(more["changed"], true, "{more}");
    assert_eq!(more["done"]["sounds"][0]["sample_rate"], 22050);
    let new = c
        .json(
            "sound_add",
            json!({ "bank": bank, "files": [file], "new": "doctor_step", "like": "a.emx" }),
        )
        .await;
    assert_eq!(new["done"]["new_emitter"][0], "doctor_step.emx", "{new}");
    let game = GameInstance::open(&f.game).unwrap();
    let ws = Workspace::open_read_only(&f.workspace).unwrap();
    let stream = game.find_stream(SOUND).unwrap();
    let parsed = doctor_toolkit::resource::sbk::SoundBank::parse(
        &ws.current_data(&game, stream, SOUND_SBK).unwrap(),
    )
    .unwrap();
    assert_eq!(parsed.emitters.len(), 3);
    assert_eq!(parsed.emitters[1].track.variations.len(), 2);
    assert_eq!(parsed.emitters[0].track.volume, 0.5);
    for bad in [
        json!({ "bank": bank, "files": [file] }),
        json!({ "bank": bank, "files": [file], "new": "doctor_step" }),
        json!({ "bank": bank, "files": [f.out.join("missing.wav")], "emitter": "a.emx" }),
    ] {
        let (text, error) = c.call("sound_add", bad).await;
        assert!(error, "{text}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn speech_tables_and_the_mixer_are_shown_and_changed() {
    let f = sound_fixture();
    let mut c = start(&f, true).await;
    let events = c.json("speech", json!({})).await;
    assert_eq!(events["records"], 1, "{events}");
    assert_eq!(events["events"][0]["name"], "F2F_HELLO");
    let banks = c
        .json("speech", json!({ "show": "banks", "filter": "d_one" }))
        .await;
    assert_eq!(banks["entries"][0]["file"], "d_one.exa", "{banks}");
    assert_eq!(banks["entries"][0]["category"], "CHATTER");
    let speakers = c.json("speech", json!({ "show": "speakers" })).await;
    assert_eq!(speakers["total"], 1, "{speakers}");
    assert_eq!(speakers["speakers"][0]["lines"], 1);
    assert_eq!(speakers["speakers"][0]["voices"], json!(["one.exa"]));
    let (text, error) = c.call("speech", json!({ "show": "nope" })).await;
    assert!(error && text.contains("events, banks, speakers"), "{text}");

    let set = c
        .json(
            "set_speech_events",
            json!({ "events": ["F2F_HELLO", "DOC_NEW_EVENT"], "cooldown": 7.5, "priority": 450 }),
        )
        .await;
    assert_eq!(set["changed"], true, "{set}");
    assert_eq!(set["done"]["events"][0]["new_record"], false);
    assert_eq!(set["done"]["events"][1]["new_record"], true);
    let events = c
        .json(
            "speech",
            json!({ "events": ["DOC_NEW_EVENT", "NOT_THERE"] }),
        )
        .await;
    assert_eq!(events["events"][0]["cooldown"], 7.5, "{events}");
    assert_eq!(events["no_record"], json!(["NOT_THERE"]));
    let (text, error) = c.call("set_speech_events", json!({ "events": [] })).await;
    assert!(error, "{text}");

    // The mixer: the configuration, a snapshot, a channel changed and added.
    let config = c.json("mix", json!({})).await;
    assert_eq!(
        config["categories"].as_array().unwrap().len(),
        2,
        "{config}"
    );
    assert_eq!(config["mixes"][0]["name"], "occlusion.mix");
    let before = c.json("mix", json!({ "name": "occlusion.mix" })).await;
    assert_eq!(before["channels"][1]["category"], "DIALOG", "{before}");
    let changed = c
        .json(
            "set_mix",
            json!({ "channel": { "name": "occlusion.mix", "category": "DIALOG", "volume": 0.25, "time": 1.5 } }),
        )
        .await;
    assert_eq!(changed["changed"], true, "{changed}");
    let after = c.json("mix", json!({ "name": "occlusion.mix" })).await;
    assert_eq!(after["channels"][1]["volume"], 0.25, "{after}");
    assert_eq!(after["channels"][1]["time"], 1.5);
    // A new category, then a channel for it.
    let added = c
        .json(
            "set_mix",
            json!({ "add_category": "DOCTOR_VOICE", "parent": "DIALOG" }),
        )
        .await;
    assert_eq!(added["changed"], true, "{added}");
    c.json(
        "set_mix",
        json!({ "channel": { "name": "occlusion.mix", "category": "DOCTOR_VOICE", "volume": 1.0 } }),
    )
    .await;
    let after = c.json("mix", json!({ "name": "occlusion.mix" })).await;
    assert_eq!(after["channels"].as_array().unwrap().len(), 3, "{after}");
    for bad in [
        json!({}),
        json!({ "add_category": "X" }),
        json!({ "channel": { "name": "occlusion.mix", "category": "NO_SUCH", "volume": 1.0 } }),
        json!({ "channel": { "name": "occlusion.mix", "category": "DIALOG", "volume": 2.0 } }),
        json!({ "channel": { "name": "nope.mix", "category": "DIALOG", "volume": 0.5 } }),
        json!({ "channel": { "name": "occlusion.mix", "category": "DIALOG" } }),
    ] {
        let (text, error) = c.call("set_mix", bad).await;
        assert!(error, "{text}");
    }
    // Reading is there without flags.
    let mut reader = start(&f, false).await;
    reader.json("mix", json!({})).await;
    reader.json("speech", json!({ "show": "speakers" })).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_voice_line_needs_the_archives_of_the_game() {
    let f = sound_fixture();
    let mut c = start(&f, true).await;
    let speakers = c.json("speech", json!({ "show": "speakers" })).await;
    let speaker = speakers["speakers"][0]["key"].as_str().unwrap().to_owned();
    let file = f.out.join("line.wav");
    wav(&file, 4000);
    // Bad names are refused before anything is read or recorded.
    for bad in [
        json!({ "speaker": speaker, "file": file, "event": "F2F_HELLO", "name": "Bad Name" }),
        json!({ "speaker": speaker, "file": file, "event": "bad event", "name": "doc_one" }),
        json!({ "speaker": "nonsense", "file": file, "event": "F2F_HELLO", "name": "doc_one" }),
        json!({ "speaker": speaker, "file": f.out.join("none.wav"), "event": "F2F_HELLO", "name": "doc_one" }),
    ] {
        let (text, error) = c.call("voice_add", bad).await;
        assert!(error, "{text}");
    }
    assert!(!Workspace::exists(&f.workspace));
    // The line itself: the sound goes into the overlay archive.
    let (text, error) = c
        .call(
            "voice_add",
            json!({
                "speaker": speaker, "file": file, "event": "F2F_HELLO", "name": "doc_two",
                "subtitles": { "en": "Hello." },
            }),
        )
        .await;
    assert!(!error, "{text}");
    let done: Value = serde_json::from_str(&common::unfence(&text, "voice_add")).unwrap();
    assert_eq!(done["changed"], true, "{done}");
    assert_eq!(done["done"]["file"], "d_doc_two.exa");
    assert_eq!(done["done"]["new_event"], false);
    let speakers = c.json("speech", json!({ "show": "speakers" })).await;
    assert_eq!(speakers["speakers"][0]["lines"], 2, "{speakers}");
}

#[tokio::test(flavor = "multi_thread")]
async fn parts_of_an_interface_file_are_exported_and_replaced() {
    let f = sound_fixture();
    let mut c = start(&f, true).await;
    let apx = sound_resource(SOUND_APX);
    let key = format!("{SOUND}:apx:00000057");
    let shown = c.data("resource_json", json!({ "resource": key })).await;
    assert!(shown.contains("$menu_start"), "{shown}");
    let exported = c
        .json("export", json!({ "resource": apx, "name": "hud" }))
        .await;
    let files: Vec<String> = exported["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap().to_owned())
        .collect();
    assert!(files.iter().any(|p| p.ends_with("hud.apt")), "{exported}");
    assert!(files.iter().any(|p| p.ends_with("hud.const")));
    // The movie back with one more byte, and the reference pointed elsewhere.
    let apt = f.out.join("hud.apt");
    let mut movie = std::fs::read(&apt).unwrap();
    movie.push(7);
    std::fs::write(&apt, &movie).unwrap();
    let done = c
        .json(
            "apx_edit",
            json!({ "apx": apx, "apt": apt, "text_refs": { "$menu_start": "menu_go" } }),
        )
        .await;
    assert_eq!(done["changed"], true, "{done}");
    assert_eq!(done["done"]["changes"].as_array().unwrap().len(), 2);
    let after = c.data("resource_json", json!({ "resource": key })).await;
    assert!(
        after.contains("$menu_go") && !after.contains("$menu_start"),
        "{after}"
    );
    for bad in [
        json!({ "apx": apx }),
        json!({ "apx": apx, "texture": "0" }),
        json!({ "apx": apx, "text_refs": { "not_there": "x" } }),
        json!({ "apx": sound_resource(SOUND_MIX), "text_refs": { "a": "b" } }),
        json!({ "apx": apx, "apt": f.out.join("hud.const") }),
    ] {
        let (text, error) = c.call("apx_edit", bad).await;
        assert!(error, "{text}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn clip_events_and_the_graph_of_an_animation_bank() {
    let f = sound_fixture();
    let mut c = start(&f, true).await;
    let bank = sound_resource(SOUND_BNK);
    let shown = c
        .json("clip_events", json!({ "bank": bank, "clip": "0x10" }))
        .await;
    assert_eq!(shown["events"][0]["name"], "footstep", "{shown}");
    let mut events = shown["events"].as_array().unwrap().clone();
    let mut second = events[0].clone();
    second["frame"] = json!(4.0);
    events.push(second);
    let set = c
        .json(
            "clip_events",
            json!({ "bank": bank, "clip": "0x10", "events": events }),
        )
        .await;
    assert_eq!(set["changed"], true, "{set}");
    assert_eq!(set["done"]["events"], 2);
    let shown = c
        .json("clip_events", json!({ "bank": bank, "clip": "0x10" }))
        .await;
    assert_eq!(shown["events"].as_array().unwrap().len(), 2, "{shown}");
    let (text, error) = c
        .call("clip_events", json!({ "bank": bank, "clip": "nope" }))
        .await;
    assert!(error && text.contains("no clip"), "{text}");
    let (text, error) = c
        .call(
            "clip_events",
            json!({ "bank": bank, "clip": "0x10", "events": [{ "nonsense": 1 }] }),
        )
        .await;
    assert!(error && text.contains("events"), "{text}");

    // The graph: entries, one entry, the whole graph as a file and back.
    let graph = c.json("animation_graph", json!({ "bank": bank })).await;
    assert_eq!(graph["entries"], 1, "{graph}");
    assert_eq!(graph["entry_names"], json!(["idle"]));
    let entry = c
        .json(
            "animation_graph",
            json!({ "bank": bank, "entry": "idle", "save_as": "graph" }),
        )
        .await;
    assert_eq!(entry["graph"]["entries"][0]["name"], "idle", "{entry}");
    let file = entry["saved"].as_str().unwrap().to_owned();
    // The same graph: nothing changes.
    let same = c
        .json("set_animation_graph", json!({ "bank": bank, "file": file }))
        .await;
    assert_eq!(same["changed"], false, "{same}");
    let mut edited = entry["graph"].clone();
    edited["entries"][0]["name"] = json!("idle_two");
    let set = c
        .json(
            "set_animation_graph",
            json!({ "bank": bank, "graph": edited }),
        )
        .await;
    assert_eq!(set["changed"], true, "{set}");
    let graph = c.json("animation_graph", json!({ "bank": bank })).await;
    assert_eq!(graph["entry_names"], json!(["idle_two"]), "{graph}");
    for bad in [
        json!({ "bank": bank }),
        json!({ "bank": bank, "graph": { "nonsense": true } }),
        json!({ "bank": sound_resource(SOUND_MIX), "graph": { "entries": [] } }),
    ] {
        let (text, error) = c.call("set_animation_graph", bad).await;
        assert!(error, "{text}");
    }
    // A file that holds no animation is refused and nothing is recorded.
    let before = Workspace::open_read_only(&f.workspace).unwrap().head();
    let not_animation = f.out.join("mesh.glb");
    std::fs::write(&not_animation, testing::triangle_glb(0.0, "m", false)).unwrap();
    let (text, error) = c
        .call(
            "import_animation",
            json!({ "bank": bank, "file": not_animation, "clip": "0x10" }),
        )
        .await;
    assert!(error, "{text}");
    assert_eq!(
        Workspace::open_read_only(&f.workspace).unwrap().head(),
        before
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_level_without_a_world_grid_says_so() {
    let f = fixture();
    let mut c = start(&f, true).await;
    let (text, error) = c.call("zone_cells", json!({ "level": LEVEL })).await;
    assert!(error && text.contains("no world grid"), "{text}");
    let (text, error) = c
        .call(
            "remove_zone_cell",
            json!({ "level": LEVEL, "stream": TEXT }),
        )
        .await;
    assert!(error, "{text}");
    assert!(!Workspace::exists(&f.workspace));
}

// ------------------------------------------------------------ 104-G

#[tokio::test(flavor = "multi_thread")]
async fn several_objects_are_edited_in_one_operation() {
    let f = fixture();
    with_logic(&f);
    let mut c = start(&f, true).await;
    let group = format!("{LEVEL}#{SIMGROUP}");
    let id = |n: usize| format!("{group}:{n}");
    // Two copies of the trigger (it stands at 10, 2, -5), 1 m and 2 m along X.
    let (text, error) = c
        .call("duplicate_object", json!({ "objects": [id(0), id(0)] }))
        .await;
    assert!(error && text.contains("named twice"), "{text}");
    let copies = c
        .json(
            "duplicate_object",
            json!({ "objects": [id(0), id(1)], "offset": [1.0, 0.0, 0.0] }),
        )
        .await;
    assert_eq!(
        copies["done"]["copies"].as_array().unwrap().len(),
        2,
        "{copies}"
    );
    assert_eq!(copies["operation"], "Duplicate 2 objects");
    let a = copies["done"]["copies"][0]["copy"]
        .as_str()
        .unwrap()
        .to_owned();
    let head = || Workspace::open_read_only(&f.workspace).unwrap().head();
    let before = head();
    // The trigger and its copy move together: one operation, one undo.
    let moved = c
        .json(
            "move_object",
            json!({ "objects": [id(0), a], "offset": [0.0, 3.0, 0.0] }),
        )
        .await;
    assert_eq!(moved["operation"], "Move 2 objects", "{moved}");
    assert_eq!(
        moved["done"]["moved"][0]["new_position"],
        json!([10.0, 5.0, -5.0])
    );
    assert_eq!(
        moved["done"]["moved"][1]["new_position"],
        json!([11.0, 5.0, -5.0])
    );
    assert_eq!(head(), before + 1);
    c.json("undo", json!({})).await;
    let objects = c
        .json(
            "level_objects",
            json!({ "stream": LEVEL, "filter": "Trigger" }),
        )
        .await;
    assert!(objects.to_string().contains("[10.0,2.0,-5.0]"), "{objects}");
    for bad in [
        json!({ "objects": [id(0), a], "position": [0, 0, 0] }),
        json!({ "object": id(0), "objects": [a], "offset": [0, 1, 0] }),
        json!({ "objects": [id(0), id(0)], "offset": [0, 1, 0] }),
        json!({ "objects": [id(0), format!("{group}:99")], "offset": [0, 1, 0] }),
        json!({ "offset": [0, 1, 0] }),
    ] {
        let (text, error) = c.call("move_object", bad).await;
        assert!(error, "{text}");
    }
    assert_eq!(head(), before, "a refused call records nothing");
    // The two copies go at once, numbered as before the call.
    let count = |v: &Value| v["total"].as_u64().unwrap();
    let all = c.json("level_objects", json!({ "stream": LEVEL })).await;
    let removed = c
        .json(
            "remove_object",
            json!({ "objects": [format!("{group}:3"), format!("{group}:4")] }),
        )
        .await;
    assert_eq!(removed["operation"], "Remove 2 objects", "{removed}");
    let left = c.json("level_objects", json!({ "stream": LEVEL })).await;
    assert_eq!(count(&left) + 2, count(&all), "{left}");
    let (text, error) = c
        .call(
            "remove_object",
            json!({ "objects": [format!("{group}:0"), format!("{group}:9")] }),
        )
        .await;
    assert!(error, "{text}");
    assert_eq!(
        count(&c.json("level_objects", json!({ "stream": LEVEL })).await),
        count(&left)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_ground_of_a_level_without_collision_is_not_made_up() {
    let f = fixture();
    with_logic(&f);
    let mut c = start(&f, true).await;
    let object = format!("{LEVEL}#{SIMGROUP}:0");
    let before = Workspace::open_read_only(&f.workspace).unwrap().head();
    let (text, error) = c
        .call(
            "move_object",
            json!({ "object": object, "on_ground": true }),
        )
        .await;
    assert!(error && text.contains("no collision"), "{text}");
    let (text, error) = c
        .call(
            "add_object",
            json!({
                "group": format!("{LEVEL}#{SIMGROUP}"), "template": object,
                "position": [1.0, 2.0, 3.0], "on_ground": true,
            }),
        )
        .await;
    assert!(error && text.contains("no collision"), "{text}");
    let (text, error) = c
        .call(
            "add_object",
            json!({ "group": format!("{LEVEL}#{SIMGROUP}"), "template": object, "on_ground": true }),
        )
        .await;
    assert!(error && text.contains("position"), "{text}");
    assert_eq!(
        Workspace::open_read_only(&f.workspace).unwrap().head(),
        before
    );
    // Reading the ground and a walk: no collision is said, not invented.
    let none = c
        .json(
            "ground",
            json!({ "streams": [LEVEL], "point": [10.0, 2.0, -5.0] }),
        )
        .await;
    assert_eq!(none["ground"], Value::Null, "{none}");
    assert_eq!(none["world"]["colliders"], 0);
    let ray = c
        .json(
            "ground",
            json!({ "streams": [LEVEL], "ray_origin": [0, 10, 0], "ray_direction": [0, -1, 0] }),
        )
        .await;
    assert_eq!(ray["ground"], Value::Null, "{ray}");
    let (text, error) = c
        .call(
            "walk_route",
            json!({ "streams": [LEVEL], "from": [0, 0, 0], "to": [3, 0, 0] }),
        )
        .await;
    assert!(error && text.contains("no collision to walk on"), "{text}");
    for (tool, bad) in [
        ("ground", json!({ "streams": [LEVEL] })),
        (
            "ground",
            json!({ "streams": ["no/such/zone"], "point": [0, 0, 0] }),
        ),
        ("ground", json!({ "streams": [], "point": [0, 0, 0] })),
        (
            "ground",
            json!({ "streams": [LEVEL], "ray_origin": [0, 0, 0], "ray_direction": [0, 0, 0] }),
        ),
        (
            "walk_route",
            json!({ "streams": [LEVEL], "from": [0, 0, 0], "to": [9000, 0, 0] }),
        ),
        (
            "walk_route",
            json!({ "streams": [LEVEL], "from": [0, 0, 0], "to": [1, 0, 0], "radius": 2.0 }),
        ),
    ] {
        let (text, error) = c.call(tool, bad).await;
        assert!(error, "{tool}: {text}");
    }
}

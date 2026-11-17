//! Mods through the server (stage 104) on the miniature game: a mod with
//! executable code is built and installed and its state is read, but the
//! consent to the code cannot be given here; a mod file is looked into
//! before it is installed; the workspace is installed as a mod; an
//! installed mod is exported; sets of mods are saved, used and deleted.

mod common;

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use common::{Client, config, logic_group, snapshot};
use doctor_toolkit::game::GameInstance;
use doctor_toolkit::mods::ModManager;
use doctor_toolkit::mods::native::{self, Answer, Consent};
use doctor_toolkit::testing::{self, LEVEL, SIMGROUP};
use doctor_toolkit::workspace::Workspace;

struct Fixture {
    _dirs: Vec<tempfile::TempDir>,
    game: PathBuf,
    workspace: PathBuf,
    out: PathBuf,
    files: PathBuf,
}

/// The miniature game and a workspace with one change.
fn fixture() -> Fixture {
    let game_dir = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    testing::write_game(game_dir.path()).unwrap();
    let game = GameInstance::open(game_dir.path()).unwrap();
    let workspace = other.path().join("ws");
    let mut ws = Workspace::create(&workspace, &game).unwrap();
    let level = game.find_stream(LEVEL).unwrap();
    ws.set_resource(&game, level, SIMGROUP, logic_group().encode(), "New logic")
        .unwrap();
    let files = other.path().join("files");
    std::fs::create_dir_all(&files).unwrap();
    Fixture {
        game: game_dir.path().to_path_buf(),
        workspace,
        out: other.path().join("out"),
        files,
        _dirs: vec![game_dir, other],
    }
}

fn start(f: &Fixture) -> Client {
    let mut c = config(&f.game);
    c.workspace = Some(f.workspace.clone());
    c.allow_edits = true;
    c.allow_game = true;
    c.output_dir = Some(f.out.clone());
    Client::start(doctor_toolkit_mcp::open(&c).unwrap())
}

/// The smallest thing the mod format takes for a 32-bit DLL.
fn dll(dir: &Path, name: &str, tag: &str) -> String {
    let mut data = vec![0u8; 0x100];
    data[..2].copy_from_slice(b"MZ");
    data[0x3C] = 0x40;
    data[0x40..0x44].copy_from_slice(b"PE\0\0");
    data[0x44..0x46].copy_from_slice(&0x014Cu16.to_le_bytes());
    data[0x56..0x58].copy_from_slice(&0x2102u16.to_le_bytes());
    data.extend_from_slice(b"doctor_mod_init ");
    data.extend_from_slice(tag.as_bytes());
    let path = dir.join(name);
    std::fs::write(&path, data).unwrap();
    path.to_string_lossy().into_owned()
}

fn game_files(dir: &Path) -> Vec<(String, (u64, std::time::SystemTime, u64))> {
    snapshot(dir)
        .into_iter()
        .filter(|(path, _)| !path.starts_with("_doctor_toolkit"))
        .collect()
}

fn text(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_mod_with_code_is_built_and_installed_but_the_consent_is_the_players() {
    let f = fixture();
    let before = game_files(&f.game);
    let mut c = start(&f);
    c.initialize().await;
    let first = dll(&f.files, "doctor_demo.dll", "one");

    // Built with its code: format 5, and the answer says who agrees.
    let built = c
        .json(
            "mod_build",
            json!({ "name": "Coded", "version": "1.0.0", "native": [first], "native_required": true }),
        )
        .await;
    assert_eq!(built["format"], 5, "{built}");
    assert_eq!(built["code"]["modules"], 1, "{built}");
    assert!(text(&built["code"]["note"]).contains("player agrees"));
    let file = text(&built["file"]).to_owned();
    let (error_text, error) = c
        .call(
            "mod_build",
            json!({ "name": "Coded", "native_required": true }),
        )
        .await;
    assert!(error && error_text.contains("native"), "{error_text}");
    let (error_text, error) = c
        .call(
            "mod_build",
            json!({ "name": "Coded", "native": ["relative.dll"] }),
        )
        .await;
    assert!(error, "{error_text}");

    // What the file holds, before it is installed.
    let info = c
        .json(
            "mods",
            json!({ "action": "info", "file": file, "resources": true }),
        )
        .await;
    assert_eq!(info["id"], "coded", "{info}");
    assert_eq!(info["code"]["required"], true);
    assert_eq!(
        info["code"]["modules"][0]["file"],
        "scripts/doctor_demo.asi"
    );
    assert!(text(&info["code"]["note"]).contains("EXECUTABLE CODE"));
    assert_eq!(info["resource_list"].as_array().unwrap().len(), 1);
    let (error_text, error) = c.call("mods", json!({ "action": "info" })).await;
    assert!(error && error_text.contains("file"), "{error_text}");
    // A server that only reads opens no file the client names.
    let mut read_only = config(&f.game);
    read_only.workspace = Some(f.workspace.clone());
    let mut r = Client::start(doctor_toolkit_mcp::open(&read_only).unwrap());
    r.initialize().await;
    let (error_text, error) = r
        .call("mods", json!({ "action": "info", "file": file }))
        .await;
    assert!(
        error && error_text.contains("--allow-edits"),
        "{error_text}"
    );
    let listed = r.json("mods", json!({ "action": "native" })).await;
    assert_eq!(listed["mods"], json!([]), "{listed}");

    // Installed: the code waits for the player.
    let installed = c
        .json(
            "mods_manage",
            json!({ "action": "install", "files": [file] }),
        )
        .await;
    let code = &installed["done"]["installed"][0]["code"];
    assert_eq!(
        code["answer"], "waits for the player's consent",
        "{installed}"
    );
    assert!(text(&code["where_to_agree"]).contains("mods native"));
    assert!(
        text(&installed["mods"][0]["code"]).contains("consent"),
        "{installed}"
    );
    let state = c.json("mods", json!({ "action": "native" })).await;
    assert_eq!(state["mods"][0]["id"], "coded", "{state}");
    assert_eq!(state["mods"][0]["answer"], "waits for the player's consent");
    assert_eq!(state["in_the_game"], json!([]));
    let one = c
        .json("mods", json!({ "action": "native", "id": "coded" }))
        .await;
    assert_eq!(
        one["would_install"][0]["file"], "scripts/doctor_demo.asi",
        "{one}"
    );
    assert_eq!(one["required"], true);
    let (error_text, error) = c
        .call("mods", json!({ "action": "native", "id": "nope" }))
        .await;
    assert!(error, "{error_text}");

    // No tool and no action gives the answer.
    let tools = c.request("tools/list", json!({})).await;
    let listed = tools["result"]["tools"].to_string();
    for word in ["\"allow\"", "\"consent\"", "data_only", "\"agree\""] {
        assert!(!listed.contains(word), "a parameter {word}");
    }
    for action in ["native", "allow", "agree", "consent"] {
        let (error_text, error) = c
            .call("mods_manage", json!({ "action": action, "ids": ["coded"] }))
            .await;
        assert!(error, "{action}: {error_text}");
    }
    let game = GameInstance::open(&f.game).unwrap();
    let manager = ModManager::open(&game).unwrap();
    assert_eq!(
        native::consent(&manager, "coded").unwrap(),
        Consent::Unasked
    );
    // Written into the game without the answer: a mod that needs its code
    // is not written at all, and nothing executable is there.
    let (_, _) = c.call("mods_manage", json!({ "action": "apply" })).await;
    assert!(!f.game.join("scripts/doctor_demo.asi").exists());
    c.json("mods_manage", json!({ "action": "unapply" })).await;

    // The player agrees (in the mod manager, here through the library).
    native::set_consent(&manager, "coded", Some(Answer::Code)).unwrap();
    let state = c
        .json("mods", json!({ "action": "native", "id": "coded" }))
        .await;
    assert_eq!(state["answer"], "the player agreed to the code", "{state}");

    // The same code in a new version installs; other code does not: the
    // answer holds for later versions, so the server must not change it.
    let same = c
        .json(
            "mod_build",
            json!({ "name": "Coded", "version": "1.1.0", "native": [first] }),
        )
        .await;
    c.json(
        "mods_manage",
        json!({ "action": "install", "files": [same["file"]] }),
    )
    .await;
    let second = dll(&f.files, "doctor_demo.dll", "two");
    let other = c
        .json(
            "mod_build",
            json!({ "name": "Coded", "version": "1.2.0", "native": [second] }),
        )
        .await;
    let (error_text, error) = c
        .call(
            "mods_manage",
            json!({ "action": "install", "files": [other["file"]] }),
        )
        .await;
    assert!(error && error_text.contains("other code"), "{error_text}");
    let (error_text, error) = c
        .call(
            "mods_manage",
            json!({ "action": "install_workspace", "mod": { "name": "Coded", "native": [second] } }),
        )
        .await;
    assert!(error && error_text.contains("other code"), "{error_text}");
    let list = c.json("mods", json!({})).await;
    assert_eq!(list["mods"][0]["version"], "1.1.0", "{list}");

    c.json(
        "mods_manage",
        json!({ "action": "uninstall", "ids": ["coded"] }),
    )
    .await;
    assert_eq!(game_files(&f.game), before, "the game files are untouched");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_workspace_is_installed_a_mod_is_exported_and_sets_are_kept() {
    let f = fixture();
    let before = game_files(&f.game);
    let mut c = start(&f);
    c.initialize().await;

    // The workspace as a mod.
    let (error_text, error) = c
        .call("mods_manage", json!({ "action": "install_workspace" }))
        .await;
    assert!(error && error_text.contains("mod"), "{error_text}");
    let installed = c
        .json(
            "mods_manage",
            json!({ "action": "install_workspace", "mod": { "name": "My work", "version": "0.1.0" } }),
        )
        .await;
    assert_eq!(
        installed["done"]["installed"]["id"], "my-work",
        "{installed}"
    );
    assert_eq!(installed["done"]["code"], Value::Null);
    assert_eq!(installed["mods"][0]["source"], "workspace");
    assert_eq!(installed["mods"][0]["enabled"], true);

    // Exported as a file into the output folder, and looked into.
    let exported = c
        .json(
            "mods_manage",
            json!({ "action": "export", "ids": ["my-work"] }),
        )
        .await;
    let file = PathBuf::from(text(&exported["done"]["file"]));
    assert!(file.starts_with(&f.out) && file.is_file(), "{exported}");
    let info = c
        .json("mods", json!({ "action": "info", "file": file }))
        .await;
    assert_eq!(info["id"], "my-work", "{info}");
    assert_eq!(info["resources"], 1);
    assert_eq!(info["code"], Value::Null);
    let (error_text, error) = c
        .call("mods_manage", json!({ "action": "export", "ids": [] }))
        .await;
    assert!(error, "{error_text}");

    // Sets: the order and which mods are enabled.
    let saved = c
        .json(
            "mods_manage",
            json!({ "action": "save_set", "set": "all on" }),
        )
        .await;
    assert_eq!(saved["current_set"], "all on", "{saved}");
    c.json(
        "mods_manage",
        json!({ "action": "disable", "ids": ["my-work"] }),
    )
    .await;
    c.json(
        "mods_manage",
        json!({ "action": "save_set", "set": "vanilla" }),
    )
    .await;
    let sets = c.json("mods", json!({ "action": "sets" })).await;
    assert_eq!(sets["current"], "vanilla", "{sets}");
    assert_eq!(sets["sets"].as_array().unwrap().len(), 2);
    assert_eq!(sets["sets"][0]["mods"][0]["enabled"], true);
    let list = c.json("mods", json!({})).await;
    assert_eq!(list["sets"], json!(["all on", "vanilla"]), "{list}");
    let used = c
        .json(
            "mods_manage",
            json!({ "action": "use_set", "set": "all on" }),
        )
        .await;
    assert_eq!(used["mods"][0]["enabled"], true, "{used}");
    assert_eq!(used["done"]["not_installed"], json!([]));
    c.json(
        "mods_manage",
        json!({ "action": "delete_set", "set": "vanilla" }),
    )
    .await;
    let (error_text, error) = c
        .call(
            "mods_manage",
            json!({ "action": "delete_set", "set": "vanilla" }),
        )
        .await;
    assert!(error && error_text.contains("vanilla"), "{error_text}");
    let (error_text, error) = c.call("mods_manage", json!({ "action": "use_set" })).await;
    assert!(error && error_text.contains("set"), "{error_text}");

    // A game the old toolkit never patched has nothing to take over.
    let (error_text, error) = c
        .call("mods_manage", json!({ "action": "import_old" }))
        .await;
    assert!(error && error_text.contains("old toolkit"), "{error_text}");

    c.json(
        "mods_manage",
        json!({ "action": "uninstall", "ids": ["my-work"] }),
    )
    .await;
    assert_eq!(game_files(&f.game), before, "the game files are untouched");
}

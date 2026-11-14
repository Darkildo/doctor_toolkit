//! `doctor-toolkit mcp` as a client starts it: a child process speaking
//! JSON-RPC on stdin/stdout.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

#[test]
fn mcp_serves_over_stdio_and_prints_only_protocol_messages() {
    let game = tempfile::tempdir().unwrap();
    let logs = tempfile::tempdir().unwrap();
    doctor_toolkit::testing::write_game(game.path()).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_doctor-toolkit"))
        .arg("mcp")
        .arg(game.path())
        .arg("--no-cache")
        .env("DOCTOR_TOOLKIT_LOG_DIR", logs.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let send = |stdin: &mut std::process::ChildStdin, message: Value| {
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
    };
    let mut read = |id: u64| -> Value {
        loop {
            let mut line = String::new();
            assert!(stdout.read_line(&mut line).unwrap() > 0, "stdout closed");
            let message: Value = serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("stdout must carry only JSON-RPC: {e}: {line}"));
            if message["id"] == json!(id) {
                return message;
            }
        }
    };
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "cli-test", "version": "1" },
            },
        }),
    );
    let init = read(1);
    assert_eq!(
        init["result"]["serverInfo"]["name"], "doctor-toolkit",
        "{init}"
    );
    send(
        &mut stdin,
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    );
    send(
        &mut stdin,
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    );
    let tools = read(2);
    assert_eq!(
        tools["result"]["tools"].as_array().unwrap().len(),
        25,
        "{tools}"
    );
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": { "name": "search", "arguments": { "query": "brick_wall" } },
        }),
    );
    let found = read(3);
    let text = found["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("<game-data source=\"search\">"), "{text}");
    assert!(text.contains("material"), "{text}");
    drop(stdin);
    let status = child.wait().unwrap();
    assert!(status.success(), "{status}");
    assert!(!game.path().join("_doctor_toolkit").exists());
}

#[test]
fn mcp_with_allow_edits_records_in_the_workspace_only() {
    let game = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    doctor_toolkit::testing::write_game(game.path()).unwrap();
    let ws = other.path().join("ws");
    let mut child = Command::new(env!("CARGO_BIN_EXE_doctor-toolkit"))
        .arg("mcp")
        .arg(game.path())
        .args(["--no-cache", "--allow-edits", "--workspace"])
        .arg(&ws)
        .env("DOCTOR_TOOLKIT_LOG_DIR", other.path().join("logs"))
        // Never the bridge of an editor the user runs.
        .env("DOCTOR_TOOLKIT_BRIDGE", other.path().join("no-editor.sock"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut call = |id: u64, method: &str, params: Value| -> Value {
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
        loop {
            let mut line = String::new();
            assert!(stdout.read_line(&mut line).unwrap() > 0, "stdout closed");
            let message: Value = serde_json::from_str(&line).unwrap();
            if message["id"] == json!(id) {
                return message;
            }
        }
    };
    let init = call(
        1,
        "initialize",
        json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "cli-test", "version": "1" },
        }),
    );
    assert!(
        init["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("--allow-edits")
    );
    let tools = call(2, "tools/list", json!({}));
    let names: Vec<String> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        names.iter().any(|n| n == "edit_string") && !names.iter().any(|n| n == "play"),
        "{names:?}"
    );
    let edited = call(
        3,
        "tools/call",
        json!({ "name": "edit_string", "arguments": { "entry": "0x00000001", "text": "Salut" } }),
    );
    let text = edited["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("\"changed\": true") && text.contains("Salut"),
        "{text}"
    );
    drop(stdin);
    assert!(child.wait().unwrap().success());
    assert!(ws.join("manifest.json").is_file());
    assert!(!game.path().join("_doctor_toolkit").exists());
}

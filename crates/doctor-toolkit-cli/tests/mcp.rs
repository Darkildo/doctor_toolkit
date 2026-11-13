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
        12,
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

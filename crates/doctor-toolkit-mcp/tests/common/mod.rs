//! A JSON-RPC client over a pipe to the server, and the miniature game.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::Arc;

use serde_json::{Value, json};
use tokio::io::{
    AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines, ReadHalf, WriteHalf,
};

use doctor_toolkit_mcp::{Config, Toolkit};

/// A client speaking newline-delimited JSON-RPC, as MCP over stdio does.
pub struct Client {
    write: WriteHalf<DuplexStream>,
    lines: Lines<BufReader<ReadHalf<DuplexStream>>>,
    next: u64,
    pub server: tokio::task::JoinHandle<()>,
}

impl Client {
    /// Starts the server for `kit` on a pipe and connects to it.
    pub fn start(kit: Arc<Toolkit>) -> Self {
        let (client, server) = tokio::io::duplex(1 << 20);
        let (sr, sw) = tokio::io::split(server);
        let handle = tokio::spawn(async move {
            doctor_toolkit_mcp::serve(kit, (sr, sw)).await.unwrap();
        });
        let (cr, cw) = tokio::io::split(client);
        Self {
            write: cw,
            lines: BufReader::new(cr).lines(),
            next: 1,
            server: handle,
        }
    }

    pub async fn send(&mut self, message: &Value) {
        let mut line = serde_json::to_string(message).unwrap();
        line.push('\n');
        self.write.write_all(line.as_bytes()).await.unwrap();
        self.write.flush().await.unwrap();
    }

    /// Sends a request and returns the whole response (`result` or `error`).
    pub async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .await;
        loop {
            let line =
                tokio::time::timeout(std::time::Duration::from_secs(120), self.lines.next_line())
                    .await
                    .expect("the server answers in time")
                    .unwrap()
                    .expect("the server keeps the pipe open");
            let message: Value = serde_json::from_str(&line).expect("one JSON message per line");
            assert_eq!(message["jsonrpc"], "2.0", "{line}");
            if message["id"] == json!(id) {
                return message;
            }
        }
    }

    pub async fn notify(&mut self, method: &str) {
        self.send(&json!({ "jsonrpc": "2.0", "method": method }))
            .await;
    }

    /// `initialize` and `notifications/initialized`; returns the result.
    pub async fn initialize(&mut self) -> Value {
        let response = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": { "name": "test-client", "version": "1.0" },
                }),
            )
            .await;
        self.notify("notifications/initialized").await;
        response["result"].clone()
    }

    /// Calls a tool; returns `(text, is_error)`.
    pub async fn call(&mut self, tool: &str, arguments: Value) -> (String, bool) {
        let response = self
            .request(
                "tools/call",
                json!({ "name": tool, "arguments": arguments }),
            )
            .await;
        let result = &response["result"];
        assert!(result.is_object(), "{tool}: {response}");
        let text = result["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        (text, result["isError"] == json!(true))
    }

    /// Calls a tool that must succeed and returns its fenced data.
    pub async fn data(&mut self, tool: &str, arguments: Value) -> String {
        let (text, error) = self.call(tool, arguments.clone()).await;
        assert!(!error, "{tool} {arguments}: {text}");
        unfence(&text, tool)
    }

    /// Calls a tool that must succeed and returns its data as JSON.
    pub async fn json(&mut self, tool: &str, arguments: Value) -> Value {
        let data = self.data(tool, arguments).await;
        serde_json::from_str(&data).unwrap_or_else(|e| panic!("{tool}: {e}: {data}"))
    }
}

/// The data inside the fence of a tool result.
pub fn unfence(text: &str, tool: &str) -> String {
    assert!(text.starts_with("Data read from the game files"), "{text}");
    let open = format!("<game-data source=\"{tool}\">\n");
    let start = text
        .find(&open)
        .unwrap_or_else(|| panic!("no fence: {text}"))
        + open.len();
    let end = text.rfind("\n</game-data>").expect("closing fence");
    text[start..end].to_owned()
}

/// Every file under `dir`: path → (size, modification time, hash of the bytes).
pub fn snapshot(dir: &Path) -> BTreeMap<String, (u64, std::time::SystemTime, u64)> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let path = e.path();
            let meta = e.metadata().unwrap();
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            let mut h = std::collections::hash_map::DefaultHasher::new();
            std::fs::read(&path).unwrap().hash(&mut h);
            out.insert(
                path.strip_prefix(dir).unwrap().display().to_string(),
                (meta.len(), meta.modified().unwrap(), h.finish()),
            );
        }
    }
    out
}

/// Config for a game without an index cache (nothing written anywhere).
pub fn config(game: &Path) -> Config {
    let mut config = Config::new(game);
    config.index_cache = None;
    config
}

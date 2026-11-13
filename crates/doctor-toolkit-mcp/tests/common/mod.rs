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

use doctor_toolkit::hash::string_id;
use doctor_toolkit::logic_graph::LogicSchema;
use doctor_toolkit::resource::simgroup::{
    ATTR_LOAD_MATRIX, CLASS_SYSTEM_COMMANDS, Guid, SimGroup, Value as SimValue,
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
    // Never the bridge of an editor the user runs.
    config.bridge = None;
    config
}

/// Class of the packet with `CMD_SetName` (attribute 0).
pub const CLASS_SET_NAME: u32 = 0xc8f2_456f;

/// Adds an object of class `name` called `title` with string attributes;
/// it has the packets of a placed object (matrix, name) first.
pub fn add(
    group: &mut SimGroup,
    name: &str,
    title: &str,
    n: u8,
    values: &[(&str, u16, &str)],
) -> usize {
    let class = string_id(name);
    let mut layout = vec![(CLASS_SYSTEM_COMMANDS, 2), (CLASS_SET_NAME, 1)];
    layout.extend_from_slice(LogicSchema::gf2().layout(class).expect("layout"));
    let mut guid = [n; 16];
    guid[8..].copy_from_slice(b"TESTTEST");
    let o = group.push_object(class, Guid(guid), &layout).unwrap();
    group
        .set_class_attr(o, CLASS_SET_NAME, 0, Some(SimValue::String(title.into())))
        .unwrap();
    for &(packet, index, value) in values {
        group
            .set_class_attr(
                o,
                string_id(packet),
                index,
                Some(SimValue::String(value.into())),
            )
            .unwrap();
    }
    o
}

/// A trigger starts a MultiManager that spawns guards and sends a message
/// nobody hears; the trigger stands at (10, 2, -5).
pub fn logic_group() -> SimGroup {
    let mut g = SimGroup::empty();
    let trigger = add(
        &mut g,
        "TriggerAuto",
        "Trigger",
        1,
        &[("TriggerAuto", 0, "start")],
    );
    add(
        &mut g,
        "MultiManager",
        "Manager",
        2,
        &[
            ("MultiManager", 0, "start"),
            ("MultiManager", 3, "Spawn_Guards"),
            ("MultiManager", 5, "nobody_hears"),
        ],
    );
    add(
        &mut g,
        "FuncSpawn",
        "Spawner",
        3,
        &[("FuncSpawn", 0, "spawn_guards")],
    );
    let mut m = [[0.0f32; 4]; 4];
    for (i, row) in m.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    m[3] = [10.0, 2.0, -5.0, 1.0];
    g.set_class_attr(
        trigger,
        CLASS_SYSTEM_COMMANDS,
        ATTR_LOAD_MATRIX,
        Some(SimValue::Matrix(m)),
    )
    .unwrap();
    SimGroup::parse(&g.encode()).unwrap()
}

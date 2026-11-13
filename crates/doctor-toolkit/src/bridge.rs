//! The bridge between the open editor and the other programs of the
//! toolkit — the MCP server (`docs/mcp.md` §6): what is selected in the
//! editor, opening a resource or an object there, and lending the editor's
//! workspace for one write that the user allows in the editor.
//!
//! The editor listens on a Unix socket in its settings folder
//! ([`socket_path`]); one JSON object per line each way, a [`Request`]
//! and its [`Reply`]. Only the user's own programs reach it (the folder is
//! the user's; the socket is made with the user's permissions). Other
//! systems have no bridge yet.
//!
//! Writing: the editor holds the lock of its workspace
//! ([`crate::workspace::WorkspaceLock`]). A program that wants to write
//! sends [`Request::Lend`]; the editor asks the user in a window and, when
//! allowed, lets the lock go and answers. The program takes the lock,
//! records its edit (one undoable operation) and sends [`Request::Return`];
//! the editor takes the lock back and reads the workspace again, so the
//! edit is in its history and its Changes panel.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// How the editor names itself as the holder of the workspace lock; a
/// program that finds the lock held by it may ask on the bridge.
pub const EDITOR: &str = "the editor";

/// Name of the socket in the editor's settings folder.
pub const SOCKET_NAME: &str = "editor.sock";

/// What a program asks the editor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// Who answers: the editor, its game and workspace.
    Hello,
    /// What the user has selected: the resource of the stream tab, the
    /// object of the level tab, the tab in front.
    Selection,
    /// Show a resource (a key `stream:type:ids`) in its stream tab.
    Open { key: String },
    /// Show an object (`<stream>#<resource>:<object>`) in the level tab,
    /// selected.
    ShowObject { object: String },
    /// Show an object in the logic graph tab, selected.
    ShowLogic { object: String },
    /// Ask the user to let `client` write the workspace once: `summary`
    /// says what it wants to do. Answered when the user decides.
    Lend { client: String, summary: String },
    /// The write is done: the editor takes its workspace back.
    Return,
}

/// The editor's answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reply {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub result: serde_json::Value,
}

impl Reply {
    pub fn ok(result: serde_json::Value) -> Self {
        Self {
            ok: true,
            error: None,
            result,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(text.into()),
            result: serde_json::Value::Null,
        }
    }
}

/// The settings folder of the editor (`DOCTOR_TOOLKIT_CONFIG_DIR`, else the
/// platform's configuration folder), as the editor finds it.
pub fn config_dir() -> Option<PathBuf> {
    let var = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if let Some(dir) = var("DOCTOR_TOOLKIT_CONFIG_DIR") {
        return Some(dir);
    }
    let base = if cfg!(windows) {
        var("APPDATA")
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|h| h.join("Library").join("Application Support"))
    } else {
        var("XDG_CONFIG_HOME").or_else(|| var("HOME").map(|h| h.join(".config")))
    };
    base.map(|b| b.join("doctor-toolkit"))
}

/// Where the editor listens: `DOCTOR_TOOLKIT_BRIDGE`, else
/// [`SOCKET_NAME`] in [`config_dir`].
pub fn socket_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("DOCTOR_TOOLKIT_BRIDGE").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(p));
    }
    config_dir().map(|d| d.join(SOCKET_NAME))
}

#[cfg(unix)]
pub use unix::{Client, Incoming, Server};

#[cfg(unix)]
mod unix {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::time::Duration;

    use super::{Reply, Request};

    /// A connection to the editor.
    pub struct Client {
        reader: BufReader<UnixStream>,
        stream: UnixStream,
    }

    impl Client {
        /// Connects to the editor listening on `path`.
        pub fn connect(path: &Path) -> std::io::Result<Self> {
            let stream = UnixStream::connect(path)?;
            Ok(Self {
                reader: BufReader::new(stream.try_clone()?),
                stream,
            })
        }

        /// Sends `request` and waits at most `timeout` for the answer.
        pub fn request(&mut self, request: &Request, timeout: Duration) -> std::io::Result<Reply> {
            self.stream.set_read_timeout(Some(timeout))?;
            let mut line = serde_json::to_string(request).map_err(std::io::Error::other)?;
            line.push('\n');
            self.stream.write_all(line.as_bytes())?;
            let mut answer = String::new();
            if self.reader.read_line(&mut answer)? == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "the editor closed the bridge",
                ));
            }
            serde_json::from_str(&answer).map_err(std::io::Error::other)
        }
    }

    /// A request that arrived, with the way back to its sender.
    pub struct Incoming {
        pub request: Request,
        pub reply: Sender<Reply>,
    }

    /// The editor's side: requests arrive on [`Server::incoming`]; the
    /// socket goes when the server is dropped.
    pub struct Server {
        path: PathBuf,
        incoming: Receiver<Incoming>,
    }

    impl Server {
        /// Listens on `path`. Another editor listening there already is
        /// left alone (an error); a socket left by an editor that ended is
        /// replaced. `wake` is called when a request arrives (the editor
        /// asks for a frame).
        pub fn start(
            path: &Path,
            wake: impl Fn() + Send + Sync + 'static,
        ) -> std::io::Result<Self> {
            if UnixStream::connect(path).is_ok() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AddrInUse,
                    "another editor answers on the bridge",
                ));
            }
            let _ = std::fs::remove_file(path);
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let listener = UnixListener::bind(path)?;
            // Only the user's own programs connect.
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
            }
            let (tx, rx) = channel();
            let wake = std::sync::Arc::new(wake);
            std::thread::Builder::new()
                .name("bridge".into())
                .spawn(move || {
                    for stream in listener.incoming() {
                        let Ok(stream) = stream else { continue };
                        let tx = tx.clone();
                        let wake = wake.clone();
                        std::thread::spawn(move || serve(stream, &tx, &*wake));
                    }
                })?;
            Ok(Self {
                path: path.to_path_buf(),
                incoming: rx,
            })
        }

        /// The requests that arrived since the last call.
        pub fn incoming(&self) -> Vec<Incoming> {
            self.incoming.try_iter().collect()
        }

        pub fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    fn serve(stream: UnixStream, tx: &Sender<Incoming>, wake: &dyn Fn()) {
        let Ok(mut out) = stream.try_clone() else {
            return;
        };
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            let reply = match serde_json::from_str::<Request>(&line) {
                Ok(request) => {
                    let (reply_tx, reply_rx) = channel();
                    if tx
                        .send(Incoming {
                            request,
                            reply: reply_tx,
                        })
                        .is_err()
                    {
                        return;
                    }
                    wake();
                    match reply_rx.recv() {
                        Ok(r) => r,
                        Err(_) => Reply::error("the editor did not answer"),
                    }
                }
                Err(e) => Reply::error(format!("not a request: {e}")),
            };
            let Ok(mut text) = serde_json::to_string(&reply) else {
                return;
            };
            text.push('\n');
            if out.write_all(text.as_bytes()).is_err() {
                return;
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn requests_go_over_the_socket_and_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SOCKET_NAME);
        let server = Server::start(&path, || {}).unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "only the user connects");
        }
        // A second editor does not take the bridge over.
        assert!(Server::start(&path, || {}).is_err());
        let answer = std::thread::spawn(move || {
            loop {
                if let Some(i) = server.incoming().into_iter().next() {
                    let reply = match i.request {
                        Request::Open { key } => Reply::ok(serde_json::json!({ "opened": key })),
                        _ => Reply::error("no"),
                    };
                    i.reply.send(reply).unwrap();
                    return server;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        let mut client = Client::connect(&path).unwrap();
        let reply = client
            .request(
                &Request::Open {
                    key: "gf2/level:simgroup:00000004".into(),
                },
                Duration::from_secs(5),
            )
            .unwrap();
        assert!(reply.ok);
        assert_eq!(reply.result["opened"], "gf2/level:simgroup:00000004");
        drop(answer.join().unwrap());
        assert!(!path.exists(), "the socket goes with the server");
        let text = serde_json::to_string(&Request::ShowObject {
            object: "gf2/level#4:1".into(),
        })
        .unwrap();
        assert_eq!(text, r#"{"op":"show_object","object":"gf2/level#4:1"}"#);
    }
}

//! The bridge with other programs of the toolkit — the MCP server
//! (`doctor_toolkit::bridge`, `docs/mcp.md` §6): the editor tells what is
//! selected, shows a resource or an object when asked, and lends its
//! workspace for one write when the user allows it in a window.
//!
//! The editor listens on `editor.sock` in its settings folder (Unix);
//! `DOCTOR_TOOLKIT_BRIDGE=off` turns the bridge off, the screenshot and
//! self-test modes run without it.

use doctor_toolkit::bridge::Reply;
#[cfg(unix)]
use doctor_toolkit::bridge::Request;
use doctor_toolkit_mods::i18n::{tr, trf};
use egui::RichText;

use super::{BrowserApp, Level};

/// A write asked for, waiting for the user's answer.
pub(super) struct Ask {
    client: String,
    summary: String,
    reply: std::sync::mpsc::Sender<Reply>,
}

#[derive(Default)]
pub(super) struct BridgeState {
    #[cfg(unix)]
    server: Option<doctor_toolkit::bridge::Server>,
    started: bool,
    ask: Option<Ask>,
    /// The workspace is lent: another program writes it now.
    lent: bool,
}

impl BrowserApp {
    /// Answers the requests that arrived on the bridge; shows the window
    /// that asks the user about a write.
    pub(super) fn poll_bridge(&mut self, ctx: &egui::Context) {
        #[cfg(unix)]
        {
            if !self.bridge.started {
                self.bridge.started = true;
                let off = cfg!(test)
                    || std::env::var_os("DOCTOR_TOOLKIT_GUI_SHOT").is_some()
                    || std::env::var("DOCTOR_TOOLKIT_BRIDGE").is_ok_and(|v| v == "off");
                if !off && let Some(path) = doctor_toolkit::bridge::socket_path() {
                    let wake = ctx.clone();
                    match doctor_toolkit::bridge::Server::start(&path, move || {
                        wake.request_repaint()
                    }) {
                        Ok(server) => self.bridge.server = Some(server),
                        Err(e) => doctor_toolkit::report::log(&format!("bridge: {e}")),
                    }
                }
            }
            self.bridge_requests(ctx);
        }
        self.bridge_ask_ui(ctx);
    }

    /// Answers the requests that arrived; a request to lend the workspace
    /// waits for the user ([`Self::bridge_ask_ui`]).
    #[cfg(unix)]
    pub(super) fn bridge_requests(&mut self, ctx: &egui::Context) {
        {
            let incoming = self
                .bridge
                .server
                .as_ref()
                .map(|s| s.incoming())
                .unwrap_or_default();
            for i in incoming {
                if let Request::Lend {
                    client,
                    summary,
                    workspace,
                } = i.request
                {
                    // Another folder than the one this window works with
                    // (a project's workspace, say): lending this one would
                    // not let the program write its own.
                    let mine = self.workspace_dir();
                    if let (Some(theirs), Some(mine)) = (&workspace, &mine)
                        && !same_folder(theirs, mine)
                    {
                        let _ = i.reply.send(Reply::error(format!(
                            "this window works with the workspace in {}, not in {}",
                            mine.display(),
                            theirs.display()
                        )));
                    } else if self.bridge.ask.is_some() || self.bridge.lent {
                        let _ = i
                            .reply
                            .send(Reply::error("another write waits for the user"));
                    } else {
                        self.bridge.ask = Some(Ask {
                            client,
                            summary,
                            reply: i.reply,
                        });
                    }
                    continue;
                }
                let reply = self.bridge_request(i.request, ctx);
                let _ = i.reply.send(reply);
            }
        }
    }

    /// Answers on `server` (the editor starts none in tests).
    #[cfg(all(test, unix))]
    pub(super) fn serve_bridge_for_test(&mut self, server: doctor_toolkit::bridge::Server) {
        self.bridge.started = true;
        self.bridge.server = Some(server);
    }

    /// Whether a request to lend the workspace waits for the user.
    #[cfg(all(test, unix))]
    pub(super) fn bridge_asks_for_test(&self) -> bool {
        self.bridge.ask.is_some()
    }

    #[cfg(unix)]
    fn bridge_request(&mut self, request: Request, ctx: &egui::Context) -> Reply {
        use doctor_toolkit::workspace::ResourceKey;
        let Some(game) = self.game.clone() else {
            return match request {
                Request::Hello => Reply::ok(serde_json::json!({
                    "editor": env!("CARGO_PKG_VERSION"),
                    "game": null,
                })),
                _ => Reply::error("no game is open in the editor"),
            };
        };
        let object = |text: &str| -> Result<(doctor_toolkit::level::Location, usize), String> {
            let bad = || format!("`{text}`: `<stream>#<resource>:<object>` expected");
            let (group, object) = text.rsplit_once(':').ok_or_else(bad)?;
            let (stream, resource) = group.rsplit_once('#').ok_or_else(bad)?;
            let stream = game
                .find_stream(stream)
                .ok_or_else(|| format!("no stream `{stream}` in the open game"))?;
            Ok((
                doctor_toolkit::level::Location {
                    stream,
                    resource: resource.parse().map_err(|_| bad())?,
                },
                object.parse().map_err(|_| bad())?,
            ))
        };
        match request {
            Request::Hello => Reply::ok(serde_json::json!({
                "editor": env!("CARGO_PKG_VERSION"),
                "game": game.root(),
                "workspace": self.workspace.as_ref().map(|w| w.dir().to_path_buf()),
            })),
            Request::Selection => Reply::ok(self.selection_json()),
            Request::Open { key } => match key.parse::<ResourceKey>() {
                Ok(key) => {
                    self.open_key(&key);
                    Reply::ok(serde_json::json!({ "shown": key.to_string() }))
                }
                Err(e) => Reply::error(e.to_string()),
            },
            Request::ShowObject { object: text } => match object(&text) {
                Ok((group, o)) => {
                    self.open_level_for(group, o, ctx);
                    Reply::ok(serde_json::json!({ "shown": text, "view": "level" }))
                }
                Err(e) => Reply::error(e),
            },
            Request::ShowLogic { object: text } => match object(&text) {
                Ok((group, o)) => {
                    let scope = format!(
                        "group:{}#{}",
                        game.streams()[group.stream].stream_name(),
                        group.resource
                    );
                    self.open_logic_tab(&scope, Some((group, o)));
                    Reply::ok(serde_json::json!({ "shown": text, "view": "logic" }))
                }
                Err(e) => Reply::error(e),
            },
            Request::Return => {
                if !self.bridge.lent {
                    return Reply::error("the workspace was not lent");
                }
                self.bridge.lent = false;
                // Take the lock back and read what the other program recorded.
                self.open_workspace();
                self.refresh_status();
                self.reload_docs(None);
                for level in self.levels.values_mut() {
                    level.refresh(self.workspace.as_ref(), None);
                }
                for logic in self.logics.values_mut() {
                    logic.refresh();
                }
                let label = self
                    .workspace
                    .as_ref()
                    .and_then(|w| w.undo_label())
                    .unwrap_or_default()
                    .to_owned();
                self.notify(Level::Info, trf("bridge.recorded", &[("label", &label)]));
                Reply::ok(serde_json::json!({ "taken_back": true }))
            }
            Request::Lend { .. } => Reply::error("asked twice"),
        }
    }

    /// What the user has selected, for [`Request::Selection`].
    #[cfg(unix)]
    fn selection_json(&self) -> serde_json::Value {
        let game = self.game.as_ref();
        let stream_tab = self
            .focus_stream
            .and_then(|id| self.streams.get(&id))
            .and_then(|t| {
                let stream = t.stream.as_ref()?;
                let resource = t.selected?;
                let game = game?;
                let file = self.files.peek(stream.game_index)?;
                let header = file.layout().resources.get(resource)?.header;
                let name = game.streams()[stream.game_index].stream_name();
                Some(serde_json::json!({
                    "stream": name,
                    "resource": resource,
                    "key": doctor_toolkit::workspace::ResourceKey::new(&name, &header).to_string(),
                }))
            });
        let level_object = self
            .levels
            .iter()
            .filter(|(id, _)| self.live(**id))
            .find_map(|(_, l)| l.selected_location())
            .and_then(|(group, object)| {
                let game = game?;
                Some(format!(
                    "{}#{}:{object}",
                    game.streams()[group.stream].stream_name(),
                    group.resource
                ))
            });
        serde_json::json!({
            "game": game.map(|g| g.root().to_path_buf()),
            "workspace": self.workspace.as_ref().map(|w| w.dir().to_path_buf()),
            "stream_tab": stream_tab,
            "level_object": level_object,
        })
    }

    fn bridge_ask_ui(&mut self, ctx: &egui::Context) {
        let Some(ask) = &self.bridge.ask else {
            return;
        };
        let mut answer = None;
        egui::Modal::new(egui::Id::unique("bridge-ask")).show(ctx, |ui| {
            ui.set_max_width(480.0);
            ui.heading(tr("bridge.ask_title"));
            ui.label(trf("bridge.ask_text", &[("client", &ask.client)]));
            ui.add_space(4.0);
            ui.label(RichText::new(&ask.summary).monospace());
            ui.add_space(4.0);
            ui.label(RichText::new(tr("bridge.ask_note")).weak().small());
            ui.horizontal(|ui| {
                if ui.button(tr("bridge.allow")).clicked() {
                    answer = Some(true);
                }
                if ui.button(tr("dialog.cancel")).clicked() {
                    answer = Some(false);
                }
            });
        });
        let Some(yes) = answer else {
            return;
        };
        let ask = self.bridge.ask.take().expect("checked");
        let reply = if yes {
            // The other program takes the lock; the editor takes it back on Return.
            self.ws_lock = None;
            self.bridge.lent = true;
            Reply::ok(serde_json::json!({ "allowed": true }))
        } else {
            Reply::error(tr("bridge.refused").to_owned())
        };
        if ask.reply.send(reply).is_err() && yes {
            // The program gave up waiting: keep the workspace.
            self.bridge.lent = false;
            self.open_workspace();
        }
    }
}

/// Whether two paths name one folder (through links and `..`, when both exist).
#[cfg(unix)]
fn same_folder(a: &std::path::Path, b: &std::path::Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => std::path::absolute(a).ok() == std::path::absolute(b).ok(),
    }
}

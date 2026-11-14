//! The script editor tab (Tools → Script editor; a window up to stage 55)
//! and its deploy actions.

use doctor_toolkit::script::deploy;
use doctor_toolkit_mods::i18n::{tr, trf};

use super::dock::{DockOp, Pane};
use super::{BrowserApp, Level};
use crate::script_editor::{ScriptAction, ScriptEditor};

/// The script editor at start: `DOCTOR_TOOLKIT_GUI_SCRIPT=<graph.json>`
/// opens it with that graph (for checks without a user).
pub(super) fn editor_from_env() -> ScriptEditor {
    let mut editor = ScriptEditor::default();
    if let Some(path) = std::env::var_os("DOCTOR_TOOLKIT_GUI_SCRIPT").map(std::path::PathBuf::from)
    {
        match std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|t| {
                doctor_toolkit::script::ScriptGraph::from_json(&t).map_err(|e| e.to_string())
            }) {
            Ok(graph) => {
                editor.load(graph, Some(path));
                // With the live link of a check (stage 35): the game's tab.
                if std::env::var_os("DOCTOR_TOOLKIT_GUI_LIVE").is_some() {
                    editor.show_game_journal();
                }
            }
            Err(error) => {
                editor.status = error;
                editor.open = true;
            }
        }
    }
    editor
}

impl BrowserApp {
    /// Opens the script editor's tab, or shows it.
    pub(super) fn open_script_editor(&mut self) {
        self.script_editor.open = true;
        self.dock_ops.push(DockOp::Open(Pane::Script));
    }

    /// The script editor's tab.
    pub(super) fn script_pane(&mut self, ui: &mut egui::Ui) {
        self.script_editor.open = true;
        let details = self.details_follow();
        let action = self.script_editor.show(ui, details);
        if let Some(a) = action {
            self.script_action(a);
        }
    }

    fn script_action(&mut self, action: ScriptAction) {
        let (Some(game), Some(index)) = (self.game.clone(), self.index.clone()) else {
            self.script_editor.status = tr("files.no_game").to_owned();
            return;
        };
        let ws = match self.workspace_mut() {
            Ok(ws) => ws,
            Err(error) => {
                self.status = trf("level.cannot_open_workspace", &[("error", &error)]);
                return;
            }
        };
        let result = match action {
            ScriptAction::Deploy {
                stream,
                graph,
                chunk,
            } => match game.find_stream(&stream) {
                None => Err(trf("script.no_stream", &[("stream", &stream)])),
                Some(s) => deploy::deploy(&game, &index, ws, s, &graph, &chunk)
                    .map_err(|e| e.to_string())
                    .and_then(|d| {
                        deploy::save_graph(ws, &graph).map_err(|e| e.to_string())?;
                        let name = graph.chunk_name();
                        Ok(if d.updated {
                            trf("script.updated", &[("name", &name), ("stream", &d.stream)])
                        } else {
                            trf(
                                "script.deployed",
                                &[
                                    ("name", &name),
                                    ("stream", &d.stream),
                                    ("object", &d.object),
                                ],
                            )
                        })
                    }),
            },
            ScriptAction::Remove { stream, name } => match game.find_stream(&stream) {
                None => Err(trf("script.no_stream", &[("stream", &stream)])),
                Some(s) => deploy::remove(&game, ws, s, &name)
                    .map_err(|e| e.to_string())
                    .map(|done| {
                        let key = if done {
                            "script.removed"
                        } else {
                            "script.not_found"
                        };
                        trf(key, &[("name", &name), ("stream", &stream)])
                    }),
            },
        };
        match result {
            Ok(text) => {
                self.script_editor.status = text.clone();
                self.notify(Level::Info, text);
                if self.direct {
                    self.apply(false);
                } else {
                    self.refresh_status();
                    self.reload_docs(None);
                }
            }
            Err(error) => {
                self.script_editor.status = error.clone();
                self.notify(Level::Error, error);
            }
        }
    }
}

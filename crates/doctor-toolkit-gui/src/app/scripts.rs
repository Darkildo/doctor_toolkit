//! The script editor window (Tools → Script editor) and its deploy actions.

use doctor_toolkit::script::deploy;
use doctor_toolkit_mods::i18n::{tr, trf};

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
            Ok(graph) => editor.load(graph, Some(path)),
            Err(error) => {
                editor.status = error;
                editor.open = true;
            }
        }
    }
    editor
}

impl BrowserApp {
    pub(super) fn show_script_editor(&mut self, ctx: &egui::Context) {
        if !self.script_editor.open {
            return;
        }
        let mut open = true;
        let mut action = None;
        let mark = if self.script_editor.has_unsaved_edits() {
            " *"
        } else {
            ""
        };
        let title = match &self.script_editor.path {
            Some(p) => format!("{} — {}{mark}", tr("script.title"), p.display()),
            None => format!(
                "{} — {}{mark}",
                tr("script.title"),
                self.script_editor.graph().name
            ),
        };
        egui::Window::new(title)
            .id(egui::Id::unique("script-editor"))
            .open(&mut open)
            .default_size([1200.0, 760.0])
            .resizable(true)
            .show(ctx, |ui| {
                action = self.script_editor.show(ui);
            });
        self.script_editor.open = open;
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

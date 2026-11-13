//! Logic graph tabs: opening them and recording their edits.

use doctor_toolkit::level::Location;
use doctor_toolkit::logic_graph::edit;
use doctor_toolkit_mods::i18n::{tr, trf, trn};

use super::BrowserApp;
use super::dock::{DockOp, Pane};
use crate::logic_editor::{LogicAction, LogicEditor};

impl BrowserApp {
    /// `DOCTOR_TOOLKIT_GUI_LOGIC=<scope>[;find=<text>][;show]` opens a logic
    /// tab shown alone (for screenshots and checks); `show` then shows the
    /// first found node in the level view.
    pub(super) fn logic_preset(&mut self) {
        let Ok(spec) = std::env::var("DOCTOR_TOOLKIT_GUI_LOGIC") else {
            return;
        };
        let mut parts = spec.split(';');
        let scope = parts.next().unwrap_or_default().to_owned();
        self.open_logic_tab(&scope, None);
        let id = self.logics.keys().copied().next();
        for part in parts {
            let Some(editor) = id.and_then(|id| self.logics.get_mut(&id)) else {
                continue;
            };
            if let Some(find) = part.strip_prefix("find=") {
                editor.find_after_load(find);
            } else if part == "show" {
                editor.preset_show = true;
            }
        }
        if let Some(id) = id {
            self.dock_ops.push(DockOp::Maximize(Some(id)));
        }
    }

    /// Opens the logic graph of `scope` in the last logic tab (a new one when
    /// there is none), selecting `select`. An empty scope keeps the tab's.
    pub(super) fn open_logic_tab(&mut self, scope: &str, select: Option<(Location, usize)>) {
        let existing = self.logics.keys().copied().find(|&id| self.live(id));
        let id = match existing {
            Some(id) => {
                self.dock_ops.push(DockOp::Focus(id));
                id
            }
            None => {
                let id = self.new_pane_id();
                self.dock_ops.push(DockOp::Open(Pane::Logic {
                    id,
                    scope: scope.to_owned(),
                }));
                id
            }
        };
        let editor = self.logics.entry(id).or_insert_with(|| {
            let mut editor = LogicEditor::new(scope);
            if let (Some(game), Some(index)) = (&self.game, &self.index) {
                editor.open_game(game.clone(), index.clone());
            }
            editor
        });
        if !scope.is_empty() {
            editor.set_scope(scope, select);
        }
    }

    /// A logic tab: made when first shown (also for a tab of a saved layout).
    pub(super) fn logic_pane(&mut self, ui: &mut egui::Ui, id: u64, scope: &mut String) {
        if !self.logics.contains_key(&id) {
            let mut editor = LogicEditor::new(scope);
            if let (Some(game), Some(index)) = (&self.game, &self.index) {
                editor.open_game(game.clone(), index.clone());
            }
            self.logics.insert(id, editor);
        }
        let editor = self.logics.get_mut(&id).expect("just made");
        let action = editor.ui(ui, self.workspace.as_ref());
        // The layout keeps the scope for the next start.
        if editor.scope() != scope.as_str() {
            *scope = editor.scope().to_owned();
        }
        if let Some(action) = action {
            let ctx = ui.ctx().clone();
            self.logic_action(action, &ctx);
        }
    }

    /// Records an edit of a logic tab in the workspace.
    fn logic_action(&mut self, action: LogicAction, ctx: &egui::Context) {
        let (Some(game), Some(index)) = (self.game.clone(), self.index.clone()) else {
            return;
        };
        match action {
            LogicAction::ShowInLevel { group, object } => {
                self.open_level_for(group, object, ctx);
                return;
            }
            LogicAction::OpenGroup(group) => {
                self.open_resource_tab(crate::nav::Location {
                    stream: group.stream,
                    resource: group.resource,
                });
                return;
            }
            _ => {}
        }
        let taken = self
            .levels
            .values()
            .flat_map(|l| l.guids())
            .collect::<std::collections::HashSet<_>>();
        let ws = match self.workspace_mut() {
            Ok(ws) => ws,
            Err(error) => {
                self.status = trf("level.cannot_open_workspace", &[("error", &error)]);
                return;
            }
        };
        let result: Result<String, String> = match action {
            LogicAction::Apply { changes, label } => {
                edit::apply(&game, &index, ws, &changes, &label)
                    .map(|changed| {
                        if changed {
                            trf("logic.recorded", &[("what", &label)])
                        } else {
                            tr("logic.no_change").to_owned()
                        }
                    })
                    .map_err(|e| e.to_string())
            }
            LogicAction::AddNodes(adds) => {
                let mut done = 0;
                let mut error = None;
                for (group, template) in adds {
                    match edit::add_node(&game, &index, ws, group, template, &|g| taken.contains(g))
                    {
                        Ok(_) => done += 1,
                        Err(e) => {
                            error = Some(e.to_string());
                            break;
                        }
                    }
                }
                match error {
                    Some(e) => Err(e),
                    None => Ok(trn("logic.added_nodes", done as u64, &[])),
                }
            }
            LogicAction::RemoveNodes(mut nodes) => {
                // Later objects first: removing one shifts the indices after it.
                nodes.sort_by_key(|n| std::cmp::Reverse(n.1));
                let mut dangling = 0;
                let mut error = None;
                for (group, object) in &nodes {
                    match edit::remove_node(&game, ws, *group, *object) {
                        Ok(d) => dangling += d,
                        Err(e) => {
                            error = Some(e.to_string());
                            break;
                        }
                    }
                }
                match error {
                    Some(e) => Err(e),
                    None => Ok(trf(
                        "logic.removed",
                        &[
                            ("nodes", &trn("count.nodes", nodes.len() as u64, &[])),
                            ("dangling", &trn("count.attributes", dangling as u64, &[])),
                        ],
                    )),
                }
            }
            LogicAction::ShowInLevel { .. } | LogicAction::OpenGroup(_) => unreachable!(),
        };
        match result {
            Ok(text) => {
                self.status = text;
                self.after_edit(None);
            }
            Err(error) => self.notify(
                super::Level::Error,
                trf("logic.cannot_edit", &[("error", &error)]),
            ),
        }
    }

    /// Opens the level tab on the zone of an object and selects it.
    fn open_level_for(&mut self, group: Location, object: usize, ctx: &egui::Context) {
        let existing = self.levels.keys().copied().find(|&id| self.live(id));
        let id = match existing {
            Some(id) => {
                self.dock_ops.push(DockOp::Focus(id));
                id
            }
            None => {
                let id = self.new_pane_id();
                self.dock_ops.push(DockOp::Open(Pane::Level { id }));
                let mut level = crate::level_view::LevelView::new(self.gpu.clone(), ctx);
                level.forget_preset();
                if let (Some(game), Some(index)) = (&self.game, &self.index) {
                    level.open_game(game.clone(), index.clone());
                }
                self.levels.insert(id, level);
                id
            }
        };
        if let Some(level) = self.levels.get_mut(&id) {
            level.show_object(group, object);
        }
    }
}

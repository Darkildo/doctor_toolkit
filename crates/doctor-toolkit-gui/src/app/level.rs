//! Level editor tabs: their edits are recorded in the workspace here.

use doctor_toolkit::level::Location;
use doctor_toolkit::level::edit;
use doctor_toolkit_mods::i18n::{tr, trf, trn};

use super::BrowserApp;
use crate::level_view::LevelAction;

impl BrowserApp {
    /// Records an edit of level tab `tab` in the workspace.
    pub(super) fn level_action(&mut self, tab: u64, action: LevelAction) {
        if let LevelAction::StartWizard { task, model } = action {
            if let Some(kind) = super::wizard::Kind::from_slug(task) {
                let start = super::wizard::Start { loc: None, model };
                self.start_wizard(kind, start);
            }
            return;
        }
        if let LevelAction::PlayHere { eye, streams } = &action {
            self.play_here(tab, *eye, streams);
            return;
        }
        if let LevelAction::LiveTeleport { .. } | LevelAction::LiveCamera(_) = &action {
            self.live_level_action(action);
            return;
        }
        let (Some(game), Some(index)) = (self.game.clone(), self.index.clone()) else {
            return;
        };
        if let LevelAction::ShowLogic { group, object } = action {
            let stream = game.streams()[group.stream].stream_name();
            self.open_logic_tab(&format!("stream:{stream}"), Some((group, object)));
            return;
        }
        let removing = matches!(action, LevelAction::Remove { .. });
        let taken = self.levels.get(&tab).map(|l| l.guids()).unwrap_or_default();
        let is_taken = |g: &doctor_toolkit::resource::simgroup::Guid| taken.contains(g);
        let ws = match self.workspace_mut() {
            Ok(ws) => ws,
            Err(error) => {
                self.status = trf("level.cannot_open_workspace", &[("error", &error)]);
                return;
            }
        };
        let result: Result<(String, Option<(Location, usize)>), String> = match action {
            LevelAction::SetTransform {
                group,
                object,
                matrix,
            } => edit::set_transform(&game, ws, group, object, matrix)
                .map(|changed| {
                    let text = if changed {
                        trf("level.moved", &[("object", &object)])
                    } else {
                        tr("logic.no_change").to_owned()
                    };
                    (text, Some((group, object)))
                })
                .map_err(|e| e.to_string()),
            LevelAction::Duplicate { group, object } => {
                edit::duplicate(&game, ws, group, object, [2.0, 0.0, 0.0], &is_taken)
                    .map(|(copy, guid)| {
                        (
                            trf(
                                "level.duplicated",
                                &[("object", &object), ("copy", &copy), ("guid", &guid)],
                            ),
                            Some((group, copy)),
                        )
                    })
                    .map_err(|e| e.to_string())
            }
            LevelAction::Remove { group, object } => edit::remove(&game, ws, group, object)
                .map(|dangling| {
                    let mut text = trf("level.removed", &[("object", &object)]);
                    if dangling > 0 {
                        text += &trn("level.dangling_attributes", dangling as u64, &[]);
                    }
                    (text, None)
                })
                .map_err(|e| e.to_string()),
            LevelAction::Add {
                group,
                template,
                model,
                position,
            } => edit::add(
                &game,
                &index,
                ws,
                group,
                template,
                model,
                Some(position),
                &is_taken,
                true,
            )
            .map(|added| {
                let mut text = trf(
                    "level.added",
                    &[("object", &added.object), ("guid", &added.guid)],
                );
                if let Some(m) = added.model {
                    text += &trf("level.added_model", &[("model", &format!("{m:08x}"))]);
                }
                if !added.cloned.is_empty() {
                    text += &trf(
                        "level.added_cloned",
                        &[(
                            "resources",
                            &trn("count.resources", added.cloned.len() as u64, &[]),
                        )],
                    );
                }
                text += tr("level.added_collision_note");
                (text, Some((group, added.object)))
            })
            .map_err(|e| e.to_string()),
            LevelAction::StartWizard { .. } => Ok(("".to_owned(), None)),
            LevelAction::ShowLogic { .. }
            | LevelAction::PlayHere { .. }
            | LevelAction::LiveTeleport { .. }
            | LevelAction::LiveCamera(_) => Ok((tr("logic.no_change").to_owned(), None)),
            LevelAction::Graph {
                graph,
                renumbered,
                label,
            } => doctor_toolkit::level::navgraph::save_graph(
                &game,
                ws,
                &graph,
                renumbered.as_deref(),
                &label,
            )
            .map(|others| {
                let mut text = trf("logic.recorded", &[("what", &label)]);
                if others > 0 {
                    text += &trf("navgraph.others_renumbered", &[("count", &others)]);
                }
                (text, None)
            })
            .map_err(|e| e.to_string()),
            LevelAction::Save { group, data, label } => {
                doctor_toolkit::level::compat::check_resource_data(
                    &game,
                    &index,
                    Some(ws),
                    (group.stream, group.resource),
                    &data,
                )
                .map_err(|e| e.to_string())
                .and_then(|problems| problems.into_iter().next().map_or(Ok(()), Err))
                .and_then(|()| {
                    ws.set_resource(&game, group.stream, group.resource, data, &label)
                        .map_err(|e| e.to_string())
                })
                .map(|changed| {
                    let text = if changed {
                        trf("logic.recorded", &[("what", &label)])
                    } else {
                        tr("logic.no_change").to_owned()
                    };
                    (text, None)
                })
            }
        };
        match result {
            Ok((text, select)) => {
                if removing && let Some(level) = self.levels.get_mut(&tab) {
                    level.clear_selection();
                }
                self.status = text;
                self.after_level_edit(tab, select);
            }
            Err(error) => self.notify(
                super::Level::Error,
                trf("level.cannot_edit", &[("error", &error)]),
            ),
        }
    }

    /// After a level edit: write it in direct mode, refresh the marks and the
    /// scenes, selecting `select` in the tab that made the edit.
    pub(super) fn after_level_edit(&mut self, tab: u64, select: Option<(Location, usize)>) {
        if self.direct {
            let status = self.status.clone();
            self.apply(false);
            self.status = format!("{status}; {}", self.status);
        } else {
            self.refresh_status();
            for (&id, level) in &mut self.levels {
                level.refresh(
                    self.workspace.as_ref(),
                    if id == tab { select } else { None },
                );
            }
            self.reload_docs(None);
        }
    }

    /// A level tab: made when first shown (also for a tab of a saved layout).
    pub(super) fn level_pane(&mut self, ui: &mut egui::Ui, id: u64) {
        if !self.levels.contains_key(&id) {
            let mut level = crate::level_view::LevelView::new(self.gpu.clone(), ui.ctx());
            if std::mem::replace(&mut self.level_preset_used, true) {
                level.forget_preset();
            }
            if let (Some(game), Some(index)) = (&self.game, &self.index) {
                level.open_game(game.clone(), index.clone());
            }
            self.levels.insert(id, level);
        }
        let marks = self.play.live.marks();
        // The shell's Outliner and Details show the side panels of the level
        // in front; the tab then leaves them out.
        let hosted = if self.host_level == Some(id) {
            self.level_hosts
        } else {
            (false, false)
        };
        self.last_level = Some(id);
        let level = self.levels.get_mut(&id).expect("just made");
        level.live = marks;
        level.set_hosted(hosted.0, hosted.1);
        if let Some(action) = level.ui(ui, self.workspace.as_ref()) {
            self.level_action(id, action);
        }
    }

    /// Before the dock is drawn: which level the Outliner and Details
    /// panels show, and whether they are shown.
    pub(super) fn host_level_panels(&mut self) {
        let alive = |id: &u64| self.levels.contains_key(id) && self.live(*id);
        self.host_level = self
            .front_level()
            .or(self.last_level.filter(alive))
            .or_else(|| self.levels.keys().copied().filter(alive).min());
        self.level_hosts = (
            self.panel_shown(&super::dock::Pane::Outliner),
            self.panel_shown(&super::dock::Pane::ObjectDetails),
        );
    }

    /// The Outliner panel: for now the zone browser of the level in front
    /// (stage 53 makes it the outliner of the level's objects).
    pub(super) fn outliner_pane(&mut self, ui: &mut egui::Ui) {
        let Some(level) = self.host_level.and_then(|id| self.levels.get_mut(&id)) else {
            self.no_level(ui);
            return;
        };
        level.browser_panel(ui, self.workspace.as_ref());
    }

    /// The object Details panel: the selected object of the level in front.
    pub(super) fn object_details_pane(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.host_level.filter(|id| self.levels.contains_key(id)) else {
            self.no_level(ui);
            return;
        };
        let level = self.levels.get_mut(&id).expect("checked");
        match level.details_panel(ui) {
            Some(Some(action)) => self.level_action(id, action),
            Some(None) => {}
            None => {
                ui.add_space(8.0);
                ui.label(egui::RichText::new(tr("level.details_load_first")).weak());
            }
        }
    }

    fn no_level(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new(tr("level.no_level")).weak());
            let command = super::Command::LevelEditor;
            let mut button = egui::Button::new(command.label());
            if let Some(keys) = command.shortcut_text(ui.ctx()) {
                button = button.shortcut_text(keys);
            }
            if ui.add_enabled(self.enabled(command), button).clicked() {
                self.pending.push(command);
            }
        });
    }
}

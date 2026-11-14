//! Level editor tabs: their edits are recorded in the workspace here, and
//! the shell's level panels (Outliner, Zones, Details) find the level they
//! show.

use doctor_toolkit::game::{GameInstance, ResourceIndex};
use doctor_toolkit::level::Location;
use doctor_toolkit::level::edit;
use doctor_toolkit::resource::simgroup::Guid;
use doctor_toolkit::workspace::Workspace;
use doctor_toolkit_mods::i18n::{tr, trf, trn};

use super::BrowserApp;
use super::dock::{DockOp, Pane};
use crate::level_view::LevelAction;

/// What one recorded edit says and selects afterwards.
type Recorded = (String, Option<(Location, usize)>);

impl BrowserApp {
    /// Records an edit of level tab `tab` in the workspace.
    pub(super) fn level_action(&mut self, tab: u64, action: LevelAction) {
        match action {
            LevelAction::StartWizard { task, model } => {
                if let Some(kind) = super::wizard::Kind::from_slug(task) {
                    let start = super::wizard::Start { loc: None, model };
                    self.start_wizard(kind, start);
                }
                return;
            }
            LevelAction::PlayHere { eye, streams } => {
                self.play_here(tab, eye, &streams);
                return;
            }
            action @ (LevelAction::LiveTeleport { .. } | LevelAction::LiveCamera(_)) => {
                self.live_level_action(action);
                return;
            }
            LevelAction::ShowZones => {
                self.dock_ops.push(DockOp::ShowPanel(Pane::Zones));
                return;
            }
            LevelAction::OpenResource { group } => {
                self.open_resource_tab(crate::nav::Location {
                    stream: group.stream,
                    resource: group.resource,
                });
                return;
            }
            LevelAction::ShowLogic { group, object } => {
                if let Some(game) = &self.game {
                    let stream = game.streams()[group.stream].stream_name();
                    self.open_logic_tab(&format!("stream:{stream}"), Some((group, object)));
                }
                return;
            }
            _ => {}
        }
        let (Some(game), Some(index)) = (self.game.clone(), self.index.clone()) else {
            return;
        };
        let removing = match &action {
            LevelAction::Remove { .. } => true,
            LevelAction::Many { actions, .. } => actions
                .iter()
                .any(|a| matches!(a, LevelAction::Remove { .. })),
            _ => false,
        };
        let taken = self.levels.get(&tab).map(|l| l.guids()).unwrap_or_default();
        let is_taken = |g: &Guid| taken.contains(g);
        let ws = match self.workspace_mut() {
            Ok(ws) => ws,
            Err(error) => {
                self.status = trf("level.cannot_open_workspace", &[("error", &error)]);
                return;
            }
        };
        let result = match action {
            LevelAction::Many { actions, label } => {
                // One operation: undone and redone as one.
                let since = ws.head();
                let mut last = Ok((String::new(), None));
                for action in actions {
                    last = record(&game, &index, ws, action, &is_taken);
                    if last.is_err() {
                        break;
                    }
                }
                let squashed = ws.squash(since, &label).map_err(|e| e.to_string());
                match (last, squashed) {
                    (Err(e), _) | (_, Err(e)) => Err(e),
                    (Ok((_, select)), Ok(_)) => {
                        Ok((trf("logic.recorded", &[("what", &label)]), select))
                    }
                }
            }
            action => record(&game, &index, ws, action, &is_taken),
        };
        match result {
            Ok((text, select)) => {
                if removing && let Some(level) = self.levels.get_mut(&tab) {
                    level.clear_selection();
                }
                self.status = text;
                self.after_level_edit(tab, select);
            }
            Err(error) => {
                if removing {
                    // Part of the edits may be in: show what there is.
                    self.after_level_edit(tab, None);
                }
                self.notify(
                    super::Level::Error,
                    trf("level.cannot_edit", &[("error", &error)]),
                )
            }
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

    /// Makes the view of level tab `id` when it does not exist yet (also for
    /// a tab of a saved layout).
    pub(super) fn ensure_level(&mut self, id: u64, ctx: &egui::Context) {
        if self.levels.contains_key(&id) {
            return;
        }
        let mut level = crate::level_view::LevelView::new(self.gpu.clone(), ctx);
        if std::mem::replace(&mut self.level_preset_used, true) {
            level.forget_preset();
        }
        if let (Some(game), Some(index)) = (&self.game, &self.index) {
            level.open_game(game.clone(), index.clone());
        }
        self.levels.insert(id, level);
    }

    /// A level tab: the view alone (stage 53).
    pub(super) fn level_pane(&mut self, ui: &mut egui::Ui, id: u64) {
        self.ensure_level(id, ui.ctx());
        let marks = self.play.live.marks();
        self.last_level = Some(id);
        let level = self.levels.get_mut(&id).expect("just made");
        level.live = marks;
        if let Some(action) = level.ui(ui, self.workspace.as_ref()) {
            self.level_action(id, action);
        }
    }

    /// A level view with its Zones panel on the left and Details on the
    /// right, inside one tab (the Place wizard: it does not depend on the
    /// panels of the layout).
    pub(super) fn level_with_panels(&mut self, ui: &mut egui::Ui, id: u64) {
        if !self.levels.contains_key(&id) {
            return;
        }
        egui::Panel::left(("embedded-zones", id))
            .resizable(true)
            .default_size(240.0)
            .max_size(420.0)
            .show(ui, |ui| {
                if let Some(level) = self.levels.get_mut(&id) {
                    level.zones_panel(ui, self.workspace.as_ref());
                }
            });
        egui::Panel::right(("embedded-details", id))
            .resizable(true)
            .default_size(360.0)
            .max_size(640.0)
            .show(ui, |ui| self.embedded_details(ui, id));
        egui::CentralPanel::default().show(ui, |ui| self.level_pane(ui, id));
    }

    /// Before the dock is drawn: the level the Outliner, Zones and Details
    /// panels show (the level in front, a wizard's in front, the level drawn
    /// last or any), and which selection Details follows.
    pub(super) fn update_panel_level(&mut self) {
        let alive = |id: &u64| self.levels.contains_key(id);
        let wizard = self
            .front_wizard()
            .and_then(|tab| self.wizards.level_of_tab(tab))
            .filter(alive);
        self.panel_level = self
            .front_level()
            .or(wizard)
            .or(self.last_level.filter(|id| alive(id) && self.live(*id)))
            .or_else(|| {
                self.levels
                    .keys()
                    .copied()
                    .filter(|id| self.live(*id))
                    .min()
            });
        // Details follows the selection made last.
        let mut changed: Vec<u64> = self
            .levels
            .iter()
            .filter(|(id, l)| self.selection_seen.get(id) != Some(&l.selection_serial()))
            .map(|(&id, _)| id)
            .collect();
        changed.sort_unstable();
        for id in changed {
            let serial = self.levels[&id].selection_serial();
            let first = self.selection_seen.insert(id, serial).is_none();
            if !first || self.levels[&id].primary().is_some() {
                self.inspect = super::details::Focus::Level(id);
            }
        }
        self.selection_seen
            .retain(|id, _| self.levels.contains_key(id));
    }

    /// The wizard tab in front, if any.
    fn front_wizard(&self) -> Option<u64> {
        let tree = self.dock.as_ref()?;
        let wizard = |t| match super::dock::pane(tree, t) {
            Some(Pane::Wizard { id, .. }) => Some(*id),
            _ => None,
        };
        let shown = super::dock::shown_documents(tree);
        self.active_doc
            .filter(|t| shown.contains(t))
            .and_then(wizard)
    }

    /// The Zones panel: the zones the level in front loads and what its
    /// view shows.
    pub(super) fn zones_pane(&mut self, ui: &mut egui::Ui) {
        let Some(level) = self.panel_level.and_then(|id| self.levels.get_mut(&id)) else {
            self.no_level(ui);
            return;
        };
        level.zones_panel(ui, self.workspace.as_ref());
    }

    /// "No level is open" and the command that opens one.
    pub(super) fn no_level(&mut self, ui: &mut egui::Ui) {
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

/// Records one edit in the workspace.
fn record(
    game: &GameInstance,
    index: &ResourceIndex,
    ws: &mut Workspace,
    action: LevelAction,
    is_taken: &dyn Fn(&Guid) -> bool,
) -> Result<Recorded, String> {
    match action {
        LevelAction::SetTransform {
            group,
            object,
            matrix,
        } => edit::set_transform(game, ws, group, object, matrix)
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
            edit::duplicate(game, ws, group, object, [2.0, 0.0, 0.0], is_taken)
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
        LevelAction::Remove { group, object } => edit::remove(game, ws, group, object)
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
            game,
            index,
            ws,
            group,
            template,
            model,
            Some(position),
            is_taken,
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
        LevelAction::Graph {
            graph,
            renumbered,
            label,
        } => doctor_toolkit::level::navgraph::save_graph(
            game,
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
                game,
                index,
                Some(ws),
                (group.stream, group.resource),
                &data,
            )
            .map_err(|e| e.to_string())
            .and_then(|problems| problems.into_iter().next().map_or(Ok(()), Err))
            .and_then(|()| {
                ws.set_resource(game, group.stream, group.resource, data, &label)
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
        LevelAction::Many { actions, .. } => {
            let mut last = Ok((String::new(), None));
            for action in actions {
                last = record(game, index, ws, action, is_taken);
                last.as_ref().map_err(Clone::clone)?;
            }
            last
        }
        LevelAction::StartWizard { .. }
        | LevelAction::ShowLogic { .. }
        | LevelAction::PlayHere { .. }
        | LevelAction::LiveTeleport { .. }
        | LevelAction::LiveCamera(_)
        | LevelAction::OpenResource { .. }
        | LevelAction::ShowZones => Ok((tr("logic.no_change").to_owned(), None)),
    }
}

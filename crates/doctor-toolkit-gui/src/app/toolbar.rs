//! The main toolbar under the menu (stage 52), in the manner of Unreal
//! Engine: save; for a level in front the gizmo modes, world or local axes,
//! snapping and "Add"; Mods and Movies; on the right the changes not in the
//! game, "Apply to game" and the green Play with its menu. Other documents
//! get the part that fits them.

use egui::RichText;

use doctor_toolkit_mods::i18n::{tr, trf};

use super::dock::{self, DockOp, Pane};
use super::theme::{self, Icon, Tone};
use super::{BrowserApp, Command};
use crate::level_view::{GizmoMode, GizmoSpace};

/// What the toolbar shows: depends on the document in front.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Context {
    /// No game is open.
    NoGame,
    /// A level tab with this id is in front.
    Level(u64),
    /// Any other document.
    Other,
}

impl BrowserApp {
    /// The level tab in front: the active document when it is one, else a
    /// level shown in the dock.
    pub(super) fn front_level(&self) -> Option<u64> {
        let tree = self.dock.as_ref()?;
        let level_of = |t| match dock::pane(tree, t) {
            Some(Pane::Level { id }) => Some(*id),
            _ => None,
        };
        if let Some(t) = self.maximized {
            return level_of(t);
        }
        let shown = dock::shown_documents(tree);
        self.active_doc
            .filter(|t| shown.contains(t))
            .and_then(level_of)
            .or_else(|| shown.iter().find_map(|&t| level_of(t)))
    }

    pub(super) fn toolbar_context(&self) -> Context {
        if self.game.is_none() {
            Context::NoGame
        } else if let Some(id) = self.front_level() {
            Context::Level(id)
        } else {
            Context::Other
        }
    }

    /// The main toolbar.
    pub(super) fn toolbar(&mut self, ui: &mut egui::Ui) {
        let context = self.toolbar_context();
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if context == Context::NoGame {
                if theme::tool_button(
                    ui,
                    Some(Icon::Folder),
                    Command::OpenGame.label(),
                    false,
                    Tone::Primary,
                )
                .clicked()
                {
                    self.pending.push(Command::OpenGame);
                }
                return;
            }
            self.command_button(ui, Icon::Save, Command::Save, "");
            theme::separator(ui);
            if let Context::Level(id) = context {
                self.gizmo_buttons(ui, id);
                theme::separator(ui);
                self.add_menu(ui, id);
                theme::separator(ui);
            }
            let mods = theme::tool_button(ui, None, tr("window.mods"), self.mods_open, Tone::Plain)
                .on_hover_text(tr("mods.window_hint"));
            if mods.clicked() {
                self.pending.push(Command::Mods);
            }
            let movies =
                theme::tool_button(ui, None, tr("movies.title"), self.movies.open, Tone::Plain)
                    .on_hover_text(tr("movies.button_hint"));
            if movies.clicked() {
                self.movies.toggle();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                self.play_button(ui);
                theme::separator(ui);
                self.apply_menu(ui);
                let apply = ui
                    .add_enabled_ui(self.enabled(Command::ApplyToGame), |ui| {
                        theme::tool_button(
                            ui,
                            Some(Icon::Apply),
                            tr("toolbar.apply"),
                            false,
                            Tone::Primary,
                        )
                    })
                    .inner
                    .on_hover_text(tr("toolbar.apply_hint"))
                    .on_disabled_hover_text(tr("toolbar.apply_hint"));
                if apply.clicked() {
                    self.pending.push(Command::ApplyToGame);
                }
                self.workspace_bar(ui);
            });
        });
    }

    /// An icon button that runs a command, with its label and shortcut as
    /// the hint.
    fn command_button(&mut self, ui: &mut egui::Ui, icon: Icon, command: Command, text: &str) {
        let hint = match command.shortcut_text(ui.ctx()) {
            Some(keys) => format!("{} ({keys})", command.label()),
            None => command.label().to_owned(),
        };
        let response = ui
            .add_enabled_ui(self.enabled(command), |ui| {
                theme::tool_button(ui, Some(icon), text, false, Tone::Plain)
            })
            .inner
            .on_hover_text(&hint)
            .on_disabled_hover_text(&hint);
        if response.clicked() {
            self.pending.push(command);
        }
    }

    /// Select, move, rotate, scale; world or local axes; snapping.
    fn gizmo_buttons(&mut self, ui: &mut egui::Ui, id: u64) {
        let Some(level) = self.levels.get(&id) else {
            return;
        };
        let mode = level.gizmo_mode();
        let space = level.gizmo_space();
        let mut snap = level.snap();
        let mut new_mode = None;
        for (m, icon, command) in [
            (GizmoMode::Select, Icon::Select, Command::GizmoSelect),
            (GizmoMode::Move, Icon::Move, Command::GizmoMove),
            (GizmoMode::Rotate, Icon::Rotate, Command::GizmoRotate),
            (GizmoMode::Scale, Icon::Scale, Command::GizmoScale),
        ] {
            let hint = match m {
                GizmoMode::Select => tr("toolbar.select_hint"),
                GizmoMode::Move => tr("toolbar.move_hint"),
                GizmoMode::Rotate => tr("toolbar.rotate_hint"),
                GizmoMode::Scale => tr("toolbar.scale_hint"),
            };
            if theme::tool_button(ui, Some(icon), "", mode == m, Tone::Plain)
                .on_hover_text(format!("{}\n{hint}", command.label()))
                .clicked()
            {
                new_mode = Some(m);
            }
        }
        let (icon, text) = match space {
            GizmoSpace::World => (Icon::World, tr("toolbar.world")),
            GizmoSpace::Local => (Icon::Local, tr("toolbar.local")),
        };
        let space_clicked = theme::tool_button(ui, Some(icon), text, false, Tone::Plain)
            .on_hover_text(tr("toolbar.space_hint"))
            .clicked();
        let label = format!(
            "{} · {}° · ×{}",
            trf("toolbar.metres", &[("m", &snap.step)]),
            snap.angle,
            snap.scale
        );
        let snap_clicked = theme::tool_button(ui, Some(Icon::Snap), &label, snap.on, Tone::Plain)
            .on_hover_text(tr("toolbar.snap_hint"))
            .clicked();
        let arrow = theme::menu_arrow(ui, Tone::Plain).on_hover_text(tr("toolbar.snap_steps"));
        let mut changed = false;
        egui::Popup::menu(&arrow).show(|ui| {
            ui.set_min_width(200.0);
            changed |= ui
                .checkbox(&mut snap.on, tr("toolbar.snap_always"))
                .changed();
            ui.separator();
            ui.label(RichText::new(tr("toolbar.snap_move")).weak());
            ui.horizontal_wrapped(|ui| {
                for step in crate::level_view::SNAP_STEPS {
                    changed |= ui
                        .selectable_value(
                            &mut snap.step,
                            step,
                            trf("toolbar.metres", &[("m", &step)]),
                        )
                        .changed();
                }
            });
            ui.label(RichText::new(tr("toolbar.snap_rotate")).weak());
            ui.horizontal_wrapped(|ui| {
                for angle in crate::level_view::SNAP_ANGLES {
                    changed |= ui
                        .selectable_value(&mut snap.angle, angle, format!("{angle}°"))
                        .changed();
                }
            });
            ui.label(RichText::new(tr("toolbar.snap_scale")).weak());
            ui.horizontal_wrapped(|ui| {
                for scale in crate::level_view::SNAP_SCALES {
                    changed |= ui
                        .selectable_value(&mut snap.scale, scale, format!("×{scale}"))
                        .changed();
                }
            });
        });
        if snap_clicked {
            snap.on = !snap.on;
            changed = true;
        }
        let Some(level) = self.levels.get_mut(&id) else {
            return;
        };
        if let Some(m) = new_mode {
            level.set_gizmo_mode(m);
        }
        if space_clicked {
            level.set_gizmo_space(match space {
                GizmoSpace::World => GizmoSpace::Local,
                GizmoSpace::Local => GizmoSpace::World,
            });
        }
        if changed {
            level.set_snap(snap);
        }
    }

    /// "Add ▾": objects for the level in front.
    fn add_menu(&mut self, ui: &mut egui::Ui, id: u64) {
        let response = theme::tool_button(
            ui,
            Some(Icon::Object),
            &format!("{} ▾", tr("toolbar.add")),
            false,
            Tone::Plain,
        )
        .on_hover_text(tr("toolbar.add_hint"));
        let (selected, loaded) = self
            .levels
            .get(&id)
            .map_or((false, false), |l| (l.has_selection(), l.loaded()));
        let mut chosen = None;
        egui::Popup::menu(&response).show(|ui| {
            ui.set_min_width(240.0);
            if ui
                .add_enabled(selected, egui::Button::new(tr("toolbar.add_duplicate")))
                .on_disabled_hover_text(tr("toolbar.add_needs_selection"))
                .clicked()
            {
                chosen = Some(0);
            }
            if ui
                .add_enabled(selected, egui::Button::new(tr("toolbar.add_object")))
                .on_hover_text(tr("level.add_object_hint"))
                .on_disabled_hover_text(tr("toolbar.add_needs_selection"))
                .clicked()
            {
                chosen = Some(1);
            }
            ui.separator();
            for command in [Command::WizardPlace, Command::NewLevel] {
                if ui
                    .add_enabled(self.enabled(command), egui::Button::new(command.label()))
                    .clicked()
                {
                    self.pending.push(command);
                }
            }
            if !loaded {
                ui.label(RichText::new(tr("toolbar.add_load_first")).weak().small());
            }
        });
        match chosen {
            Some(0) => {
                if let Some(level) = self.levels.get_mut(&id) {
                    level.request_duplicate();
                }
            }
            Some(1) => {
                if let Some(level) = self.levels.get_mut(&id) {
                    level.request_add();
                }
                let docked = self
                    .dock
                    .as_ref()
                    .is_some_and(|t| dock::find(t, |p| *p == Pane::Details).is_some());
                if docked || self.drawers.contains(&Pane::Details) {
                    self.dock_ops.push(DockOp::ShowPanel(Pane::Details));
                }
                self.inspect = super::details::Focus::Level(id);
            }
            _ => {}
        }
    }

    /// "▾" next to Apply: direct mode, revert, restore.
    fn apply_menu(&mut self, ui: &mut egui::Ui) {
        let arrow = theme::menu_arrow(ui, Tone::Plain).on_hover_text(tr("toolbar.apply_more"));
        egui::Popup::menu(&arrow).show(|ui| {
            ui.set_min_width(240.0);
            ui.checkbox(&mut self.direct, tr("edits.direct"))
                .on_hover_text(tr("edits.direct_hint"));
            ui.separator();
            for command in [
                Command::RevertGame,
                Command::RestoreOriginals,
                Command::ToggleChanges,
            ] {
                if ui
                    .add_enabled(self.enabled(command), egui::Button::new(command.label()))
                    .clicked()
                {
                    self.pending.push(command);
                }
            }
        });
    }

    /// "▾" next to Play: options, play from the level camera, find games.
    pub(super) fn play_menu(&mut self, ui: &mut egui::Ui) {
        let arrow = theme::menu_arrow(ui, Tone::Play).on_hover_text(tr("cmd.play_options"));
        let level = match self.toolbar_context() {
            Context::Level(id) => Some(id),
            _ => None,
        };
        egui::Popup::menu(&arrow).show(|ui| {
            ui.set_min_width(220.0);
            for command in [Command::PlayOptions, Command::FindGames] {
                if ui
                    .add_enabled(self.enabled(command), egui::Button::new(command.label()))
                    .clicked()
                {
                    self.pending.push(command);
                }
            }
            if let Some(id) = level {
                let loaded = self.levels.get(&id).is_some_and(|l| l.loaded());
                if ui
                    .add_enabled(loaded, egui::Button::new(tr("gametab.play_here")))
                    .on_hover_text(tr("gametab.play_here_hint"))
                    .clicked()
                    && let Some(level) = self.levels.get_mut(&id)
                {
                    level.request_play_here();
                }
            }
        });
    }
}

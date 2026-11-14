//! The menu bar, built from the command table, with back and forward and the
//! search box at its right end (the main toolbar under it is `toolbar`).

use doctor_toolkit_mods::i18n::tr;

use super::commands::{COMMANDS, Menu};
use super::{BrowserApp, Command};

impl BrowserApp {
    pub(super) fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            for menu in Menu::ALL {
                ui.menu_button(tr(menu.key()), |ui| {
                    ui.set_min_width(220.0);
                    for spec in COMMANDS.iter().filter(|s| s.menu == menu) {
                        if spec.separator {
                            ui.separator();
                        }
                        self.menu_item(ui, spec.command);
                        if spec.command == Command::OpenStream {
                            self.recent_menu(ui);
                        }
                        if spec.command == Command::ResetLayout {
                            self.layouts_menu(ui);
                        }
                    }
                });
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_enabled_ui(self.game.is_some(), |ui| {
                    if let Some(action) = self.nav.search_box(ui) {
                        self.nav_action(action);
                    }
                    for (command, text) in [(Command::Forward, ">"), (Command::Back, "<")] {
                        let hover = format!(
                            "{} ({})",
                            command.label(),
                            command.shortcut_text(ui.ctx()).unwrap_or_default()
                        );
                        if ui
                            .add_enabled(self.enabled(command), egui::Button::new(text).small())
                            .on_hover_text(hover)
                            .clicked()
                        {
                            self.pending.push(command);
                        }
                    }
                });
            });
        });
    }

    fn menu_item(&mut self, ui: &mut egui::Ui, command: Command) {
        let label = command.label();
        let mut button = match self.checked(command) {
            Some(checked) => egui::Button::selectable(checked, label),
            None => egui::Button::new(label),
        };
        if let Some(text) = command.shortcut_text(ui.ctx()) {
            button = button.shortcut_text(text);
        }
        if ui.add_enabled(self.enabled(command), button).clicked() {
            self.pending.push(command);
        }
    }

    /// File → Recent games.
    fn recent_menu(&mut self, ui: &mut egui::Ui) {
        let enabled = !self.settings.recent_games.is_empty() && self.job.is_none();
        ui.add_enabled_ui(enabled, |ui| {
            ui.menu_button(tr("menu.recent"), |ui| {
                let mut open = None;
                for dir in &self.settings.recent_games {
                    if ui.button(dir.display().to_string()).clicked() {
                        open = Some(dir.clone());
                    }
                }
                ui.separator();
                if ui.button(tr("menu.clear_recent")).clicked() {
                    self.settings.recent_games.clear();
                }
                if let Some(dir) = open {
                    self.open_game(dir, ui.ctx());
                }
            });
        });
    }
}

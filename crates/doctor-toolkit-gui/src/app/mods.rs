//! The Mods window of the editor: the mod manager of the open game, plus
//! building a mod from the workspace. The window is the shared panel of
//! Open Mod Manager (`open-mod-manager`).

use std::sync::Arc;

use doctor_toolkit_mods::PanelEvent;
use doctor_toolkit_mods::i18n::tr;

use super::BrowserApp;

impl BrowserApp {
    /// Shows the Mods window and reacts to what it did to the game files.
    pub(super) fn show_mods(&mut self, ctx: &egui::Context) {
        if !self.mods_open {
            return;
        }
        // Open the mods of the current game the first time the window shows it.
        let same_game = match (self.mods.game(), &self.game) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if !same_game {
            self.mods.set_game(self.game.clone(), ctx);
        }
        if self.integrity_queued && !self.mods.is_busy() && self.mods.game().is_some() {
            self.integrity_queued = false;
            self.mods.check_integrity(ctx);
        }
        let mut open = self.mods_open;
        let mut event = None;
        egui::Window::new(tr("window.mods"))
            .open(&mut open)
            .default_size([860.0, 600.0])
            .resizable(true)
            .show(ctx, |ui| {
                event = self.mods.show(ui, self.workspace.as_ref());
            });
        self.mods_open = open;
        match event {
            // Release the memory-mapped streams before their files are replaced.
            Some(PanelEvent::WillWrite) => self.release_files(),
            Some(PanelEvent::GameChanged) => {
                self.refresh_status();
                self.files_changed();
            }
            Some(PanelEvent::PlayWithMods) | None => {}
        }
    }

    /// Opens the Mods window and checks which game files differ from the originals.
    /// The check starts as soon as the window has read the mods.
    pub(super) fn integrity_check(&mut self) {
        self.mods_open = true;
        self.integrity_queued = true;
    }

    /// Whether the Mods window is still working, for the screenshot mode.
    pub(super) fn mods_busy(&self) -> bool {
        self.mods_open && (self.mods.is_busy() || self.mods.game().is_none())
    }
}

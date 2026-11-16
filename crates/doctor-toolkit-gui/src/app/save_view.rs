//! Tools → Save file: a look into a save of the game
//! (`docs/formats/save.md`). The file is read once into memory and never
//! written; the texts are those of `doctor-toolkit save`.

use std::path::PathBuf;

use doctor_toolkit::save::{SaveFile, view};
use doctor_toolkit_mods::i18n::{tr, trf};
use egui::RichText;

use super::BrowserApp;

/// A save read from disk.
struct Loaded {
    path: PathBuf,
    save: SaveFile,
    summary: String,
    /// The chosen chunk and its text; `None` shows the summary.
    chunk: Option<(usize, String)>,
    /// Whether the text of the chunk was made with names of the game.
    named: bool,
}

/// The Save file window.
#[derive(Default)]
pub(super) struct SaveWindow {
    open: bool,
    loaded: Option<Loaded>,
    error: Option<String>,
}

impl SaveWindow {
    /// The window at start: with `DOCTOR_TOOLKIT_GUI_SAVE=<file>` it shows
    /// that save (for screenshots).
    pub(super) fn from_env() -> Self {
        let mut window = Self::default();
        if let Some(path) = std::env::var_os("DOCTOR_TOOLKIT_GUI_SAVE") {
            window.load(PathBuf::from(path));
        }
        window
    }

    /// Reads `path`; an error is shown in the window.
    pub(super) fn load(&mut self, path: PathBuf) {
        self.open = true;
        match SaveFile::open(&path) {
            Ok(save) => {
                self.error = None;
                self.loaded = Some(Loaded {
                    summary: view::summary(&save),
                    path,
                    save,
                    chunk: None,
                    named: false,
                });
            }
            Err(e) => {
                self.error = Some(trf(
                    "save_view.error",
                    &[("file", &path.display()), ("error", &e)],
                ));
            }
        }
    }
}

impl BrowserApp {
    /// Tools → Save file: asks for a file and shows it.
    pub(super) fn open_save_file(&mut self) {
        let mut dialog = rfd::FileDialog::new().set_title(tr("save_view.pick"));
        if let Some(dir) = self.save_view.loaded.as_ref().and_then(|l| l.path.parent()) {
            dialog = dialog.set_directory(dir);
        }
        match dialog.pick_file() {
            Some(path) => self.save_view.load(path),
            // Cancelled: show what was open before, if anything.
            None => self.save_view.open = self.save_view.loaded.is_some(),
        }
    }

    pub(super) fn show_save_view(&mut self, ctx: &egui::Context) {
        if !self.save_view.open {
            return;
        }
        let names = self.play.live.names_now();
        let mut open = true;
        let mut pick = false;
        let window = &mut self.save_view;
        egui::Window::new(tr("window.save"))
            .open(&mut open)
            .default_size([900.0, 560.0])
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    pick = ui.button(tr("save_view.open")).clicked();
                    if let Some(loaded) = &window.loaded {
                        ui.label(RichText::new(loaded.path.display().to_string()).weak());
                    }
                });
                ui.label(RichText::new(tr("save_view.read_only")).weak());
                if let Some(error) = &window.error {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
                let Some(loaded) = &mut window.loaded else {
                    return;
                };
                if !loaded.save.checksums.ok() {
                    ui.colored_label(ui.visuals().error_fg_color, tr("save_view.bad_checksums"));
                }
                if names.is_none() {
                    ui.label(RichText::new(tr("save_view.no_names")).weak());
                }
                ui.separator();
                // A chunk shown without names is made again once they load.
                if let Some((index, _)) = loaded.chunk
                    && names.is_some() != loaded.named
                {
                    let text = view::chunk_text(&loaded.save.chunks[index], names.as_deref());
                    loaded.chunk = Some((index, text));
                    loaded.named = names.is_some();
                }
                let mut choose = None;
                egui::Panel::left("save_chunks")
                    .resizable(true)
                    .default_size(190.0)
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            let chosen = loaded.chunk.as_ref().map(|c| c.0);
                            if ui
                                .selectable_label(chosen.is_none(), tr("save_view.summary"))
                                .clicked()
                            {
                                choose = Some(None);
                            }
                            for (i, chunk) in loaded.save.chunks.iter().enumerate() {
                                let label = format!("{} ({})", chunk.name(), chunk.data.len());
                                if ui.selectable_label(chosen == Some(i), label).clicked() {
                                    choose = Some(Some(i));
                                }
                            }
                        });
                    });
                match choose {
                    Some(None) => loaded.chunk = None,
                    Some(Some(i)) => {
                        let text = view::chunk_text(&loaded.save.chunks[i], names.as_deref());
                        loaded.chunk = Some((i, text));
                        loaded.named = names.is_some();
                    }
                    None => {}
                }
                let text = match &loaded.chunk {
                    Some((_, text)) => text.as_str(),
                    None => loaded.summary.as_str(),
                };
                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(RichText::new(text).monospace())
                                .selectable(true)
                                .extend(),
                        );
                    });
            });
        self.save_view.open = open;
        if pick {
            self.open_save_file();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_save_loads_and_a_wrong_file_is_an_error() {
        let dir = std::env::temp_dir().join(format!("dt-save-view-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("GF2-EN-autosave");
        let data = doctor_toolkit::save::build("P", "Cuba", 5, 0xB, &[(0x99, vec![1])]).unwrap();
        std::fs::write(&good, &data).unwrap();
        let mut window = SaveWindow::default();
        window.load(good.clone());
        assert!(window.open && window.error.is_none());
        let loaded = window.loaded.as_ref().unwrap();
        assert!(loaded.summary.contains("P - Cuba - 0:00:05"));
        // The file was only read.
        assert_eq!(std::fs::read(&good).unwrap(), data);

        let bad = dir.join("notes.txt");
        std::fs::write(&bad, b"not a save").unwrap();
        window.load(bad);
        assert!(window.error.is_some());
        // The save shown before stays.
        assert!(window.loaded.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

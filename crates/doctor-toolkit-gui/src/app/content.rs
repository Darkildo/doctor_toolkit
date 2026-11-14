//! The Content Browser panel as stage 52 has it: the folders of the game's
//! streams on the left, their streams on the right; a click opens a stream.
//! Stage 55 replaces it with assets and previews. It is a drawer by default
//! (Ctrl+Space).

use std::collections::BTreeMap;

use egui::RichText;

use doctor_toolkit_mods::i18n::{tr, trn};

use super::theme::{self, Icon, Tone};
use super::{BrowserApp, FileSel};

/// What the panel shows.
#[derive(Default)]
pub struct ContentState {
    /// The folder chosen on the left (`gf2/cuba`); `None` for every one.
    folder: Option<String>,
    filter: String,
}

/// The folder of a stream path: its first two parts.
fn folder_of(path: &str) -> &str {
    let mut ends = path.match_indices('/').map(|(i, _)| i);
    match (ends.next(), ends.next()) {
        (Some(_), Some(second)) => &path[..second],
        (Some(first), None) => &path[..first],
        _ => "",
    }
}

impl BrowserApp {
    pub(super) fn content_pane(&mut self, ui: &mut egui::Ui) {
        let Some(game) = self.game.clone() else {
            ui.label(RichText::new(tr("files.no_game")).weak());
            return;
        };
        let mut folders: BTreeMap<&str, usize> = BTreeMap::new();
        for file in game.streams() {
            *folders.entry(folder_of(&file.rel_path)).or_default() += 1;
        }
        let mut open = None;
        let state = &mut self.content;
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut state.filter)
                    .hint_text(tr("content.filter"))
                    .desired_width(220.0),
            );
            let indexed = self.index.as_ref().map_or(0, |i| i.entries.len());
            ui.label(RichText::new(trn("count.resources", indexed as u64, &[])).weak());
            ui.label(RichText::new(tr("content.hint")).weak().small());
        });
        ui.separator();
        let filter = state.filter.to_lowercase();
        egui::Panel::left("content-folders")
            .resizable(true)
            .default_size(180.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("content-folders")
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        if ui
                            .selectable_label(state.folder.is_none(), tr("content.all"))
                            .clicked()
                        {
                            state.folder = None;
                        }
                        for (folder, count) in &folders {
                            let name = if folder.is_empty() { "/" } else { folder };
                            let on = state.folder.as_deref() == Some(*folder);
                            if ui
                                .selectable_label(on, format!("{name}  ({count})"))
                                .clicked()
                            {
                                state.folder = Some((*folder).to_owned());
                            }
                        }
                    });
            });
        egui::ScrollArea::vertical()
            .id_salt("content-streams")
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let mut shown = 0;
                    for (i, file) in game.streams().iter().enumerate() {
                        let path = &file.rel_path;
                        if state
                            .folder
                            .as_deref()
                            .is_some_and(|f| folder_of(path) != f)
                            || (!filter.is_empty() && !path.to_lowercase().contains(&filter))
                        {
                            continue;
                        }
                        shown += 1;
                        if shown > 400 {
                            break;
                        }
                        let name = path.rsplit('/').next().unwrap_or(path);
                        let response =
                            theme::tool_button(ui, Some(Icon::Folder), name, false, Tone::Plain)
                                .on_hover_text(format!("{path}\n{}", super::human_size(file.len)));
                        if response.clicked() {
                            open = Some(FileSel::Stream(i));
                        }
                    }
                    if shown == 0 {
                        ui.label(RichText::new(tr("palette.nothing")).weak());
                    }
                });
            });
        if let Some(sel) = open {
            self.open_file(sel, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::folder_of;

    #[test]
    fn folders_are_the_first_two_parts() {
        assert_eq!(folder_of("gf2/cuba/props/crates.str"), "gf2/cuba");
        assert_eq!(folder_of("gf2/frontend.str"), "gf2");
        assert_eq!(folder_of("frontend.str"), "");
    }
}

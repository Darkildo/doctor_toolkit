//! Details of an `apx` (Flash interface) resource: blocks and embedded
//! textures, with import of textures and of the `.apt` and `.const` data.
//!
//! Imports are not written here: [`ApxView::show`] returns the new data of
//! the resource and the app records it in the workspace.

use doctor_toolkit::game::StreamFile;
use doctor_toolkit::hash::sdbm;
use doctor_toolkit::resource::apx::{AptFile, TAG_ALIGN, TAG_APT, TAG_CONSTANTS};
use doctor_toolkit::resource::texture::Texture;
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::RichText;

use crate::changes_view::PENDING;
use crate::texture_view::TexturePreview;

pub const APX: u32 = sdbm(b"apx");

/// What the user asked for.
pub enum ApxAction {
    /// Record new data for the resource.
    Save { data: Vec<u8>, label: String },
    /// Point the `$old` text references at `$new` (stage 103).
    Retarget { old: String, new: String },
}

pub struct ApxView {
    file: Result<AptFile, String>,
    textures: Vec<Texture>,
    /// The game's textures when the file is an edit.
    original: Option<Vec<Texture>>,
    selected: usize,
    preview: Option<TexturePreview>,
    /// The `$name` text references: (name, how many, the new name typed).
    refs: Vec<(String, usize, String)>,
    /// Stream and resource of the movie in the game.
    at: Option<(usize, usize)>,
}

impl ApxView {
    /// Loads resource `index` if it is an apx file.
    pub fn load(stream: &StreamFile, index: usize) -> Option<Self> {
        if stream.layout().resources[index].header.type_id != APX {
            return None;
        }
        Some(Self::from_data(
            stream.resource_data(index).map_err(|e| e.to_string()),
            None,
        ))
    }

    /// A view of `data`; `original` is the game's version when `data` is an edit.
    pub fn from_data(data: Result<Vec<u8>, String>, original: Option<Vec<u8>>) -> Self {
        let file = data.and_then(|data| AptFile::parse(&data).map_err(|e| e.to_string()));
        let textures = file
            .as_ref()
            .ok()
            .and_then(|f| f.textures().ok())
            .unwrap_or_default();
        let original = original
            .and_then(|d| AptFile::parse(&d).ok())
            .and_then(|f| f.textures().ok());
        let mut refs: Vec<(String, usize, String)> = Vec::new();
        if let Ok(file) = &file {
            for (_, _, name) in file.text_references() {
                match refs.iter_mut().find(|r| r.0 == name) {
                    Some(r) => r.1 += 1,
                    None => refs.push((name, 1, String::new())),
                }
            }
        }
        let mut view = Self {
            file,
            textures,
            original,
            selected: 0,
            preview: None,
            refs,
            at: None,
        };
        view.select(0);
        view
    }

    /// The `$name` text references of the movie: (name, how many).
    #[cfg(test)]
    pub fn text_refs(&self) -> Vec<(&str, usize)> {
        self.refs.iter().map(|r| (r.0.as_str(), r.1)).collect()
    }

    /// The list of `$name` references, each with a field for another
    /// string and its button.
    fn refs_ui(&mut self, ui: &mut egui::Ui) -> Option<ApxAction> {
        if self.refs.is_empty() {
            return None;
        }
        let mut action = None;
        egui::CollapsingHeader::new(trf("apx.text_refs", &[("count", &self.refs.len())]))
            .id_salt("apx-text-refs")
            .default_open(true)
            .show(ui, |ui| {
                ui.label(RichText::new(tr("apx.text_refs_hint")).weak().small());
                egui::ScrollArea::vertical()
                    .id_salt("apx-text-refs-list")
                    .max_height(220.0)
                    .show(ui, |ui| {
                        egui::Grid::new("apx-text-refs-grid")
                            .num_columns(4)
                            .striped(true)
                            .show(ui, |ui| {
                                for (name, count, new) in &mut self.refs {
                                    ui.monospace(format!("${name}"));
                                    ui.label(
                                        RichText::new(format!(
                                            "{:08x} ×{count}",
                                            doctor_toolkit::hash::string_id(name)
                                        ))
                                        .weak()
                                        .monospace(),
                                    );
                                    ui.add_sized(
                                        [200.0, ui.spacing().interact_size.y],
                                        egui::TextEdit::singleline(new)
                                            .hint_text(tr("apx.new_name"))
                                            .char_limit(name.len()),
                                    );
                                    let typed = new.trim().trim_start_matches('$');
                                    let ok = doctor_toolkit::text::valid_name(typed)
                                        && typed.len() <= name.len()
                                        && !typed.eq_ignore_ascii_case(name);
                                    if ui
                                        .add_enabled(ok, egui::Button::new(tr("apx.retarget")))
                                        .on_hover_text(tr("apx.retarget_hint"))
                                        .clicked()
                                    {
                                        action = Some(ApxAction::Retarget {
                                            old: name.clone(),
                                            new: typed.to_owned(),
                                        });
                                    }
                                    ui.end_row();
                                }
                            });
                    });
            });
        action
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Shows embedded texture `index`, marked when it differs from the game's.
    pub fn select(&mut self, index: usize) {
        if let Some(preview) = preview(&self.textures, self.original.as_deref(), index) {
            self.selected = index;
            self.preview = Some(preview);
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, status: &mut String) -> Option<ApxAction> {
        let file = match &self.file {
            Ok(file) => file,
            Err(error) => {
                ui.colored_label(egui::Color32::RED, error);
                return None;
            }
        };
        let title = file.title().unwrap_or_default();
        let mut action = self.refs_ui(ui);
        let file = self.file.as_ref().ok()?;
        ui.horizontal_wrapped(|ui| {
            ui.strong(&title);
            let blocks: Vec<String> = file
                .blocks
                .iter()
                .filter(|b| b.tag != TAG_ALIGN)
                .map(|b| format!("{} {}", String::from_utf8_lossy(&b.tag), b.data.len()))
                .collect();
            ui.label(RichText::new(blocks.join(" · ")).weak());
        });
        ui.horizontal(|ui| {
            for (tag, extension, what) in
                [(TAG_APT, "apt", "apt"), (TAG_CONSTANTS, "const", "const")]
            {
                let Some(block) = file.block(tag) else {
                    continue;
                };
                if ui
                    .button(trf("apx.export", &[("extension", &extension)]))
                    .on_hover_text(tr(&format!("apx.export_{what}_hint")))
                    .clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .set_file_name(format!("{title}.{extension}"))
                        .save_file()
                {
                    *status = match std::fs::write(&path, &block.data) {
                        Ok(()) => trf("model.saved", &[("path", &path.display())]),
                        Err(error) => trf("apx.cannot_save", &[("error", &error)]),
                    };
                }
                if ui
                    .button(trf("apx.import", &[("extension", &extension)]))
                    .on_hover_text(tr(&format!("apx.import_{what}_hint")))
                    .clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .add_filter(extension, &[extension])
                        .pick_file()
                {
                    let mut edited = file.clone();
                    let result =
                        std::fs::read(&path)
                            .map_err(|e| e.to_string())
                            .and_then(|bytes| {
                                if tag == TAG_APT {
                                    edited.set_apt(bytes)
                                } else {
                                    edited.set_constants(bytes)
                                }
                                .and_then(|()| edited.to_bytes())
                                .map_err(|e| e.to_string())
                            });
                    match result {
                        Ok(data) => {
                            action = Some(ApxAction::Save {
                                data,
                                label: trf(&format!("apx.label_{what}"), &[("title", &title)]),
                            });
                        }
                        Err(error) => *status = trf("apx.cannot_import", &[("error", &error)]),
                    }
                }
            }
        });
        if action.is_some() || self.textures.is_empty() {
            return action;
        }
        let names: Vec<String> = self
            .textures
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let edited = self
                    .original
                    .as_ref()
                    .and_then(|o| o.get(i))
                    .is_some_and(|o| o.header != t.header || o.data != t.data);
                format!(
                    "{i}: {} ({}×{}){}",
                    t.header.name,
                    t.header.width,
                    t.header.height,
                    if edited { " *" } else { "" }
                )
            })
            .collect();
        let mut selected = self.selected;
        let mut text = RichText::new(trf(
            "apx.embedded",
            &[
                (
                    "textures",
                    &trn("apx.embedded_textures", names.len() as u64, &[]),
                ),
                ("name", &names[self.selected]),
            ],
        ));
        if names.iter().any(|n| n.ends_with('*')) {
            text = text.color(PENDING);
        }
        egui::ComboBox::from_id_salt("apx-texture")
            .selected_text(text)
            .show_ui(ui, |ui| {
                for (i, name) in names.iter().enumerate() {
                    ui.selectable_value(&mut selected, i, name);
                }
            });
        if selected != self.selected
            && let Some(preview) = preview(&self.textures, self.original.as_deref(), selected)
        {
            self.selected = selected;
            self.preview = Some(preview);
        }
        let import = self.preview.as_mut().and_then(|p| p.show(ui, status));
        if let Some(path) = import {
            let index = self.selected;
            let is_png = path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("png"));
            let mut edited = file.clone();
            let result = std::fs::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|bytes| {
                    let texture = edited
                        .import_texture(index, &bytes, is_png)
                        .map_err(|e| e.to_string())?;
                    Ok((texture, edited.to_bytes().map_err(|e| e.to_string())?))
                });
            match result {
                Ok((texture, data)) => {
                    action = Some(ApxAction::Save {
                        data,
                        label: trf(
                            "apx.label_texture",
                            &[
                                ("index", &index),
                                ("name", &texture.header.name),
                                ("title", &title),
                            ],
                        ),
                    });
                }
                Err(error) => *status = trf("apx.cannot_import", &[("error", &error)]),
            }
        }
        action
    }
}

/// The preview of embedded texture `index`, compared with the game's when it differs.
fn preview(
    textures: &[Texture],
    original: Option<&[Texture]>,
    index: usize,
) -> Option<TexturePreview> {
    let texture = textures.get(index)?.clone();
    let original = original
        .and_then(|o| o.get(index))
        .filter(|o| o.header != texture.header || o.data != texture.data)
        .cloned();
    Some(TexturePreview::embedded(texture, original))
}

// ------------------------------------------------------------ view registry

/// Flash interfaces (`apx`): blocks and embedded textures.
pub const VIEW: crate::views::ViewEntry = crate::views::ViewEntry {
    id: "apx",
    load: load_view,
};

fn load_view(
    source: &mut crate::views::ViewSource<'_>,
) -> Option<Box<dyn crate::views::ResourceViewer>> {
    if source.type_id != APX {
        return None;
    }
    let mut view = match source.edited(source.index) {
        Some(data) => ApxView::from_data(data, source.original(source.index)),
        None => ApxView::load(source.stream, source.index)?,
    };
    view.at = Some((source.stream_index, source.index));
    Some(Box::new(view))
}

impl crate::views::ResourceViewer for ApxView {
    fn show(
        &mut self,
        ui: &mut egui::Ui,
        cx: &mut crate::views::ViewContext<'_>,
    ) -> Option<crate::views::ViewAction> {
        match ApxView::show(self, ui, cx.status)? {
            ApxAction::Save { data, label } => {
                Some(crate::views::ViewAction::SaveData { data, label })
            }
            ApxAction::Retarget { old, new } => {
                let (stream, resource) = self.at?;
                Some(crate::views::ViewAction::Apply {
                    label: trf("apx.retarget_label", &[("old", &old), ("new", &new)]),
                    run: Box::new(move |game, ws| {
                        let changed = doctor_toolkit::text::retarget_apx(
                            game, ws, stream, resource, &old, &new,
                        )?;
                        if changed == 0 {
                            return Err(doctor_toolkit::Error::InvalidArgument(format!(
                                "the movie has no reference ${old}"
                            )));
                        }
                        Ok(())
                    }),
                })
            }
        }
    }

    fn state(&self) -> Option<Box<dyn std::any::Any>> {
        Some(Box::new(self.selected()))
    }

    fn restore(&mut self, state: Box<dyn std::any::Any>) {
        if let Ok(index) = state.downcast::<usize>() {
            self.select(*index);
        }
    }
}

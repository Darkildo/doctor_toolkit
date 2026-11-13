//! "Open in external editor" for textures, string tables, logic, scripts and
//! models: a bar above the other views of the resource. The app exports the
//! resource, opens it with the program of the settings, watches the file and
//! imports every save (`app/external.rs`, `doctor_toolkit::external`).

use doctor_toolkit::external::{self, Format};
use doctor_toolkit::resource::texture;
use doctor_toolkit::workspace::ResourceKey;
use doctor_toolkit_mods::i18n::{tr, trf};
use egui::RichText;

use crate::views::{ResourceViewer, ViewAction, ViewContext, ViewEntry, ViewSource};

pub const VIEW: ViewEntry = ViewEntry {
    id: "external",
    load,
};

/// A resource linked to a working file, as the views see it.
#[derive(Debug, Clone)]
pub struct LinkInfo {
    pub key: ResourceKey,
    pub path: std::path::PathBuf,
    /// What happened last, for the bar.
    pub state: String,
}

pub struct ExternalBar {
    /// The key a link of this resource has: the header of a texture.
    key: Option<ResourceKey>,
    formats: &'static [Format],
    format: Format,
}

fn load(source: &mut ViewSource<'_>) -> Option<Box<dyn ResourceViewer>> {
    let formats = external::formats_for(source.type_id);
    let first = *formats.first()?;
    // The data half of a texture shares the link of its header.
    let index =
        texture::pair_of(source.stream.layout(), source.index).map_or(source.index, |p| p.header);
    Some(Box::new(ExternalBar {
        key: source.key(index),
        formats,
        format: first,
    }))
}

impl ResourceViewer for ExternalBar {
    fn show(&mut self, ui: &mut egui::Ui, cx: &mut ViewContext<'_>) -> Option<ViewAction> {
        let mut action = None;
        let link = self
            .key
            .as_ref()
            .and_then(|k| cx.links.iter().find(|l| &l.key == k));
        ui.horizontal_wrapped(|ui| {
            let open = ui
                .button(tr("external.open"))
                .on_hover_text(tr("external.open_hint"));
            if self.formats.len() > 1 {
                egui::ComboBox::from_id_salt("external-format")
                    .selected_text(self.format.extension().to_uppercase())
                    .width(70.0)
                    .show_ui(ui, |ui| {
                        for &f in self.formats {
                            ui.selectable_value(&mut self.format, f, f.extension().to_uppercase());
                        }
                    });
            }
            if open.clicked() {
                action = Some(ViewAction::OpenExternal {
                    format: self.format,
                });
            }
        });
        if let Some(link) = link {
            ui.horizontal_wrapped(|ui| {
                let name = link
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                ui.add(
                    egui::Label::new(
                        RichText::new(trf("external.linked", &[("file", &name)]))
                            .color(crate::changes_view::PENDING),
                    )
                    .truncate(),
                )
                .on_hover_text(format!(
                    "{}\n{}",
                    link.path.display(),
                    tr("external.linked_hint")
                ));
                if ui
                    .small_button(tr("external.unlink"))
                    .on_hover_text(tr("external.unlink_hint"))
                    .clicked()
                {
                    action = Some(ViewAction::Unlink(link.key.clone()));
                }
            });
            if !link.state.is_empty() {
                ui.add(egui::Label::new(RichText::new(&link.state).weak()).truncate());
            }
        }
        action
    }
}

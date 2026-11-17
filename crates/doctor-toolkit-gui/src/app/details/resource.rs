//! Details of a resource: its header, references and views (the Details
//! panel of the resource lists up to stage 52).

use doctor_toolkit_mods::i18n::tr;
use egui::RichText;

use super::super::BrowserApp;
use super::super::docs::ResourceDoc;

impl BrowserApp {
    /// The resource selected in a list.
    pub(super) fn resource_details(&mut self, ui: &mut egui::Ui) {
        let Some(mut doc) = self.details.take() else {
            ui.label(RichText::new(tr("details.select")).weak());
            return;
        };
        // The note of the resource type in the knowledge base (stage 103).
        if let Some(rel) = super::super::knowledge::resource_note(doc.type_id) {
            super::super::knowledge::note_link(ui, &rel);
        }
        // The commands of the asset, as on the bar of its tab (stage 103).
        let mut cmds = Vec::new();
        ui.menu_button(tr("doc.asset"), |ui| {
            super::super::asset_tab::asset_menu(ui, false, doc.type_id, &mut cmds);
        })
        .response
        .on_hover_text(tr("doc.asset_hint"));
        self.asset_cmds
            .extend(cmds.into_iter().map(|c| (doc.loc, c)));
        let action = self.doc_ui(ui, &mut doc, false);
        // An action may load the doc again: put it back first.
        if self.details.is_none() {
            self.details = Some(doc);
        }
        if let Some(action) = action {
            let loc = self.details.as_ref().map(|d| d.loc);
            self.doc_action(loc, action);
        }
    }

    /// A resource a Details panel is pinned to.
    pub(super) fn pinned_resource_details(&mut self, ui: &mut egui::Ui, doc: &mut ResourceDoc) {
        let action = self.doc_ui(ui, doc, false);
        if let Some(action) = action {
            self.doc_action(Some(doc.loc), action);
        }
    }
}

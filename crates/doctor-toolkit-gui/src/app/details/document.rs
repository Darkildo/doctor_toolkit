//! Details of an asset editor (stage 56): what is selected in the editor
//! in front — a node of the logic graph or of a script, a line of a
//! dialogue, a part of a model, a movie — as the selection of a level shows
//! its object. The editors draw their side column only while Details is
//! closed (`ViewContext::details`).

use doctor_toolkit_mods::i18n::tr;
use egui::RichText;

use super::super::BrowserApp;
use super::DocRef;

impl BrowserApp {
    pub(super) fn document_details(&mut self, ui: &mut egui::Ui, doc: DocRef) {
        match doc {
            DocRef::Tab(id) if self.docs.contains_key(&id) => self.resource_tab_details(ui, id),
            DocRef::Tab(id) => {
                let Some(editor) = self.logics.get_mut(&id) else {
                    return;
                };
                ui.heading(editor.title());
                ui.separator();
                // The overview's names are short: let them wrap, not cut.
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                if let Some(action) = editor.inspector(ui) {
                    let ctx = ui.ctx().clone();
                    self.logic_action(action, &ctx);
                }
            }
            DocRef::Script => {
                ui.heading(tr("script.title"));
                ui.label(RichText::new(&self.script_editor.graph().name).weak());
                ui.separator();
                self.script_editor.inspector_ui(ui);
            }
            DocRef::Movies => {
                ui.heading(tr("movies.title"));
                ui.separator();
                self.movies.details_ui(ui);
            }
        }
    }
}

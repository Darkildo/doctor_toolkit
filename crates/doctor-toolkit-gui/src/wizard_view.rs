//! "Start wizard" buttons above the views of resources a wizard works on:
//! models (retexture, replace), parts databases (retexture a character, new
//! outfit), string tables (edit texts) and SimGroups (game parameters).
//! The wizards themselves are in `app/wizard/`.

use doctor_toolkit::hash::sdbm;
use doctor_toolkit_mods::i18n::tr;
use egui::RichText;

use crate::views::{ResourceViewer, ViewAction, ViewContext, ViewEntry, ViewSource};

pub const VIEW: ViewEntry = ViewEntry {
    id: "wizards",
    load,
};

/// Wizards for a resource type: `(task, text key of the button)`.
pub fn tasks_for(type_id: u32) -> &'static [(&'static str, &'static str)] {
    const MESH: u32 = sdbm(b"mesh");
    const METAMODEL: u32 = sdbm(b"metamodel");
    const PARTSDB: u32 = sdbm(b"partsdb");
    const LH2: u32 = sdbm(b"lh2");
    const SIMGROUP: u32 = sdbm(b"simgroup");
    match type_id {
        MESH | METAMODEL => &[
            ("retexture", "wizard.retexture.title"),
            ("model", "wizard.model.title"),
            ("place", "wizard.place.title"),
        ],
        PARTSDB => &[
            ("outfit", "wizard.outfit.title"),
            ("retexture", "wizard.retexture.title"),
        ],
        LH2 => &[("texts", "wizard.texts.title")],
        SIMGROUP => &[("params", "wizard.params.title")],
        _ => &[],
    }
}

pub struct WizardBar {
    tasks: &'static [(&'static str, &'static str)],
}

fn load(source: &mut ViewSource<'_>) -> Option<Box<dyn ResourceViewer>> {
    let tasks = tasks_for(source.type_id);
    // A metamodel can be placed; a bare mesh cannot.
    let tasks = if source.type_id == sdbm(b"mesh") {
        &tasks[..2]
    } else {
        tasks
    };
    (!tasks.is_empty()).then(|| Box::new(WizardBar { tasks }) as Box<dyn ResourceViewer>)
}

impl ResourceViewer for WizardBar {
    fn show(&mut self, ui: &mut egui::Ui, _cx: &mut ViewContext<'_>) -> Option<ViewAction> {
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(tr("wizard.view_label")).weak());
            for &(task, key) in self.tasks {
                if ui
                    .small_button(tr(key))
                    .on_hover_text(tr("wizard.view_hint"))
                    .clicked()
                {
                    action = Some(ViewAction::StartWizard(task));
                }
            }
        });
        action
    }
}

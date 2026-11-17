//! Guided wizards for typical modding tasks (stage 23, `docs/guide/15-wizards.md`).
//!
//! A wizard is a tab ([`Pane::Wizard`]) that walks through the steps of one
//! task with Back and Next; Next is offered only when the step is complete.
//! Every wizard ends with the same two steps: a **summary** of what will be
//! recorded, whose button records it, and **done**, with "Test in game"
//! (Play) and "Build mod…".
//!
//! Whatever a wizard records becomes **one undoable operation**: the
//! workspace history length is noted when the task starts recording
//! ([`Wizard::mark`]) and the operations made since then are joined
//! ([`doctor_toolkit::workspace::Workspace::squash`]). Wizards that record
//! as they go (placing an object in the level view) open the mark early;
//! Cancel then takes their edits back
//! ([`doctor_toolkit::workspace::Workspace::rollback`]).
//!
//! The tasks live in submodules; each implements [`Task`]:
//!
//! - `retexture`: the textures of a model, character or level object;
//! - `model`: replace a weapon, prop or car model;
//! - `outfit`: a new piece of clothing for a character set;
//! - `params`: game parameters of `data/gf2/tunables.toml`;
//! - `texts`: string tables;
//! - `place`: a new object in a level;
//! - `voice`: a new voice line made from a recording;
//! - `zone`: a new zone (a stream, a SimGroup and a cell of the world grid);
//! - `package`: a mod package from the workspace.

mod model;
mod outfit;
mod package;
mod params;
mod place;
mod retexture;
#[cfg(test)]
mod tests;
mod texts;
mod voice;
mod zone;
#[cfg(test)]
mod zone_tests;

use std::collections::HashMap;

use doctor_toolkit_mods::i18n::{tr, trf};
use egui::{Color32, RichText};
use serde::{Deserialize, Serialize};

use super::dock::{DockOp, Pane};
use super::{BrowserApp, Command, Level};
use crate::nav::Location;

/// The tasks a wizard can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Retexture,
    ReplaceModel,
    Outfit,
    Parameters,
    Texts,
    PlaceObject,
    VoiceLine,
    NewZone,
    PackageMod,
}

impl Kind {
    pub const ALL: [Kind; 9] = [
        Kind::Retexture,
        Kind::ReplaceModel,
        Kind::Outfit,
        Kind::Parameters,
        Kind::Texts,
        Kind::PlaceObject,
        Kind::VoiceLine,
        Kind::NewZone,
        Kind::PackageMod,
    ];

    pub(crate) fn slug(self) -> &'static str {
        match self {
            Self::Retexture => "retexture",
            Self::ReplaceModel => "model",
            Self::Outfit => "outfit",
            Self::Parameters => "params",
            Self::Texts => "texts",
            Self::PlaceObject => "place",
            Self::VoiceLine => "voice",
            Self::NewZone => "zone",
            Self::PackageMod => "package",
        }
    }

    pub fn from_slug(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.slug() == text)
    }

    pub fn title(self) -> &'static str {
        tr(match self {
            Self::Retexture => "wizard.retexture.title",
            Self::ReplaceModel => "wizard.model.title",
            Self::Outfit => "wizard.outfit.title",
            Self::Parameters => "wizard.params.title",
            Self::Texts => "wizard.texts.title",
            Self::PlaceObject => "wizard.place.title",
            Self::VoiceLine => "wizard.voice.title",
            Self::NewZone => "wizard.zone.title",
            Self::PackageMod => "wizard.package.title",
        })
    }

    pub fn blurb(self) -> &'static str {
        tr(match self {
            Self::Retexture => "wizard.retexture.blurb",
            Self::ReplaceModel => "wizard.model.blurb",
            Self::Outfit => "wizard.outfit.blurb",
            Self::Parameters => "wizard.params.blurb",
            Self::Texts => "wizard.texts.blurb",
            Self::PlaceObject => "wizard.place.blurb",
            Self::VoiceLine => "wizard.voice.blurb",
            Self::NewZone => "wizard.zone.blurb",
            Self::PackageMod => "wizard.package.blurb",
        })
    }

    /// The command of the Wizards menu that starts it.
    pub fn command(self) -> Command {
        match self {
            Self::Retexture => Command::WizardRetexture,
            Self::ReplaceModel => Command::WizardModel,
            Self::Outfit => Command::WizardOutfit,
            Self::Parameters => Command::WizardParams,
            Self::Texts => Command::WizardTexts,
            Self::PlaceObject => Command::WizardPlace,
            Self::VoiceLine => Command::WizardVoice,
            Self::NewZone => Command::WizardZone,
            Self::PackageMod => Command::WizardPackage,
        }
    }
}

/// Where a wizard was started from.
#[derive(Debug, Clone, Default)]
pub struct Start {
    /// A resource: the model to retexture or replace, the table to edit…
    pub loc: Option<Location>,
    /// A metamodel shown by an object of the level view.
    pub model: Option<u32>,
}

/// What a task tells the frame about one of its steps.
pub(super) trait Task {
    /// Text keys of the task's own steps; the summary and done steps follow.
    fn steps(&self) -> &'static [&'static str];
    /// Draws step `step`.
    fn ui(&mut self, step: usize, ui: &mut egui::Ui, app: &mut BrowserApp, tx: &mut Tx);
    /// Whether step `step` is complete; the error says what is missing.
    fn check(&self, step: usize, app: &BrowserApp) -> Result<(), String>;
    /// Called when Next leaves step `step`; an error keeps the step.
    fn leave(&mut self, _step: usize, _app: &mut BrowserApp, _tx: &mut Tx) -> Result<(), String> {
        Ok(())
    }
    /// What will be recorded, one line each.
    fn summary(&self, app: &BrowserApp) -> Vec<String>;
    /// Records the task; returns the name of the operation.
    fn record(&mut self, app: &mut BrowserApp, tx: &mut Tx) -> Result<String, String>;
    /// The task records in the workspace (the package does not).
    fn records_edits(&self) -> bool {
        true
    }
    /// The text of the button that records.
    fn record_label(&self) -> &'static str {
        tr("wizard.record")
    }
}

/// The open recording of a task: the workspace history length before it.
#[derive(Debug, Default)]
pub(super) struct Tx {
    pub mark: Option<usize>,
}

impl Tx {
    /// Starts recording now, unless it has started already.
    pub fn begin(&mut self, app: &mut BrowserApp) -> Result<(), String> {
        if self.mark.is_none() {
            self.mark = Some(app.workspace_mut()?.head());
        }
        Ok(())
    }
}

/// State of one task.
pub(super) enum State {
    Retexture(Box<retexture::Retexture>),
    Model(Box<model::ReplaceModel>),
    Outfit(Box<outfit::Outfit>),
    Params(Box<params::Params>),
    Texts(Box<texts::Texts>),
    Place(Box<place::Place>),
    Voice(Box<voice::Voice>),
    Zone(Box<zone::NewZone>),
    Package(Box<package::Package>),
}

impl State {
    fn task(&mut self) -> &mut dyn Task {
        match self {
            Self::Retexture(t) => t.as_mut(),
            Self::Model(t) => t.as_mut(),
            Self::Outfit(t) => t.as_mut(),
            Self::Params(t) => t.as_mut(),
            Self::Texts(t) => t.as_mut(),
            Self::Place(t) => t.as_mut(),
            Self::Voice(t) => t.as_mut(),
            Self::Zone(t) => t.as_mut(),
            Self::Package(t) => t.as_mut(),
        }
    }

    fn task_ref(&self) -> &dyn Task {
        match self {
            Self::Retexture(t) => t.as_ref(),
            Self::Model(t) => t.as_ref(),
            Self::Outfit(t) => t.as_ref(),
            Self::Params(t) => t.as_ref(),
            Self::Texts(t) => t.as_ref(),
            Self::Place(t) => t.as_ref(),
            Self::Voice(t) => t.as_ref(),
            Self::Zone(t) => t.as_ref(),
            Self::Package(t) => t.as_ref(),
        }
    }
}

/// One wizard tab.
pub(crate) struct Wizard {
    pub kind: Kind,
    pub step: usize,
    pub tx: Tx,
    /// Why the last Next or recording failed.
    pub error: Option<String>,
    /// The name of the recorded operation, once done.
    pub done: Option<String>,
    pub state: State,
}

impl Wizard {
    /// Text keys of every step, the summary and done included.
    fn step_keys(&self) -> Vec<&'static str> {
        let mut keys = self.state.task_ref().steps().to_vec();
        keys.push("wizard.step.summary");
        keys.push("wizard.step.done");
        keys
    }

    fn summary_step(&self) -> usize {
        self.state.task_ref().steps().len()
    }

    fn is_done(&self) -> bool {
        self.done.is_some()
    }
}

/// The wizards of the open tabs.
#[derive(Default)]
pub(crate) struct Wizards {
    pub tabs: HashMap<u64, Wizard>,
    /// `DOCTOR_TOOLKIT_GUI_WIZARD` was used.
    preset_used: bool,
}

impl Wizards {
    pub fn retain(&mut self, keep: impl Fn(u64) -> bool) {
        self.tabs.retain(|id, _| keep(*id));
    }

    /// Ids of the level views the wizards show.
    pub fn levels(&self) -> Vec<u64> {
        self.tabs
            .values()
            .filter_map(|w| place::level_of(&w.state))
            .collect()
    }

    /// The level view of the wizard tab `tab`, if it has one.
    pub fn level_of_tab(&self, tab: u64) -> Option<u64> {
        place::level_of(&self.tabs.get(&tab)?.state)
    }
}

impl BrowserApp {
    /// Opens a wizard tab for `kind`, started from `start`.
    pub(super) fn start_wizard(&mut self, kind: Kind, start: Start) {
        if self.game.is_none() {
            self.notify(Level::Warning, tr("wizard.need_game").to_owned());
            return;
        }
        let id = self.new_pane_id();
        let wizard = self.new_wizard(kind, &start);
        self.wizards.tabs.insert(id, wizard);
        self.dock_ops.push(DockOp::Open(Pane::Wizard { id, kind }));
        self.dock_ops.push(DockOp::Focus(id));
    }

    fn new_wizard(&mut self, kind: Kind, start: &Start) -> Wizard {
        let state = match kind {
            Kind::Retexture => State::Retexture(Box::new(retexture::Retexture::new(self, start))),
            Kind::ReplaceModel => State::Model(Box::new(model::ReplaceModel::new(self, start))),
            Kind::Outfit => State::Outfit(Box::new(outfit::Outfit::new(self, start))),
            Kind::Parameters => State::Params(Box::new(params::Params::new(self, start))),
            Kind::Texts => State::Texts(Box::new(texts::Texts::new(self, start))),
            Kind::PlaceObject => State::Place(Box::new(place::Place::new(self, start))),
            Kind::VoiceLine => State::Voice(Box::new(voice::Voice::new(self, start))),
            Kind::NewZone => State::Zone(Box::new(zone::NewZone::new(self, start))),
            Kind::PackageMod => State::Package(Box::new(package::Package::new(self, start))),
        };
        Wizard {
            kind,
            step: 0,
            tx: Tx::default(),
            error: None,
            done: None,
            state,
        }
    }

    /// The "What do you want to do?" tab.
    pub(super) fn open_task_list(&mut self) {
        self.dock_ops.push(DockOp::ShowPanel(Pane::Tasks));
    }

    /// Next: checks the step, lets the task finish it and moves on.
    pub(super) fn wizard_next(&mut self, w: &mut Wizard) {
        w.error = None;
        let step = w.step;
        if step >= w.summary_step() {
            return;
        }
        let result = w
            .state
            .task_ref()
            .check(step, self)
            .and_then(|()| w.state.task().leave(step, self, &mut w.tx));
        match result {
            Ok(()) => w.step += 1,
            Err(error) => w.error = Some(error),
        }
    }

    pub(super) fn wizard_back(&mut self, w: &mut Wizard) {
        w.error = None;
        if !w.is_done() {
            w.step = w.step.saturating_sub(1);
        }
    }

    /// Records the task as one operation and moves to the done step.
    pub(super) fn wizard_record(&mut self, w: &mut Wizard) {
        w.error = None;
        if w.is_done() {
            return;
        }
        let edits = w.state.task_ref().records_edits();
        let opened_before = w.tx.mark.is_some();
        if edits && let Err(error) = w.tx.begin(self) {
            w.error = Some(error);
            return;
        }
        let result = w.state.task().record(self, &mut w.tx);
        let mark = w.tx.mark;
        let result = result.and_then(|label| {
            let Some(mark) = mark.filter(|_| edits) else {
                return Ok(label);
            };
            let name = trf("wizard.operation", &[("task", &label)]);
            let ws = self.workspace_mut()?;
            // Undone below the mark meanwhile: join what is there.
            let since = mark.min(ws.head());
            ws.squash(since, &name).map_err(|e| e.to_string())?;
            Ok(name)
        });
        match result {
            Ok(label) => {
                w.tx.mark = None;
                self.notify(Level::Info, trf("wizard.recorded", &[("label", &label)]));
                w.done = Some(label);
                w.step = w.summary_step() + 1;
                if edits {
                    self.after_edit(None);
                }
            }
            Err(error) => {
                // A recording that failed half-way leaves nothing behind.
                if edits
                    && !opened_before
                    && let Some(mark) = w.tx.mark.take()
                    && let Some(ws) = self.workspace.as_mut()
                {
                    let _ = ws.rollback(mark.min(ws.head()));
                    self.after_edit(None);
                }
                w.error = Some(trf("wizard.failed", &[("error", &error)]));
            }
        }
    }

    /// Cancel: takes back what the task recorded so far and closes the tab.
    pub(super) fn wizard_cancel(&mut self, id: u64, w: &mut Wizard) {
        if let Some(mark) = w.tx.mark.take()
            && let Some(ws) = self.workspace.as_mut()
        {
            let result = ws.rollback(mark.min(ws.head()));
            if let Err(error) = result {
                self.notify(Level::Error, trf("wizard.failed", &[("error", &error)]));
            }
            self.after_edit(None);
            self.notify(Level::Info, tr("wizard.cancelled").to_owned());
        }
        self.close_pane(id);
    }

    fn close_pane(&mut self, id: u64) {
        let tile = self
            .dock
            .as_ref()
            .and_then(|tree| super::dock::find(tree, |p| p.id() == Some(id)));
        if let Some(tile) = tile {
            self.dock_ops.push(DockOp::Close(tile));
        }
    }

    /// A wizard tab: made when first shown (also for a tab of a saved layout).
    pub(super) fn wizard_pane(&mut self, ui: &mut egui::Ui, id: u64, kind: Kind) {
        if self.game.is_none() {
            ui.add_space(12.0);
            ui.label(tr("wizard.need_game"));
            if ui.button(Command::OpenGame.label()).clicked() {
                self.pending.push(Command::OpenGame);
            }
            return;
        }
        if !self.wizards.tabs.contains_key(&id) {
            let wizard = self.new_wizard(kind, &Start::default());
            self.wizards.tabs.insert(id, wizard);
        }
        let Some(mut w) = self.wizards.tabs.remove(&id) else {
            return;
        };
        let cancel = self.wizard_frame(ui, id, &mut w);
        if cancel {
            self.wizard_cancel(id, &mut w);
        }
        self.wizards.tabs.insert(id, w);
    }

    /// Title, steps, the body of the current step and the buttons. Returns
    /// whether Cancel was pressed.
    fn wizard_frame(&mut self, ui: &mut egui::Ui, id: u64, w: &mut Wizard) -> bool {
        let keys = w.step_keys();
        let summary = w.summary_step();
        let mut cancel = false;
        egui::Panel::top(egui::IdSalt::new(("wizard-head", id))).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading(w.kind.title());
                if w.tx.mark.is_some() {
                    ui.label(RichText::new(tr("wizard.recording_open")).weak().small());
                }
            });
            ui.horizontal_wrapped(|ui| {
                for (i, key) in keys.iter().enumerate() {
                    if i > 0 {
                        ui.label(RichText::new("›").weak());
                    }
                    let text = format!("{}. {}", i + 1, tr(key));
                    let text = match i.cmp(&w.step) {
                        std::cmp::Ordering::Equal => RichText::new(text).strong(),
                        std::cmp::Ordering::Less => RichText::new(text),
                        std::cmp::Ordering::Greater => RichText::new(text).weak(),
                    };
                    ui.label(text);
                }
            });
        });
        egui::Panel::bottom(egui::IdSalt::new(("wizard-foot", id))).show(ui, |ui| {
            ui.add_space(4.0);
            if let Some(error) = &w.error {
                ui.colored_label(Color32::from_rgb(230, 90, 80), error);
            }
            ui.horizontal(|ui| {
                if w.is_done() {
                    if ui.button(tr("wizard.close")).clicked() {
                        self.close_pane(id);
                    }
                    return;
                }
                if ui
                    .button(tr("wizard.cancel"))
                    .on_hover_text(tr("wizard.cancel_hint"))
                    .clicked()
                {
                    cancel = true;
                }
                if ui
                    .add_enabled(w.step > 0, egui::Button::new(tr("wizard.back")))
                    .clicked()
                {
                    self.wizard_back(w);
                }
                if w.step == summary {
                    let label = w.state.task_ref().record_label();
                    if ui
                        .add(egui::Button::new(RichText::new(label).strong()))
                        .on_hover_text(tr("wizard.record_hint"))
                        .clicked()
                    {
                        self.wizard_record(w);
                    }
                } else {
                    let check = w.state.task_ref().check(w.step, self);
                    let next = ui
                        .add_enabled(check.is_ok(), egui::Button::new(tr("wizard.next")))
                        .clicked();
                    if let Err(missing) = check {
                        ui.label(RichText::new(missing).weak());
                    }
                    if next {
                        self.wizard_next(w);
                    }
                }
            });
            ui.add_space(2.0);
        });
        egui::CentralPanel::default().show(ui, |ui| {
            if w.step == summary {
                self.wizard_summary(ui, w);
            } else if w.step > summary {
                self.wizard_done(ui, w);
            } else {
                let step = w.step;
                w.state.task().ui(step, ui, self, &mut w.tx);
            }
        });
        cancel
    }

    fn wizard_summary(&mut self, ui: &mut egui::Ui, w: &mut Wizard) {
        ui.label(RichText::new(tr("wizard.summary_title")).strong());
        let lines = w.state.task_ref().summary(self);
        egui::ScrollArea::vertical()
            .id_salt("wizard-summary")
            .max_height(ui.available_height() - 60.0)
            .show(ui, |ui| {
                for line in &lines {
                    ui.label(format!("• {line}"));
                }
            });
        ui.add_space(8.0);
        let hint = if w.state.task_ref().records_edits() {
            tr("wizard.summary_hint")
        } else {
            tr("wizard.summary_hint_package")
        };
        ui.label(RichText::new(hint).weak());
    }

    fn wizard_done(&mut self, ui: &mut egui::Ui, w: &mut Wizard) {
        let label = w.done.clone().unwrap_or_default();
        ui.add_space(8.0);
        ui.label(RichText::new(tr("wizard.done_title")).heading());
        ui.label(&label);
        ui.add_space(8.0);
        if w.state.task_ref().records_edits() {
            ui.label(RichText::new(tr("wizard.done_hint")).weak());
        }
        ui.add_space(8.0);
        if let State::Package(p) = &mut w.state {
            p.done_ui(ui, self);
            ui.add_space(8.0);
        }
        if let State::Zone(z) = &mut w.state {
            z.done_ui(ui, self);
            ui.add_space(8.0);
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    self.enabled(Command::Play),
                    egui::Button::new(tr("wizard.test_in_game")),
                )
                .on_hover_text(tr("wizard.test_in_game_hint"))
                .clicked()
            {
                self.pending.push(Command::Play);
            }
            if w.kind != Kind::PackageMod && ui.button(tr("wizard.build_mod")).clicked() {
                self.start_wizard(Kind::PackageMod, Start::default());
            }
            if ui.button(tr("wizard.another")).clicked() {
                self.open_task_list();
            }
        });
    }

    /// The "What do you want to do?" page: every task with a line about it.
    pub(super) fn tasks_ui(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .id_salt("tasks")
            .auto_shrink(false)
            .show(ui, |ui| {
                egui::Frame::NONE.inner_margin(16).show(ui, |ui| {
                    ui.heading(tr("wizard.start_title"));
                    ui.label(RichText::new(tr("wizard.start_hint")).weak());
                    ui.add_space(12.0);
                    self.task_buttons(ui, true);
                });
            });
    }

    /// One button per task (with its description when `blurbs`).
    pub(super) fn task_buttons(&mut self, ui: &mut egui::Ui, blurbs: bool) {
        let enabled = self.game.is_some();
        if !enabled {
            ui.label(RichText::new(tr("wizard.need_game")).weak());
        }
        for kind in Kind::ALL {
            ui.horizontal(|ui| {
                let button = egui::Button::new(kind.title()).min_size(egui::vec2(220.0, 0.0));
                if ui
                    .add_enabled(enabled, button)
                    .on_hover_text(kind.blurb())
                    .clicked()
                {
                    self.start_wizard(kind, Start::default());
                }
                if blurbs {
                    ui.add(egui::Label::new(RichText::new(kind.blurb()).weak()).wrap());
                }
            });
            ui.add_space(4.0);
        }
    }

    /// `DOCTOR_TOOLKIT_GUI_WIZARD=<task>[,step=N][,select]`: opens a wizard
    /// once the game is open (screenshots); `select` starts it from the
    /// resource selected with `DOCTOR_TOOLKIT_GUI_SELECT`.
    pub(super) fn wizard_preset(&mut self) {
        if self.wizards.preset_used || self.game.is_none() || self.index.is_none() {
            return;
        }
        let Ok(spec) = std::env::var("DOCTOR_TOOLKIT_GUI_WIZARD") else {
            self.wizards.preset_used = true;
            return;
        };
        self.run_wizard_preset(&spec);
    }

    /// Runs the preset `spec` (see [`Self::wizard_preset`]); waits (does
    /// nothing yet) while `select` has no selection to start from.
    pub(super) fn run_wizard_preset(&mut self, spec: &str) {
        let items = split_preset(spec);
        let mut items = items.iter().map(String::as_str);
        let name = items.next().unwrap_or_default();
        if name == "tasks" {
            self.wizards.preset_used = true;
            self.open_task_list();
            return;
        }
        let Some(kind) = Kind::from_slug(name) else {
            self.wizards.preset_used = true;
            self.preset_failed(&format!("DOCTOR_TOOLKIT_GUI_WIZARD: unknown task {name}"));
            return;
        };
        let mut start = Start::default();
        let mut steps = 0;
        let mut options = Vec::new();
        for item in items {
            let (key, value) = preset_item(item);
            let (key, value) = (key.as_str(), value.as_str());
            match key {
                "select" => {
                    // Waits for the selection of the screenshot mode.
                    let Some(loc) = self.details.as_ref().map(|d| d.loc) else {
                        return;
                    };
                    start.loc = Some(loc);
                }
                "step" => steps = value.parse().unwrap_or(0),
                _ => options.push((key.to_owned(), value.to_owned())),
            }
        }
        self.wizards.preset_used = true;
        let id = self.new_pane_id();
        let mut wizard = self.new_wizard(kind, &start);
        for (key, value) in &options {
            match &mut wizard.state {
                State::Retexture(t) => t.preset(key, value, self),
                State::Model(t) => t.preset(key, value, self),
                State::Outfit(t) => t.preset(key, value, self),
                State::Params(t) => t.preset(key, value, self),
                State::Texts(t) => t.preset(key, value, self),
                State::Place(t) => t.preset(key, value, self),
                State::Voice(t) => t.preset(key, value, self),
                State::Zone(t) => t.preset(key, value, self),
                State::Package(t) => t.preset(key, value, self),
            }
        }
        for _ in 0..steps {
            let last = wizard.step == wizard.summary_step();
            if last {
                self.wizard_record(&mut wizard);
            } else {
                self.wizard_next(&mut wizard);
            }
            if let Some(error) = &wizard.error {
                // The first step that fails stops the run: nothing after it
                // is made, and the screenshot mode exits with an error.
                let step = wizard.step + 1;
                self.preset_failed(&format!(
                    "DOCTOR_TOOLKIT_GUI_WIZARD: {name}, step {step}: {error}"
                ));
                break;
            }
            if last {
                break;
            }
        }
        // The New zone wizard driven hands-free must not leave a script
        // going on without its zone.
        if kind == Kind::NewZone
            && let Some(error) = &wizard.error
        {
            eprintln!("DOCTOR_TOOLKIT_GUI_WIZARD: {error}");
            std::process::exit(2);
        }
        self.wizards.tabs.insert(id, wizard);
        self.dock_ops.push(DockOp::Open(Pane::Wizard { id, kind }));
        self.dock_ops.push(DockOp::Maximize(Some(id)));
    }
}

/// The wizard that packages a mod, for the tests of other modules.
#[cfg(test)]
pub(crate) fn package_for_tests(app: &mut BrowserApp) -> package::Package {
    package::Package::new(app, &Start::default())
}

/// Splits a preset `item,item,…` at commas. A comma of a value is written
/// `\,`; a value in double quotes is a JSON string (`key="a, b"`), kept
/// whole for [`preset_item`]. Nothing else is special, so the presets
/// written before (Windows paths with `\` among them) read as they did.
pub(crate) fn split_preset(spec: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut item = String::new();
    let mut chars = spec.chars().peekable();
    // Inside the quotes of a JSON value.
    let mut quoted = false;
    while let Some(c) = chars.next() {
        match c {
            '\\' if quoted => {
                item.push(c);
                item.extend(chars.next());
            }
            '\\' if chars.peek() == Some(&',') => {
                chars.next();
                item.push(',');
            }
            '"' if quoted => {
                quoted = false;
                item.push(c);
            }
            '"' if item.trim_start().ends_with('=') => {
                quoted = true;
                item.push(c);
            }
            ',' if !quoted => items.push(std::mem::take(&mut item).trim().to_owned()),
            _ => item.push(c),
        }
    }
    items.push(item.trim().to_owned());
    items
}

/// `key=value` of a preset item; a value in double quotes is read as a
/// JSON string (see [`split_preset`]).
pub(crate) fn preset_item(item: &str) -> (String, String) {
    let (key, value) = item.split_once('=').unwrap_or((item, ""));
    let value = match serde_json::from_str::<String>(value) {
        Ok(text) if value.starts_with('"') => text,
        _ => value.to_owned(),
    };
    (key.to_owned(), value)
}

/// A folder for the working files of a wizard, below the system's
/// temporary folder (one per process).
pub(super) fn work_dir(name: &str) -> std::path::PathBuf {
    std::env::temp_dir()
        .join("doctor-toolkit-wizard")
        .join(std::process::id().to_string())
        .join(name)
}

/// A working file edited by another program: notices its saves.
#[derive(Debug, Clone)]
pub(super) struct Watched {
    pub path: std::path::PathBuf,
    stamp: Option<(std::time::SystemTime, u64)>,
    checked: Option<std::time::Instant>,
}

impl Watched {
    /// Writes `data` to `path` and remembers how the file looks.
    pub fn write(path: std::path::PathBuf, data: &[u8]) -> Result<Self, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        std::fs::write(&path, data).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut watched = Self {
            path,
            stamp: None,
            checked: None,
        };
        watched.stamp = watched.current();
        Ok(watched)
    }

    fn current(&self) -> Option<(std::time::SystemTime, u64)> {
        let meta = std::fs::metadata(&self.path).ok()?;
        Some((meta.modified().ok()?, meta.len()))
    }

    /// The new contents when the file was saved since the last call
    /// (checked at most twice a second).
    pub fn poll(&mut self) -> Option<Vec<u8>> {
        let now = std::time::Instant::now();
        if self
            .checked
            .is_some_and(|t| now.duration_since(t) < std::time::Duration::from_millis(500))
        {
            return None;
        }
        self.checked = Some(now);
        let stamp = self.current();
        if stamp.is_none() || stamp == self.stamp {
            return None;
        }
        self.stamp = stamp;
        std::fs::read(&self.path).ok()
    }
}

impl BrowserApp {
    /// Opens a working file with the program of the settings for `role`.
    pub(super) fn wizard_open_editor(
        &mut self,
        role: doctor_toolkit::external::EditorRole,
        path: &std::path::Path,
    ) {
        use doctor_toolkit::external::EditorRole;
        let editors = &self.settings.external_editors;
        let command = match role {
            EditorRole::Image => editors.image.clone(),
            EditorRole::Model => editors.model.clone(),
            EditorRole::Text => editors.text.clone(),
        };
        match doctor_toolkit::external::launch(&command, path) {
            Ok(_) => self.notify(
                Level::Info,
                trf("wizard.opened_editor", &[("file", &path.display())]),
            ),
            Err(error) => self.notify(
                Level::Error,
                trf("wizard.cannot_open_editor", &[("error", &error)]),
            ),
        }
    }
}

/// Finds resources of some types by name, hash or stream.
#[derive(Debug, Default)]
pub(super) struct Picker {
    pub text: String,
    results: Vec<(Location, String)>,
    /// The query the results are for.
    searched: Option<String>,
}

/// Most results a picker lists.
const PICKER_LIMIT: usize = 200;

impl BrowserApp {
    /// A name for a resource: its name from the game index, stream and type.
    pub(super) fn location_label(&self, loc: Location) -> String {
        use doctor_toolkit::resource::names::type_tags;
        if let Some(index) = &self.nav.index
            && let Some(id) = index.locate(loc.stream, loc.resource)
        {
            let record = index.record(id);
            return format!(
                "{}  {}  {}",
                index.label(id),
                type_tags().display(record.header.type_id),
                index.stream_name(loc.stream)
            );
        }
        let Some(game) = &self.game else {
            return String::new();
        };
        let type_id = self.index.as_ref().and_then(|index| {
            index
                .entries
                .iter()
                .find(|e| e.stream as usize == loc.stream && e.resource as usize == loc.resource)
                .map(|e| e.header.type_id)
        });
        format!(
            "#{}  {}  {}",
            loc.resource,
            type_id.map(|t| type_tags().display(t)).unwrap_or_default(),
            game.streams()
                .get(loc.stream)
                .map(|s| s.stream_name())
                .unwrap_or_default()
        )
    }

    /// Resources of `types` matching `text`: through the game index when it
    /// is built, else by stream path.
    fn picker_search(&self, types: &[u32], text: &str) -> Vec<(Location, String)> {
        use doctor_toolkit::resource::names::type_tags;
        let text = text.trim();
        if self.nav.index.is_some() {
            let mut out = Vec::new();
            for &t in types {
                let query = format!("type:{} {text}", type_tags().display(t));
                out.extend(self.nav.quick_search(&query, PICKER_LIMIT));
            }
            out.truncate(PICKER_LIMIT);
            return out;
        }
        let (Some(game), Some(index)) = (&self.game, &self.index) else {
            return Vec::new();
        };
        let needle = text.to_ascii_lowercase();
        index
            .entries
            .iter()
            .filter(|e| types.contains(&e.header.type_id))
            .filter(|e| {
                needle.is_empty()
                    || game.streams()[e.stream as usize]
                        .rel_path
                        .to_ascii_lowercase()
                        .contains(&needle)
                    || format!("{:08x}", e.header.ids[3]).contains(&needle)
            })
            .take(PICKER_LIMIT)
            .map(|e| {
                let loc = Location {
                    stream: e.stream as usize,
                    resource: e.resource as usize,
                };
                (loc, self.location_label(loc))
            })
            .collect()
    }

    /// A search field and the matching resources of `types`; a click picks
    /// one into `chosen`. Returns whether the choice changed.
    pub(super) fn picker_ui(
        &mut self,
        ui: &mut egui::Ui,
        picker: &mut Picker,
        types: &[u32],
        chosen: &mut Option<Location>,
    ) -> bool {
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label(tr("wizard.search"));
            ui.add(
                egui::TextEdit::singleline(&mut picker.text)
                    .hint_text(tr("wizard.search_hint"))
                    .desired_width(320.0),
            );
            if self.nav.index.is_none() {
                ui.label(RichText::new(tr("wizard.index_pending")).weak().small());
            }
        });
        let key = format!("{}|{}", picker.text, self.nav.index.is_some());
        if picker.searched.as_deref() != Some(key.as_str()) {
            picker.results = self.picker_search(types, &picker.text);
            picker.searched = Some(key);
        }
        if let Some(loc) = *chosen {
            ui.label(
                RichText::new(trf("wizard.chosen", &[("what", &self.location_label(loc))]))
                    .strong(),
            );
        }
        egui::ScrollArea::vertical()
            .id_salt(("picker", types.first().copied()))
            .max_height(220.0)
            .show(ui, |ui| {
                if picker.results.is_empty() {
                    ui.label(RichText::new(tr("wizard.nothing_found")).weak());
                }
                for (loc, label) in &picker.results {
                    if ui.selectable_label(*chosen == Some(*loc), label).clicked()
                        && *chosen != Some(*loc)
                    {
                        *chosen = Some(*loc);
                        changed = true;
                    }
                }
            });
        changed
    }

    /// The data of a resource as the editor shows it (the workspace version).
    pub(super) fn current_data(&self, loc: Location) -> Result<Vec<u8>, String> {
        let game = self.game.as_ref().ok_or(tr("edits.no_game"))?;
        match &self.workspace {
            Some(ws) => ws.current_data(game, loc.stream, loc.resource),
            None => game
                .open_stream(loc.stream)
                .and_then(|s| s.resource_data(loc.resource)),
        }
        .map_err(|e| e.to_string())
    }

    /// The type of a resource.
    pub(super) fn type_of(&self, loc: Location) -> Option<u32> {
        let game = self.game.as_ref()?;
        let file = self.files.peek(loc.stream).cloned();
        let header = match file {
            Some(file) => file.layout().resources.get(loc.resource)?.header,
            None => {
                game.open_stream(loc.stream)
                    .ok()?
                    .layout()
                    .resources
                    .get(loc.resource)?
                    .header
            }
        };
        Some(header.type_id)
    }
}

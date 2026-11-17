//! New zone: a new stream, an empty SimGroup in it and its cell of the
//! world grid, so that the game loads the stream when the player comes
//! near (stage 103). The three are the forms of Tools → New stream…, New
//! SimGroup… and World grid cell… in one go and one undoable operation; the
//! done step opens the zone in the level view, where objects are added to
//! it (Details → Add object → Add to).

use doctor_toolkit::zone_ops::{self, NewGroupParams, StreamLoad};
use doctor_toolkit_mods::i18n::{tr, trf};
use egui::RichText;

use super::{Start, Task, Tx};
use crate::app::BrowserApp;
use crate::app::zone::{CellForm, StreamForm, ZoneAction};

pub(crate) struct NewZone {
    pub stream: StreamForm,
    pub cell: CellForm,
    /// Put an empty SimGroup into the stream.
    pub group: bool,
    /// Add the stream to the world grid of a level.
    pub with_cell: bool,
    /// The level field follows the parent until it is edited.
    level_edited: bool,
    /// The name of the recorded stream.
    made: Option<String>,
}

impl NewZone {
    pub fn new(_app: &mut BrowserApp, _start: &Start) -> Self {
        Self {
            stream: StreamForm::default(),
            cell: CellForm::default(),
            group: true,
            with_cell: true,
            level_edited: false,
            made: None,
        }
    }

    pub fn preset(&mut self, key: &str, value: &str, _app: &mut BrowserApp) {
        match key {
            "name" => self.stream.name = value.to_owned(),
            "parent" => self.stream.parent = value.to_owned(),
            "load" => {
                if let Some(load) = StreamLoad::from_slug(value) {
                    self.stream.load = load;
                }
            }
            "level" => {
                self.cell.level = value.to_owned();
                self.level_edited = true;
            }
            "min" => self.cell.min = value.replace('/', ", "),
            "max" => self.cell.max = value.replace('/', ", "),
            "lod" => self.cell.lod = value.to_owned(),
            "nogroup" => self.group = false,
            "nocell" => self.with_cell = false,
            _ => {}
        }
        self.follow_parent();
    }

    /// The level of the cell is the root of the parent (`gf2/cuba` for
    /// `gf2/cuba/local`) until the user names another.
    fn follow_parent(&mut self) {
        if self.level_edited {
            return;
        }
        let parent = self.stream.parent.trim().replace('\\', "/");
        let mut parts = parent.splitn(3, '/');
        self.cell.level = match (parts.next(), parts.next()) {
            (Some(a), Some(b)) => format!("{a}/{b}"),
            (Some(a), None) => a.to_owned(),
            _ => String::new(),
        };
    }

    fn name(&self) -> String {
        doctor_toolkit::workspace::key::normalize_stream(self.stream.name.trim())
    }

    /// The zone of the done step.
    #[cfg(test)]
    pub fn made(&self) -> Option<&str> {
        self.made.as_deref()
    }
}

impl Task for NewZone {
    fn steps(&self) -> &'static [&'static str] {
        &["wizard.zone.step.stream", "wizard.zone.step.cell"]
    }

    fn ui(&mut self, step: usize, ui: &mut egui::Ui, _app: &mut BrowserApp, _tx: &mut Tx) {
        match step {
            0 => {
                ui.label(tr("wizard.zone.stream_intro"));
                ui.add_space(6.0);
                self.stream.ui(ui, "wizard");
                self.follow_parent();
                ui.add_space(6.0);
                ui.checkbox(&mut self.group, tr("wizard.zone.with_group"))
                    .on_hover_text(tr("wizard.zone.with_group_hint"));
            }
            _ => {
                ui.label(tr("wizard.zone.cell_intro"));
                ui.add_space(6.0);
                ui.checkbox(&mut self.with_cell, tr("wizard.zone.with_cell"));
                ui.add_enabled_ui(self.with_cell, |ui| {
                    let before = self.cell.level.clone();
                    self.cell.ui(ui, "wizard", None);
                    self.level_edited |= before != self.cell.level;
                });
                if !self.with_cell {
                    ui.label(RichText::new(tr("wizard.zone.no_cell_note")).weak());
                }
            }
        }
    }

    fn check(&self, step: usize, app: &BrowserApp) -> Result<(), String> {
        let game = app.game.as_ref().ok_or(tr("edits.no_game"))?;
        match step {
            0 => {
                let name = self.name();
                if name.is_empty() {
                    return Err(tr("wizard.zone.need_name").to_owned());
                }
                let taken = game.find_stream(&name).is_some()
                    || app
                        .workspace
                        .as_ref()
                        .is_some_and(|ws| ws.new_stream(&name).is_some());
                if taken {
                    return Err(trf("wizard.zone.name_taken", &[("name", &name)]));
                }
                let parent = self.stream.parent.trim();
                if parent.is_empty() || game.find_stream(parent).is_none() {
                    return Err(tr("wizard.zone.need_parent").to_owned());
                }
                Ok(())
            }
            _ if !self.with_cell => Ok(()),
            _ => {
                let level = self.cell.level.trim();
                if level.is_empty() || game.find_stream(level).is_none() {
                    return Err(tr("wizard.zone.need_level").to_owned());
                }
                let p = self.cell.params()?;
                if (0..3).any(|k| p.min[k] >= p.max[k]) {
                    return Err(tr("wizard.zone.need_box").to_owned());
                }
                if p.on_enable && p.name.is_none() {
                    return Err(tr("wizard.zone.need_cell_name").to_owned());
                }
                Ok(())
            }
        }
    }

    fn summary(&self, _app: &BrowserApp) -> Vec<String> {
        let name = self.name();
        let mut lines = vec![trf(
            "wizard.zone.summary_stream",
            &[
                ("name", &name),
                ("parent", &self.stream.parent.trim()),
                ("load", &StreamForm::load_label(self.stream.load)),
            ],
        )];
        if self.group {
            lines.push(tr("wizard.zone.summary_group").to_owned());
        }
        if self.with_cell {
            lines.push(trf(
                "wizard.zone.summary_cell",
                &[
                    ("level", &self.cell.level.trim()),
                    ("min", &self.cell.min.trim()),
                    ("max", &self.cell.max.trim()),
                ],
            ));
        } else {
            lines.push(tr("wizard.zone.no_cell_note").to_owned());
        }
        lines
    }

    fn record(&mut self, app: &mut BrowserApp, _tx: &mut Tx) -> Result<String, String> {
        let name = self.name();
        app.zone_record(ZoneAction::AddStream(self.stream.params()))?;
        if self.group {
            app.zone_record(ZoneAction::AddGroup(NewGroupParams {
                stream: name.clone(),
                ..Default::default()
            }))?;
        }
        if self.with_cell {
            let mut cell = self.cell.params()?;
            cell.stream = name.clone();
            // Checked here as well: the summary can be reached by a preset.
            zone_ops::parse_lod(cell.lod.as_deref()).map_err(|e| e.to_string())?;
            app.zone_record(ZoneAction::AddCell(cell))?;
        }
        self.made = Some(name.clone());
        Ok(trf("wizard.zone.label", &[("name", &name)]))
    }
}

impl NewZone {
    /// The done step: open the zone in the level view.
    pub fn done_ui(&mut self, ui: &mut egui::Ui, app: &mut BrowserApp) {
        let Some(name) = self.made.clone() else {
            return;
        };
        ui.label(tr("wizard.zone.done_hint"));
        if ui
            .add_enabled(self.group, egui::Button::new(tr("wizard.zone.open_level")))
            .on_hover_text(tr("wizard.zone.open_level_hint"))
            .clicked()
        {
            app.open_zones_in_level(vec![name]);
        }
    }
}

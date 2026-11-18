//! Presets and saved sets of the Zones panel (stage 113): what to load
//! without ticking dozens of boxes.
//!
//! - **Presets** (`doctor_toolkit::level::presets`) are computed from the
//!   world grid of the game: a level at near detail, one of its districts,
//!   with the missions and the checkpoints as two switches. A click chooses
//!   the zones; "Load" at the top of the panel loads them.
//! - **Saved sets** are choices the user named. The view shows them and asks
//!   the app to save or remove one ([`ZoneRequest`]): they are the open
//!   project's (`[[zones]]` of `project.toml`), else the game's in the
//!   settings of the editor.
//! - **The choice loaded last** goes to the app the same way, so that the
//!   level tab opens with it next time ([`LevelView::restore_on_open`]).

use std::collections::BTreeSet;
use std::sync::Arc;

use doctor_toolkit::level::presets::{self, Resolved};
use doctor_toolkit::level::{LevelPresets, Preset, ZoneSet};
use doctor_toolkit::project::EditorCamera;
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::RichText;

use super::LevelView;

/// The sets of zones the user saved, as the app keeps them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SavedSets {
    pub sets: Vec<ZoneSet>,
    /// Name of the project they are saved in; `None`: in the settings of
    /// the editor, for the open game.
    pub project: Option<String>,
}

/// What the Zones panel asks the app to do.
#[derive(Debug, Clone, PartialEq)]
pub enum ZoneRequest {
    /// Save the set; one of the same name is replaced.
    SaveSet(ZoneSet),
    RemoveSet(String),
    /// The user loaded these zones (entries of a saved choice,
    /// `presets::resolve_saved`): the level tab opens with them next time.
    Loaded(Vec<String>),
}

/// The Presets section of the Zones panel.
#[derive(Default)]
pub(super) struct PresetUi {
    /// The level whose presets are shown.
    level: String,
    /// The district chosen last; `None`: the whole level.
    district: Option<String>,
    missions: bool,
    checkpoints: bool,
    /// What a preset or a set chose last, with the entries that say the
    /// same: while the choice is still that, they are what is remembered
    /// as loaded (three words instead of hundreds of stream names).
    applied: Option<(BTreeSet<usize>, Vec<String>)>,
    /// The name typed for a new set.
    name: String,
    /// What was skipped when a set or the last choice was taken.
    pub(super) note: String,
}

/// `485 zones, 726 MB`; a small set with a decimal (`1 zone, 0.4 MB`).
fn cost(zones: usize, bytes: u64) -> String {
    let megabytes = bytes as f64 / 1e6;
    let megabytes = if megabytes < 10.0 {
        format!("{megabytes:.1}")
    } else {
        format!("{megabytes:.0}")
    };
    trf(
        "zones.preset_cost",
        &[
            ("zones", &trn("count.zones", zones as u64, &[])),
            ("mb", &megabytes),
        ],
    )
}

impl LevelView {
    /// The sets the user saved, from the app.
    pub fn set_saved_sets(&mut self, saved: Arc<SavedSets>) {
        self.saved = saved;
    }

    pub fn saved_sets(&self) -> &Arc<SavedSets> {
        &self.saved
    }

    /// What the Zones panel asked for since the last call.
    pub fn take_zone_requests(&mut self) -> Vec<ZoneRequest> {
        std::mem::take(&mut self.zone_requests)
    }

    /// The saved choice to load once the game is opened — what was loaded
    /// last — unless the view was told what to load. The camera stands
    /// where it stood over those zones. Returns whether the view takes it.
    pub fn restore_on_open(&mut self, entries: Vec<String>, camera: Option<EditorCamera>) -> bool {
        if self.autoload.is_some() || entries.is_empty() {
            return false;
        }
        self.restore = Some((entries, camera));
        true
    }

    /// [`Self::restore_on_open`] for a view that may have the game already:
    /// taken (and loaded at once) only when the view shows and has chosen
    /// nothing.
    pub fn restore_if_empty(&mut self, entries: Vec<String>, camera: Option<EditorCamera>) -> bool {
        let empty = self.level.is_none() && self.job.is_none() && self.chosen.is_empty();
        if !empty || !self.restore_on_open(entries, camera) {
            return false;
        }
        if self.input.is_some() {
            self.restore_choice();
            self.ctx.request_repaint();
        }
        true
    }

    /// The camera, to stand there again next time; `None` while nothing is
    /// loaded.
    pub fn camera_state(&self) -> Option<EditorCamera> {
        self.level.as_ref()?;
        Some(EditorCamera {
            eye: self.camera.eye,
            yaw: self.camera.yaw,
            pitch: self.camera.pitch,
        })
    }

    /// What was skipped when a set or the last choice was taken.
    #[cfg(test)]
    pub fn zones_note(&self) -> &str {
        &self.preset_ui.note
    }

    /// Names of the chosen zones.
    pub fn chosen_names(&self) -> Vec<String> {
        self.catalog
            .zones
            .iter()
            .filter(|z| self.chosen.contains(&z.stream))
            .map(|z| z.name.clone())
            .collect()
    }

    /// Chooses what `entries` of a saved choice name; the entries that
    /// name nothing in the game are skipped, returned, and not remembered
    /// with the choice.
    fn choose_saved(&mut self, entries: &[String]) -> Resolved {
        let found = presets::resolve_saved(
            entries.iter().map(String::as_str),
            &self.catalog,
            &self.presets,
        );
        self.chosen.clone_from(&found.streams);
        let kept = entries
            .iter()
            .filter(|e| {
                !found
                    .gone
                    .iter()
                    .any(|gone| gone == e.trim_end_matches('/'))
            })
            .cloned()
            .collect();
        self.preset_ui.applied = Some((self.chosen.clone(), kept));
        found
    }

    /// Chooses a preset: the whole level `level` at near detail or one of
    /// its districts, with the missions (of the district, when one is given)
    /// and the checkpoints (of the whole level). `false`: no such preset.
    pub fn choose_preset(
        &mut self,
        level: &str,
        district: Option<&str>,
        missions: bool,
        checkpoints: bool,
    ) -> bool {
        let Some(found) = self.presets.level(level) else {
            return false;
        };
        if district.is_some_and(|d| found.district(d).is_none()) {
            return false;
        }
        let place = match district {
            Some(name) => format!("preset:{}/{name}", found.level),
            None => format!("preset:{}", found.level),
        };
        let mut entries = vec![place.clone()];
        if missions && !found.missions_of(district).is_empty() {
            entries.push(format!("{place}:missions"));
        }
        if checkpoints && district.is_none() && found.checkpoints.is_some() {
            entries.push(format!("{place}:checkpoints"));
        }
        self.preset_ui.level.clone_from(&found.level);
        self.preset_ui.district = district.map(str::to_owned);
        self.preset_ui.missions = missions;
        self.preset_ui.checkpoints = checkpoints;
        self.preset_ui.note.clear();
        self.choose_saved(&entries);
        true
    }

    /// Chooses the zones of a saved set. `false`: no set of that name.
    pub fn choose_set(&mut self, name: &str) -> bool {
        let Some(set) = self.saved.sets.iter().find(|s| s.name == name).cloned() else {
            return false;
        };
        let found = self.choose_saved(&set.streams);
        // Remembered by its streams: the set may be renamed or removed.
        self.preset_ui.applied = None;
        self.preset_ui.note = match found.gone.first() {
            Some(first) => trn(
                "zones.set_gone",
                found.gone.len() as u64,
                &[("name", &set.name), ("first", first)],
            ),
            None => String::new(),
        };
        true
    }

    /// Asks the app to save the chosen zones as the set `name`.
    pub fn save_set(&mut self, name: &str) {
        let name = name.trim();
        if name.is_empty() || self.chosen.is_empty() {
            return;
        }
        self.zone_requests.push(ZoneRequest::SaveSet(ZoneSet {
            name: name.to_owned(),
            streams: self.chosen_names(),
        }));
    }

    /// The Load button was pressed: what the app remembers as loaded last.
    pub(super) fn loaded_by_user(&mut self) {
        let entries = match &self.preset_ui.applied {
            Some((streams, entries)) if *streams == self.chosen => entries.clone(),
            _ => self.chosen_names(),
        };
        self.zone_requests.push(ZoneRequest::Loaded(entries));
    }

    /// The saved choice of [`Self::restore_on_open`], now that the zones of
    /// the game are known.
    pub(super) fn restore_choice(&mut self) {
        let Some((entries, camera)) = self.restore.take() else {
            return;
        };
        let found = self.choose_saved(&entries);
        self.show_preset_of(&entries);
        self.preset_ui.note = match found.gone.first() {
            Some(first) => trn(
                "zones.last_gone",
                found.gone.len() as u64,
                &[("first", first)],
            ),
            None => String::new(),
        };
        if !self.chosen.is_empty() {
            self.restore_camera = camera;
            self.pending_autoload = true;
        }
    }

    /// Sets the controls of the Presets section to the preset `entries`
    /// name, when they are the entries [`Self::choose_preset`] remembers.
    fn show_preset_of(&mut self, entries: &[String]) {
        let Some(place) = entries.first().and_then(|e| e.strip_prefix("preset:")) else {
            return;
        };
        let (level, district) = match self.presets.level(place) {
            Some(level) => (level, None),
            None => {
                let Some((level, district)) = place.rsplit_once('/') else {
                    return;
                };
                match self.presets.level(level) {
                    Some(level) if level.district(district).is_some() => (level, Some(district)),
                    _ => return,
                }
            }
        };
        let has = |part: &str| {
            entries
                .iter()
                .any(|e| e.strip_prefix("preset:") == Some(&format!("{place}:{part}")))
        };
        self.preset_ui.level.clone_from(&level.level);
        self.preset_ui.district = district.map(str::to_owned);
        self.preset_ui.missions = has("missions");
        self.preset_ui.checkpoints = has("checkpoints");
    }

    /// The level the Presets section shows: the one chosen there, else the
    /// level of the chosen zones, else the first with districts (a city).
    fn preset_level(&self) -> Option<&LevelPresets> {
        let presets = &self.presets;
        presets
            .level(&self.preset_ui.level)
            .or_else(|| {
                let chosen = self
                    .catalog
                    .zones
                    .iter()
                    .find(|z| self.chosen.contains(&z.stream))?;
                presets.level(chosen.level())
            })
            .or_else(|| presets.levels.iter().find(|l| !l.districts.is_empty()))
            .or_else(|| presets.levels.first())
    }

    /// Whether the chosen zones are exactly what the controls of the
    /// Presets section name: the level or the district with the switches.
    fn choice_is_the_preset(&self) -> bool {
        let ui = &self.preset_ui;
        let Some(streams) = self
            .presets
            .level(&ui.level)
            .and_then(|level| level.choice(ui.district.as_deref(), ui.missions, ui.checkpoints))
        else {
            return false;
        };
        presets::resolve_saved(streams, &self.catalog, &self.presets).streams == self.chosen
    }

    /// Turns a switch of the Presets section: "+ missions" (of the
    /// district chosen last, else of the level) or, with `checkpoint`,
    /// "+ checkpoints" (of the level). A choice that was the preset of the
    /// controls becomes that preset with the new switch; a choice made by
    /// hand gets the streams added or taken out and keeps the rest.
    pub fn switch_part(&mut self, checkpoint: bool, on: bool) {
        let presets = self.presets.clone();
        let Some(level) = presets.level(&self.preset_ui.level) else {
            return;
        };
        let district = self.preset_ui.district.clone();
        let preset = self.choice_is_the_preset();
        if checkpoint {
            self.preset_ui.checkpoints = on;
        } else {
            self.preset_ui.missions = on;
        }
        if preset {
            let (missions, checkpoints) = (self.preset_ui.missions, self.preset_ui.checkpoints);
            self.choose_preset(&level.level, district.as_deref(), missions, checkpoints);
            return;
        }
        let streams: Vec<&str> = match (checkpoint, &level.checkpoints, &district) {
            (false, ..) => level.missions_of(district.as_deref()),
            (true, Some(preset), None) => preset.streams.iter().map(String::as_str).collect(),
            _ => Vec::new(),
        };
        let found = presets::resolve_saved(streams, &self.catalog, &self.presets);
        for stream in found.streams {
            if on {
                self.chosen.insert(stream);
            } else {
                self.chosen.remove(&stream);
            }
        }
    }

    /// The Presets and Saved sets sections of the Zones panel.
    pub(super) fn presets_ui(&mut self, ui: &mut egui::Ui) {
        if self.presets.levels.is_empty() {
            return;
        }
        egui::CollapsingHeader::new(RichText::new(tr("zones.presets")).strong())
            .id_salt("level-presets")
            .default_open(true)
            .show(ui, |ui| self.preset_controls(ui));
        egui::CollapsingHeader::new(RichText::new(tr("zones.sets")).strong())
            .id_salt("level-sets")
            .default_open(true)
            .show(ui, |ui| self.sets_ui(ui));
        if !self.preset_ui.note.is_empty() {
            // In full: it names what was skipped.
            ui.add(
                egui::Label::new(
                    RichText::new(&self.preset_ui.note)
                        .small()
                        .color(egui::Color32::from_rgb(230, 160, 80)),
                )
                .wrap(),
            );
        }
    }

    fn preset_controls(&mut self, ui: &mut egui::Ui) {
        let presets = self.presets.clone();
        let Some(level) = self.preset_level() else {
            return;
        };
        let level = presets
            .level(&level.level)
            .expect("a level of these presets");
        let mut picked = level.level.clone();
        ui.horizontal(|ui| {
            ui.label(tr("zones.preset_level"));
            egui::ComboBox::from_id_salt("level-preset-level")
                .selected_text(&picked)
                .width(ui.available_width() - 8.0)
                .show_ui(ui, |ui| {
                    for other in &presets.levels {
                        ui.selectable_value(&mut picked, other.level.clone(), &other.level);
                    }
                });
        });
        if picked != level.level {
            self.preset_ui.level = picked;
            self.preset_ui.district = None;
            return;
        }
        self.preset_ui.level.clone_from(&level.level);
        let (missions, checkpoints) = (self.preset_ui.missions, self.preset_ui.checkpoints);
        let whole = &level.whole;
        // The cost under the button: the panel is narrow.
        if ui
            .button(tr("zones.preset_whole"))
            .on_hover_text(tr("zones.preset_whole_hint"))
            .clicked()
        {
            self.choose_preset(&level.level, None, missions, checkpoints);
        }
        ui.label(
            RichText::new(cost(whole.zones(), whole.bytes))
                .weak()
                .small(),
        );
        if !level.districts.is_empty() {
            let entry = |p: &Preset| {
                trf(
                    "zones.preset_entry",
                    &[
                        ("name", &p.district.as_deref().unwrap_or_default()),
                        ("cost", &cost(p.zones(), p.bytes)),
                    ],
                )
            };
            let district = self
                .preset_ui
                .district
                .as_deref()
                .and_then(|name| level.district(name));
            let mut choice: Option<String> = None;
            ui.horizontal(|ui| {
                ui.label(tr("zones.preset_district"));
                egui::ComboBox::from_id_salt("level-preset-district")
                    .selected_text(
                        district.map_or_else(|| tr("zones.preset_district_none").to_owned(), entry),
                    )
                    .width(ui.available_width() - 8.0)
                    .show_ui(ui, |ui| {
                        for preset in &level.districts {
                            let on = district.is_some_and(|d| d.id == preset.id);
                            if ui.selectable_label(on, entry(preset)).clicked() {
                                choice.clone_from(&preset.district);
                            }
                        }
                    })
                    .response
                    .on_hover_text(tr("zones.preset_district_hint"));
            });
            if let Some(name) = choice {
                self.choose_preset(&level.level, Some(&name), missions, checkpoints);
            }
        }
        // The switches: of the district chosen last, else of the level.
        let district = self.preset_ui.district.clone();
        let mission_streams = level.missions_of(district.as_deref());
        let checkpoint_streams: Vec<&str> = match (&level.checkpoints, &district) {
            (Some(preset), None) => preset.streams.iter().map(String::as_str).collect(),
            _ => Vec::new(),
        };
        let bytes = |streams: &[&str]| -> u64 {
            self.catalog
                .zones
                .iter()
                .filter(|z| streams.binary_search(&z.name.as_str()).is_ok())
                .map(|z| z.bytes)
                .sum()
        };
        let cost_of = |streams: &[&str]| cost(streams.len(), bytes(streams));
        let switches = [
            (
                false,
                &mission_streams,
                missions,
                trf(
                    "zones.preset_missions",
                    &[("cost", &cost_of(&mission_streams))],
                ),
                tr("zones.preset_missions_hint"),
            ),
            (
                true,
                &checkpoint_streams,
                checkpoints,
                trf(
                    "zones.preset_checkpoints",
                    &[("cost", &cost_of(&checkpoint_streams))],
                ),
                tr("zones.preset_checkpoints_hint"),
            ),
        ];
        let mut switched: Option<(bool, bool)> = None;
        ui.horizontal_wrapped(|ui| {
            for (checkpoint, streams, on, text, hint) in switches {
                // A level has missions, checkpoints, or neither.
                if streams.is_empty() && (checkpoint || level.missions.is_none()) {
                    continue;
                }
                let mut on = on;
                if ui
                    .add_enabled(!streams.is_empty(), egui::Checkbox::new(&mut on, text))
                    .on_hover_text(hint)
                    .changed()
                {
                    switched = Some((checkpoint, on));
                }
            }
        });
        if let Some((checkpoint, on)) = switched {
            self.switch_part(checkpoint, on);
        }
        if !level.from_grid {
            ui.label(RichText::new(tr("zones.preset_by_folders")).weak().small());
        }
    }

    fn sets_ui(&mut self, ui: &mut egui::Ui) {
        let saved = self.saved.clone();
        let place = match &saved.project {
            Some(name) => trf("zones.sets_of_project", &[("name", name)]),
            None => tr("zones.sets_of_game").to_owned(),
        };
        ui.label(RichText::new(place).weak().small());
        let mut pick = None;
        let mut remove = None;
        for set in &saved.sets {
            ui.horizontal(|ui| {
                let text = trf(
                    "zones.set_entry",
                    &[("name", &set.name), ("count", &set.streams.len())],
                );
                if ui
                    .button(text)
                    .on_hover_text(tr("zones.set_pick_hint"))
                    .clicked()
                {
                    pick = Some(set.name.clone());
                }
                if ui
                    .small_button("×")
                    .on_hover_text(tr("zones.set_remove_hint"))
                    .clicked()
                {
                    remove = Some(set.name.clone());
                }
            });
        }
        if let Some(name) = pick {
            self.choose_set(&name);
        }
        if let Some(name) = remove {
            self.zone_requests.push(ZoneRequest::RemoveSet(name));
        }
        ui.horizontal(|ui| {
            let save = egui::Button::new(tr("zones.set_save"));
            let ready = !self.preset_ui.name.trim().is_empty() && !self.chosen.is_empty();
            // The button first, from the right: the field takes what is left.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let clicked = ui
                    .add_enabled(ready, save)
                    .on_hover_text(tr("zones.set_save_hint"))
                    .clicked();
                let entered = ui
                    .add(
                        egui::TextEdit::singleline(&mut self.preset_ui.name)
                            .hint_text(tr("zones.set_name"))
                            .desired_width(ui.available_width()),
                    )
                    .lost_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ready && (clicked || entered) {
                    let name = std::mem::take(&mut self.preset_ui.name);
                    self.save_set(&name);
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use doctor_toolkit::level::presets::GridCells;
    use doctor_toolkit::level::{Catalog, Presets, Zone};

    use super::*;

    /// A view over a catalog with these zones (no game: the roles of the
    /// zones are told by their folders).
    fn view(names: &[&str]) -> LevelView {
        let mut zones: Vec<Zone> = names
            .iter()
            .enumerate()
            .map(|(stream, name)| Zone {
                stream,
                name: (*name).to_owned(),
                simgroups: 1,
                meshes: 0,
                metamodels: 0,
                bytes: 1_000_000,
            })
            .collect();
        zones.sort_by(|a, b| a.name.cmp(&b.name));
        let catalog = Catalog { zones };
        let mut view = LevelView::new(None, &egui::Context::default());
        view.forget_preset();
        view.presets = Arc::new(Presets::new(&catalog, &GridCells::new()));
        view.catalog = Arc::new(catalog);
        view
    }

    const TERRAIN: &str = "gf2/miami/local/bonita/high/terrain06";
    const VENUE: &str = "gf2/miami/local/bonita/interior/venue_115";
    const ALICE: &str = "gf2/miami/local/alice/high/terrain01";
    const MEDIUM: &str = "gf2/miami/local/bonita/medium/zone1";
    const OBJECTIVE: &str = "gf2/miami/global/objectives/c29/r07_common_folderstream";
    const CHECKPOINT: &str = "gf2/miami/global/checkpoints/c17/r04_common_folderstream";
    const CITY: &[&str] = &[
        "gf2/miami",
        TERRAIN,
        VENUE,
        ALICE,
        MEDIUM,
        OBJECTIVE,
        CHECKPOINT,
        "gf2/frontend",
    ];

    #[test]
    fn presets_districts_and_switches() {
        let mut view = view(CITY);
        // The whole level at near detail: no coarser copy, no missions.
        assert!(view.choose_preset("miami", None, false, false));
        assert_eq!(view.chosen_names(), ["gf2/miami", ALICE, TERRAIN, VENUE]);
        view.loaded_by_user();
        assert_eq!(
            view.take_zone_requests(),
            [ZoneRequest::Loaded(vec!["preset:gf2/miami".into()])]
        );
        // A switch keeps it a preset, remembered by its words.
        view.switch_part(false, true);
        view.switch_part(true, true);
        assert_eq!(
            view.chosen_names(),
            ["gf2/miami", CHECKPOINT, OBJECTIVE, ALICE, TERRAIN, VENUE]
        );
        view.loaded_by_user();
        assert_eq!(
            view.take_zone_requests(),
            [ZoneRequest::Loaded(vec![
                "preset:gf2/miami".into(),
                "preset:gf2/miami:missions".into(),
                "preset:gf2/miami:checkpoints".into(),
            ])]
        );
        // A district: its zones; no mission lies in its folder and the
        // checkpoints are the level's, so the switches that stay on add
        // nothing.
        assert!(view.choose_preset("gf2/miami", Some("bonita"), true, true));
        assert_eq!(view.chosen_names(), [TERRAIN, VENUE]);
        view.loaded_by_user();
        assert_eq!(
            view.take_zone_requests(),
            [ZoneRequest::Loaded(vec!["preset:gf2/miami/bonita".into()])]
        );

        // A choice made by hand: a switch adds and takes out its streams
        // and leaves the rest; it is remembered by its streams.
        view.chosen = view
            .catalog
            .zones
            .iter()
            .filter(|z| z.name == MEDIUM)
            .map(|z| z.stream)
            .collect();
        view.preset_ui.district = None;
        view.switch_part(false, true);
        assert_eq!(view.chosen_names(), [OBJECTIVE, MEDIUM]);
        view.loaded_by_user();
        assert_eq!(
            view.take_zone_requests(),
            [ZoneRequest::Loaded(vec![OBJECTIVE.into(), MEDIUM.into()])]
        );
        view.switch_part(false, false);
        assert_eq!(view.chosen_names(), [MEDIUM]);

        // A set is saved by the names of its streams.
        view.save_set(" far ");
        assert_eq!(
            view.take_zone_requests(),
            [ZoneRequest::SaveSet(ZoneSet {
                name: "far".into(),
                streams: vec![MEDIUM.into()],
            })]
        );
        view.chosen.clear();
        view.save_set("nothing");
        assert!(view.take_zone_requests().is_empty());
    }

    #[test]
    fn a_restored_preset_shows_in_the_controls() {
        let mut view = view(CITY);
        view.restore = Some((
            vec![
                "preset:gf2/miami/bonita".into(),
                "preset:gf2/miami/bonita:missions".into(),
                "gf2/miami/local/gone".into(),
            ],
            None,
        ));
        view.restore_choice();
        assert_eq!(view.chosen_names(), [TERRAIN, VENUE]);
        assert_eq!(view.preset_ui.level, "gf2/miami");
        assert_eq!(view.preset_ui.district.as_deref(), Some("bonita"));
        assert!(view.preset_ui.missions && !view.preset_ui.checkpoints);
        assert!(view.zones_note().contains("gf2/miami/local/gone"));
        assert!(view.pending_autoload);
    }
}

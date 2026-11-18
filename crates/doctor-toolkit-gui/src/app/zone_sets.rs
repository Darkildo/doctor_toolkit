//! Where the editor keeps the saved sets of zones and the choice the level
//! editor loaded last (stage 113).
//!
//! They belong to the work, so the **open project** keeps them: the sets in
//! `[[zones]]` of its `project.toml` (they travel with the project), the
//! choice loaded last and the camera in its `.doctor/editor.json` (one
//! computer's, not for version control). Without a project they are in the
//! settings of the editor, for the open game.
//!
//! The Zones panel of a level view only asks
//! ([`crate::level_view::ZoneRequest`]); [`BrowserApp::zone_sets_tick`]
//! answers once a frame.

use std::path::PathBuf;
use std::sync::Arc;

use doctor_toolkit::level::ZoneSet;
use doctor_toolkit::project::{EditorCamera, EditorState, ProjectConfig};
use doctor_toolkit_mods::i18n::{trf, trn};

use super::{BrowserApp, Level};
use crate::level_view::{SavedSets, ZoneRequest};

impl BrowserApp {
    /// Folder of the open project.
    fn zone_project(&self) -> Option<PathBuf> {
        self.document.as_ref().map(|d| d.root.clone())
    }

    /// Folder of the open game, the key of its settings.
    fn zone_game(&self) -> Option<PathBuf> {
        self.game.as_ref().map(|g| g.root().to_owned())
    }

    /// Reads the saved sets where they are kept now — a game or a project
    /// was opened or closed, a set was saved — and gives them to the level
    /// views.
    pub(super) fn reload_zone_sets(&mut self) {
        let saved = match &self.document {
            Some(document) => SavedSets {
                sets: ProjectConfig::read(&document.root)
                    .map(|config| config.zones)
                    .unwrap_or_default(),
                project: Some(document.name.clone()),
            },
            None => SavedSets {
                sets: self
                    .zone_game()
                    .map(|root| self.settings.game(&root).zone_sets)
                    .unwrap_or_default(),
                project: None,
            },
        };
        self.zone_sets = Arc::new(saved);
        for level in self.levels.values_mut() {
            level.set_saved_sets(self.zone_sets.clone());
        }
    }

    /// Writes the saved sets where they are kept.
    fn write_zone_sets(&mut self, sets: Vec<ZoneSet>) -> Result<(), String> {
        match self.zone_project() {
            Some(root) => ProjectConfig::write_zones(&root, &sets).map_err(|e| e.to_string())?,
            None => {
                let root = self.zone_game().ok_or("no game is open")?;
                self.settings.game_mut(&root).zone_sets = sets;
            }
        }
        self.reload_zone_sets();
        Ok(())
    }

    /// Saves a set; one of the same name is replaced where it stood.
    pub(super) fn save_zone_set(&mut self, set: ZoneSet) {
        let mut sets = self.zone_sets.sets.clone();
        let text = trf(
            "zones.set_saved",
            &[
                ("name", &set.name),
                ("zones", &trn("count.zones", set.streams.len() as u64, &[])),
            ],
        );
        match sets.iter_mut().find(|s| s.name == set.name) {
            Some(old) => *old = set,
            None => sets.push(set),
        }
        match self.write_zone_sets(sets) {
            Ok(()) => self.status = text,
            Err(error) => self.notify(
                Level::Error,
                trf("zones.set_cannot_save", &[("error", &error)]),
            ),
        }
    }

    pub(super) fn remove_zone_set(&mut self, name: &str) {
        let mut sets = self.zone_sets.sets.clone();
        sets.retain(|s| s.name != name);
        match self.write_zone_sets(sets) {
            Ok(()) => self.status = trf("zones.set_removed", &[("name", &name)]),
            Err(error) => self.notify(
                Level::Error,
                trf("zones.set_cannot_save", &[("error", &error)]),
            ),
        }
    }

    /// What the level editor loaded last with this project (or, without
    /// one, this game) open, and where its camera stood.
    pub(super) fn last_zones(&self) -> (Vec<String>, Option<EditorCamera>) {
        match self.zone_project() {
            Some(root) => {
                let state = EditorState::read(&root);
                (state.zones, state.camera)
            }
            None => self
                .zone_game()
                .map(|root| {
                    let game = self.settings.game(&root);
                    (game.last_zones, game.last_camera)
                })
                .unwrap_or_default(),
        }
    }

    /// Remembers what the level editor has loaded: the zones, when the user
    /// loaded others, or `None` to keep them and place the camera only.
    fn remember_zones(&mut self, zones: Option<Vec<String>>, camera: Option<EditorCamera>) {
        match self.zone_project() {
            Some(root) => {
                let mut state = EditorState::read(&root);
                let before = state.clone();
                if let Some(zones) = zones {
                    state.zones = zones;
                }
                state.camera = camera;
                if state != before
                    && let Err(error) = state.write(&root)
                {
                    self.notify(Level::Warning, error.to_string());
                }
            }
            None => {
                let Some(root) = self.zone_game() else {
                    return;
                };
                let game = self.settings.game_mut(&root);
                if let Some(zones) = zones {
                    game.last_zones = zones;
                }
                game.last_camera = camera;
            }
        }
    }

    /// Remembers where the camera stands over the zones loaded last: the
    /// window, the project or the game is about to close.
    pub(super) fn remember_level_camera(&mut self) {
        let camera = self
            .zones_level
            .and_then(|id| self.levels.get(&id))
            .and_then(|level| level.camera_state());
        if camera.is_some() {
            self.remember_zones(None, camera);
        }
    }

    /// Level view `id`, about to be given the game, opens with what was
    /// loaded last, unless it was told what to load.
    pub(super) fn restore_level_zones(&mut self, id: u64) {
        let (zones, camera) = self.last_zones();
        if let Some(level) = self.levels.get_mut(&id)
            && level.restore_on_open(zones, camera)
        {
            self.zones_level = Some(id);
        }
    }

    /// A project was opened for the game that is open: a level tab that
    /// shows nothing yet loads what the project had loaded last.
    pub(super) fn restore_empty_level(&mut self) {
        let (zones, camera) = self.last_zones();
        let mut ids: Vec<u64> = self.levels.keys().copied().collect();
        ids.sort_unstable();
        for id in ids {
            if self
                .levels
                .get_mut(&id)
                .is_some_and(|level| level.restore_if_empty(zones.clone(), camera))
            {
                self.zones_level = Some(id);
                return;
            }
        }
    }

    /// Once a frame: what the Zones panels asked for.
    pub(super) fn zone_sets_tick(&mut self) {
        let mut requests = Vec::new();
        for (&id, level) in &mut self.levels {
            if !Arc::ptr_eq(level.saved_sets(), &self.zone_sets) {
                level.set_saved_sets(self.zone_sets.clone());
            }
            requests.extend(level.take_zone_requests().into_iter().map(|r| (id, r)));
        }
        for (id, request) in requests {
            match request {
                ZoneRequest::SaveSet(set) => self.save_zone_set(set),
                ZoneRequest::RemoveSet(name) => self.remove_zone_set(&name),
                ZoneRequest::Loaded(zones) => {
                    // The camera is framed anew over other zones.
                    self.zones_level = Some(id);
                    self.remember_zones(Some(zones), None);
                }
            }
        }
    }
}

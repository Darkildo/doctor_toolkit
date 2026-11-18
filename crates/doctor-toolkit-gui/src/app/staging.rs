//! The game the editor shows: the game folder with the streams the
//! workspace creates staged as streams of it (stage 103,
//! `doctor_toolkit::workspace::stage`).
//!
//! A stream made in the workspace has no file in the game until `apply`,
//! and everything in the editor addresses a resource by the number of its
//! stream and its position in the file. So the editor's `game` is the
//! staged game: a new zone is in the Files panel, the Content Browser, the
//! zone list of the level view and the character view like any other
//! stream, before anything is written into the game. Streams of the game
//! folder keep their numbers.
//!
//! [`BrowserApp::restage`] runs after every change of the workspace
//! ([`BrowserApp::refresh_status`]) and does something only when the
//! *layout* of the new streams changed (a stream or a resource of one was
//! added or removed): edits of data are read through the workspace, as for
//! streams of the game.

use std::sync::Arc;

use doctor_toolkit::game::{GameInstance, ResourceIndex};
use doctor_toolkit::workspace::ResourceKey;
use doctor_toolkit_mods::i18n::trf;

use super::{BrowserApp, FileSel, OpenStream, StreamTab};
use crate::nav::Location;

/// What the staged game of the editor was made from.
#[derive(Default)]
pub(super) struct Staging {
    /// The new streams and their resources as staged
    /// (`Workspace::staged_layout`).
    layout: Vec<(String, Vec<ResourceKey>)>,
    /// The index of the game folder, without the staged streams.
    disk_index: Option<Arc<ResourceIndex>>,
    /// The game folder without the staged streams, while some are staged.
    disk: Option<Arc<GameInstance>>,
}

impl Staging {
    /// A game was opened: nothing is staged.
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// The resource at `loc` of `old` in `new`: the same place in a stream of
/// the game folder, the resource with the same key in a staged stream.
pub(super) fn remap(old: &GameInstance, new: &GameInstance, loc: Location) -> Option<Location> {
    let file = old.streams().get(loc.stream)?;
    if !old.is_staged(loc.stream) && !new.is_staged(loc.stream) {
        return (loc.stream < new.streams().len()).then_some(loc);
    }
    let name = file.stream_name();
    let header = old
        .open_stream(loc.stream)
        .ok()?
        .layout()
        .resources
        .get(loc.resource)?
        .header;
    let stream = new.find_stream(&name)?;
    let resource =
        ResourceKey::new(&name, &header).find_in(new.open_stream(stream).ok()?.layout())?;
    Some(Location { stream, resource })
}

impl BrowserApp {
    /// The game as its folder has it: for what writes into the game or
    /// packs from it (apply, mods, Play), which must not see staged streams.
    pub(super) fn disk_game(&self) -> Option<Arc<GameInstance>> {
        let game = self.game.as_ref()?;
        if !game.has_staged() {
            return Some(game.clone());
        }
        Some(match &self.staging.disk {
            Some(disk) => disk.clone(),
            None => Arc::new(game.on_disk()),
        })
    }

    /// Nothing was staged for this game yet: it is being opened.
    pub(super) fn staging_is_fresh(&self) -> bool {
        self.staging.disk_index.is_none()
            && self.game.as_ref().is_some_and(|g| !g.has_staged())
            && self.levels.values().all(|l| !l.has_game())
    }

    /// Stages the new streams without telling the views.
    pub(super) fn stage_quietly(&mut self) {
        self.stage_now();
    }

    /// Stages the game for the workspace as it is, without telling the
    /// views: for the moment a game is opened, before they are made.
    pub(super) fn stage_opened_game(&mut self) {
        self.staging.clear();
        self.stage_now();
    }

    /// Stages the new streams of the workspace when their layout changed.
    /// Returns the game shown before when the views now show another.
    fn stage_now(&mut self) -> Option<Arc<GameInstance>> {
        let (game, index) = (self.game.clone()?, self.index.clone()?);
        let layout = self
            .workspace
            .as_ref()
            .map(|ws| ws.staged_layout())
            .unwrap_or_default();
        if layout == self.staging.layout && (game.has_staged() || layout.is_empty()) {
            return None;
        }
        let disk_index = match &self.staging.disk_index {
            Some(index) if game.has_staged() => index.clone(),
            _ => index,
        };
        let staged = match &self.workspace {
            Some(ws) => ws.stage(&game),
            None => Ok(game.on_disk()),
        };
        let staged = match staged {
            Ok(staged) => staged,
            Err(error) => {
                self.status = trf("staging.failed", &[("error", &error)]);
                return None;
            }
        };
        let index = staged.staged_index(&disk_index);
        if self.staging.disk.is_none() || !game.has_staged() {
            self.staging.disk = Some(Arc::new(game.on_disk()));
        }
        self.staging.disk_index = Some(disk_index);
        self.staging.layout = layout;
        self.game = Some(Arc::new(staged));
        self.index = Some(Arc::new(index));
        Some(game)
    }

    /// After a change of the workspace: stages its new streams again when
    /// their layout changed and shows the views the new game. Returns the
    /// game shown before, for [`remap`], when that happened.
    pub(super) fn restage(&mut self) -> Option<Arc<GameInstance>> {
        let old = self.stage_now()?;
        let (game, index) = (self.game.clone()?, self.index.clone()?);
        // Files of staged streams are other files now.
        let moved = |s: usize| old.is_staged(s) || game.is_staged(s) || s >= game.streams().len();
        self.files.retain(|s| !moved(s));
        self.assets = None;
        self.type_summary = index.type_summary();
        crate::cutscene_view::preview::share_game(game.clone(), index.clone());
        self.nav.restage(game.clone());
        self.content.restaged();
        for level in self.levels.values_mut() {
            level.restage(game.clone(), index.clone(), self.workspace.as_ref());
        }
        if let Some(dir) = self.workspace_dir() {
            for logic in self.logics.values_mut() {
                logic.open_game(game.clone(), index.clone(), dir.clone());
            }
        }
        // Stream tabs of staged streams open the new file; a tab of a stream
        // that is gone is left empty.
        let ids: Vec<u64> = self.streams.keys().copied().collect();
        for id in ids {
            let Some(shown) = self.streams.get(&id).and_then(|t| t.stream.as_ref()) else {
                continue;
            };
            let (at, filter) = (shown.game_index, shown.type_filter);
            if !moved(at) {
                continue;
            }
            let name = old.streams().get(at).map(|f| f.stream_name());
            let again = name
                .and_then(|n| game.find_stream(&n))
                .and_then(|s| Some((s, self.files.get(&game, s).ok()?)));
            let tab = self.streams.get_mut(&id).expect("listed above");
            match again {
                Some((s, file)) => {
                    if tab
                        .selected
                        .is_some_and(|r| r >= file.layout().resources.len())
                    {
                        tab.selected = None;
                    }
                    tab.stream = Some(OpenStream::new(s, &file, filter));
                    tab.sel = Some(FileSel::Stream(s));
                }
                None => *tab = StreamTab::default(),
            }
        }
        // Resources shown in Details and in tabs follow their keys.
        if let Some(doc) = self.details.take() {
            self.details = remap(&old, &game, doc.loc).map(|loc| self.load_doc(loc, Some(doc)));
        }
        let ids: Vec<u64> = self.docs.keys().copied().collect();
        for id in ids {
            let Some(doc) = self.docs.remove(&id) else {
                continue;
            };
            if !moved(doc.loc.stream) {
                self.docs.insert(id, doc);
            } else if let Some(loc) = remap(&old, &game, doc.loc) {
                let doc = self.load_doc(loc, Some(doc));
                self.docs.insert(id, doc);
            }
        }
        if self
            .selected_file
            .is_some_and(|f| matches!(f, FileSel::Stream(s) if moved(s)))
        {
            self.selected_file = None;
        }
        self.refilter_files();
        self.refilter_index();
        for search in self.searches.values_mut() {
            search.pending = true;
        }
        Some(old)
    }
}

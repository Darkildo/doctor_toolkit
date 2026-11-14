//! Open stream files and opened resources.
//!
//! Every tab reads stream files through one [`StreamCache`], so that all of
//! them can be released before the game files are replaced (apply, revert,
//! mods, restore) and opened again afterwards. A [`ResourceDoc`] is one
//! resource with the views of the registry ([`crate::views`]); the Details
//! panel shows one, resource tabs show the others.

use std::collections::HashMap;
use std::sync::Arc;

use doctor_toolkit::game::{GameInstance, StreamFile};
use doctor_toolkit::resource::anim::TYPE_BNK;
use doctor_toolkit::resource::mesh::TYPE_MESH;
use doctor_toolkit_mods::i18n::{tr, trf};

use super::{AUTO_PREVIEW_LIMIT, BrowserApp, FileSel, OpenStream};
use crate::nav::Location;
use crate::views::{self, ResourceViewer, ViewSource};

/// Stream files opened by the tabs.
#[derive(Default)]
pub(super) struct StreamCache {
    open: HashMap<usize, Arc<StreamFile>>,
    /// The game files are being written: nothing may be opened.
    released: bool,
}

impl StreamCache {
    pub fn get(&mut self, game: &GameInstance, stream: usize) -> Result<Arc<StreamFile>, String> {
        if self.released {
            return Err(tr("docs.being_written").to_owned());
        }
        if let Some(file) = self.open.get(&stream) {
            return Ok(file.clone());
        }
        let file = Arc::new(game.open_stream(stream).map_err(|e| e.to_string())?);
        self.open.insert(stream, file.clone());
        Ok(file)
    }

    /// The file of `stream` if it is open.
    pub fn peek(&self, stream: usize) -> Option<&Arc<StreamFile>> {
        self.open.get(&stream)
    }

    /// Closes every file and refuses to open any until [`Self::resume`].
    pub fn release(&mut self) {
        self.open.clear();
        self.released = true;
    }

    pub fn resume(&mut self) {
        self.released = false;
    }

    pub fn is_released(&self) -> bool {
        self.released
    }

    /// Forgets every file, e.g. for another game.
    pub fn clear(&mut self) {
        self.open.clear();
        self.released = false;
    }

    /// Closes the files no tab shows.
    pub fn retain(&mut self, keep: impl Fn(usize) -> bool) {
        self.open.retain(|&stream, _| keep(stream));
    }
}

/// An opened resource and its views.
pub(super) struct ResourceDoc {
    pub loc: Location,
    pub type_id: u32,
    pub size: usize,
    pub views: Vec<(&'static str, Box<dyn ResourceViewer>)>,
    /// Data for the hex view: loaded with the resource unless it is large or
    /// a view fills the panel, otherwise on request.
    pub hex: Option<Result<Vec<u8>, String>>,
    /// Why the resource cannot be shown.
    pub error: Option<String>,
}

impl ResourceDoc {
    /// Whether a view holds edits that are not recorded yet.
    pub fn has_unsaved_edits(&self) -> bool {
        self.views.iter().any(|(_, v)| v.has_unsaved_edits())
    }

    pub fn hides_hex(&self) -> bool {
        self.views.iter().any(|(_, v)| v.hides_hex())
    }
}

impl BrowserApp {
    /// Loads resource `loc` with every view that applies; `previous` is the
    /// doc it replaces, whose view state carries over.
    pub(super) fn load_doc(&mut self, loc: Location, previous: Option<ResourceDoc>) -> ResourceDoc {
        let mut doc = ResourceDoc {
            loc,
            type_id: 0,
            size: 0,
            views: Vec::new(),
            hex: None,
            error: None,
        };
        let Some(game) = self.game.clone() else {
            doc.error = Some(tr("files.no_game").to_owned());
            return doc;
        };
        let file = match self.files.get(&game, loc.stream) {
            Ok(file) => file,
            Err(error) => {
                doc.error = Some(error);
                return doc;
            }
        };
        let Some(header) = file.layout().resources.get(loc.resource).map(|r| r.header) else {
            doc.error = Some(trf(
                "docs.no_resource",
                &[
                    ("stream", &game.streams()[loc.stream].rel_path),
                    ("index", &loc.resource),
                ],
            ));
            return doc;
        };
        doc.type_id = header.type_id;
        doc.size = header.data_size as usize;
        if (header.type_id == TYPE_MESH || header.type_id == TYPE_BNK)
            && self.assets.is_none()
            && let Some(index) = &self.index
        {
            self.assets = Some(doctor_toolkit::game::GameAssets::new(game.clone(), index));
        }
        let mut none = doctor_toolkit::export::gltf::NoAssets;
        let assets: &mut dyn doctor_toolkit::export::gltf::AssetSource = match &mut self.assets {
            Some(assets) => assets,
            None => &mut none,
        };
        let mut source = ViewSource {
            game: &game,
            stream_index: loc.stream,
            stream: &file,
            index: loc.resource,
            type_id: header.type_id,
            workspace: self.workspace.as_ref(),
            ws_status: self.ws_status.as_ref(),
            assets,
            gpu: self.gpu.as_ref(),
        };
        let (previous_views, had_hex) = match previous {
            Some(p) if p.loc == loc => (p.views, p.hex.is_some()),
            _ => (Vec::new(), false),
        };
        doc.views = views::load_all(&mut source, previous_views);
        // Game-logic views fill the panel; their hex view is loaded on request.
        if (doc.size <= AUTO_PREVIEW_LIMIT && !doc.hides_hex()) || had_hex {
            doc.hex = Some(source.data(loc.resource));
        }
        doc
    }

    /// Loads the hex view of a doc that did not get it automatically.
    pub(super) fn load_hex(&mut self, doc: &mut ResourceDoc) {
        let Some(game) = self.game.clone() else {
            return;
        };
        let Ok(file) = self.files.get(&game, doc.loc.stream) else {
            return;
        };
        let mut none = doctor_toolkit::export::gltf::NoAssets;
        let source = ViewSource {
            game: &game,
            stream_index: doc.loc.stream,
            stream: &file,
            index: doc.loc.resource,
            type_id: doc.type_id,
            workspace: self.workspace.as_ref(),
            ws_status: self.ws_status.as_ref(),
            assets: &mut none,
            gpu: None,
        };
        doc.hex = Some(source.data(doc.loc.resource));
    }

    /// Loads the opened resources of `stream` (all when `None`) again, e.g.
    /// after an edit, keeping the state of their views.
    pub(super) fn reload_docs(&mut self, stream: Option<usize>) {
        let matches = |loc: &Location| stream.is_none_or(|s| s == loc.stream);
        if let Some(doc) = self.details.take() {
            self.details = Some(if matches(&doc.loc) {
                self.load_doc(doc.loc, Some(doc))
            } else {
                doc
            });
        }
        let ids: Vec<u64> = self
            .docs
            .iter()
            .filter(|(_, d)| matches(&d.loc))
            .map(|(&id, _)| id)
            .collect();
        for id in ids {
            if let Some(doc) = self.docs.remove(&id) {
                let doc = self.load_doc(doc.loc, Some(doc));
                self.docs.insert(id, doc);
            }
        }
    }

    /// Shows resource `resource` of the stream tab `tab` in the Details panel;
    /// `None` clears the selection.
    pub(super) fn select_resource(&mut self, tab: u64, resource: Option<usize>) {
        self.focus_stream = Some(tab);
        let Some(t) = self.streams.get_mut(&tab) else {
            return;
        };
        t.selected = resource;
        let stream = t.stream.as_ref().map(|s| s.game_index);
        let (Some(resource), Some(stream)) = (resource, stream) else {
            return;
        };
        let loc = Location { stream, resource };
        let previous = self.details.take();
        self.details = Some(self.load_doc(loc, previous));
        self.inspect = super::details::Focus::Resource;
    }

    /// Opens resource `loc` in a tab of its own.
    pub(super) fn open_resource_tab(&mut self, loc: Location) {
        let existing = self
            .docs
            .iter()
            .find(|(id, d)| d.loc == loc && self.live(**id))
            .map(|(&id, _)| id);
        if let Some(id) = existing {
            self.dock_ops.push(super::dock::DockOp::Focus(id));
            return;
        }
        // Reuse the doc of the Details panel when it shows the same resource.
        let doc = match self.details.take() {
            Some(d) if d.loc == loc => {
                let copy = self.load_doc(loc, None);
                self.details = Some(d);
                copy
            }
            other => {
                self.details = other;
                self.load_doc(loc, None)
            }
        };
        let key = self.key_of(loc).map(|k| k.to_string()).unwrap_or_default();
        let id = self.new_pane_id();
        self.docs.insert(id, doc);
        self.dock_ops
            .push(super::dock::DockOp::Open(super::dock::Pane::Resource {
                id,
                key,
            }));
    }

    /// Opens a stream or table of contents in the stream tab used last, or
    /// in a new tab.
    pub(super) fn open_file(&mut self, sel: FileSel, new_tab: bool) {
        let target = if new_tab {
            None
        } else {
            self.focus_stream
                .filter(|id| self.live(*id))
                .or_else(|| self.saved_stream_tab(sel))
        };
        let id = match target {
            Some(id) => {
                self.streams.entry(id).or_default();
                id
            }
            None => {
                let id = self.new_pane_id();
                self.streams.insert(id, super::StreamTab::default());
                self.dock_ops
                    .push(super::dock::DockOp::Open(super::dock::Pane::Stream {
                        id,
                        path: None,
                    }));
                id
            }
        };
        self.selected_file = Some(sel);
        self.show_file(id, sel);
        self.dock_ops.push(super::dock::DockOp::Focus(id));
    }

    /// A stream tab of the dock, preferably one that shows `sel` (from a
    /// saved layout, before it was first drawn).
    fn saved_stream_tab(&self, sel: FileSel) -> Option<u64> {
        let tree = self.dock.as_ref()?;
        let path = self.game.as_ref().and_then(|game| match sel {
            FileSel::Stream(i) => game.streams().get(i).map(|f| f.rel_path.clone()),
            FileSel::Toc(i) => game.tocs().get(i).map(|f| f.rel_path.clone()),
        });
        let streams: Vec<(u64, Option<String>)> = tree
            .tiles
            .tiles()
            .filter_map(|t| match t {
                egui_tiles::Tile::Pane(super::dock::Pane::Stream { id, path }) => {
                    Some((*id, path.clone()))
                }
                _ => None,
            })
            .collect();
        streams
            .iter()
            .find(|(_, p)| p.is_some() && *p == path)
            .or_else(|| streams.first())
            .map(|(id, _)| *id)
    }

    /// Shows `sel` in stream tab `id`.
    pub(super) fn show_file(&mut self, id: u64, sel: FileSel) {
        self.focus_stream = Some(id);
        let Some(game) = self.game.clone() else {
            return;
        };
        let Some(tab) = self.streams.get_mut(&id) else {
            return;
        };
        tab.restore = None;
        if tab.sel == Some(sel) && (tab.stream.is_some() || tab.toc.is_some()) {
            return;
        }
        tab.sel = Some(sel);
        tab.selected = None;
        tab.stream = None;
        tab.toc = None;
        match sel {
            FileSel::Stream(i) => match self.files.get(&game, i) {
                Ok(file) => {
                    let tab = self.streams.get_mut(&id).expect("checked above");
                    tab.stream = Some(OpenStream::new(i, &file, None));
                    self.mark_changed_rows();
                }
                Err(error) => {
                    self.status = format!("{}: {error}", game.streams()[i].rel_path);
                }
            },
            FileSel::Toc(i) => {
                tab.toc = Some((i, game.load_toc(i).map_err(|e| e.to_string())));
            }
        }
        self.prune_files();
    }

    /// Closes stream files no tab shows any more.
    pub(super) fn prune_files(&mut self) {
        let mut used: Vec<usize> = self
            .streams
            .values()
            .filter_map(|t| t.stream.as_ref().map(|s| s.game_index))
            .collect();
        used.extend(self.details.iter().map(|d| d.loc.stream));
        used.extend(self.docs.values().map(|d| d.loc.stream));
        self.files.retain(|s| used.contains(&s));
    }

    /// Releases every open game file before the game files are replaced.
    pub(super) fn release_files(&mut self) {
        self.files.release();
        self.content.previews.release();
        for level in self.levels.values_mut() {
            level.release_files();
        }
    }

    /// The game files were replaced: open again what the tabs show.
    pub(super) fn files_changed(&mut self) {
        self.files.resume();
        self.content.files_changed();
        if let Some(game) = self.game.clone() {
            // The index reads the changed streams again.
            self.nav.refresh(game.clone());
            let ids: Vec<u64> = self.streams.keys().copied().collect();
            for id in ids {
                let Some(tab) = self.streams.get_mut(&id) else {
                    continue;
                };
                let Some(stream) = tab.stream.take() else {
                    continue;
                };
                match self.files.get(&game, stream.game_index) {
                    Ok(file) => {
                        let tab = self.streams.get_mut(&id).expect("checked above");
                        let open = OpenStream::new(stream.game_index, &file, stream.type_filter);
                        if tab
                            .selected
                            .is_some_and(|r| r >= file.layout().resources.len())
                        {
                            tab.selected = None;
                        }
                        tab.stream = Some(open);
                    }
                    Err(error) => {
                        self.status = trf("docs.cannot_reopen", &[("error", &error)]);
                    }
                }
            }
        }
        for level in self.levels.values_mut() {
            level.game_files_changed(self.workspace.as_ref());
        }
        self.mark_changed_rows();
        self.reload_docs(None);
    }
}

impl OpenStream {
    /// The resources of `file`, filtered by type.
    pub fn new(game_index: usize, file: &StreamFile, filter: Option<u32>) -> Self {
        let mut counts = std::collections::BTreeMap::new();
        for r in &file.layout().resources {
            *counts.entry(r.header.type_id).or_insert(0usize) += 1;
        }
        let mut types: Vec<_> = counts.into_iter().collect();
        types.sort_by_key(|t| std::cmp::Reverse(t.1));
        let mut stream = Self {
            game_index,
            rows: Vec::new(),
            type_filter: None,
            types,
            changed: Default::default(),
        };
        stream.set_filter(file, filter);
        stream
    }

    pub fn set_filter(&mut self, file: &StreamFile, filter: Option<u32>) {
        self.type_filter = filter;
        let resources = &file.layout().resources;
        self.rows = (0..resources.len())
            .filter(|&i| filter.is_none_or(|t| resources[i].header.type_id == t))
            .collect();
    }
}

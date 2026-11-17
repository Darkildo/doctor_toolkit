//! Tabs and dockable panels ([`egui_tiles`]).
//!
//! The dock is a tree of tiles: tab groups and splits, with [`Pane`]s as
//! leaves. Documents (streams, resources, searches, levels, all resources,
//! the welcome screen) open as tabs next to each other; tool panels (file
//! list, details, changes, log) are tabs too, by default around the
//! documents, and every tab can be dragged into another group or split.
//! Closing a tool panel hides it; View brings it back where it was.
//!
//! A pane holds only what is needed to open it again: the layout is saved as
//! JSON (`super::layouts`) and the tabs reopen their stream, resource or
//! query when the game is opened at the next start.
//!
//! Stage 52: a panel can also be a drawer (`super::drawer`) instead of a
//! tile; the tab's menu moves it there and back. Layouts — built-in, the
//! user's, per screen — are in `super::layouts`.

use std::collections::HashSet;

use egui_tiles::{
    Behavior, Container, SimplificationOptions, Tile, TileId, Tiles, Tree, UiResponse,
};
use serde::{Deserialize, Serialize};

use doctor_toolkit_mods::i18n::tr;

use super::BrowserApp;
use super::layouts::{LayoutId, Screen};

/// The content of a tab.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Pane {
    Welcome,
    /// Streams and tables of contents of the game.
    Files,
    /// The inspector of what was selected last: an object of a level or a
    /// resource of a list (stage 53). Layouts of stage 52 had a separate
    /// object panel, `ObjectDetails`: it reads as this one.
    #[serde(alias = "ObjectDetails")]
    Details,
    /// Another Details panel (View → Another Details panel), which can be
    /// pinned to an object while the first one follows the selection.
    DetailsCopy {
        id: u64,
    },
    /// Workspace changes.
    Changes,
    /// Messages, warnings and errors.
    Log,
    /// All resources of the game.
    Index,
    /// The resources of a stream (or a table of contents), by path in the game.
    Stream {
        id: u64,
        path: Option<String>,
    },
    /// One resource, by workspace key.
    Resource {
        id: u64,
        key: String,
    },
    Search {
        id: u64,
        query: String,
    },
    Level {
        id: u64,
    },
    /// "What do you want to do?": the wizards.
    Tasks,
    /// A wizard.
    Wizard {
        id: u64,
        kind: super::wizard::Kind,
    },
    /// A logic graph; `scope` as `logic-graph` takes it.
    Logic {
        id: u64,
        scope: String,
    },
    /// The project browser.
    Project,
    /// The game started by Play, shown in the editor (stage 33).
    Game,
    /// The assets of the game with previews (stage 55). A drawer by default.
    ContentBrowser,
    /// Another Content Browser (View → New Content Browser), with a place
    /// and filters of its own; closed for good, like another Details.
    ContentExtra {
        id: u64,
    },
    /// The objects of the level in front: zones → classes → objects
    /// (stage 53).
    Outliner,
    /// The zones the level in front loads and what its view shows
    /// (stage 53; the left side of the level tab before).
    Zones,
    /// The operations of the workspace to go back and forth in (stage 54).
    History,
    /// The visual script editor (stage 56; a window before).
    Script,
    /// The movies of the game (stage 56; a window before).
    Movies,
    /// The time line of the cut-scene in the level view (stage 62).
    Timeline,
    /// The picture of the shot camera of that cut-scene (stage 62).
    CutscenePreview,
    /// What "Check edits" and "Check references" found (stage 103).
    Findings,
    /// A name under every hash function, the names of a hash (stage 103).
    Hashes,
    /// The knowledge base and the data types of the game (stage 103).
    Knowledge,
    /// Where the documents go in a layout; never shown.
    Documents,
}

impl Pane {
    /// Id of a pane with state of its own.
    pub fn id(&self) -> Option<u64> {
        match self {
            Self::Stream { id, .. }
            | Self::Resource { id, .. }
            | Self::Search { id, .. }
            | Self::Level { id }
            | Self::Wizard { id, .. }
            | Self::DetailsCopy { id }
            | Self::ContentExtra { id } => Some(*id),
            Self::Logic { id, .. } => Some(*id),
            _ => None,
        }
    }

    /// Tool panels exist once and are hidden instead of closed.
    pub fn is_panel(&self) -> bool {
        matches!(
            self,
            Self::Files
                | Self::Details
                | Self::Changes
                | Self::Log
                | Self::ContentBrowser
                | Self::Outliner
                | Self::Zones
                | Self::History
                | Self::Timeline
                | Self::CutscenePreview
                | Self::DetailsCopy { .. }
                | Self::ContentExtra { .. }
        )
    }

    /// Same pane, ignoring the descriptors that change while it is open.
    fn same(&self, other: &Pane) -> bool {
        match (self.id(), other.id()) {
            (Some(a), Some(b)) => a == b,
            (None, None) => std::mem::discriminant(self) == std::mem::discriminant(other),
            _ => false,
        }
    }
}

/// A change of the dock, made after the dock was drawn.
#[derive(Debug, Clone, PartialEq)]
pub enum DockOp {
    /// Open a document tab next to the other documents and show it.
    Open(Pane),
    /// Show the tab of the pane with this id.
    Focus(u64),
    /// Show a tool panel (or the welcome screen, or all resources).
    ShowPanel(Pane),
    /// Show a tool panel, or hide it when it is shown.
    TogglePanel(Pane),
    Close(TileId),
    CloseOthers(TileId),
    /// Close the document tab used last.
    CloseActive,
    /// Show one tab alone; `None` for the active document; again to restore.
    Maximize(Option<u64>),
    ToggleMaximize(TileId),
    Reset,
    /// A game was opened: the welcome screen gives way to the documents.
    GameOpened,
    /// Arrange the dock as a layout, keeping the documents.
    ApplyLayout(LayoutId),
    /// Save the dock as a user layout.
    SaveLayout(String),
    RenameLayout {
        from: String,
        to: String,
    },
    DeleteLayout(String),
    /// Arrange the dock for a screen (when layouts are remembered).
    Screen(Screen),
    /// Take a panel out of the tree into the drawers.
    MakeDrawer(Pane),
    /// Put a drawer back into the tree, under the documents.
    DockDrawer(Pane),
}

/// The layout of a first start: files left, documents centre, details right,
/// log and changes under the documents.
pub fn default_tree(documents: Vec<Pane>) -> Tree<Pane> {
    let mut tiles = Tiles::default();
    let files = tiles.insert_pane(Pane::Files);
    let left = tiles.insert_tab_tile(vec![files]);
    let documents = if documents.is_empty() {
        vec![Pane::Welcome]
    } else {
        documents
    };
    let docs: Vec<TileId> = documents
        .into_iter()
        .map(|p| tiles.insert_pane(p))
        .collect();
    let center = tiles.insert_tab_tile(docs);
    let log = tiles.insert_pane(Pane::Log);
    let changes = tiles.insert_pane(Pane::Changes);
    let bottom = tiles.insert_tab_tile(vec![log, changes]);
    let middle = tiles.insert_vertical_tile(vec![center, bottom]);
    set_shares(&mut tiles, middle, &[(center, 0.8), (bottom, 0.2)]);
    let details = tiles.insert_pane(Pane::Details);
    let right = tiles.insert_tab_tile(vec![details]);
    let root = tiles.insert_horizontal_tile(vec![left, middle, right]);
    set_shares(
        &mut tiles,
        root,
        &[(left, 0.19), (middle, 0.46), (right, 0.35)],
    );
    Tree::new(egui::Id::unique("dock"), root, tiles)
}

fn set_shares(tiles: &mut Tiles<Pane>, container: TileId, shares: &[(TileId, f32)]) {
    if let Some(Tile::Container(Container::Linear(linear))) = tiles.get_mut(container) {
        for &(child, share) in shares {
            linear.shares.set_share(child, share);
        }
    }
}

/// The tile of the first pane that `matches`.
pub fn find(tree: &Tree<Pane>, matches: impl Fn(&Pane) -> bool) -> Option<TileId> {
    tree.tiles.iter().find_map(|(&id, tile)| match tile {
        Tile::Pane(p) if matches(p) => Some(id),
        _ => None,
    })
}

pub(super) fn pane(tree: &Tree<Pane>, tile: TileId) -> Option<&Pane> {
    match tree.tiles.get(tile)? {
        Tile::Pane(p) => Some(p),
        Tile::Container(_) => None,
    }
}

/// Makes a tile and its ancestors visible and their active tabs.
pub fn focus(tree: &mut Tree<Pane>, tile: TileId) {
    let mut next = Some(tile);
    while let Some(t) = next {
        tree.tiles.set_visible(t, true);
        next = tree.tiles.parent_of(t);
    }
    tree.make_active(|id, _| id == tile);
}

/// Whether a tile is shown: visible, and the active tab wherever it is a tab.
pub fn is_shown(tree: &Tree<Pane>, tile: TileId) -> bool {
    tree.active_tiles().contains(&tile)
}

/// Adds a document tab to the tab group of `near` (or of another document)
/// and shows it.
pub fn add_document(tree: &mut Tree<Pane>, pane: Pane, near: Option<TileId>) -> TileId {
    let tabs_of = |tree: &Tree<Pane>, tile: TileId| {
        tree.tiles
            .parent_of(tile)
            .filter(|&p| matches!(tree.tiles.get(p), Some(Tile::Container(Container::Tabs(_)))))
    };
    let target = near
        .filter(|&t| tree.tiles.get(t).is_some())
        .and_then(|t| tabs_of(tree, t))
        .or_else(|| {
            let doc = find(tree, |p| !p.is_panel())?;
            tabs_of(tree, doc)
        });
    let tile = tree.tiles.insert_pane(pane);
    match target {
        Some(tabs) => {
            if let Some(Tile::Container(container)) = tree.tiles.get_mut(tabs) {
                container.add_child(tile);
            }
        }
        None => {
            let tabs = tree.tiles.insert_tab_tile(vec![tile]);
            match tree.root() {
                None => tree.root = Some(tabs),
                Some(root) => {
                    if let Some(Tile::Container(Container::Linear(linear))) =
                        tree.tiles.get_mut(root)
                    {
                        // Next to the first child, where the file list starts out.
                        let at = 1.min(linear.children.len());
                        linear.children.insert(at, tabs);
                        linear.shares.set_share(tabs, 2.0);
                    } else {
                        let new_root = tree.tiles.insert_horizontal_tile(vec![root, tabs]);
                        tree.root = Some(new_root);
                    }
                }
            }
        }
    }
    focus(tree, tile);
    tile
}

/// Shows a tool panel; one that was removed comes back next to the documents.
pub fn show_panel(tree: &mut Tree<Pane>, pane: Pane, near: Option<TileId>) -> TileId {
    match find(tree, |p| p.same(&pane)) {
        Some(tile) => {
            focus(tree, tile);
            tile
        }
        None => add_document(tree, pane, near),
    }
}

/// Hides a shown tool panel, shows a hidden one.
pub fn toggle_panel(tree: &mut Tree<Pane>, pane: Pane, near: Option<TileId>) {
    match find(tree, |p| p.same(&pane)) {
        Some(tile) if is_shown(tree, tile) => tree.tiles.set_visible(tile, false),
        _ => {
            show_panel(tree, pane, near);
        }
    }
}

/// Document tiles shown now.
pub fn shown_documents(tree: &Tree<Pane>) -> Vec<TileId> {
    tree.active_tiles()
        .into_iter()
        .filter(|&t| pane(tree, t).is_some_and(|p| !p.is_panel()))
        .collect()
}

/// Ids of every pane in the tree.
pub fn pane_ids(tree: &Tree<Pane>) -> HashSet<u64> {
    tree.tiles
        .tiles()
        .filter_map(|t| match t {
            Tile::Pane(p) => p.id(),
            Tile::Container(_) => None,
        })
        .collect()
}

/// The documents of a tree, for a new layout that keeps them.
pub fn documents(tree: &Tree<Pane>) -> Vec<Pane> {
    let mut out = Vec::new();
    for tile in tree.tiles.tiles() {
        if let Tile::Pane(p) = tile
            && !p.is_panel()
            && *p != Pane::Welcome
            && *p != Pane::Documents
        {
            out.push(p.clone());
        }
    }
    out.sort_by_key(|p| p.id().unwrap_or(0));
    out
}

/// `close_warned` for the script editor (pane ids start at 1).
const SCRIPT_WARNED: u64 = 0;

/// What the dock asks of the app while it is drawn.
struct DockBehavior<'a> {
    app: &'a mut BrowserApp,
    /// Tiles shown when the frame started (tabs in front).
    shown: HashSet<TileId>,
}

impl Behavior<Pane> for DockBehavior<'_> {
    fn pane_ui(&mut self, ui: &mut egui::Ui, tile_id: TileId, pane: &mut Pane) -> UiResponse {
        if !pane.is_panel()
            && ui.input(|i| i.pointer.any_pressed())
            && ui.rect_contains_pointer(ui.max_rect())
        {
            self.app.active_doc = Some(tile_id);
            self.app.focus_document(pane);
        }
        let _profile = crate::profile::scope(match pane {
            Pane::Stream { .. } => "pane: stream",
            Pane::Resource { .. } => "pane: resource",
            Pane::Level { .. } => "pane: level",
            Pane::ContentBrowser | Pane::ContentExtra { .. } => "pane: content",
            Pane::Timeline => "pane: timeline",
            Pane::CutscenePreview => "pane: cut-scene preview",
            Pane::Details | Pane::DetailsCopy { .. } => "pane: details",
            Pane::Outliner => "pane: outliner",
            _ => "pane: other",
        });
        self.app.pane_ui(ui, pane);
        UiResponse::None
    }

    fn tab_title_for_pane(&mut self, pane: &Pane) -> egui::WidgetText {
        self.app.pane_title(pane).into()
    }

    fn is_tab_closable(&self, tiles: &Tiles<Pane>, tile_id: TileId) -> bool {
        matches!(tiles.get(tile_id), Some(Tile::Pane(_)))
    }

    fn on_tab_close(&mut self, tiles: &mut Tiles<Pane>, tile_id: TileId) -> bool {
        match tiles.get(tile_id) {
            // Another Details panel or Content Browser goes for good.
            Some(Tile::Pane(Pane::DetailsCopy { .. } | Pane::ContentExtra { .. })) => true,
            Some(Tile::Pane(p)) if p.is_panel() => {
                tiles.set_visible(tile_id, false);
                false
            }
            // The script editor with edits not saved to a file: the first
            // close only warns.
            Some(Tile::Pane(Pane::Script)) => {
                if self.app.script_editor.has_unsaved_edits()
                    && self.app.close_warned != Some(SCRIPT_WARNED)
                {
                    self.app.close_warned = Some(SCRIPT_WARNED);
                    self.app.warn(tr("tab.unsaved").to_owned());
                    false
                } else {
                    true
                }
            }
            // Unsaved edits of a view: the first close only warns.
            Some(Tile::Pane(Pane::Resource { id, .. })) => {
                let unsaved = self
                    .app
                    .docs
                    .get(id)
                    .is_some_and(super::docs::ResourceDoc::has_unsaved_edits);
                if unsaved && self.app.close_warned != Some(*id) {
                    self.app.close_warned = Some(*id);
                    self.app.warn(tr("tab.unsaved").to_owned());
                    false
                } else {
                    true
                }
            }
            _ => true,
        }
    }

    fn on_tab_button(
        &mut self,
        tiles: &mut Tiles<Pane>,
        tile_id: TileId,
        response: egui::Response,
    ) -> egui::Response {
        if response.double_clicked() {
            self.app.dock_ops.push(DockOp::ToggleMaximize(tile_id));
        }
        let pane = match tiles.get(tile_id) {
            Some(Tile::Pane(p)) => Some(p.clone()),
            _ => None,
        };
        // A click on a document's tab arranges the dock for its screen.
        if response.clicked()
            && let Some(screen) = pane.as_ref().and_then(Screen::of)
        {
            self.app.dock_ops.push(DockOp::Screen(screen));
        }
        // ... and Details follow its editor.
        if response.clicked()
            && let Some(p) = &pane
        {
            self.app.focus_document(p);
        }
        // A resource tab: the commands of its asset.
        let asset = match &pane {
            Some(Pane::Resource { id, .. }) => self.app.docs.get(id).map(|d| (d.loc, d.type_id)),
            _ => None,
        };
        // The tab in front of a document group: a line of the accent on top.
        if self.shown.contains(&tile_id) && pane.as_ref().is_some_and(|p| !p.is_panel()) {
            let rect = response.rect;
            response.ctx.layer_painter(response.layer_id).hline(
                rect.x_range(),
                rect.top() + 1.0,
                egui::Stroke::new(2.0, super::theme::ACCENT),
            );
        }
        let closable = pane.is_some();
        response.context_menu(|ui| {
            if closable && ui.button(tr("tab.close")).clicked() {
                self.app.dock_ops.push(DockOp::Close(tile_id));
            }
            if ui.button(tr("tab.close_others")).clicked() {
                self.app.dock_ops.push(DockOp::CloseOthers(tile_id));
            }
            let maximize = if self.app.maximized == Some(tile_id) {
                tr("tab.restore")
            } else {
                tr("tab.maximize")
            };
            if ui.button(maximize).clicked() {
                self.app.dock_ops.push(DockOp::ToggleMaximize(tile_id));
            }
            if let Some((loc, type_id)) = asset {
                ui.separator();
                let mut cmds = Vec::new();
                super::asset_tab::asset_menu(ui, true, type_id, &mut cmds);
                self.app
                    .asset_cmds
                    .extend(cmds.into_iter().map(|c| (loc, c)));
            }
            if let Some(pane) = pane.as_ref().filter(|p| p.is_panel()) {
                ui.separator();
                if ui
                    .button(tr("tab.make_drawer"))
                    .on_hover_text(tr("tab.make_drawer_hint"))
                    .clicked()
                {
                    self.app.dock_ops.push(DockOp::MakeDrawer(pane.clone()));
                }
            }
            ui.separator();
            if ui.button(super::Command::SaveLayout.label()).clicked() {
                self.app.pending.push(super::Command::SaveLayout);
            }
        });
        response
    }

    fn tab_bar_color(&self, visuals: &egui::Visuals) -> egui::Color32 {
        super::theme::tab_bar_fill(visuals)
    }

    fn tab_outline_stroke(
        &self,
        _visuals: &egui::Visuals,
        _tiles: &Tiles<Pane>,
        _tile_id: TileId,
        _state: &egui_tiles::TabState,
    ) -> egui::Stroke {
        egui::Stroke::NONE
    }

    fn tab_title_spacing(&self, _visuals: &egui::Visuals) -> f32 {
        10.0
    }

    fn simplification_options(&self) -> SimplificationOptions {
        SimplificationOptions {
            all_panes_must_have_tabs: true,
            ..SimplificationOptions::default()
        }
    }

    fn preview_dragged_panes(&self) -> bool {
        // A pane is drawn once a frame: the level and model views render to
        // the GPU and must not be drawn twice.
        false
    }

    fn tab_bar_height(&self, _style: &egui::Style) -> f32 {
        24.0
    }

    fn gap_width(&self, _style: &egui::Style) -> f32 {
        2.0
    }
}

impl BrowserApp {
    /// Draws the dock, or the maximized tab alone.
    pub(super) fn dock_ui(&mut self, ui: &mut egui::Ui) {
        let Some(mut tree) = self.dock.take() else {
            return;
        };
        let maximized = self
            .maximized
            .filter(|&t| matches!(tree.tiles.get(t), Some(Tile::Pane(_))));
        self.maximized = maximized;
        match maximized {
            Some(tile) => {
                let title = pane(&tree, tile)
                    .map(|p| self.pane_title(p))
                    .unwrap_or_default();
                ui.horizontal(|ui| {
                    ui.strong(title);
                    if ui
                        .small_button(tr("tab.restore"))
                        .on_hover_text(tr("tab.restore_hint"))
                        .clicked()
                    {
                        self.maximized = None;
                    }
                });
                ui.separator();
                let id = tile.egui_id(tree.id());
                if let Some(Tile::Pane(pane)) = tree.tiles.get_mut(tile) {
                    ui.push_id(id, |ui| self.pane_ui(ui, pane));
                }
            }
            None => {
                let shown = tree.active_tiles().into_iter().collect();
                let mut behavior = DockBehavior { app: self, shown };
                tree.ui(&mut behavior, ui);
            }
        }
        self.dock = Some(tree);
    }

    /// Applies the dock changes asked for during the frame and forgets the
    /// state of closed tabs.
    pub(super) fn apply_dock_ops(&mut self) {
        let Some(mut tree) = self.dock.take() else {
            return;
        };
        for op in std::mem::take(&mut self.dock_ops) {
            self.apply_dock_op(&mut tree, op);
        }
        // Forget the tabs that were closed.
        let ids = pane_ids(&tree);
        self.streams.retain(|id, _| ids.contains(id));
        self.docs.retain(|id, _| ids.contains(id));
        self.searches.retain(|id, _| ids.contains(id));
        self.wizards.retain(|id| ids.contains(&id));
        let wizard_levels = self.wizards.levels();
        self.levels
            .retain(|id, _| ids.contains(id) || wizard_levels.contains(id));
        self.logics.retain(|id, _| ids.contains(id));
        self.content.retain(|id| ids.contains(&id));
        // The script editor and Movies live as long as their tabs.
        let has = |pane: &Pane| find(&tree, |p| p == pane).is_some();
        self.script_editor.open = has(&Pane::Script);
        if self.movies.open && !has(&Pane::Movies) {
            self.movies.close();
        }
        self.inspectors.copies.retain(|id, _| ids.contains(id));
        let levels = &self.levels;
        self.inspectors
            .embedded
            .retain(|id, _| levels.contains_key(id));
        self.outliners.retain(|id, _| levels.contains_key(id));
        if self.live_panes != ids {
            self.live_panes = ids;
            self.prune_files();
        }
        if self.active_doc.is_some_and(|t| tree.tiles.get(t).is_none()) {
            self.active_doc = None;
        }
        self.dock = Some(tree);
    }

    fn apply_dock_op(&mut self, tree: &mut Tree<Pane>, op: DockOp) {
        match op {
            // The script editor, Movies and the knowledge base are one tab each.
            DockOp::Open(pane @ (Pane::Script | Pane::Movies | Pane::Knowledge))
                if find(tree, |p| *p == pane).is_some() =>
            {
                if let Some(screen) = Screen::of(&pane) {
                    self.switch_screen(tree, screen);
                }
                if let Some(tile) = find(tree, |p| *p == pane) {
                    focus(tree, tile);
                    self.active_doc = Some(tile);
                    if self.maximized.is_some() {
                        self.maximized = Some(tile);
                    }
                }
                self.focus_document(&pane);
            }
            DockOp::Open(pane) => {
                self.focus_document(&pane);
                let was_empty = documents(tree).is_empty();
                let switched = Screen::of(&pane).is_some_and(|s| self.switch_screen(tree, s));
                let tile = add_document(tree, pane, self.active_doc);
                // The welcome screen that stood in for no documents goes.
                if switched
                    && was_empty
                    && let Some(welcome) = find(tree, |p| *p == Pane::Welcome)
                {
                    close(tree, welcome);
                }
                self.active_doc = Some(tile);
                if self.maximized.is_some() || std::mem::take(&mut self.maximize_pinned) {
                    self.maximized = Some(tile);
                }
            }
            DockOp::Focus(id) => {
                if let Some(screen) = find(tree, |p| p.id() == Some(id))
                    .and_then(|t| pane(tree, t))
                    .and_then(Screen::of)
                {
                    self.switch_screen(tree, screen);
                }
                if let Some(tile) = find(tree, |p| p.id() == Some(id)) {
                    focus(tree, tile);
                    self.active_doc = Some(tile);
                    if self.maximized.is_some() {
                        self.maximized = Some(tile);
                    }
                    if let Some(p) = pane(tree, tile).cloned() {
                        self.focus_document(&p);
                    }
                }
            }
            DockOp::ShowPanel(pane) if self.drawers.contains(&pane) => {
                self.drawers.open = Some(pane)
            }
            DockOp::ShowPanel(pane) => {
                let tile = show_panel(tree, pane, self.active_doc);
                if self.maximized.is_some() && self.maximized != Some(tile) {
                    self.maximized = None;
                }
            }
            DockOp::TogglePanel(pane) if self.drawers.contains(&pane) => self.drawers.toggle(&pane),
            DockOp::TogglePanel(pane) => {
                toggle_panel(tree, pane, self.active_doc);
                self.maximized = None;
            }
            DockOp::MakeDrawer(pane) => {
                if let Some(tile) = find(tree, |p| *p == pane) {
                    tree.remove_recursively(tile);
                    if self.maximized == Some(tile) {
                        self.maximized = None;
                    }
                }
                self.drawers.add(pane.clone());
                self.drawers.open = Some(pane);
            }
            DockOp::DockDrawer(pane) => {
                self.drawers.remove(&pane);
                if find(tree, |p| *p == pane).is_none() {
                    dock_below(tree, pane);
                }
            }
            op @ (DockOp::ApplyLayout(_)
            | DockOp::SaveLayout(_)
            | DockOp::RenameLayout { .. }
            | DockOp::DeleteLayout(_)
            | DockOp::Screen(_)) => self.apply_layout_op(tree, op),
            DockOp::Close(tile) => close(tree, tile),
            DockOp::CloseOthers(tile) => {
                if let Some(parent) = tree.tiles.parent_of(tile)
                    && let Some(Tile::Container(container)) = tree.tiles.get(parent)
                {
                    let others: Vec<TileId> = container
                        .children()
                        .copied()
                        .filter(|&c| c != tile)
                        .collect();
                    for other in others {
                        close(tree, other);
                    }
                }
            }
            DockOp::CloseActive => {
                let shown = shown_documents(tree);
                let tile = self
                    .maximized
                    .or(self.active_doc.filter(|t| shown.contains(t)))
                    .or_else(|| shown.first().copied());
                if let Some(tile) = tile {
                    close(tree, tile);
                }
            }
            DockOp::Maximize(id) => {
                let tile = match id {
                    Some(id) => find(tree, |p| p.id() == Some(id)),
                    None => self
                        .active_doc
                        .or_else(|| shown_documents(tree).first().copied()),
                };
                self.maximized = tile;
            }
            DockOp::ToggleMaximize(tile) => {
                self.maximized = if self.maximized == Some(tile) {
                    None
                } else {
                    Some(tile)
                };
            }
            DockOp::Reset => {
                // Back to the built-in layout of the screen.
                let builtin = match &self.layouts.current {
                    LayoutId::Builtin(b) => *b,
                    LayoutId::User(_) => self.layouts.screen.builtin(),
                };
                self.apply_layout(tree, LayoutId::Builtin(builtin));
            }
            DockOp::GameOpened => {
                // The welcome screen steps back once there is something else.
                if let Some(welcome) = find(tree, |p| *p == Pane::Welcome)
                    && find(tree, |p| !p.is_panel() && *p != Pane::Welcome).is_some()
                {
                    close(tree, welcome);
                }
            }
        }
    }

    /// Writes the current stream path, resource key and query into the panes,
    /// for the saved layout.
    pub(super) fn sync_descriptors(&self, tree: &mut Tree<Pane>) {
        let Some(game) = &self.game else { return };
        for tile in tree.tiles.tiles_mut() {
            let Tile::Pane(pane) = tile else { continue };
            match pane {
                Pane::Stream { id, path } => {
                    if let Some(tab) = self.streams.get(id)
                        && let Some(p) = tab.path(game).or_else(|| tab.restore.clone())
                    {
                        *path = Some(p);
                    }
                }
                Pane::Search { id, query } => {
                    if let Some(search) = self.searches.get(id) {
                        query.clone_from(&search.query);
                    }
                }
                Pane::Resource { id, key } => {
                    if let Some(doc) = self.docs.get(id)
                        && let Some(k) = self.key_of(doc.loc)
                    {
                        *key = k.to_string();
                    }
                }
                _ => {}
            }
        }
    }

    /// Title of a tab.
    pub(super) fn pane_title(&self, pane: &Pane) -> String {
        match pane {
            Pane::Welcome => tr("pane.welcome").to_owned(),
            Pane::Files => tr("pane.files").to_owned(),
            Pane::Details => tr("pane.details").to_owned(),
            Pane::Changes => match &self.ws_status {
                Some(s) if !s.changes.is_empty() => {
                    format!("{} ({})", tr("pane.changes"), s.changes.len())
                }
                _ => tr("pane.changes").to_owned(),
            },
            Pane::Log => self.notes.title(),
            Pane::Index => tr("pane.index").to_owned(),
            Pane::Stream { id, path } => {
                let path = self
                    .game
                    .as_ref()
                    .and_then(|g| self.streams.get(id).and_then(|t| t.path(g)))
                    .or_else(|| path.clone());
                match path {
                    Some(p) => p.rsplit('/').next().unwrap_or(&p).to_owned(),
                    None => tr("pane.stream").to_owned(),
                }
            }
            Pane::Resource { id, key } => match self.docs.get(id) {
                Some(doc) => self.doc_title(doc),
                None => key.rsplit(':').next().unwrap_or(key).to_owned(),
            },
            Pane::Search { id, query } => match self.searches.get(id) {
                Some(search) => search.title(),
                None => format!("{}: {query}", tr("pane.search")),
            },
            Pane::Level { .. } => tr("pane.level").to_owned(),
            Pane::Tasks => tr("pane.tasks").to_owned(),
            Pane::Project => tr("project.tab").to_owned(),
            Pane::Game => tr("gametab.title").to_owned(),
            Pane::ContentBrowser => tr("pane.content").to_owned(),
            Pane::ContentExtra { id } => {
                let mut extra: Vec<u64> = self.content.ids().filter(|&i| i != 0).collect();
                extra.sort_unstable();
                let n = extra.iter().position(|c| c == id).unwrap_or(0) + 2;
                format!("{} {n}", tr("pane.content"))
            }
            Pane::Outliner => tr("pane.outliner").to_owned(),
            Pane::Zones => tr("pane.zones").to_owned(),
            Pane::History => tr("pane.history").to_owned(),
            Pane::Timeline => tr("pane.timeline").to_owned(),
            Pane::CutscenePreview => tr("pane.cutscene_preview").to_owned(),
            Pane::Script => {
                let mark = if self.script_editor.has_unsaved_edits() {
                    " *"
                } else {
                    ""
                };
                format!("{}{mark}", tr("script.title"))
            }
            Pane::Movies => tr("movies.title").to_owned(),
            Pane::Findings => match self.findings.errors() {
                0 => tr("pane.findings").to_owned(),
                n => format!("{} ({n})", tr("pane.findings")),
            },
            Pane::Hashes => tr("pane.hashes").to_owned(),
            Pane::Knowledge => tr("pane.knowledge").to_owned(),

            Pane::DetailsCopy { id } => {
                let mut copies: Vec<u64> = self.inspectors.copies.keys().copied().collect();
                copies.sort_unstable();
                let n = copies.iter().position(|c| c == id).unwrap_or(0) + 2;
                format!("{} {n}", tr("pane.details"))
            }
            Pane::Documents => String::new(),
            Pane::Wizard { kind, .. } => kind.title().to_owned(),
            Pane::Logic { id, .. } => self
                .logics
                .get(id)
                .map_or_else(|| tr("logic.tab").to_owned(), |l| l.title()),
        }
    }
}

/// Puts a pane into the tree under everything, as a band across the window
/// (where a drawer sits).
pub fn dock_below(tree: &mut Tree<Pane>, pane: Pane) -> TileId {
    let tile = tree.tiles.insert_pane(pane);
    let tabs = tree.tiles.insert_tab_tile(vec![tile]);
    match tree.root() {
        None => tree.root = Some(tabs),
        Some(root) => {
            let column = tree.tiles.insert_vertical_tile(vec![root, tabs]);
            set_shares(&mut tree.tiles, column, &[(root, 0.72), (tabs, 0.28)]);
            tree.root = Some(column);
        }
    }
    focus(tree, tile);
    tile
}

/// Removes a tab; a tool panel is hidden instead.
fn close(tree: &mut Tree<Pane>, tile: TileId) {
    match tree.tiles.get(tile) {
        Some(Tile::Pane(Pane::DetailsCopy { .. } | Pane::ContentExtra { .. })) => {
            tree.remove_recursively(tile);
        }
        Some(Tile::Pane(p)) if p.is_panel() => tree.tiles.set_visible(tile, false),
        Some(Tile::Pane(_)) => {
            tree.remove_recursively(tile);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panes(tree: &Tree<Pane>) -> Vec<Pane> {
        let mut out: Vec<Pane> = tree
            .tiles
            .tiles()
            .filter_map(|t| match t {
                Tile::Pane(p) => Some(p.clone()),
                Tile::Container(_) => None,
            })
            .collect();
        out.sort_by_key(|p| format!("{p:?}"));
        out
    }

    #[test]
    fn default_layout_has_every_panel() {
        let tree = default_tree(Vec::new());
        for pane in [
            Pane::Files,
            Pane::Details,
            Pane::Changes,
            Pane::Log,
            Pane::Welcome,
        ] {
            assert!(find(&tree, |p| *p == pane).is_some(), "{pane:?}");
        }
    }

    #[test]
    fn layout_round_trip_restores_tabs() {
        let mut tree = default_tree(Vec::new());
        let stream = add_document(
            &mut tree,
            Pane::Stream {
                id: 3,
                path: Some("gf2/frontend.str".into()),
            },
            None,
        );
        add_document(
            &mut tree,
            Pane::Search {
                id: 4,
                query: "type:mesh car".into(),
            },
            Some(stream),
        );
        add_document(
            &mut tree,
            Pane::Resource {
                id: 7,
                key: "gf2/frontend:apx:00000000.00000000.00000000.12345678".into(),
            },
            Some(stream),
        );
        toggle_panel(&mut tree, Pane::Details, None);
        let layouts = super::super::layouts::Layouts::default();
        let json = super::super::layouts::to_json(&tree, 7, &[], &layouts);
        let loaded = super::super::layouts::parse(&json).expect("parses");
        let restored = loaded.tree;
        assert_eq!(loaded.next_pane, 7);
        assert_eq!(panes(&restored), panes(&tree));
        let details = find(&restored, |p| *p == Pane::Details).unwrap();
        assert!(
            !restored.tiles.is_visible(details),
            "hidden panel stays hidden"
        );
        // The new documents share the tab group of the welcome screen.
        let welcome = find(&restored, |p| *p == Pane::Welcome).unwrap();
        let search = find(&restored, |p| p.id() == Some(4)).unwrap();
        assert_eq!(
            restored.tiles.parent_of(welcome),
            restored.tiles.parent_of(search)
        );
    }

    #[test]
    fn a_panel_docked_below_spans_the_window() {
        let mut tree = default_tree(Vec::new());
        let root = tree.root().unwrap();
        let tile = dock_below(&mut tree, Pane::ContentBrowser);
        let column = tree.root().unwrap();
        assert_ne!(column, root);
        let tabs = tree.tiles.parent_of(tile).unwrap();
        assert_eq!(tree.tiles.parent_of(tabs), Some(column));
        assert!(is_shown(&tree, tile));
    }

    #[test]
    fn panels_toggle_and_documents_survive_a_reset() {
        let mut tree = default_tree(Vec::new());
        let files = find(&tree, |p| *p == Pane::Files).unwrap();
        toggle_panel(&mut tree, Pane::Files, None);
        assert!(!tree.tiles.is_visible(files));
        toggle_panel(&mut tree, Pane::Files, None);
        assert!(tree.tiles.is_visible(files));
        add_document(&mut tree, Pane::Level { id: 9 }, None);
        let reset = default_tree(documents(&tree));
        assert!(find(&reset, |p| p.id() == Some(9)).is_some());
        assert!(find(&reset, |p| *p == Pane::Welcome).is_none());
    }

    #[test]
    fn documents_open_even_without_a_document_group() {
        let mut tree = default_tree(Vec::new());
        let welcome = find(&tree, |p| *p == Pane::Welcome).unwrap();
        close(&mut tree, welcome);
        tree.simplify(&SimplificationOptions::default());
        let tile = add_document(&mut tree, Pane::Index, None);
        assert!(tree.tiles.get(tile).is_some());
        assert!(tree.tiles.parent_of(tile).is_some());
    }
}

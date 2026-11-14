//! Layouts of the dock (stage 52): the built-in ones — "Level Editor"
//! (the view large in the middle, Outliner over Details on the right, the
//! Content Browser, Log and Changes as drawers) and "Resources" (files left,
//! documents in the middle, details right, log and changes below) — the
//! user's own named layouts, and the layout remembered for each kind of
//! screen.
//!
//! A layout is an [`Arrangement`]: a tree of tiles whose documents are
//! replaced by one placeholder ([`Pane::Documents`]), and the panels shown
//! as drawers. Applying a layout keeps the open documents: they take the
//! place of the placeholder. Every layout stays a starting point only: each
//! panel can still be dragged, split, closed or made a drawer, and the dock
//! as it is goes into the layout file.
//!
//! The file (`layout.json`) is versioned; versions 1 (the dock alone, up to
//! stage 51), 2 (stage 52, with the object panel `ObjectDetails` and the
//! zones in the Outliner), 3 (stage 53, without the History panel) and 4
//! (stage 54, without the screen of asset editors) are read and migrated:
//! see [`parse`].
//!
//! Stage 56: "Asset Editor" — the screen of the asset editors (a resource
//! in its tab, the logic graph, the script editor, Movies): the editor large
//! with Details on the right, the rest as drawers.

use std::collections::BTreeMap;

use egui_tiles::{Container, SimplificationOptions, Tile, TileId, Tiles, Tree};
use serde::{Deserialize, Serialize};

use doctor_toolkit_mods::i18n::{tr, trf};

use super::dock::{self, DockOp, Pane};
use super::{BrowserApp, Command};

/// Version of the layout file written now.
pub const LAYOUT_VERSION: u32 = 5;

/// A kind of screen, by the document in front: each remembers its layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Screen {
    Resources,
    Level,
    /// An asset editor in front (stage 56).
    Assets,
}

impl Screen {
    /// The screen of a document; `None` for panes that keep the layout
    /// (panels, the welcome screen, wizards, the game).
    pub fn of(pane: &Pane) -> Option<Screen> {
        match pane {
            Pane::Level { .. } => Some(Self::Level),
            Pane::Index | Pane::Stream { .. } | Pane::Search { .. } => Some(Self::Resources),
            Pane::Resource { .. } | Pane::Logic { .. } | Pane::Script | Pane::Movies => {
                Some(Self::Assets)
            }
            _ => None,
        }
    }

    /// The built-in layout a screen starts with.
    pub fn builtin(self) -> Builtin {
        match self {
            Self::Resources => Builtin::Resources,
            Self::Level => Builtin::Level,
            Self::Assets => Builtin::Assets,
        }
    }
}

/// The layouts that come with the editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Builtin {
    Resources,
    Level,
    /// The asset editors (stage 56).
    Assets,
}

impl Builtin {
    pub const ALL: [Builtin; 3] = [Builtin::Level, Builtin::Assets, Builtin::Resources];

    pub fn name(self) -> &'static str {
        match self {
            Self::Resources => tr("layout.resources"),
            Self::Level => tr("layout.level"),
            Self::Assets => tr("layout.assets"),
        }
    }

    pub fn arrangement(self) -> Arrangement {
        match self {
            Self::Resources => Arrangement {
                tree: dock::default_tree(vec![Pane::Documents]),
                drawers: vec![Pane::ContentBrowser],
            },
            Self::Level => level_arrangement(),
            Self::Assets => assets_arrangement(),
        }
    }
}

/// A layout: built in or the user's, by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LayoutId {
    Builtin(Builtin),
    User(String),
}

impl Default for LayoutId {
    fn default() -> Self {
        Self::Builtin(Builtin::Resources)
    }
}

impl LayoutId {
    pub fn name(&self) -> String {
        match self {
            Self::Builtin(b) => b.name().to_owned(),
            Self::User(name) => name.clone(),
        }
    }
}

/// Tiles with a placeholder for the documents, and the drawers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Arrangement {
    pub tree: Tree<Pane>,
    #[serde(default)]
    pub drawers: Vec<Pane>,
}

/// A layout of the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Named {
    pub name: String,
    pub arrangement: Arrangement,
}

/// The layout a screen had when it was left.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Remembered {
    pub layout: LayoutId,
    pub arrangement: Arrangement,
}

/// Everything about layouts besides the dock itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Layouts {
    /// The layout the dock started from.
    pub current: LayoutId,
    /// The screen the dock is arranged for.
    pub screen: Screen,
    /// Switch to the screen's own layout when a document of another kind
    /// of screen is opened or its tab is clicked.
    pub remember: bool,
    pub screens: BTreeMap<Screen, Remembered>,
    pub user: Vec<Named>,
}

impl Default for Layouts {
    fn default() -> Self {
        Self {
            current: LayoutId::default(),
            screen: Screen::Resources,
            remember: true,
            screens: BTreeMap::new(),
            user: Vec::new(),
        }
    }
}

impl Layouts {
    /// The arrangement of a layout; `None` for a user layout that is gone.
    pub fn arrangement(&self, id: &LayoutId) -> Option<Arrangement> {
        match id {
            LayoutId::Builtin(b) => Some(b.arrangement()),
            LayoutId::User(name) => self
                .user
                .iter()
                .find(|n| n.name == *name)
                .map(|n| n.arrangement.clone()),
        }
    }

    /// Saves (or replaces) a user layout.
    pub fn save(&mut self, name: &str, arrangement: Arrangement) {
        match self.user.iter_mut().find(|n| n.name == name) {
            Some(named) => named.arrangement = arrangement,
            None => self.user.push(Named {
                name: name.to_owned(),
                arrangement,
            }),
        }
        self.current = LayoutId::User(name.to_owned());
    }

    /// Renames a user layout; false when the name is empty or taken.
    pub fn rename(&mut self, from: &str, to: &str) -> bool {
        let to = to.trim();
        if to.is_empty() || self.user.iter().any(|n| n.name == to) {
            return false;
        }
        let Some(named) = self.user.iter_mut().find(|n| n.name == from) else {
            return false;
        };
        to.clone_into(&mut named.name);
        let renamed = LayoutId::User(to.to_owned());
        let old = LayoutId::User(from.to_owned());
        if self.current == old {
            self.current = renamed.clone();
        }
        for remembered in self.screens.values_mut() {
            if remembered.layout == old {
                remembered.layout = renamed.clone();
            }
        }
        true
    }

    /// Deletes a user layout; the dock stays as it is.
    pub fn delete(&mut self, name: &str) -> bool {
        let before = self.user.len();
        self.user.retain(|n| n.name != name);
        let old = LayoutId::User(name.to_owned());
        for (screen, remembered) in &mut self.screens {
            if remembered.layout == old {
                remembered.layout = LayoutId::Builtin(screen.builtin());
            }
        }
        if self.current == old {
            self.current = LayoutId::Builtin(self.screen.builtin());
        }
        self.user.len() != before
    }
}

const SIMPLIFY: SimplificationOptions = SimplificationOptions {
    prune_empty_tabs: true,
    prune_single_child_tabs: true,
    prune_empty_containers: true,
    prune_single_child_containers: true,
    all_panes_must_have_tabs: true,
    join_nested_linear_containers: true,
    flatten_tabs_in_tabs: false,
};

fn set_shares(tiles: &mut Tiles<Pane>, container: TileId, shares: &[(TileId, f32)]) {
    if let Some(Tile::Container(Container::Linear(linear))) = tiles.get_mut(container) {
        for &(child, share) in shares {
            linear.shares.set_share(child, share);
        }
    }
}

/// "Level Editor": the documents (the level view) large in the middle,
/// Outliner (with Zones and Files behind it) over Details on the right; the
/// Content Browser, the Log and Changes are drawers.
fn level_arrangement() -> Arrangement {
    let mut tiles = Tiles::default();
    let docs = tiles.insert_pane(Pane::Documents);
    let center = tiles.insert_tab_tile(vec![docs]);
    let outliner = tiles.insert_pane(Pane::Outliner);
    let zones = tiles.insert_pane(Pane::Zones);
    let files = tiles.insert_pane(Pane::Files);
    let top = tiles.insert_tab_tile(vec![outliner, zones, files]);
    let details = tiles.insert_pane(Pane::Details);
    let bottom = tiles.insert_tab_tile(vec![details]);
    let right = tiles.insert_vertical_tile(vec![top, bottom]);
    set_shares(&mut tiles, right, &[(top, 0.42), (bottom, 0.58)]);
    let root = tiles.insert_horizontal_tile(vec![center, right]);
    set_shares(&mut tiles, root, &[(center, 0.75), (right, 0.25)]);
    Arrangement {
        tree: Tree::new(egui::Id::unique("dock"), root, tiles),
        drawers: vec![
            Pane::ContentBrowser,
            Pane::Log,
            Pane::Changes,
            Pane::History,
        ],
    }
}

/// "Asset Editor" (stage 56): the editor large, Details on the right over
/// the whole height; the Content Browser, Files, the Log, Changes and
/// History are drawers.
fn assets_arrangement() -> Arrangement {
    let mut tiles = Tiles::default();
    let docs = tiles.insert_pane(Pane::Documents);
    let center = tiles.insert_tab_tile(vec![docs]);
    let details = tiles.insert_pane(Pane::Details);
    let right = tiles.insert_tab_tile(vec![details]);
    let root = tiles.insert_horizontal_tile(vec![center, right]);
    set_shares(&mut tiles, root, &[(center, 0.74), (right, 0.26)]);
    Arrangement {
        tree: Tree::new(egui::Id::unique("dock"), root, tiles),
        drawers: vec![
            Pane::ContentBrowser,
            Pane::Files,
            Pane::Log,
            Pane::Changes,
            Pane::History,
        ],
    }
}

/// The dock of an arrangement with these documents (the welcome screen
/// when there are none) in place of its placeholder.
pub fn arrange(arrangement: &Arrangement, documents: Vec<Pane>) -> (Tree<Pane>, Vec<Pane>) {
    let mut tree = arrangement.tree.clone();
    let documents = if documents.is_empty() {
        vec![Pane::Welcome]
    } else {
        documents
    };
    // Panels that are drawers are not in the tree, documents are placed anew.
    let drawers: Vec<Pane> = arrangement
        .drawers
        .iter()
        .filter(|p| p.is_panel())
        .cloned()
        .collect();
    let stale: Vec<TileId> = tree
        .tiles
        .iter()
        .filter_map(|(&id, tile)| match tile {
            Tile::Pane(p) if !p.is_panel() && *p != Pane::Documents => Some(id),
            Tile::Pane(p) if drawers.contains(p) => Some(id),
            _ => None,
        })
        .collect();
    for id in stale {
        tree.remove_recursively(id);
    }
    let placeholder = dock::find(&tree, |p| *p == Pane::Documents);
    let group = placeholder.and_then(|t| tree.tiles.parent_of(t));
    match (placeholder, group) {
        (Some(placeholder), Some(group))
            if matches!(
                tree.tiles.get(group),
                Some(Tile::Container(Container::Tabs(_)))
            ) =>
        {
            let ids: Vec<TileId> = documents
                .into_iter()
                .map(|p| tree.tiles.insert_pane(p))
                .collect();
            if let Some(Tile::Container(container)) = tree.tiles.get_mut(group) {
                for &id in &ids {
                    container.add_child(id);
                }
            }
            tree.remove_recursively(placeholder);
            if let Some(&first) = ids.first() {
                dock::focus(&mut tree, first);
            }
        }
        (placeholder, _) => {
            if let Some(placeholder) = placeholder {
                tree.remove_recursively(placeholder);
            }
            for pane in documents {
                dock::add_document(&mut tree, pane, None);
            }
        }
    }
    tree.simplify(&SIMPLIFY);
    (tree, drawers)
}

/// The arrangement of a dock: its documents give way to the placeholder,
/// in the tab group of `active` (or of the first document).
pub fn template_of(tree: &Tree<Pane>, drawers: &[Pane], active: Option<TileId>) -> Arrangement {
    let mut tree = tree.clone();
    let is_document = |tree: &Tree<Pane>, t: TileId| matches!(tree.tiles.get(t), Some(Tile::Pane(p)) if !p.is_panel());
    let anchor = active
        .filter(|&t| is_document(&tree, t))
        .or_else(|| dock::find(&tree, |p| !p.is_panel()));
    let group = anchor.and_then(|t| tree.tiles.parent_of(t));
    let documents: Vec<TileId> = tree
        .tiles
        .iter()
        .filter_map(|(&id, tile)| match tile {
            Tile::Pane(p) if !p.is_panel() => Some(id),
            _ => None,
        })
        .collect();
    let placeholder = tree.tiles.insert_pane(Pane::Documents);
    match group.filter(|&g| tree.tiles.get(g).is_some()) {
        Some(group) => {
            if let Some(Tile::Container(container)) = tree.tiles.get_mut(group) {
                container.add_child(placeholder);
            }
            for id in documents {
                tree.remove_recursively(id);
            }
        }
        None => {
            for id in documents {
                tree.remove_recursively(id);
            }
            tree.tiles.remove(placeholder);
            dock::add_document(&mut tree, Pane::Documents, None);
        }
    }
    tree.simplify(&SIMPLIFY);
    Arrangement {
        tree,
        drawers: drawers.to_vec(),
    }
}

/// What the layout file holds.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub tree: Tree<Pane>,
    pub next_pane: u64,
    pub drawers: Vec<Pane>,
    pub layouts: Layouts,
}

impl Loaded {
    /// A first start: the "Resources" layout.
    pub fn first_start() -> Self {
        let (tree, drawers) = arrange(&Builtin::Resources.arrangement(), Vec::new());
        Self {
            tree,
            next_pane: 0,
            drawers,
            layouts: Layouts::default(),
        }
    }
}

#[derive(Deserialize)]
struct Version {
    version: u32,
}

/// Version 1: the dock alone.
#[derive(Deserialize)]
struct FileV1 {
    next_pane: u64,
    tree: Tree<Pane>,
}

#[derive(Serialize, Deserialize)]
struct FileV2 {
    version: u32,
    next_pane: u64,
    tree: Tree<Pane>,
    #[serde(default)]
    drawers: Vec<Pane>,
    #[serde(default)]
    layouts: Layouts,
}

/// Reads a layout file; `None` when it is broken or of an unknown version.
///
/// Version 2 (stage 52) had the object panel `ObjectDetails` next to the
/// resource panel `Details`, and the zones in the Outliner. Both panels now
/// read as the one Details ([`Pane::Details`] takes the old name too): the
/// second of them goes, wherever it was, in the dock, the drawers, the
/// user's layouts and the remembered ones. And where an Outliner has no
/// Zones panel anywhere, Zones joins it as a tab behind it, so that zones
/// can still be loaded.
pub fn parse(text: &str) -> Option<Loaded> {
    let version: Version = serde_json::from_str(text).ok()?;
    let mut loaded = match version.version {
        1 => {
            let file: FileV1 = serde_json::from_str(text).ok()?;
            Loaded {
                tree: file.tree,
                next_pane: file.next_pane,
                drawers: Vec::new(),
                layouts: Layouts::default(),
            }
        }
        2..=LAYOUT_VERSION => {
            let file: FileV2 = serde_json::from_str(text).ok()?;
            Loaded {
                tree: file.tree,
                next_pane: file.next_pane,
                drawers: file.drawers,
                layouts: file.layouts,
            }
        }
        _ => return None,
    };
    loaded.tree.root()?;
    if version.version < 3 {
        migrate(&mut loaded.tree, &mut loaded.drawers);
        for named in &mut loaded.layouts.user {
            migrate(&mut named.arrangement.tree, &mut named.arrangement.drawers);
        }
        for remembered in loaded.layouts.screens.values_mut() {
            migrate(
                &mut remembered.arrangement.tree,
                &mut remembered.arrangement.drawers,
            );
        }
    }
    if version.version < 4 {
        add_history(&loaded.tree, &mut loaded.drawers);
        for named in &mut loaded.layouts.user {
            add_history(&named.arrangement.tree, &mut named.arrangement.drawers);
        }
        for remembered in loaded.layouts.screens.values_mut() {
            add_history(
                &remembered.arrangement.tree,
                &mut remembered.arrangement.drawers,
            );
        }
    }
    if version.version < 5 {
        seed_assets(&mut loaded.layouts);
    }
    // A pane is a drawer or in the tree, never both; placeholders never show.
    loaded.drawers.retain(|p| p.is_panel());
    let mut seen = Vec::new();
    loaded.drawers.retain(|p| {
        let first = !seen.contains(p);
        seen.push(p.clone());
        first
    });
    let drop: Vec<TileId> = loaded
        .tree
        .tiles
        .iter()
        .filter_map(|(&id, tile)| match tile {
            Tile::Pane(p) if *p == Pane::Documents || loaded.drawers.contains(p) => Some(id),
            _ => None,
        })
        .collect();
    for id in drop {
        loaded.tree.remove_recursively(id);
    }
    loaded.tree.root()?;
    let max_id = loaded
        .tree
        .tiles
        .tiles()
        .filter_map(|t| match t {
            Tile::Pane(p) => p.id(),
            Tile::Container(_) => None,
        })
        .max()
        .unwrap_or(0);
    loaded.next_pane = loaded.next_pane.max(max_id);
    Some(loaded)
}

/// Version 4 → 5: up to stage 55 the resource tabs and the logic graph were
/// on the Resources screen. A user who made that screen a layout of their
/// own keeps it for the asset editors too; otherwise they get the built-in
/// Asset Editor.
fn seed_assets(layouts: &mut Layouts) {
    if layouts.screens.contains_key(&Screen::Assets) {
        return;
    }
    if let Some(remembered) = layouts.screens.get(&Screen::Resources)
        && matches!(remembered.layout, LayoutId::User(_))
    {
        let remembered = remembered.clone();
        layouts.screens.insert(Screen::Assets, remembered);
    }
}

/// Version 2 → 3: one Details where there were two, Zones next to the
/// Outliner.
fn migrate(tree: &mut Tree<Pane>, drawers: &mut Vec<Pane>) {
    let details: Vec<TileId> = tree
        .tiles
        .iter()
        .filter_map(|(&id, tile)| match tile {
            Tile::Pane(Pane::Details) => Some(id),
            _ => None,
        })
        .collect();
    if drawers.contains(&Pane::Details) {
        // A drawer: no tile of it stays (`parse` removes them).
        let mut first = true;
        drawers.retain(|p| *p != Pane::Details || std::mem::take(&mut first));
    } else if details.len() > 1 {
        let shown = tree.active_tiles();
        let keep = details
            .iter()
            .copied()
            .find(|t| shown.contains(t))
            .unwrap_or(details[0]);
        for t in details.into_iter().filter(|&t| t != keep) {
            tree.remove_recursively(t);
        }
    }
    let has_zones =
        dock::find(tree, |p| *p == Pane::Zones).is_some() || drawers.contains(&Pane::Zones);
    if !has_zones {
        if let Some(outliner) = dock::find(tree, |p| *p == Pane::Outliner) {
            let zones = tree.tiles.insert_pane(Pane::Zones);
            match tree.tiles.parent_of(outliner) {
                Some(parent)
                    if matches!(
                        tree.tiles.get(parent),
                        Some(Tile::Container(Container::Tabs(_)))
                    ) =>
                {
                    if let Some(Tile::Container(Container::Tabs(tabs))) = tree.tiles.get_mut(parent)
                    {
                        let active = tabs.active;
                        tabs.add_child(zones);
                        tabs.active = active;
                    }
                }
                _ => {
                    let tabs = tree.tiles.insert_tab_tile(vec![zones]);
                    if let Some(root) = tree.root() {
                        let column = tree.tiles.insert_vertical_tile(vec![root, tabs]);
                        tree.root = Some(column);
                    }
                }
            }
        } else if drawers.contains(&Pane::Outliner) {
            drawers.push(Pane::Zones);
        }
    }
    tree.simplify(&SIMPLIFY);
}

/// Version 3 → 4: a layout of the level (with the Outliner) gets the
/// History panel as a drawer, as the Level Editor has it.
fn add_history(tree: &Tree<Pane>, drawers: &mut Vec<Pane>) {
    let has = |pane: &Pane| dock::find(tree, |p| p == pane).is_some() || drawers.contains(pane);
    if has(&Pane::Outliner) && !has(&Pane::History) {
        drawers.push(Pane::History);
    }
}

/// The layout file.
pub fn to_json(tree: &Tree<Pane>, next_pane: u64, drawers: &[Pane], layouts: &Layouts) -> String {
    #[derive(Serialize)]
    struct Out<'a> {
        version: u32,
        next_pane: u64,
        tree: &'a Tree<Pane>,
        drawers: &'a [Pane],
        layouts: &'a Layouts,
    }
    serde_json::to_string_pretty(&Out {
        version: LAYOUT_VERSION,
        next_pane,
        tree,
        drawers,
        layouts,
    })
    .unwrap_or_default()
}

/// The saved layout, or the first start's.
pub fn load(store: &crate::settings::Store) -> Loaded {
    store
        .layout_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| parse(&text))
        .unwrap_or_else(Loaded::first_start)
}

/// The window of the user's layouts: save as, rename, delete.
#[derive(Default)]
pub struct LayoutWindow {
    pub open: bool,
    /// The name typed for "Save as".
    name: String,
    /// The layout being renamed and its new name.
    renaming: Option<(String, String)>,
    /// A delete waits for a second click.
    confirm_delete: Option<String>,
    focus: bool,
}

impl LayoutWindow {
    /// Opens the window with the name field focused.
    pub fn show(&mut self, current: &LayoutId) {
        self.open = true;
        self.focus = true;
        self.name = match current {
            LayoutId::User(name) => name.clone(),
            LayoutId::Builtin(_) => String::new(),
        };
    }
}

impl BrowserApp {
    /// Applies a layout to the dock, keeping the documents.
    pub(super) fn apply_layout(&mut self, tree: &mut Tree<Pane>, id: LayoutId) {
        let Some(arrangement) = self.layouts.arrangement(&id) else {
            return;
        };
        self.sync_descriptors(tree);
        let (new, drawers) = arrange(&arrangement, dock::documents(tree));
        *tree = new;
        self.drawers.set(drawers);
        self.layouts.current = id;
        self.maximized = None;
        self.active_doc = None;
    }

    /// The dock as an arrangement, for a user layout or a screen.
    pub(super) fn current_arrangement(&self, tree: &Tree<Pane>) -> Arrangement {
        template_of(tree, &self.drawers.panes, self.active_doc)
    }

    /// Arranges the dock for the screen of a document, when layouts are
    /// remembered per screen and it is another screen; whether it did.
    pub(super) fn switch_screen(&mut self, tree: &mut Tree<Pane>, screen: Screen) -> bool {
        if !self.layouts.remember || self.layouts.screen == screen || self.maximized.is_some() {
            return false;
        }
        let left = Remembered {
            layout: self.layouts.current.clone(),
            arrangement: self.current_arrangement(tree),
        };
        self.layouts.screens.insert(self.layouts.screen, left);
        let (id, arrangement) = match self.layouts.screens.get(&screen) {
            Some(r) => (r.layout.clone(), r.arrangement.clone()),
            None => {
                let b = screen.builtin();
                (LayoutId::Builtin(b), b.arrangement())
            }
        };
        self.sync_descriptors(tree);
        let (new, drawers) = arrange(&arrangement, dock::documents(tree));
        *tree = new;
        self.drawers.set(drawers);
        self.layouts.current = id;
        self.layouts.screen = screen;
        self.active_doc = None;
        true
    }

    /// View → Layouts.
    pub(super) fn layouts_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button(tr("layout.menu"), |ui| {
            ui.set_min_width(240.0);
            let current = self.layouts.current.clone();
            for b in Builtin::ALL {
                let id = LayoutId::Builtin(b);
                if ui
                    .add(egui::Button::selectable(current == id, b.name()))
                    .clicked()
                {
                    self.dock_ops.push(DockOp::ApplyLayout(id));
                }
            }
            if !self.layouts.user.is_empty() {
                ui.separator();
            }
            let names: Vec<String> = self.layouts.user.iter().map(|n| n.name.clone()).collect();
            for name in names {
                let id = LayoutId::User(name.clone());
                if ui
                    .add(egui::Button::selectable(current == id, &name))
                    .clicked()
                {
                    self.dock_ops.push(DockOp::ApplyLayout(id));
                }
            }
            ui.separator();
            for command in [Command::SaveLayout, Command::ResetLayout] {
                let mut button = egui::Button::new(command.label());
                if let Some(text) = command.shortcut_text(ui.ctx()) {
                    button = button.shortcut_text(text);
                }
                if ui.add_enabled(self.enabled(command), button).clicked() {
                    self.pending.push(command);
                }
            }
            ui.separator();
            ui.checkbox(&mut self.layouts.remember, tr("layout.remember"))
                .on_hover_text(tr("layout.remember_hint"));
        });
    }

    /// The window of the user's layouts.
    pub(super) fn layouts_window(&mut self, ctx: &egui::Context) {
        if !self.layout_window.open {
            return;
        }
        let mut open = true;
        let mut ops = Vec::new();
        egui::Window::new(tr("layout.window"))
            .id(egui::Id::unique("layouts-window"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .show(ctx, |ui| {
                let w = &mut self.layout_window;
                ui.label(egui::RichText::new(tr("layout.window_hint")).weak());
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut w.name)
                            .hint_text(tr("layout.name_hint"))
                            .desired_width(240.0),
                    );
                    if std::mem::take(&mut w.focus) {
                        response.request_focus();
                    }
                    let name = w.name.trim().to_owned();
                    let enter =
                        response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let exists = self.layouts.user.iter().any(|n| n.name == name);
                    let text = if exists {
                        tr("layout.replace")
                    } else {
                        tr("layout.save")
                    };
                    if ui
                        .add_enabled(!name.is_empty(), egui::Button::new(text))
                        .clicked()
                        || (enter && !name.is_empty())
                    {
                        ops.push(DockOp::SaveLayout(name));
                    }
                });
                ui.separator();
                if self.layouts.user.is_empty() {
                    ui.label(egui::RichText::new(tr("layout.none")).weak());
                }
                let current = self.layouts.current.clone();
                let names: Vec<String> = self.layouts.user.iter().map(|n| n.name.clone()).collect();
                egui::Grid::new("user-layouts")
                    .num_columns(2)
                    .striped(true)
                    .show(ui, |ui| {
                        for name in names {
                            let id = LayoutId::User(name.clone());
                            match &mut w.renaming {
                                Some((from, to)) if *from == name => {
                                    let response =
                                        ui.add(egui::TextEdit::singleline(to).desired_width(200.0));
                                    let mut done = false;
                                    ui.horizontal(|ui| {
                                        let enter = response.lost_focus()
                                            && ui.input(|i| i.key_pressed(egui::Key::Enter));
                                        if ui.small_button(tr("layout.ok")).clicked() || enter {
                                            ops.push(DockOp::RenameLayout {
                                                from: from.clone(),
                                                to: to.trim().to_owned(),
                                            });
                                            done = true;
                                        } else if ui.small_button(tr("dialog.cancel")).clicked() {
                                            done = true;
                                        }
                                    });
                                    if done {
                                        w.renaming = None;
                                    }
                                }
                                _ => {
                                    if ui.selectable_label(current == id, &name).clicked() {
                                        ops.push(DockOp::ApplyLayout(id));
                                    }
                                    ui.horizontal(|ui| {
                                        if ui.small_button(tr("layout.apply")).clicked() {
                                            ops.push(DockOp::ApplyLayout(LayoutId::User(
                                                name.clone(),
                                            )));
                                        }
                                        if ui.small_button(tr("layout.rename")).clicked() {
                                            w.renaming = Some((name.clone(), name.clone()));
                                        }
                                        let confirm = w.confirm_delete.as_deref() == Some(&name);
                                        let text = if confirm {
                                            tr("layout.delete_confirm")
                                        } else {
                                            tr("layout.delete")
                                        };
                                        if ui.small_button(text).clicked() {
                                            if confirm {
                                                ops.push(DockOp::DeleteLayout(name.clone()));
                                                w.confirm_delete = None;
                                            } else {
                                                w.confirm_delete = Some(name.clone());
                                            }
                                        }
                                    });
                                }
                            }
                            ui.end_row();
                        }
                    });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(tr("layout.current"));
                    ui.strong(self.layouts.current.name());
                });
                ui.checkbox(&mut self.layouts.remember, tr("layout.remember"))
                    .on_hover_text(tr("layout.remember_hint"));
            });
        self.dock_ops.extend(ops);
        if !open {
            self.layout_window.open = false;
        }
    }

    /// Carries out a layout change of the dock.
    pub(super) fn apply_layout_op(&mut self, tree: &mut Tree<Pane>, op: DockOp) {
        match op {
            DockOp::ApplyLayout(id) => self.apply_layout(tree, id),
            DockOp::SaveLayout(name) => {
                self.sync_descriptors(tree);
                let arrangement = self.current_arrangement(tree);
                self.layouts.save(&name, arrangement);
                self.notify(super::Level::Info, trf("layout.saved", &[("name", &name)]));
            }
            DockOp::RenameLayout { from, to } => {
                if !self.layouts.rename(&from, &to) {
                    self.warn(trf("layout.cannot_rename", &[("name", &to)]));
                }
            }
            DockOp::DeleteLayout(name) => {
                if self.layouts.delete(&name) {
                    self.notify(
                        super::Level::Info,
                        trf("layout.deleted", &[("name", &name)]),
                    );
                }
            }
            DockOp::Screen(screen) => {
                self.switch_screen(tree, screen);
            }
            _ => {}
        }
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

    fn reachable(tree: &Tree<Pane>) -> Vec<Pane> {
        fn walk(tree: &Tree<Pane>, t: TileId, out: &mut Vec<Pane>) {
            match tree.tiles.get(t) {
                Some(Tile::Pane(p)) => out.push(p.clone()),
                Some(Tile::Container(c)) => {
                    for &child in c.children() {
                        walk(tree, child, out);
                    }
                }
                None => {}
            }
        }
        let mut out = Vec::new();
        if let Some(root) = tree.root() {
            walk(tree, root, &mut out);
        }
        out.sort_by_key(|p| format!("{p:?}"));
        out
    }

    #[test]
    fn level_layout_puts_the_view_in_the_middle() {
        let (tree, drawers) = arrange(
            &Builtin::Level.arrangement(),
            vec![Pane::Level { id: 3 }, Pane::Stream { id: 4, path: None }],
        );
        assert_eq!(
            drawers,
            [
                Pane::ContentBrowser,
                Pane::Log,
                Pane::Changes,
                Pane::History
            ],
            "the bottom panels are drawers"
        );
        for pane in [Pane::Outliner, Pane::Zones, Pane::Details, Pane::Files] {
            assert!(dock::find(&tree, |p| *p == pane).is_some(), "{pane:?}");
        }
        assert!(dock::find(&tree, |p| *p == Pane::Documents).is_none());
        assert!(dock::find(&tree, |p| *p == Pane::Log).is_none());
        let level = dock::find(&tree, |p| p.id() == Some(3)).unwrap();
        let stream = dock::find(&tree, |p| p.id() == Some(4)).unwrap();
        assert_eq!(tree.tiles.parent_of(level), tree.tiles.parent_of(stream));
        assert!(
            dock::is_shown(&tree, level),
            "the first document is in front"
        );
        assert_eq!(reachable(&tree), panes(&tree), "no tile is left over");
        // Without documents the welcome screen takes their place.
        let (empty, _) = arrange(&Builtin::Level.arrangement(), Vec::new());
        assert!(dock::find(&empty, |p| *p == Pane::Welcome).is_some());
    }

    #[test]
    fn a_saved_layout_keeps_panels_and_takes_new_documents() {
        let mut tree = dock::default_tree(vec![Pane::Index, Pane::Level { id: 9 }]);
        dock::toggle_panel(&mut tree, Pane::Details, None);
        let active = dock::find(&tree, |p| p.id() == Some(9));
        let saved = template_of(&tree, &[Pane::ContentBrowser], active);
        assert!(dock::find(&saved.tree, |p| *p == Pane::Documents).is_some());
        assert!(dock::find(&saved.tree, |p| p.id() == Some(9)).is_none());
        assert!(dock::find(&saved.tree, |p| *p == Pane::Index).is_none());
        let mut layouts = Layouts::default();
        layouts.save("Mine", saved);
        assert_eq!(layouts.current, LayoutId::User("Mine".into()));
        let mine = layouts.arrangement(&LayoutId::User("Mine".into())).unwrap();
        let (restored, drawers) = arrange(
            &mine,
            vec![Pane::Search {
                id: 12,
                query: "x".into(),
            }],
        );
        assert_eq!(drawers, [Pane::ContentBrowser]);
        let details = dock::find(&restored, |p| *p == Pane::Details).unwrap();
        assert!(
            !restored.tiles.is_visible(details),
            "the hidden panel stays hidden"
        );
        assert!(dock::find(&restored, |p| p.id() == Some(12)).is_some());
        assert!(dock::find(&restored, |p| p.id() == Some(9)).is_none());
        // The document takes the place of the old ones: next to the welcome
        // screen's group, between Files and Details.
        let files = dock::find(&restored, |p| *p == Pane::Files).unwrap();
        let search = dock::find(&restored, |p| p.id() == Some(12)).unwrap();
        assert_ne!(
            restored.tiles.parent_of(files),
            restored.tiles.parent_of(search)
        );
        assert_eq!(reachable(&restored), panes(&restored));
    }

    #[test]
    fn user_layouts_are_renamed_and_deleted() {
        let mut layouts = Layouts::default();
        let a = Builtin::Resources.arrangement();
        layouts.save("One", a.clone());
        layouts.save("Two", a.clone());
        layouts.save("One", Builtin::Level.arrangement());
        assert_eq!(layouts.user.len(), 2, "saving under a name replaces it");
        assert!(dock::find(&layouts.user[0].arrangement.tree, |p| *p == Pane::Outliner).is_some());
        layouts.screens.insert(
            Screen::Level,
            Remembered {
                layout: LayoutId::User("Two".into()),
                arrangement: a,
            },
        );
        assert!(!layouts.rename("Two", "One"), "names are unique");
        assert!(!layouts.rename("Two", "  "));
        assert!(layouts.rename("Two", "Three"));
        assert_eq!(layouts.current, LayoutId::User("One".into()));
        assert_eq!(
            layouts.screens[&Screen::Level].layout,
            LayoutId::User("Three".into())
        );
        assert!(layouts.rename("One", "Four"));
        assert_eq!(layouts.current, LayoutId::User("Four".into()));
        assert!(layouts.delete("Three"));
        assert!(!layouts.delete("Three"));
        assert_eq!(
            layouts.screens[&Screen::Level].layout,
            LayoutId::Builtin(Builtin::Level),
            "the screen falls back to its built-in layout"
        );
        assert!(
            layouts
                .arrangement(&LayoutId::User("Three".into()))
                .is_none()
        );
        assert!(layouts.delete("Four"));
        assert_eq!(layouts.current, LayoutId::Builtin(Builtin::Resources));
        assert!(layouts.user.is_empty());
    }

    #[test]
    fn layout_file_round_trip() {
        let (tree, drawers) = arrange(&Builtin::Level.arrangement(), vec![Pane::Level { id: 5 }]);
        let mut layouts = Layouts {
            current: LayoutId::Builtin(Builtin::Level),
            screen: Screen::Level,
            remember: false,
            ..Layouts::default()
        };
        layouts.save("Wide", Builtin::Resources.arrangement());
        let json = to_json(&tree, 5, &drawers, &layouts);
        let loaded = parse(&json).expect("parses");
        assert_eq!(loaded.next_pane, 5);
        assert_eq!(panes(&loaded.tree), panes(&tree));
        assert_eq!(loaded.drawers, drawers);
        assert_eq!(loaded.layouts.current, LayoutId::User("Wide".into()));
        assert_eq!(loaded.layouts.screen, Screen::Level);
        assert!(!loaded.layouts.remember);
        assert_eq!(loaded.layouts.user[0].name, "Wide");
        assert!(parse("{").is_none());
        assert!(parse(&json.replace("\"version\": 5", "\"version\": 999")).is_none());
    }

    #[test]
    fn version_one_files_are_migrated() {
        // A layout file as stage 51 wrote it.
        let mut tree = dock::default_tree(Vec::new());
        dock::add_document(
            &mut tree,
            Pane::Stream {
                id: 3,
                path: Some("gf2/frontend.str".into()),
            },
            None,
        );
        dock::toggle_panel(&mut tree, Pane::Details, None);
        let old = serde_json::json!({ "version": 1, "next_pane": 3, "tree": tree });
        let loaded = parse(&old.to_string()).expect("version 1 is read");
        assert_eq!(loaded.next_pane, 3);
        assert_eq!(panes(&loaded.tree), panes(&tree));
        let details = dock::find(&loaded.tree, |p| *p == Pane::Details).unwrap();
        assert!(!loaded.tree.tiles.is_visible(details));
        assert!(loaded.drawers.is_empty());
        assert_eq!(
            loaded.layouts.current,
            LayoutId::Builtin(Builtin::Resources)
        );
        assert!(loaded.layouts.remember);
        // A pane that is a drawer is not also in the tree.
        let both = serde_json::json!({
            "version": 2, "next_pane": 3, "tree": tree, "drawers": ["Log"],
        });
        let loaded = parse(&both.to_string()).unwrap();
        assert!(dock::find(&loaded.tree, |p| *p == Pane::Log).is_none());
        assert_eq!(loaded.drawers, [Pane::Log]);
    }

    /// The "Level Editor" of stage 52: Outliner (zones inside) and Files
    /// over the object panel and the resource Details.
    fn stage_52_level_tree() -> Tree<Pane> {
        let mut tiles = Tiles::default();
        let level = tiles.insert_pane(Pane::Level { id: 1 });
        let center = tiles.insert_tab_tile(vec![level]);
        let outliner = tiles.insert_pane(Pane::Outliner);
        let files = tiles.insert_pane(Pane::Files);
        let top = tiles.insert_tab_tile(vec![outliner, files]);
        // Written as "ObjectDetails" below.
        let object = tiles.insert_pane(Pane::Details);
        let details = tiles.insert_pane(Pane::Details);
        let bottom = tiles.insert_tab_tile(vec![object, details]);
        let right = tiles.insert_vertical_tile(vec![top, bottom]);
        let root = tiles.insert_horizontal_tile(vec![center, right]);
        Tree::new(egui::Id::unique("old"), root, tiles)
    }

    /// A stage 52 file: `ObjectDetails` for the first Details.
    fn as_stage_52(json: &str) -> String {
        json.replacen("\"Details\"", "\"ObjectDetails\"", 1)
            .replace("\"version\": 5", "\"version\": 2")
    }

    #[test]
    fn version_two_files_get_one_details_and_zones() {
        let tree = stage_52_level_tree();
        let mut layouts = Layouts::default();
        layouts.save(
            "Mine",
            Arrangement {
                tree: stage_52_level_tree(),
                drawers: vec![Pane::Log],
            },
        );
        let json = as_stage_52(&to_json(&tree, 1, &[Pane::ContentBrowser], &layouts));
        assert!(json.contains("ObjectDetails") && json.contains("\"version\": 2"));
        let loaded = parse(&json).expect("a stage 52 file reads");
        for tree in [&loaded.tree, &loaded.layouts.user[0].arrangement.tree] {
            let all = panes(tree);
            assert_eq!(
                all.iter().filter(|p| **p == Pane::Details).count(),
                1,
                "one Details: {all:?}"
            );
            // Zones joins the Outliner's tabs; the Outliner stays in front.
            let outliner = dock::find(tree, |p| *p == Pane::Outliner).unwrap();
            let zones = dock::find(tree, |p| *p == Pane::Zones).expect("Zones");
            assert_eq!(tree.tiles.parent_of(outliner), tree.tiles.parent_of(zones));
            assert!(dock::is_shown(tree, outliner));
            assert!(dock::find(tree, |p| p.id() == Some(1)).is_some());
        }
        assert_eq!(loaded.drawers, [Pane::ContentBrowser, Pane::History]);
        assert_eq!(
            loaded.layouts.user[0].arrangement.drawers,
            [Pane::Log, Pane::History]
        );
        // Saved again it is version 4, and reads the same.
        let again = to_json(&loaded.tree, 1, &loaded.drawers, &loaded.layouts);
        assert!(again.contains("\"version\": 5") && !again.contains("ObjectDetails"));

        assert_eq!(panes(&parse(&again).unwrap().tree), panes(&loaded.tree));

        // The object panel as a drawer: one Details drawer, none in the tree.
        let json = as_stage_52(&to_json(
            &dock::default_tree(Vec::new()),
            0,
            &[Pane::Details],
            &Layouts::default(),
        ))
        .replacen("\"Details\"", "\"ObjectDetails\"", 1);
        let loaded = parse(&json).unwrap();
        assert_eq!(loaded.drawers, [Pane::Details]);
        assert!(dock::find(&loaded.tree, |p| *p == Pane::Details).is_none());
        // No Outliner: no Zones is added.
        assert!(dock::find(&loaded.tree, |p| *p == Pane::Zones).is_none());
    }

    #[test]
    fn version_three_files_get_the_history_drawer() {
        // The Level Editor of stage 53 and a layout without a level.
        let (tree, _) = arrange(&Builtin::Level.arrangement(), vec![Pane::Level { id: 1 }]);
        let mut layouts = Layouts::default();
        layouts.save("Files only", Builtin::Resources.arrangement());
        let stage_53 = [Pane::ContentBrowser, Pane::Log, Pane::Changes];
        let json =
            to_json(&tree, 1, &stage_53, &layouts).replace("\"version\": 5", "\"version\": 3");
        let loaded = parse(&json).expect("a stage 53 file reads");
        assert_eq!(
            loaded.drawers,
            [
                Pane::ContentBrowser,
                Pane::Log,
                Pane::Changes,
                Pane::History
            ]
        );
        assert_eq!(panes(&loaded.tree), panes(&tree));
        // Without an Outliner nothing is added.
        assert_eq!(
            loaded.layouts.user[0].arrangement.drawers,
            [Pane::ContentBrowser]
        );
        // History docked already: no drawer.
        let mut docked = tree.clone();
        dock::show_panel(&mut docked, Pane::History, None);
        let json = to_json(&docked, 1, &stage_53, &Layouts::default())
            .replace("\"version\": 5", "\"version\": 3");
        let loaded = parse(&json).unwrap();
        assert_eq!(loaded.drawers, stage_53);
        assert!(dock::find(&loaded.tree, |p| *p == Pane::History).is_some());
    }

    #[test]
    fn screens_follow_their_documents() {
        assert_eq!(Screen::of(&Pane::Level { id: 1 }), Some(Screen::Level));
        assert_eq!(
            Screen::of(&Pane::Stream { id: 1, path: None }),
            Some(Screen::Resources)
        );
        assert_eq!(Screen::of(&Pane::Welcome), None);
        assert_eq!(Screen::of(&Pane::Log), None);
        assert_eq!(Screen::Level.builtin(), Builtin::Level);
        // Stage 56: the asset editors have a screen of their own.
        for pane in [
            Pane::Resource {
                id: 1,
                key: String::new(),
            },
            Pane::Logic {
                id: 2,
                scope: String::new(),
            },
            Pane::Script,
            Pane::Movies,
        ] {
            assert_eq!(Screen::of(&pane), Some(Screen::Assets), "{pane:?}");
        }
        assert_eq!(Screen::Assets.builtin(), Builtin::Assets);
    }

    #[test]
    fn asset_editor_layout_puts_details_beside_the_editor() {
        let (tree, drawers) = arrange(
            &Builtin::Assets.arrangement(),
            vec![Pane::Script, Pane::Movies],
        );
        let script = dock::find(&tree, |p| *p == Pane::Script).unwrap();
        let movies = dock::find(&tree, |p| *p == Pane::Movies).unwrap();
        assert_eq!(tree.tiles.parent_of(script), tree.tiles.parent_of(movies));
        let details = dock::find(&tree, |p| *p == Pane::Details).unwrap();
        assert!(dock::is_shown(&tree, details));
        assert!(dock::find(&tree, |p| *p == Pane::Files).is_none());
        for pane in [Pane::ContentBrowser, Pane::Files, Pane::History] {
            assert!(drawers.contains(&pane), "{pane:?}");
        }
        let mut names: Vec<&str> = Builtin::ALL.iter().map(|b| b.name()).collect();
        names.dedup();
        assert_eq!(names.len(), 3);
    }

    #[test]
    fn version_four_files_are_read_and_keep_a_users_layout_for_the_editors() {
        let (tree, drawers) = arrange(&Builtin::Resources.arrangement(), Vec::new());
        // A stage-55 user with a layout of their own for resources.
        let mut layouts = Layouts::default();
        layouts.save("Mine", Builtin::Resources.arrangement());
        layouts.screens.insert(
            Screen::Resources,
            Remembered {
                layout: LayoutId::User("Mine".into()),
                arrangement: Builtin::Resources.arrangement(),
            },
        );
        let v4 = to_json(&tree, 3, &drawers, &layouts).replace("\"version\": 5", "\"version\": 4");
        let loaded = parse(&v4).expect("version 4 is read");
        assert_eq!(
            loaded.layouts.screens[&Screen::Assets].layout,
            LayoutId::User("Mine".into())
        );
        // Without a layout of their own: the built-in Asset Editor.
        let v4 = to_json(&tree, 3, &drawers, &Layouts::default())
            .replace("\"version\": 5", "\"version\": 4");
        let loaded = parse(&v4).unwrap();
        assert!(!loaded.layouts.screens.contains_key(&Screen::Assets));
        // Version 5 with the new tabs round-trips.
        let (tree, drawers) = arrange(
            &Builtin::Assets.arrangement(),
            vec![Pane::Script, Pane::Movies],
        );
        let json = to_json(&tree, 3, &drawers, &Layouts::default());
        assert!(json.contains("\"version\": 5"));
        let loaded = parse(&json).unwrap();
        assert!(dock::find(&loaded.tree, |p| *p == Pane::Script).is_some());
        assert!(dock::find(&loaded.tree, |p| *p == Pane::Movies).is_some());
        assert_eq!(loaded.drawers, drawers);
    }
}

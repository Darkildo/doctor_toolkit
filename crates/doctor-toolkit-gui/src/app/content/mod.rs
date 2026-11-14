//! The Content Browser (stage 55): the assets of the game and the workspace
//! with previews.
//!
//! - Left, the folders of the streams (`Content → gf2 → cuba → local`); a
//!   stream is a folder of assets ([`catalog`]).
//! - Right, tiles with previews (or a list): the sub-folders and streams of
//!   a folder, the assets of a stream; with a type filter, the assets of
//!   every stream under the folder; with a search, the matches of the game
//!   index (the same search as Ctrl+F) under the folder.
//! - Previews of models and textures are made on worker threads and cached
//!   in memory and on disk ([`preview`], [`thumb`]); only the tiles on the
//!   screen ask for them.
//! - A click shows the asset in Details, a double click opens it in its
//!   editor; the context menu opens, shows it in its stream, copies its key,
//!   clones it into the workspace or exports it. A sound bank plays its
//!   first sound from the button on its tile.
//! - A model is dragged into a level view to place it there
//!   (a [`DraggedAsset`] of [`super::dnd`]; `level_view::drop`), or onto
//!   the model field of Details to give the object that model.
//!
//! The browser is an ordinary panel: a drawer by default (Ctrl+Space),
//! docked anywhere, and more of them can be opened (View → New Content
//! Browser), each with its own place, filters and tile size.
//!
//! `DOCTOR_TOOLKIT_GUI_CONTENT=<folder or stream>[;option…]` sets up new
//! browsers for screenshots (`<path>/` is the folder when a stream has the
//! same name): `filter=model,texture,…` (categories),
//! `size=<pixels>`, `list`, `search=<text>`, `select=<name>` (selects the
//! first asset whose name contains the text and shows it in Details).

pub mod catalog;
pub mod preview;
#[cfg(test)]
mod tests;
pub mod thumb;

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use egui::{Color32, Pos2, Rect, RichText, Sense, Stroke, Vec2, pos2, vec2};

use doctor_toolkit::hash::sdbm;
use doctor_toolkit::navigation::{GameIndex, Query, RecordId};
use doctor_toolkit::workspace::{ResourceKey, Workspace};
use doctor_toolkit_mods::i18n::{tr, trf, trn};

use super::dnd::DraggedAsset;
use super::dock::{DockOp, Pane};
use super::theme::{self, Icon};
use super::{BrowserApp, FileSel};
use crate::level_view::drop::{self as level_drop, Ghost};
use crate::nav::Location;
use catalog::{Asset, Category, Filter, FolderTree, Mark, Marks, Place, StreamEntry};
use preview::{Kind, PreviewId, PreviewJob, Previews, Shown};

/// Assets listed at most for a folder with a type filter.
const FOLDER_LIMIT: usize = 5000;
/// Results of a search.
const SEARCH_LIMIT: usize = 20_000;
/// Tile sizes, pixels of the preview.
const TILE: (f32, f32) = (56.0, 256.0);
const ROW: f32 = 22.0;
const GAP: f32 = 6.0;
/// Typing pauses this long before the search runs.
const SEARCH_DELAY: Duration = Duration::from_millis(250);

/// Grid of tiles or list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Grid,
    List,
}

/// What is selected in a browser.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Selected {
    Folder(String),
    Stream(String),
    Asset(ResourceKey),
}

/// One entry of the right side.
enum Item<'a> {
    Folder(&'a FolderItem),
    Stream(&'a StreamEntry),
    Asset(&'a Asset),
}

#[derive(Debug, Clone, PartialEq)]
struct FolderItem {
    path: String,
    name: String,
    total: usize,
}

/// The entries of a place.
#[derive(Default)]
struct Listing {
    folders: Vec<FolderItem>,
    streams: Vec<StreamEntry>,
    assets: Vec<Asset>,
    /// More assets than [`FOLDER_LIMIT`].
    truncated: bool,
}

impl Listing {
    fn len(&self) -> usize {
        self.folders.len() + self.streams.len() + self.assets.len()
    }

    fn item(&self, i: usize) -> Option<Item<'_>> {
        let (f, s) = (self.folders.len(), self.streams.len());
        if i < f {
            Some(Item::Folder(&self.folders[i]))
        } else if i < f + s {
            Some(Item::Stream(&self.streams[i - f]))
        } else {
            self.assets.get(i - f - s).map(Item::Asset)
        }
    }
}

/// What a listing was made for.
#[derive(Debug, Clone, PartialEq)]
struct ListingKey {
    place: Place,
    filter: Filter,
    search: Option<String>,
    names: bool,
    revision: u64,
    copies: bool,
}

/// A search of the game index under way.
struct SearchJob {
    text: String,
    rx: Receiver<Result<Vec<RecordId>, String>>,
}

/// Settings of new browsers from `DOCTOR_TOOLKIT_GUI_CONTENT`.
#[derive(Debug, Clone, Default)]
struct Preset {
    place: Option<String>,
    categories: Vec<Category>,
    size: Option<f32>,
    list: bool,
    search: Option<String>,
    select: Option<String>,
}

impl Preset {
    fn from_env() -> Option<Self> {
        let spec = std::env::var("DOCTOR_TOOLKIT_GUI_CONTENT").ok()?;
        let mut parts = spec.split(';');
        let mut preset = Preset {
            place: parts.next().filter(|p| !p.is_empty()).map(str::to_owned),
            ..Preset::default()
        };
        for part in parts {
            let (name, value) = part.split_once('=').unwrap_or((part, ""));
            match name {
                "filter" => {
                    preset.categories = value
                        .split(',')
                        .filter_map(|v| {
                            Category::ALL
                                .into_iter()
                                .find(|c| format!("{c:?}").eq_ignore_ascii_case(v))
                        })
                        .collect();
                }
                "size" => preset.size = value.parse().ok(),
                "list" => preset.list = true,
                "search" => preset.search = Some(value.to_owned()),
                "select" => preset.select = Some(value.to_lowercase()),
                _ => eprintln!("DOCTOR_TOOLKIT_GUI_CONTENT: unknown option {name}"),
            }
        }
        Some(preset)
    }
}

/// One Content Browser: where it is and how it shows it.
pub struct ContentState {
    place: Place,
    /// Places left by Up or a click, for Back.
    back: Vec<Place>,
    mode: Mode,
    tile: f32,
    filter: Filter,
    search: String,
    edited: Option<Instant>,
    job: Option<SearchJob>,
    /// The last search: its text and the records found.
    results: Option<(String, Result<Vec<RecordId>, String>)>,
    /// The place and the game index (its address) of the last search: the
    /// search runs again in another place or a new index.
    searched: (Place, usize),
    selected: Option<Selected>,
    listing: Listing,
    key: Option<ListingKey>,
    /// Open the tree down to the place in the next frame.
    reveal: bool,
    scroll_top: bool,
    /// Select the first asset whose name contains this (preset).
    select: Option<String>,
    /// List the same resource of every stream that has it (a folder or a
    /// search over many streams); off, the first one only.
    copies: bool,
}

impl Default for ContentState {
    fn default() -> Self {
        Self {
            place: Place::default(),
            back: Vec::new(),
            mode: Mode::Grid,
            tile: 96.0,
            filter: Filter::default(),
            search: String::new(),
            edited: None,
            job: None,
            results: None,
            searched: (Place::default(), 0),
            selected: None,
            listing: Listing::default(),
            key: None,
            reveal: false,
            scroll_top: false,
            select: None,
            copies: false,
        }
    }
}

impl ContentState {
    fn from_preset(preset: &Preset, tree: &FolderTree) -> Self {
        let mut state = Self::default();
        if let Some(place) = &preset.place {
            // `path/` is the folder when a stream has the same name.
            state.place = match place.strip_suffix('/') {
                Some(folder) => Place::Folder(folder.to_owned()),
                None if tree.stream(place).is_some() => Place::Stream(place.clone()),
                None => Place::Folder(place.clone()),
            };
            state.reveal = true;
        }
        state.filter.categories.clone_from(&preset.categories);
        if let Some(size) = preset.size {
            state.tile = size.clamp(TILE.0, TILE.1);
        }
        if preset.list {
            state.mode = Mode::List;
        }
        if let Some(search) = &preset.search {
            state.search.clone_from(search);
            state.edited = Some(Instant::now() - SEARCH_DELAY);
        }
        state.select.clone_from(&preset.select);
        state
    }

    fn go(&mut self, place: Place) {
        if place != self.place {
            self.back.push(std::mem::replace(&mut self.place, place));
            self.reveal = true;
            self.scroll_top = true;
        }
    }
}

/// The game (its address) and the state of the workspace (head,
/// operations, changes) the tree and the marks were made for.
type MadeFor = (usize, Option<(usize, usize, usize)>);

/// Interleaved samples, channels and rate of a decoded sound.
type Pcm = (Vec<i16>, u16, u32);

/// The Content Browsers of the editor and what they share: the folder
/// tree, the marks of the workspace and the previews.
pub struct Content {
    browsers: HashMap<u64, ContentState>,
    pub previews: Previews,
    tree: Arc<FolderTree>,
    marks: Arc<Marks>,
    /// Changes of the tree or the marks.
    revision: u64,
    /// The game and workspace the tree and marks were made for.
    made_for: Option<MadeFor>,
    /// The game files changed: the tree, marks and sources are made again.
    stale: bool,
    preset: Option<Preset>,
    sound: Option<Receiver<(ResourceKey, Result<Pcm, String>)>>,
    /// The asset being dragged out of a browser.
    dragging: Option<(DraggedAsset, Asset)>,
    #[cfg(feature = "audio")]
    playing: Option<(ResourceKey, crate::audio_out::Playback)>,
}

impl Content {
    pub fn new(ctx: &egui::Context, gpu: Option<crate::render::GpuContext>) -> Self {
        let mut previews = Previews::new(ctx);
        previews.set_gpu(gpu);
        Self {
            browsers: HashMap::new(),
            previews,
            tree: Arc::default(),
            marks: Arc::default(),
            revision: 0,
            made_for: None,
            stale: false,
            preset: Preset::from_env(),
            sound: None,
            dragging: None,
            #[cfg(feature = "audio")]
            playing: None,
        }
    }

    /// Ids of the browsers drawn so far (0: the main one).
    pub fn ids(&self) -> impl Iterator<Item = u64> + '_ {
        self.browsers.keys().copied()
    }

    /// Forgets the browsers that were closed (the main one, 0, stays).
    pub fn retain(&mut self, keep: impl Fn(u64) -> bool) {
        self.browsers.retain(|&id, _| id == 0 || keep(id));
    }

    /// Shows a resource in the main Content Browser: its stream, the
    /// resource selected (stage 56, "Show in Content Browser").
    pub fn reveal(&mut self, key: &ResourceKey) {
        let state = self.state(0);
        state.go(Place::Stream(key.stream.clone()));
        state.selected = Some(Selected::Asset(key.clone()));
    }

    fn state(&mut self, id: u64) -> &mut ContentState {
        let (preset, tree) = (&self.preset, &self.tree);
        self.browsers.entry(id).or_insert_with(|| match preset {
            Some(p) => ContentState::from_preset(p, tree),
            None => ContentState::default(),
        })
    }

    /// Follows the game and the workspace: the tree, the marks and what
    /// the previews read.
    fn sync(
        &mut self,
        game: &Arc<doctor_toolkit::game::GameInstance>,
        index: &Arc<doctor_toolkit::game::ResourceIndex>,
        ws: Option<&Workspace>,
    ) {
        let made_for = (
            Arc::as_ptr(game) as usize,
            ws.map(|w| (w.head(), w.operations().len(), w.len())),
        );
        if self.made_for == Some(made_for) && !self.stale {
            return;
        }
        self.stale = false;
        let new_game = self.made_for.is_none_or(|(g, _)| g != made_for.0);
        self.made_for = Some(made_for);
        self.tree = Arc::new(FolderTree::of_game(game, ws));
        self.marks = Arc::new(Marks::of(ws));
        self.revision += 1;
        self.previews.set_source(game.clone(), index.clone(), ws);
        if new_game {
            let preset = self.preset.clone();
            for state in self.browsers.values_mut() {
                let mut fresh = match &preset {
                    Some(p) => ContentState::from_preset(p, &self.tree),
                    None => ContentState::default(),
                };
                (fresh.mode, fresh.tile) = (state.mode, state.tile);
                *state = fresh;
            }
        }
    }

    /// The game files changed: previews are made again.
    pub fn files_changed(&mut self) {
        self.previews.files_changed();
        self.stale = true;
    }

    /// Plays the first sound of a bank, decoded in the background.
    fn play(&mut self, game: Arc<doctor_toolkit::game::GameInstance>, asset: &Asset) {
        #[cfg(feature = "audio")]
        if self
            .playing
            .as_ref()
            .is_some_and(|(k, p)| *k == asset.key && !p.finished())
        {
            self.playing = None;
            return;
        }
        let Some(at) = asset.at else { return };
        let key = asset.key.clone();
        let (tx, rx) = channel();
        self.sound = Some(rx);
        std::thread::spawn(move || {
            let result = game
                .open_stream(at.stream)
                .and_then(|s| s.resource_data(at.resource))
                .map_err(|e| e.to_string())
                .and_then(|data| {
                    let sounds = doctor_toolkit::resource::audio::parse_sbk(&data)
                        .map_err(|e| e.to_string())?;
                    let sample = &sounds.first().ok_or(tr("content.no_sounds"))?.sample;
                    let pcm = sample.decode().map_err(|e| e.to_string())?;
                    Ok((
                        pcm,
                        sample.header.channels as u16,
                        sample.header.sample_rate,
                    ))
                });
            let _ = tx.send((key, result));
        });
    }

    /// The sound bank whose first sound is playing.
    fn playing_key(&self) -> Vec<ResourceKey> {
        #[cfg(feature = "audio")]
        {
            self.playing
                .iter()
                .filter(|(_, p)| !p.finished())
                .map(|(k, _)| k.clone())
                .collect()
        }
        #[cfg(not(feature = "audio"))]
        {
            Vec::new()
        }
    }

    /// Takes a decoded sound and plays it; returns an error to show.
    fn poll_sound(&mut self) -> Option<String> {
        let (key, result) = self.sound.as_ref()?.try_recv().ok()?;
        self.sound = None;
        match result {
            #[cfg(feature = "audio")]
            Ok((pcm, channels, rate)) => match crate::audio_out::play(&pcm, channels, rate) {
                Ok(playback) => {
                    self.playing = Some((key, playback));
                    None
                }
                Err(error) => Some(trf("sound.cannot_play", &[("error", &error)])),
            },
            #[cfg(not(feature = "audio"))]
            Ok(_) => {
                let _ = key;
                Some(tr("sound.no_playback").to_owned())
            }
            Err(error) => Some(trf("sound.cannot_play", &[("error", &error)])),
        }
    }
}

/// The type ids of a category, for a search with a type filter.
fn types_of(categories: &[Category]) -> Vec<u32> {
    static TAGS: OnceLock<Vec<u32>> = OnceLock::new();
    let all = TAGS.get_or_init(|| {
        include_str!("../../../../../data/gf2/type_tags.txt")
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|tag| sdbm(tag.as_bytes()))
            .collect()
    });
    all.iter()
        .copied()
        .filter(|&t| categories.iter().any(|c| c.accepts(t)))
        .collect()
}

/// A short text of a size.
fn size_text(bytes: u64) -> String {
    super::human_size(bytes)
}

/// The preview a tile asks for, if its asset has one.
fn preview_job(asset: &Asset) -> Option<PreviewJob> {
    let kind = match asset.category {
        Category::Texture => Kind::Texture,
        Category::Model => Kind::Model,
        _ => return None,
    };
    Some(PreviewJob {
        id: PreviewId {
            key: asset.key.clone(),
            version: asset.version.clone(),
        },
        at: asset.at,
        kind,
    })
}

/// What a click, a double click, a drag or the context menu asks for.
enum Act {
    Select(Selected),
    Go(Place),
    Details(Location),
    Open(Asset),
    OpenStream(usize),
    Reveal(Asset),
    CopyKey(String),
    Clone(Asset),
    Export(Location),
    Play(Asset),
    Drag(Asset),
    NewBrowser,
}

impl BrowserApp {
    /// A Content Browser panel; `id` 0 is the main one (the drawer).
    pub(super) fn content_pane(&mut self, ui: &mut egui::Ui, id: u64) {
        let (Some(game), Some(index)) = (self.game.clone(), self.index.clone()) else {
            ui.label(RichText::new(tr("files.no_game")).weak());
            return;
        };
        self.content.sync(&game, &index, self.workspace.as_ref());
        let names = self.nav.index.clone();
        let tree = self.content.tree.clone();
        let marks = self.content.marks.clone();
        let revision = self.content.revision;
        self.content.state(id);
        let mut acts = Vec::new();

        let state = self.content.browsers.get_mut(&id).expect("made above");
        poll_search(state, names.as_ref(), ui.ctx());
        let source = catalog::Source {
            game: &game,
            index: &index,
            names: names.as_deref(),
            marks: &marks,
        };
        relist(state, &source, &tree, revision);
        if let Some(text) = state.select.clone()
            && names.is_some()
            && let Some(asset) = state
                .listing
                .assets
                .iter()
                .find(|a| a.name.to_lowercase().contains(&text))
        {
            state.select = None;
            state.selected = Some(Selected::Asset(asset.key.clone()));
            if let Some(at) = asset.at {
                acts.push(Act::Details(at));
            }
        }

        top_bar(ui, state, &tree, &mut acts);
        let state = self.content.browsers.get_mut(&id).expect("made above");
        filter_bar(ui, state, names.is_some(), &mut acts);
        ui.add_space(2.0);
        egui::Panel::left(egui::Id::unique(("content-tree", id)))
            .resizable(true)
            .default_size(200.0)
            .min_size(120.0)
            .show(ui, |ui| folder_tree(ui, id, state, &tree));
        // The status line under the tiles.
        let stats = self.content.previews.stats;
        let memory = self.content.previews.memory();
        let state = self.content.browsers.get_mut(&id).expect("made above");
        egui::Panel::bottom(egui::Id::unique(("content-status", id)))
            .show_separator_line(false)
            .show(ui, |ui| {
                let mut text = trn("content.items", state.listing.len() as u64, &[]);
                if state.listing.truncated {
                    text += &trf("content.truncated", &[("limit", &FOLDER_LIMIT)]);
                }
                if stats.made + stats.from_disk > 0 {
                    let each = |time: Duration, n: usize| {
                        format!("{:.0}", time.as_secs_f64() * 1000.0 / n.max(1) as f64)
                    };
                    let textures = stats.made - stats.models;
                    let mut parts = Vec::new();
                    if stats.models > 0 {
                        parts.push(trf(
                            "content.stats_models",
                            &[
                                ("n", &stats.models),
                                ("ms", &each(stats.models_time, stats.models)),
                            ],
                        ));
                    }
                    if textures > 0 {
                        parts.push(trf(
                            "content.stats_textures",
                            &[
                                ("n", &textures),
                                ("ms", &each(stats.made_time - stats.models_time, textures)),
                            ],
                        ));
                    }
                    if stats.from_disk > 0 {
                        parts.push(trf("content.stats_disk", &[("n", &stats.from_disk)]));
                    }
                    parts.push(trf("content.stats_memory", &[("mib", &(memory >> 20))]));
                    text += &trf("content.preview_stats", &[("parts", &parts.join(", "))]);
                }
                if names.is_none() {
                    text += tr("content.names_pending");
                }
                ui.label(RichText::new(text).weak().small());
            });
        let playing = self.content.playing_key();
        let state = self.content.browsers.get_mut(&id).expect("made above");
        let previews = &mut self.content.previews;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| match state.mode {
                Mode::Grid => grid(ui, id, state, previews, &playing, &mut acts),
                Mode::List => list(ui, id, state, previews, &mut acts),
            });
        let state = self.content.browsers.get_mut(&id).expect("made above");
        if state.selected.is_some()
            && ui.ctx().input(|i| i.key_pressed(egui::Key::Escape))
            && ui.ui_contains_pointer()
        {
            state.selected = None;
        }
        self.content_acts(ui.ctx(), acts, &game, names.as_ref());
    }

    fn content_acts(
        &mut self,
        ctx: &egui::Context,
        acts: Vec<Act>,
        game: &Arc<doctor_toolkit::game::GameInstance>,
        names: Option<&Arc<GameIndex>>,
    ) {
        for act in acts {
            match act {
                Act::Select(_) | Act::Go(_) => {}
                Act::Details(loc) => {
                    self.nav.visit(loc);
                    let previous = self.details.take();
                    self.details = Some(self.load_doc(loc, previous));
                }
                Act::Open(asset) => match asset.at {
                    Some(loc) => {
                        self.open_resource_tab(loc);
                    }
                    None => self.open_key(&asset.key),
                },
                Act::OpenStream(i) => self.open_file(FileSel::Stream(i), false),
                Act::Reveal(asset) => match asset.at {
                    Some(loc) => {
                        self.open_location(loc);
                        self.dock_ops.push(DockOp::ShowPanel(Pane::Files));
                    }
                    None => self.open_key(&asset.key),
                },
                Act::CopyKey(text) => {
                    self.status = trf("content.copied", &[("key", &text)]);
                    ctx.copy_text(text);
                }
                Act::Clone(asset) => self.content_clone(&asset),
                Act::Export(loc) => self.nav.export(game, vec![loc]),
                Act::Play(asset) => self.content.play(game.clone(), &asset),
                Act::Drag(asset) => {
                    let dragged = dragged_asset(&asset, game, names);
                    if asset.key.type_id == sdbm(b"mesh") && dragged.type_id != sdbm(b"metamodel") {
                        self.status = tr("content.mesh_without_model").to_owned();
                    }
                    level_drop::set_ghost(
                        ctx,
                        Ghost {
                            asset: dragged,
                            label: asset.name.clone(),
                            bounds: self.content.previews.bounds_of(&asset.key),
                        },
                    );
                    self.content.dragging = Some((dragged, asset));
                    egui::DragAndDrop::set_payload(ctx, dragged);
                }
                Act::NewBrowser => self.pending.push(super::Command::NewContentBrowser),
            }
        }
    }

    /// Clones an asset (with its companions) into its stream in the
    /// workspace: one undoable operation.
    fn content_clone(&mut self, asset: &Asset) {
        use doctor_toolkit::create::{self, CloneOptions};
        use doctor_toolkit::workspace::{NewResourceEdit, StorageChoice};
        let (Some(game), Some(index), Some(loc)) =
            (self.game.clone(), self.index.clone(), asset.at)
        else {
            return;
        };
        let result = (|| -> Result<(String, usize), String> {
            let file = game.open_stream(loc.stream).map_err(|e| e.to_string())?;
            let name = game.streams()[loc.stream].stream_name();
            let ws = self.workspace_mut()?;
            let mut used = ws.used_ids(&index);
            let options = CloneOptions {
                seed: name.clone(),
                rewrite_references: true,
                storage: None,
            };
            let (clones, _) = create::clone_resources(&file, &[loc.resource], &mut used, &options)
                .map_err(|e| e.to_string())?;
            let last = clones.iter().filter_map(|c| c.source).max().unwrap_or(0);
            let before = file
                .layout()
                .resources
                .get(last + 1)
                .map(|r| ResourceKey::new(&name, &r.header));
            let additions: Vec<NewResourceEdit> = clones
                .iter()
                .map(|c| NewResourceEdit {
                    stream: name.clone(),
                    header: c.header,
                    data: c.data.clone(),
                    storage: match c.storage {
                        doctor_toolkit::container::ustream::Storage::Raw => StorageChoice::Raw,
                        doctor_toolkit::container::ustream::Storage::RefPack => {
                            StorageChoice::Refpack
                        }
                    },
                    before: before.clone(),
                })
                .collect();
            let label = trf("content.clone_label", &[("name", &asset.name)]);
            ws.add_resources(&game, &label, &additions)
                .map_err(|e| e.to_string())?;
            Ok((label, additions.len()))
        })();
        match result {
            Ok((label, count)) => {
                self.status = trn("content.cloned", count as u64, &[("label", &label)]);
                self.after_edit(Some(loc.stream));
            }
            Err(error) => self.notify(
                super::Level::Error,
                trf("content.cannot_clone", &[("error", &error)]),
            ),
        }
    }

    /// While an asset is dragged: its preview and name at the pointer, and
    /// the drawer it came from gets out of the way.
    pub(super) fn content_drag(&mut self, ctx: &egui::Context) {
        if let Some(error) = self.content.poll_sound() {
            self.notify(super::Level::Warning, error);
        }
        let Some(dragged) = egui::DragAndDrop::payload::<DraggedAsset>(ctx) else {
            self.content.dragging = None;
            return;
        };
        let Some((_, asset)) = self
            .content
            .dragging
            .as_ref()
            .filter(|(d, _)| *d == *dragged)
        else {
            return;
        };
        let Some(pointer) = ctx.input(|i| i.pointer.latest_pos()) else {
            return;
        };
        let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("content-drag"));
        let painter = ctx.layer_painter(layer);
        let at = pointer + vec2(14.0, 14.0);
        let size = 64.0;
        let mut x = at.x;
        if let Some(texture) = self.content.previews.ready(&asset.key) {
            let rect = Rect::from_min_size(at, Vec2::splat(size));
            painter.rect_filled(rect, 3.0, Color32::from_black_alpha(140));
            painter.image(
                texture.id(),
                rect,
                Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                Color32::WHITE,
            );
            x += size + 6.0;
        }
        let text = if dragged.type_id == sdbm(b"metamodel") {
            trf("content.drag_model", &[("name", &asset.name)])
        } else {
            asset.name.clone()
        };
        let galley = painter.layout_no_wrap(
            text,
            egui::FontId::proportional(13.0),
            ctx.global_style().visuals.text_color(),
        );
        let rect = Rect::from_min_size(pos2(x, at.y), galley.size()).expand(4.0);
        painter.rect_filled(rect, 3.0, ctx.global_style().visuals.panel_fill);
        painter.galley(pos2(x, at.y), galley, Color32::WHITE);
    }
}

/// The payload of a dragged asset. A mesh is placed by a metamodel that
/// shows it; an asset the workspace adds has no resource index
/// (`usize::MAX`).
fn dragged_asset(
    asset: &Asset,
    game: &doctor_toolkit::game::GameInstance,
    names: Option<&Arc<GameIndex>>,
) -> DraggedAsset {
    let at = asset.at.map(|l| (l.stream, l.resource));
    if asset.key.type_id == sdbm(b"mesh")
        && let (Some(names), Some((s, r))) = (names, at)
        && let Some(record) = names.locate(s, r)
        && let Some(by) = names
            .used_by(record)
            .iter()
            .map(|(by, _)| *by)
            .find(|by| names.record(*by).header.type_id == sdbm(b"metamodel"))
    {
        let record = names.record(by);
        return DraggedAsset {
            type_id: record.header.type_id,
            id: record.header.ids[3],
            stream: names.stream_of(by),
            resource: record.resource as usize,
        };
    }
    let (stream, resource) = at.unwrap_or_else(|| {
        (
            game.find_stream(&asset.key.stream).unwrap_or(usize::MAX),
            usize::MAX,
        )
    });
    DraggedAsset {
        type_id: asset.key.type_id,
        id: asset.key.ids[3],
        stream,
        resource,
    }
}

/// Starts or takes the search of the game index.
fn poll_search(state: &mut ContentState, names: Option<&Arc<GameIndex>>, ctx: &egui::Context) {
    if let Some(job) = &state.job
        && let Ok(result) = job.rx.try_recv()
    {
        let text = job.text.clone();
        state.job = None;
        state.results = Some((text, result));
    }
    let text = state.search.trim().to_owned();
    if text.is_empty() {
        state.results = None;
        state.edited = None;
        return;
    }
    let index_at = names.map_or(0, |n| Arc::as_ptr(n) as usize);
    if state.job.is_none()
        && state.edited.is_none()
        && state.results.is_some()
        && state.searched != (state.place.clone(), index_at)
    {
        state.edited = Some(Instant::now() - SEARCH_DELAY);
    }
    let due = state.edited.is_some_and(|t| t.elapsed() >= SEARCH_DELAY);
    if !due {
        if state.edited.is_some() {
            ctx.request_repaint_after(SEARCH_DELAY);
        }
        return;
    }
    let Some(index) = names.cloned() else {
        return;
    };
    state.edited = None;
    state.searched = (state.place.clone(), index_at);
    let mut query = match Query::parse(&text) {
        Ok(q) => q,
        Err(error) => {
            state.results = Some((text, Err(error.to_string())));
            return;
        }
    };
    if !state.place.path().is_empty() {
        query.streams.push(state.place.path().to_owned());
    }
    if query.types.is_empty() {
        query.types = types_of(&state.filter.categories);
    }
    let (tx, rx) = channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let (results, _) = doctor_toolkit::navigation::search::search(&index, &query, SEARCH_LIMIT);
        let _ = tx.send(Ok(results.into_iter().map(|r| r.record).collect()));
        ctx.request_repaint();
    });
    state.job = Some(SearchJob { text, rx });
}

/// Lists the place again when what it shows changed.
fn relist(state: &mut ContentState, source: &catalog::Source, tree: &FolderTree, revision: u64) {
    let search = state
        .results
        .as_ref()
        .filter(|_| !state.search.trim().is_empty())
        .map(|(t, _)| t.clone());
    let key = ListingKey {
        place: state.place.clone(),
        filter: state.filter.clone(),
        search,
        names: source.names.is_some(),
        revision,
        copies: state.copies,
    };
    if state.key.as_ref() == Some(&key) {
        return;
    }
    let mut listing = Listing::default();
    match (&key.search, &state.results) {
        (Some(_), Some((_, Ok(ids)))) => {
            for &id in ids {
                let Some(asset) = source.record_asset(id) else {
                    continue;
                };
                if state.place.contains(&asset.key.stream) && state.filter.passes(asset.key.type_id)
                {
                    listing.assets.push(asset);
                }
            }
            catalog::sort(&mut listing.assets);
        }
        (Some(_), _) => {}
        (None, _) => match &state.place {
            Place::Folder(path) => {
                if let Some(folder) = tree.folder(path) {
                    listing.folders = folder
                        .children
                        .iter()
                        .map(|&c| {
                            let f = &tree.folders[c];
                            FolderItem {
                                path: f.path.clone(),
                                name: f.name.clone(),
                                total: f.total,
                            }
                        })
                        .collect();
                    listing.streams.clone_from(&folder.streams);
                }
                if !state.filter.categories.is_empty() {
                    for stream in tree.streams_under(path) {
                        listing
                            .assets
                            .extend(source.stream_assets(stream, &state.filter));
                        if listing.assets.len() > FOLDER_LIMIT {
                            listing.assets.truncate(FOLDER_LIMIT);
                            listing.truncated = true;
                            break;
                        }
                    }
                    catalog::sort(&mut listing.assets);
                }
            }
            Place::Stream(path) => {
                if let Some(stream) = tree.stream(path) {
                    listing.assets = source.stream_assets(stream, &state.filter);
                    catalog::sort(&mut listing.assets);
                }
            }
        },
    }
    if !state.copies {
        let mut seen = std::collections::HashSet::new();
        listing
            .assets
            .retain(|a| seen.insert((a.key.type_id, a.key.ids)));
    }
    state.listing = listing;
    state.key = Some(key);
}

/// Back, Up, the path as buttons.
fn top_bar(ui: &mut egui::Ui, state: &mut ContentState, tree: &FolderTree, acts: &mut Vec<Act>) {
    // Wrapped: a long path goes on to the next line rather than widen the
    // panel.
    ui.horizontal_wrapped(|ui| {
        let parent = match &state.place {
            Place::Stream(p) | Place::Folder(p) if !p.is_empty() => Some(Place::Folder(
                p.rsplit_once('/').map_or("", |(f, _)| f).to_owned(),
            )),
            _ => None,
        };
        if ui
            .add_enabled(
                !state.back.is_empty(),
                egui::Button::new(tr("content.back")),
            )
            .on_hover_text(tr("content.back_hint"))
            .clicked()
            && let Some(place) = state.back.pop()
        {
            state.place = place;
            state.reveal = true;
            state.scroll_top = true;
        }
        if ui
            .add_enabled(parent.is_some(), egui::Button::new(tr("content.up")))
            .on_hover_text(tr("content.up_hint"))
            .clicked()
            && let Some(p) = parent
        {
            state.go(p);
        }
        ui.add_space(4.0);
        // Breadcrumbs.
        let mut go = None;
        if ui
            .add(egui::Button::new(RichText::new(tr("content.root")).strong()).frame(false))
            .clicked()
        {
            go = Some(Place::Folder(String::new()));
        }
        let path = state.place.path().to_owned();
        let crumbs = FolderTree::crumbs(&path);
        let last = crumbs.len().saturating_sub(1);
        for (i, crumb) in crumbs.into_iter().enumerate() {
            ui.label(RichText::new("›").weak());
            let name = crumb.rsplit('/').next().unwrap_or(crumb);
            let is_stream = i == last && matches!(state.place, Place::Stream(_));
            let text = if is_stream {
                RichText::new(name).color(theme::ACCENT)
            } else {
                RichText::new(name)
            };
            if ui.add(egui::Button::new(text).frame(false)).clicked() {
                go = Some(if is_stream {
                    Place::Stream(crumb.to_owned())
                } else {
                    Place::Folder(crumb.to_owned())
                });
            }
        }
        if let Some(place) = go {
            state.go(place);
        }
        if let Place::Stream(p) = &state.place
            && let Some(i) = tree.stream(p).and_then(|s| s.game)
        {
            ui.add_space(6.0);
            if ui
                .small_button(tr("content.open_stream"))
                .on_hover_text(tr("content.open_stream_hint"))
                .clicked()
            {
                acts.push(Act::OpenStream(i));
            }
        }
    });
}

/// The search box with the size of the tiles, the mode and a new browser;
/// the category filters under it.
fn filter_bar(ui: &mut egui::Ui, state: &mut ContentState, names: bool, acts: &mut Vec<Act>) {
    ui.horizontal(|ui| {
        let search = ui
            .add(
                egui::TextEdit::singleline(&mut state.search)
                    .hint_text(if names {
                        tr("content.search")
                    } else {
                        tr("content.search_wait")
                    })
                    .desired_width(200.0),
            )
            .on_hover_text(tr("content.search_hint"));
        if search.changed() {
            state.edited = Some(Instant::now());
        }
        if search.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            state.edited = Some(Instant::now() - SEARCH_DELAY);
        }
        if state.job.is_some() {
            ui.spinner();
        }
        if let Some((_, Err(error))) = &state.results {
            ui.colored_label(Color32::from_rgb(0xe5, 0x53, 0x4b), error);
        }
        if !matches!(state.place, Place::Stream(_)) || !state.search.trim().is_empty() {
            ui.checkbox(&mut state.copies, tr("content.copies"))
                .on_hover_text(tr("content.copies_hint"));
        }
        ui.add_space(8.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .small_button("+")
                .on_hover_text(super::Command::NewContentBrowser.label())
                .clicked()
            {
                acts.push(Act::NewBrowser);
            }
            if ui
                .selectable_label(state.mode == Mode::List, tr("content.list"))
                .clicked()
            {
                state.mode = Mode::List;
            }
            if ui
                .selectable_label(state.mode == Mode::Grid, tr("content.tiles"))
                .clicked()
            {
                state.mode = Mode::Grid;
            }
            ui.spacing_mut().slider_width = 72.0;
            ui.add_enabled(
                state.mode == Mode::Grid,
                egui::Slider::new(&mut state.tile, TILE.0..=TILE.1)
                    .show_value(false)
                    .step_by(4.0),
            )
            .on_hover_text(tr("content.size_hint"));
        });
    });
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let all = state.filter.categories.is_empty();
        if ui
            .selectable_label(all, tr("content.all_types"))
            .on_hover_text(tr("content.all_types_hint"))
            .clicked()
        {
            state.filter.categories.clear();
            state.edited = state.edited.or(Some(Instant::now() - SEARCH_DELAY));
        }
        for category in Category::ALL {
            let on = state.filter.categories.contains(&category);
            let text = RichText::new(category.label()).color(if on {
                ui.visuals().strong_text_color()
            } else {
                ui.visuals().text_color()
            });
            let response = ui.selectable_label(on, text);
            // The colour of the category under its chip.
            let r = response.rect;
            ui.painter().hline(
                r.x_range().shrink(3.0),
                r.bottom() - 1.0,
                Stroke::new(
                    2.0,
                    category.color().gamma_multiply(if on { 1.0 } else { 0.5 }),
                ),
            );
            if response.clicked() {
                // Ctrl adds to the filter; a plain click shows this one.
                if ui.input(|i| i.modifiers.command) {
                    if on {
                        state.filter.categories.retain(|c| *c != category);
                    } else {
                        state.filter.categories.push(category);
                    }
                } else if on && state.filter.categories.len() == 1 {
                    state.filter.categories.clear();
                } else {
                    state.filter.categories = vec![category];
                }
                if !state.search.trim().is_empty() {
                    state.edited = Some(Instant::now() - SEARCH_DELAY);
                }
            }
        }
    });
}

/// The folders and streams on the left.
fn folder_tree(ui: &mut egui::Ui, id: u64, state: &mut ContentState, tree: &FolderTree) {
    let reveal = std::mem::take(&mut state.reveal);
    let open: Vec<String> = if reveal {
        let mut v: Vec<String> = FolderTree::crumbs(state.place.path())
            .into_iter()
            .map(str::to_owned)
            .collect();
        if matches!(state.place, Place::Stream(_)) {
            v.pop();
        }
        v
    } else {
        Vec::new()
    };
    egui::ScrollArea::both()
        .id_salt(("content-tree", id))
        .auto_shrink(false)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            let mut go = None;
            node(ui, id, tree, 0, &state.place, &open, reveal, &mut go);
            if let Some(place) = go {
                state.go(place);
                state.reveal = false;
            }
        });
}

#[allow(clippy::too_many_arguments)]
fn node(
    ui: &mut egui::Ui,
    id: u64,
    tree: &FolderTree,
    f: usize,
    place: &Place,
    open: &[String],
    reveal: bool,
    go: &mut Option<Place>,
) {
    let folder = &tree.folders[f];
    let key = ui.make_persistent_id(("content-node", id, &folder.path));
    let mut collapsing =
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), key, f == 0);
    if open.contains(&folder.path) || (f == 0 && reveal) {
        collapsing.set_open(true);
    }
    let selected = matches!(place, Place::Folder(p) if *p == folder.path);
    let header = collapsing.show_header(ui, |ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
        theme::paint_icon(
            ui.painter(),
            rect,
            Icon::Folder,
            if selected {
                theme::ACCENT
            } else {
                ui.visuals().weak_text_color()
            },
        );
        let name = if f == 0 {
            tr("content.root").to_owned()
        } else {
            folder.name.clone()
        };
        let response = ui.selectable_label(selected, name).on_hover_text(trn(
            "count.streams",
            folder.total as u64,
            &[],
        ));
        if selected && reveal {
            response.scroll_to_me(Some(egui::Align::Center));
        }
        if response.clicked() {
            *go = Some(Place::Folder(folder.path.clone()));
        }
    });
    header.body(|ui| {
        for &c in &folder.children {
            node(ui, id, tree, c, place, open, reveal, go);
        }
        for stream in &folder.streams {
            let on = matches!(place, Place::Stream(p) if *p == stream.path);
            let text = if stream.game.is_none() {
                RichText::new(format!("{}  ({})", stream.name, tr("content.badge_new"))).color(NEW)
            } else {
                RichText::new(&stream.name)
            };
            let response = ui
                .horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                    stream_icon(ui.painter(), rect);
                    ui.selectable_label(on, text)
                })
                .inner
                .on_hover_text(format!("{}\n{}", stream.path, size_text(stream.size)));
            if on && reveal {
                response.scroll_to_me(Some(egui::Align::Center));
            }
            if response.clicked() {
                *go = Some(Place::Stream(stream.path.clone()));
            }
        }
    });
}

/// A stream: a box of assets.
fn stream_icon(painter: &egui::Painter, rect: Rect) {
    let r = rect.shrink(rect.width() * 0.14);
    let stroke = Stroke::new((rect.width() / 14.0).max(1.3), theme::ACCENT);
    painter.rect_stroke(r, 2.0, stroke, egui::StrokeKind::Inside);
    painter.hline(r.x_range(), r.top() + r.height() * 0.35, stroke);
    painter.vline(r.center().x, r.top()..=r.top() + r.height() * 0.35, stroke);
}

/// Green of new assets, amber of changed ones.
const NEW: Color32 = Color32::from_rgb(0x57, 0xab, 0x5a);
const CHANGED: Color32 = theme::ACCENT;

/// The tiles.
fn grid(
    ui: &mut egui::Ui,
    id: u64,
    state: &mut ContentState,
    previews: &mut Previews,
    playing: &[ResourceKey],
    acts: &mut Vec<Act>,
) {
    let n = state.listing.len();
    if n == 0 {
        empty(ui, state);
        return;
    }
    let tile = state.tile;
    let size = vec2(tile + 8.0, tile + 52.0);
    let width = ui.available_width() - 12.0;
    let columns = (((width + GAP) / (size.x + GAP)).floor() as usize).max(1);
    let rows = n.div_ceil(columns);
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt(("content-grid", id))
        .auto_shrink(false);
    if std::mem::take(&mut state.scroll_top) {
        scroll = scroll.vertical_scroll_offset(0.0);
    }
    scroll.show_rows(ui, size.y + GAP, rows, |ui, range| {
        ui.spacing_mut().item_spacing = vec2(GAP, GAP);
        for row in range {
            ui.horizontal(|ui| {
                for c in 0..columns {
                    let i = row * columns + c;
                    let Some(item) = state.listing.item(i) else {
                        break;
                    };
                    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
                    if ui.is_rect_visible(rect) {
                        tile_ui(
                            ui,
                            rect,
                            &response,
                            &item,
                            &state.selected,
                            previews,
                            playing,
                            acts,
                        );
                    }
                    interact(&response, &item, acts);
                }
            });
        }
    });
    apply_selection(state, acts);
}

/// The rows of the list.
fn list(
    ui: &mut egui::Ui,
    id: u64,
    state: &mut ContentState,
    previews: &mut Previews,
    acts: &mut Vec<Act>,
) {
    let n = state.listing.len();
    if n == 0 {
        empty(ui, state);
        return;
    }
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt(("content-list", id))
        .auto_shrink(false);
    if std::mem::take(&mut state.scroll_top) {
        scroll = scroll.vertical_scroll_offset(0.0);
    }
    scroll.show_rows(ui, ROW, n, |ui, range| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for i in range {
            let Some(item) = state.listing.item(i) else {
                break;
            };
            let width = ui.available_width();
            let (rect, response) =
                ui.allocate_exact_size(vec2(width, ROW), Sense::click_and_drag());
            let selected = is_selected(&item, &state.selected);
            let visuals = ui.visuals();
            if selected {
                ui.painter()
                    .rect_filled(rect, 2.0, visuals.selection.bg_fill.gamma_multiply(0.6));
            } else if response.hovered() {
                ui.painter()
                    .rect_filled(rect, 2.0, visuals.widgets.hovered.weak_bg_fill);
            }
            let icon = Rect::from_min_size(rect.min + vec2(2.0, 1.0), Vec2::splat(ROW - 2.0));
            let text_color = visuals.text_color();
            let weak = visuals.weak_text_color();
            let (name, kind, place, size, mark, color) = match &item {
                Item::Folder(f) => {
                    theme::paint_icon(ui.painter(), icon, Icon::Folder, weak);
                    (
                        f.name.clone(),
                        tr("content.folder").to_owned(),
                        f.path.clone(),
                        trn("count.streams", f.total as u64, &[]),
                        Mark::None,
                        weak,
                    )
                }
                Item::Stream(s) => {
                    stream_icon(ui.painter(), icon);
                    (
                        s.name.clone(),
                        tr("content.stream").to_owned(),
                        s.path.clone(),
                        size_text(s.size),
                        if s.game.is_none() {
                            Mark::New
                        } else {
                            Mark::None
                        },
                        theme::ACCENT,
                    )
                }
                Item::Asset(a) => {
                    match preview_job(a).map(|job| previews.get(&job)) {
                        Some(Shown::Ready(texture)) => {
                            ui.painter().image(
                                texture.id(),
                                fit(icon, texture.size_vec2()),
                                Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                                Color32::WHITE,
                            );
                        }
                        _ => {
                            ui.painter()
                                .rect_filled(icon.shrink(3.0), 2.0, a.category.color());
                        }
                    }
                    (
                        a.name.clone(),
                        a.type_name(),
                        a.key.stream.clone(),
                        size_text(a.size as u64),
                        a.mark,
                        a.category.color(),
                    )
                }
            };
            let painter = ui.painter_at(rect);
            let y = rect.center().y;
            let font = egui::FontId::proportional(13.0);
            let x0 = icon.right() + 6.0;
            let columns = [
                x0,
                rect.left() + width * 0.45,
                rect.left() + width * 0.58,
                rect.left() + width * 0.88,
            ];
            painter.text(
                pos2(x0, y),
                egui::Align2::LEFT_CENTER,
                &name,
                font.clone(),
                text_color,
            );
            painter.text(
                pos2(columns[1], y),
                egui::Align2::LEFT_CENTER,
                &kind,
                font.clone(),
                color,
            );
            painter.text(
                pos2(columns[2], y),
                egui::Align2::LEFT_CENTER,
                &place,
                font.clone(),
                weak,
            );
            let size_x = columns[3];
            painter.text(
                pos2(size_x, y),
                egui::Align2::LEFT_CENTER,
                &size,
                font,
                weak,
            );
            if mark != Mark::None {
                badge(
                    &painter,
                    pos2(rect.right() - 4.0, y),
                    mark,
                    egui::Align2::RIGHT_CENTER,
                );
            }
            interact(&response, &item, acts);
        }
    });
    apply_selection(state, acts);
}

/// The text of an empty place.
fn empty(ui: &mut egui::Ui, state: &ContentState) {
    ui.add_space(12.0);
    ui.vertical_centered(|ui| {
        let text = if state.job.is_some() {
            tr("content.searching")
        } else if !state.search.trim().is_empty() {
            tr("palette.nothing")
        } else if matches!(state.place, Place::Stream(_)) {
            tr("content.nothing_here")
        } else {
            tr("content.empty_folder")
        };
        ui.label(RichText::new(text).weak());
    });
}

fn is_selected(item: &Item, selected: &Option<Selected>) -> bool {
    match (item, selected) {
        (Item::Folder(f), Some(Selected::Folder(p))) => f.path == *p,
        (Item::Stream(s), Some(Selected::Stream(p))) => s.path == *p,
        (Item::Asset(a), Some(Selected::Asset(k))) => a.key == *k,
        _ => false,
    }
}

/// `size` fitted into `rect`, centred.
fn fit(rect: Rect, size: Vec2) -> Rect {
    let scale = (rect.width() / size.x.max(1.0)).min(rect.height() / size.y.max(1.0));
    Rect::from_center_size(rect.center(), size * scale)
}

/// "NEW" or "EDITED" at `at`.
fn badge(painter: &egui::Painter, at: Pos2, mark: Mark, align: egui::Align2) {
    let (text, color) = match mark {
        Mark::New => (tr("content.badge_new"), NEW),
        Mark::Changed => (tr("content.badge_changed"), CHANGED),
        Mark::None => return,
    };
    let galley = painter.layout_no_wrap(
        text.to_owned(),
        egui::FontId::proportional(10.0),
        Color32::from_rgb(0x10, 0x10, 0x10),
    );
    let rect = align.anchor_size(at, galley.size() + vec2(6.0, 2.0));
    painter.rect_filled(rect, 3.0, color);
    painter.galley(rect.min + vec2(3.0, 1.0), galley, Color32::BLACK);
}

/// One tile.
#[allow(clippy::too_many_arguments)]
fn tile_ui(
    ui: &egui::Ui,
    rect: Rect,
    response: &egui::Response,
    item: &Item,
    selected: &Option<Selected>,
    previews: &mut Previews,
    playing: &[ResourceKey],
    acts: &mut Vec<Act>,
) {
    let visuals = ui.visuals();
    let painter = ui.painter_at(rect.expand(1.0));
    let is_selected = is_selected(item, selected);
    let fill = if is_selected {
        visuals.selection.bg_fill.gamma_multiply(0.55)
    } else if response.hovered() {
        visuals.widgets.hovered.weak_bg_fill
    } else {
        visuals.widgets.inactive.weak_bg_fill.gamma_multiply(0.7)
    };
    painter.rect_filled(rect, 4.0, fill);
    if is_selected {
        painter.rect_stroke(
            rect,
            4.0,
            Stroke::new(1.5, theme::FOCUS),
            egui::StrokeKind::Inside,
        );
    }
    let image = Rect::from_min_size(rect.min + vec2(4.0, 4.0), Vec2::splat(rect.width() - 8.0));
    let (name, kind, color, mark) = match item {
        Item::Folder(f) => {
            theme::paint_icon(
                &painter,
                image.shrink(image.width() * 0.18),
                Icon::Folder,
                visuals.weak_text_color(),
            );
            (
                f.name.clone(),
                trn("count.streams", f.total as u64, &[]),
                visuals.weak_text_color(),
                Mark::None,
            )
        }
        Item::Stream(s) => {
            stream_icon(&painter, image.shrink(image.width() * 0.12));
            (
                s.name.clone(),
                size_text(s.size),
                theme::ACCENT,
                if s.game.is_none() {
                    Mark::New
                } else {
                    Mark::None
                },
            )
        }
        Item::Asset(a) => {
            let shown = preview_job(a).map(|job| previews.get(&job));
            match shown {
                Some(Shown::Ready(texture)) => {
                    painter.image(
                        texture.id(),
                        fit(image, texture.size_vec2()),
                        Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
                other => {
                    placeholder(&painter, image, a, matches!(other, Some(Shown::Pending)));
                    if let Some(Shown::Failed(why)) = other {
                        response.clone().on_hover_text(why);
                    }
                }
            }
            if a.category == Category::Sound && a.at.is_some() {
                sound_button(ui, image, a, playing.contains(&a.key), acts);
            }
            (a.name.clone(), a.type_name(), a.category.color(), a.mark)
        }
    };
    // The colour of the type under the image.
    let strip = Rect::from_min_size(
        pos2(rect.left(), image.bottom() + 2.0),
        vec2(rect.width(), 2.0),
    );
    painter.rect_filled(strip, 0.0, color);
    // Name (two lines at most) and type.
    let mut job = egui::text::LayoutJob::simple(
        name,
        egui::FontId::proportional(12.0),
        visuals.text_color(),
        rect.width() - 8.0,
    );
    job.wrap.max_rows = 2;
    job.wrap.break_anywhere = true;
    let galley = painter.layout_job(job);
    painter.galley(
        pos2(rect.left() + 4.0, strip.bottom() + 2.0),
        galley,
        Color32::WHITE,
    );
    painter.text(
        pos2(rect.left() + 4.0, rect.bottom() - 3.0),
        egui::Align2::LEFT_BOTTOM,
        kind,
        egui::FontId::proportional(10.0),
        visuals.weak_text_color(),
    );
    if mark != Mark::None {
        badge(
            &painter,
            image.min + vec2(2.0, 2.0),
            mark,
            egui::Align2::LEFT_TOP,
        );
    }
}

/// A coloured square with the type of the asset while there is no preview.
fn placeholder(painter: &egui::Painter, rect: Rect, asset: &Asset, pending: bool) {
    let color = asset.category.color();
    painter.rect_filled(
        rect.shrink(rect.width() * 0.12),
        6.0,
        color.gamma_multiply(0.18),
    );
    painter.rect_stroke(
        rect.shrink(rect.width() * 0.12),
        6.0,
        Stroke::new(1.0, color.gamma_multiply(0.6)),
        egui::StrokeKind::Inside,
    );
    let size = (rect.width() * 0.11).clamp(9.0, 20.0);
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        asset.type_name(),
        egui::FontId::proportional(size),
        color,
    );
    if pending {
        // Three dots that move.
        let t = painter.ctx().input(|i| i.time) as f32;
        for k in 0..3 {
            let phase = ((t * 3.0 - k as f32 * 0.4).sin() + 1.0) / 2.0;
            painter.circle_filled(
                pos2(
                    rect.center().x + (k as f32 - 1.0) * 8.0,
                    rect.bottom() - rect.height() * 0.22,
                ),
                2.5,
                color.gamma_multiply(0.3 + 0.7 * phase),
            );
        }
        painter
            .ctx()
            .request_repaint_after(Duration::from_millis(60));
    }
}

/// Play / stop on a sound bank's tile.
fn sound_button(ui: &egui::Ui, image: Rect, asset: &Asset, playing: bool, acts: &mut Vec<Act>) {
    let r = (image.width() * 0.16).clamp(10.0, 18.0);
    let centre = image.right_bottom() - vec2(r + 4.0, r + 4.0);
    let rect = Rect::from_center_size(centre, Vec2::splat(r * 2.0));
    let response = ui
        .interact(
            rect,
            ui.scope_id().with(("play", &asset.key)),
            Sense::click(),
        )
        .on_hover_text(if playing {
            tr("content.stop")
        } else {
            tr("content.play_first")
        });
    let painter = ui.painter();
    let fill = if response.hovered() {
        theme::PLAY_HOVER
    } else {
        theme::PLAY
    };
    painter.circle_filled(centre, r, fill);
    let s = r * 0.45;
    if playing {
        painter.rect_filled(
            Rect::from_center_size(centre, Vec2::splat(s * 1.6)),
            1.0,
            Color32::WHITE,
        );
        ui.ctx().request_repaint_after(Duration::from_millis(100));
    } else {
        painter.add(egui::Shape::convex_polygon(
            vec![
                centre + vec2(-s * 0.7, -s),
                centre + vec2(s, 0.0),
                centre + vec2(-s * 0.7, s),
            ],
            Color32::WHITE,
            Stroke::NONE,
        ));
    }
    if response.clicked() {
        acts.push(Act::Play(asset.clone()));
    }
}

/// Clicks, double clicks, drags and the context menu of an entry.
fn interact(response: &egui::Response, item: &Item, acts: &mut Vec<Act>) {
    match item {
        Item::Folder(f) => {
            if response.clicked() {
                acts.push(Act::Select(Selected::Folder(f.path.clone())));
            }
            if response.double_clicked() {
                acts.push(Act::Go(Place::Folder(f.path.clone())));
            }
            response.context_menu(|ui| {
                if ui.button(tr("content.open_folder")).clicked() {
                    acts.push(Act::Go(Place::Folder(f.path.clone())));
                }
                if ui.button(tr("content.copy_path")).clicked() {
                    acts.push(Act::CopyKey(f.path.clone()));
                }
            });
        }
        Item::Stream(s) => {
            if response.clicked() {
                acts.push(Act::Select(Selected::Stream(s.path.clone())));
            }
            if response.double_clicked() {
                acts.push(Act::Go(Place::Stream(s.path.clone())));
            }
            response
                .clone()
                .on_hover_text(format!("{}\n{}", s.path, size_text(s.size)));
            response.context_menu(|ui| {
                if ui.button(tr("content.browse_stream")).clicked() {
                    acts.push(Act::Go(Place::Stream(s.path.clone())));
                }
                if let Some(i) = s.game
                    && ui.button(tr("content.open_stream")).clicked()
                {
                    acts.push(Act::OpenStream(i));
                }
                if ui.button(tr("content.copy_path")).clicked() {
                    acts.push(Act::CopyKey(s.path.clone()));
                }
            });
        }
        Item::Asset(a) => {
            if response.clicked() {
                acts.push(Act::Select(Selected::Asset(a.key.clone())));
                if let Some(at) = a.at {
                    acts.push(Act::Details(at));
                }
            }
            if response.double_clicked() {
                acts.push(Act::Open((*a).clone()));
            }
            if response.drag_started() {
                acts.push(Act::Drag((*a).clone()));
            }
            response.clone().on_hover_ui(|ui| {
                ui.strong(&a.name);
                ui.label(a.key.to_string());
                ui.label(
                    RichText::new(format!(
                        "{} · {}",
                        a.category.label(),
                        size_text(a.size as u64)
                    ))
                    .weak(),
                );
                if a.category == Category::Model {
                    ui.label(RichText::new(tr("content.drag_hint")).weak().small());
                }
            });
            response.context_menu(|ui| {
                if ui.button(tr("content.open")).clicked() {
                    acts.push(Act::Open((*a).clone()));
                }
                if ui.button(tr("content.reveal")).clicked() {
                    acts.push(Act::Reveal((*a).clone()));
                }
                if ui.button(tr("content.copy_key")).clicked() {
                    acts.push(Act::CopyKey(a.key.to_string()));
                }
                ui.separator();
                if ui
                    .add_enabled(a.at.is_some(), egui::Button::new(tr("content.clone")))
                    .on_hover_text(tr("content.clone_hint"))
                    .clicked()
                {
                    acts.push(Act::Clone((*a).clone()));
                }
                if let Some(at) = a.at
                    && ui.button(tr("content.export")).clicked()
                {
                    acts.push(Act::Export(at));
                }
                if a.category == Category::Sound
                    && a.at.is_some()
                    && ui.button(tr("content.play_first")).clicked()
                {
                    acts.push(Act::Play((*a).clone()));
                }
            });
        }
    }
}

/// Selection and navigation asked for by the entries.
fn apply_selection(state: &mut ContentState, acts: &mut Vec<Act>) {
    acts.retain(|act| match act {
        Act::Select(s) => {
            state.selected = Some(s.clone());
            false
        }
        Act::Go(place) => {
            state.go(place.clone());
            state.selected = None;
            false
        }
        _ => true,
    });
}

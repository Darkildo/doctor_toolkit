//! What the Content Browser lists, without a window: the folder tree of the
//! streams, the asset categories and the assets of a folder or a stream
//! with the marks of the workspace.
//!
//! A stream is a folder of assets: the tree is `Content → gf2 → cuba →
//! local`, and selecting `local` lists its resources. Companions that only
//! make sense with their main resource (`tg4d` with `tg4h`, `meshvolatile`
//! with `mesh`) are not listed.

use std::collections::{BTreeMap, HashMap};

use doctor_toolkit::game::{GameInstance, ResourceIndex};
use doctor_toolkit::hash::sdbm;
use doctor_toolkit::navigation::{GameIndex, RecordId};
use doctor_toolkit::workspace::{ResourceKey, Workspace};
use doctor_toolkit_mods::i18n::tr;

use crate::nav::Location;

/// Kinds of assets, for the filters and the colour of the tiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    /// `metamodel` and `mesh`.
    Model,
    /// `tg4h` (with its `tg4d`).
    Texture,
    Material,
    /// Sound banks and mixes.
    Sound,
    /// SimGroups and variable dictionaries.
    Logic,
    /// Lua.
    Script,
    /// Cut-scenes (`trinity_seq_master`).
    Cutscene,
    /// String tables.
    Text,
    Dialogue,
    /// Animation banks, rigs, graphs, Havok animation.
    Animation,
    /// Havok collision.
    Collision,
    Other,
}

impl Category {
    pub const ALL: [Category; 12] = [
        Self::Model,
        Self::Texture,
        Self::Material,
        Self::Sound,
        Self::Logic,
        Self::Script,
        Self::Cutscene,
        Self::Text,
        Self::Dialogue,
        Self::Animation,
        Self::Collision,
        Self::Other,
    ];

    /// The category of a resource type.
    pub fn of(type_id: u32) -> Category {
        const fn t(tag: &[u8]) -> u32 {
            sdbm(tag)
        }
        match type_id {
            x if x == t(b"metamodel") || x == t(b"mesh") || x == t(b"treemodel") => Self::Model,
            x if x == t(b"tg4h") => Self::Texture,
            x if x == t(b"material") || x == t(b"materialswaplistresource") => Self::Material,
            x if x == t(b"sbk") || x == t(b"mix") || x == t(b"emx") || x == t(b"auc") => {
                Self::Sound
            }
            x if x == t(b"simgroup") || x == t(b"variabledictionary") => Self::Logic,
            x if x == t(b"lua") => Self::Script,
            x if x == t(b"trinity_seq_master") || x == t(b"score") => Self::Cutscene,
            x if x == t(b"lh2") || x == t(b"lhr") => Self::Text,
            x if x == t(b"dlg") => Self::Dialogue,
            x if x == t(b"bnk")
                || x == t(b"rcb")
                || x == t(b"apx")
                || x == t(b"hka")
                || x == t(b"graph") =>
            {
                Self::Animation
            }
            x if x == t(b"hkx") => Self::Collision,
            _ => Self::Other,
        }
    }

    /// Text key of the category's name.
    pub fn key(self) -> &'static str {
        match self {
            Self::Model => "content.cat_model",
            Self::Texture => "content.cat_texture",
            Self::Material => "content.cat_material",
            Self::Sound => "content.cat_sound",
            Self::Logic => "content.cat_logic",
            Self::Script => "content.cat_script",
            Self::Cutscene => "content.cat_cutscene",
            Self::Text => "content.cat_text",
            Self::Dialogue => "content.cat_dialogue",
            Self::Animation => "content.cat_animation",
            Self::Collision => "content.cat_collision",
            Self::Other => "content.cat_other",
        }
    }

    pub fn label(self) -> &'static str {
        tr(self.key())
    }

    /// The colour of the strip under a tile, as in Unreal's asset types.
    pub fn color(self) -> egui::Color32 {
        use egui::Color32 as C;
        match self {
            Self::Model => C::from_rgb(0x4f, 0xc3, 0xf7),
            Self::Texture => C::from_rgb(0xe5, 0x53, 0x4b),
            Self::Material => C::from_rgb(0x8b, 0xc3, 0x4a),
            Self::Sound => C::from_rgb(0x2b, 0x8c, 0xbe),
            Self::Logic => C::from_rgb(0xd9, 0xa4, 0x41),
            Self::Script => C::from_rgb(0xb3, 0x88, 0xff),
            Self::Cutscene => C::from_rgb(0xe0, 0x6c, 0xc4),
            Self::Text => C::from_rgb(0xa0, 0xa0, 0xa0),
            Self::Dialogue => C::from_rgb(0x26, 0xa6, 0x9a),
            Self::Animation => C::from_rgb(0x57, 0xab, 0x5a),
            Self::Collision => C::from_rgb(0xff, 0x98, 0x00),
            Self::Other => C::from_rgb(0x70, 0x74, 0x7c),
        }
    }

    /// Types listed under this category's filter.
    pub fn accepts(self, type_id: u32) -> bool {
        !is_companion(type_id) && Self::of(type_id) == self
    }
}

/// Resources listed with another one: texture data with its header, the
/// volatile part of a mesh with the mesh.
pub fn is_companion(type_id: u32) -> bool {
    type_id == sdbm(b"tg4d") || type_id == sdbm(b"meshvolatile")
}

/// What the workspace did to an asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mark {
    #[default]
    None,
    /// Added by the workspace (a clone, a new resource).
    New,
    /// Its data is changed.
    Changed,
}

/// One asset of the browser.
#[derive(Debug, Clone, PartialEq)]
pub struct Asset {
    pub key: ResourceKey,
    /// Where it is in the game files; `None` for a resource the workspace
    /// adds.
    pub at: Option<Location>,
    pub name: String,
    pub category: Category,
    pub mark: Mark,
    /// Size of the data in bytes.
    pub size: u32,
    /// SHA-256 of the workspace's data (for a texture, of its data part),
    /// when the workspace changes or adds it: previews follow it.
    pub version: Option<String>,
}

impl Asset {
    /// The resource's type tag.
    pub fn type_name(&self) -> String {
        self.key.type_name()
    }
}

/// A stream in the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamEntry {
    /// Its name without the folder (`local`).
    pub name: String,
    /// The stream name (`gf2/cuba/local`).
    pub path: String,
    /// Index in [`GameInstance::streams`]; `None` for a stream the
    /// workspace creates.
    pub game: Option<usize>,
    pub size: u64,
}

/// A folder of the tree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Folder {
    /// Its name (`cuba`); empty for the root.
    pub name: String,
    /// The path from the root (`gf2/cuba`); empty for the root.
    pub path: String,
    /// Sub-folders, by index into [`FolderTree::folders`], sorted by name.
    pub children: Vec<usize>,
    /// Streams directly in the folder, sorted by name.
    pub streams: Vec<StreamEntry>,
    /// Streams in the folder and below.
    pub total: usize,
}

/// The folders of the game's streams (and of the streams the workspace
/// creates). Folder 0 is the root, "Content".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderTree {
    pub folders: Vec<Folder>,
    by_path: HashMap<String, usize>,
}

impl Default for FolderTree {
    fn default() -> Self {
        Self::build(std::iter::empty())
    }
}

impl FolderTree {
    /// The tree of streams given as `(stream name, game index, size)`.
    pub fn build(streams: impl IntoIterator<Item = (String, Option<usize>, u64)>) -> Self {
        let mut tree = Self {
            folders: vec![Folder::default()],
            by_path: HashMap::from([(String::new(), 0)]),
        };
        for (path, game, size) in streams {
            let (folder, name) = match path.rsplit_once('/') {
                Some((folder, name)) => (folder.to_owned(), name.to_owned()),
                None => (String::new(), path.clone()),
            };
            let f = tree.folder_index(&folder);
            tree.folders[f].streams.push(StreamEntry {
                name,
                path,
                game,
                size,
            });
        }
        // Sort and count.
        let names: Vec<String> = tree.folders.iter().map(|f| f.name.clone()).collect();
        for folder in &mut tree.folders {
            folder.children.sort_by(|a, b| names[*a].cmp(&names[*b]));
            folder.streams.sort_by(|a, b| a.name.cmp(&b.name));
        }
        fn count(tree: &mut FolderTree, f: usize) -> usize {
            let children = tree.folders[f].children.clone();
            let total = tree.folders[f].streams.len()
                + children.into_iter().map(|c| count(tree, c)).sum::<usize>();
            tree.folders[f].total = total;
            total
        }
        count(&mut tree, 0);
        tree
    }

    /// The tree of a game's streams and the new streams of a workspace.
    pub fn of_game(game: &GameInstance, ws: Option<&Workspace>) -> Self {
        let mut streams: Vec<(String, Option<usize>, u64)> = game
            .streams()
            .iter()
            .enumerate()
            .map(|(i, f)| (f.stream_name(), Some(i), f.len))
            .collect();
        if let Some(ws) = ws {
            for new in ws.new_streams() {
                if game.find_stream(&new.name).is_none() {
                    streams.push((new.name.clone(), None, 0));
                }
            }
        }
        Self::build(streams)
    }

    /// The folder at `path`, made with its parents when missing.
    fn folder_index(&mut self, path: &str) -> usize {
        if let Some(&f) = self.by_path.get(path) {
            return f;
        }
        let (parent, name) = match path.rsplit_once('/') {
            Some((parent, name)) => (parent, name),
            None => ("", path),
        };
        let parent = self.folder_index(parent);
        let f = self.folders.len();
        self.folders.push(Folder {
            name: name.to_owned(),
            path: path.to_owned(),
            ..Folder::default()
        });
        self.folders[parent].children.push(f);
        self.by_path.insert(path.to_owned(), f);
        f
    }

    /// The folder at `path`.
    pub fn folder(&self, path: &str) -> Option<&Folder> {
        self.by_path.get(path).map(|&f| &self.folders[f])
    }

    /// The stream with this name.
    pub fn stream(&self, path: &str) -> Option<&StreamEntry> {
        let folder = path.rsplit_once('/').map_or("", |(f, _)| f);
        self.folder(folder)?.streams.iter().find(|s| s.path == path)
    }

    /// Every stream in the folder at `path` and below.
    pub fn streams_under(&self, path: &str) -> Vec<&StreamEntry> {
        let mut out = Vec::new();
        let mut stack: Vec<usize> = self.by_path.get(path).copied().into_iter().collect();
        while let Some(f) = stack.pop() {
            out.extend(self.folders[f].streams.iter());
            stack.extend(self.folders[f].children.iter().rev());
        }
        out
    }

    /// The parts of a path from the root, for the breadcrumbs:
    /// `gf2/cuba/local` → `gf2`, `gf2/cuba`, `gf2/cuba/local`.
    pub fn crumbs(path: &str) -> Vec<&str> {
        if path.is_empty() {
            return Vec::new();
        }
        let mut out: Vec<&str> = path.match_indices('/').map(|(i, _)| &path[..i]).collect();
        out.push(path);
        out
    }
}

/// Where the browser is: a folder or a stream.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Place {
    Folder(String),
    Stream(String),
}

impl Default for Place {
    fn default() -> Self {
        Self::Folder(String::new())
    }
}

impl Place {
    pub fn path(&self) -> &str {
        match self {
            Self::Folder(p) | Self::Stream(p) => p,
        }
    }

    /// Whether stream `stream` is this place or inside it.
    pub fn contains(&self, stream: &str) -> bool {
        match self {
            Self::Stream(s) => s == stream,
            Self::Folder(f) => {
                f.is_empty()
                    || stream
                        .strip_prefix(f.as_str())
                        .is_some_and(|rest| rest.starts_with('/'))
            }
        }
    }
}

/// What the workspace changes, by key: the mark and the SHA-256 of its data.
#[derive(Debug, Clone, Default)]
pub struct Marks {
    pub by_key: HashMap<ResourceKey, (Mark, String)>,
    /// Resources the workspace adds, by stream name.
    pub added: BTreeMap<String, Vec<(ResourceKey, u32)>>,
}

impl Marks {
    pub fn of(ws: Option<&Workspace>) -> Self {
        let mut marks = Self::default();
        let Some(ws) = ws else { return marks };
        for change in ws.changes() {
            let mark = if change.is_added() {
                marks
                    .added
                    .entry(change.key.stream.clone())
                    .or_default()
                    .push((change.key.clone(), change.payload.size as u32));
                Mark::New
            } else {
                Mark::Changed
            };
            marks
                .by_key
                .insert(change.key.clone(), (mark, change.payload.sha256.clone()));
        }
        marks
    }

    /// The mark and the version of an asset: a texture is changed when its
    /// header or its data is.
    fn of_key(&self, key: &ResourceKey) -> (Mark, Option<String>) {
        let own = self.by_key.get(key);
        let data = (key.type_id == sdbm(b"tg4h"))
            .then(|| {
                let mut ids = key.ids;
                ids[1] = sdbm(b"tg4d");
                let data_key = ResourceKey {
                    stream: key.stream.clone(),
                    type_id: sdbm(b"tg4d"),
                    ids,
                };
                self.by_key.get(&data_key).or_else(|| {
                    self.by_key.iter().find_map(|(k, v)| {
                        (k.stream == key.stream
                            && k.type_id == sdbm(b"tg4d")
                            && k.ids[3] == key.ids[3])
                            .then_some(v)
                    })
                })
            })
            .flatten();
        let mark = match (own, data) {
            (Some((m, _)), _) => *m,
            (None, Some(_)) => Mark::Changed,
            (None, None) => Mark::None,
        };
        let version = data.or(own).map(|(_, sha)| sha.clone());
        (mark, version)
    }
}

/// Where the names and types of assets come from: the game index once it is
/// built (names), the plain resource index before (types and ids only).
pub struct Source<'a> {
    pub game: &'a GameInstance,
    pub index: &'a ResourceIndex,
    pub names: Option<&'a GameIndex>,
    pub marks: &'a Marks,
}

/// What to list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    /// Categories to show; empty for every one.
    pub categories: Vec<Category>,
}

impl Filter {
    pub fn passes(&self, type_id: u32) -> bool {
        !is_companion(type_id)
            && (self.categories.is_empty() || self.categories.contains(&Category::of(type_id)))
    }
}

impl Source<'_> {
    /// The asset of resource `resource` of stream `stream`.
    fn asset(&self, stream: usize, resource: usize, record: Option<RecordId>) -> Option<Asset> {
        let header = match (record, self.names) {
            (Some(id), Some(names)) => names.record(id).header,
            _ => self.entry(stream, resource)?.header,
        };
        let name = match (record, self.names) {
            (Some(id), Some(names)) => display_name(names, id),
            _ => format!("{:08x}", header.ids[3]),
        };
        let key = ResourceKey::new(&self.game.streams()[stream].stream_name(), &header);
        let (mark, version) = self.marks.of_key(&key);
        Some(Asset {
            category: Category::of(header.type_id),
            name,
            size: header.data_size,
            mark,
            version,
            at: Some(Location { stream, resource }),
            key,
        })
    }

    fn entry(&self, stream: usize, resource: usize) -> Option<&doctor_toolkit::game::IndexEntry> {
        let entries = &self.index.entries;
        let start = entries.partition_point(|e| (e.stream as usize) < stream);
        entries[start..]
            .iter()
            .take_while(|e| e.stream as usize == stream)
            .find(|e| e.resource as usize == resource)
    }

    /// The assets of one stream that pass the filter, with the resources
    /// the workspace adds to it.
    pub fn stream_assets(&self, stream: &StreamEntry, filter: &Filter) -> Vec<Asset> {
        let mut out = Vec::new();
        if let Some(s) = stream.game {
            match self.names.filter(|n| s < n.stream_count()) {
                Some(names) => {
                    for id in names.stream_records(s) {
                        let record = names.record(id);
                        if filter.passes(record.header.type_id)
                            && let Some(a) = self.asset(s, record.resource as usize, Some(id))
                        {
                            out.push(a);
                        }
                    }
                }
                None => {
                    let entries = &self.index.entries;
                    let start = entries.partition_point(|e| (e.stream as usize) < s);
                    for e in entries[start..]
                        .iter()
                        .take_while(|e| e.stream as usize == s)
                    {
                        if filter.passes(e.header.type_id)
                            && let Some(a) = self.asset(s, e.resource as usize, None)
                        {
                            out.push(a);
                        }
                    }
                }
            }
        }
        for (key, size) in self.marks.added.get(&stream.path).into_iter().flatten() {
            if filter.passes(key.type_id) {
                out.push(Asset {
                    key: key.clone(),
                    at: None,
                    name: format!("{:08x}", key.ids[3]),
                    category: Category::of(key.type_id),
                    mark: Mark::New,
                    size: *size,
                    version: self.marks.by_key.get(key).map(|(_, sha)| sha.clone()),
                });
            }
        }
        out
    }

    /// The asset of a search result.
    pub fn record_asset(&self, id: RecordId) -> Option<Asset> {
        let names = self.names?;
        let stream = names.stream_of(id);
        self.asset(stream, names.record(id).resource as usize, Some(id))
    }
}

/// The name of an asset. Models are named after their first material: a
/// metamodel has no name of its own (its names are states and parts) and
/// the meshes of the game are `model_high`, `lodmodel01`, `dmg0_model`.
pub fn display_name(names: &GameIndex, id: RecordId) -> String {
    let (metamodel, mesh, material) = (sdbm(b"metamodel"), sdbm(b"mesh"), sdbm(b"material"));
    let type_id = names.record(id).header.type_id;
    if type_id != metamodel && type_id != mesh {
        return names.label(id);
    }
    // Breadth first: metamodel → nested metamodels and meshes → materials.
    let mut queue = std::collections::VecDeque::from([id]);
    let mut seen = Vec::new();
    let mut first_mesh = None;
    while let Some(next) = queue.pop_front() {
        if seen.contains(&next) || seen.len() > 16 {
            continue;
        }
        seen.push(next);
        for (_, targets) in names.uses(next) {
            for &t in targets {
                match names.record(t).header.type_id {
                    x if x == material => return names.label(t),
                    x if x == mesh => {
                        first_mesh.get_or_insert(t);
                        queue.push_back(t);
                    }
                    x if x == metamodel => queue.push_back(t),
                    _ => {}
                }
            }
        }
    }
    names.label(first_mesh.unwrap_or(id))
}

/// Sorts assets by category (models first, as in the filters), then by
/// name (case aside).
pub fn sort(assets: &mut [Asset]) {
    assets.sort_by_cached_key(|a| (a.category, a.name.to_lowercase(), a.key.type_id, a.key.ids));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> FolderTree {
        FolderTree::build(
            [
                "gf2/cuba/local",
                "gf2/cuba/global",
                "gf2/frontend",
                "gf2/cuba/props/crates",
                "frontend",
            ]
            .into_iter()
            .enumerate()
            .map(|(i, p)| (p.to_owned(), Some(i), 10)),
        )
    }

    #[test]
    fn the_tree_has_folders_streams_and_counts() {
        let tree = tree();
        let root = &tree.folders[0];
        assert_eq!(root.total, 5);
        assert_eq!(root.streams.len(), 1, "frontend at the root");
        let gf2 = tree.folder("gf2").unwrap();
        assert_eq!(gf2.total, 4);
        assert_eq!(gf2.streams[0].path, "gf2/frontend");
        let cuba = tree.folder("gf2/cuba").unwrap();
        let names: Vec<&str> = cuba.streams.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["global", "local"], "sorted");
        assert_eq!(cuba.children.len(), 1);
        assert_eq!(tree.folders[cuba.children[0]].path, "gf2/cuba/props");
        assert_eq!(tree.stream("gf2/cuba/local").unwrap().game, Some(0));
        assert!(tree.stream("gf2/cuba").is_none());
        let under: Vec<&str> = tree
            .streams_under("gf2/cuba")
            .iter()
            .map(|s| s.path.as_str())
            .collect();
        assert_eq!(
            under,
            ["gf2/cuba/global", "gf2/cuba/local", "gf2/cuba/props/crates"]
        );
        assert_eq!(tree.streams_under("").len(), 5);
        assert_eq!(
            FolderTree::crumbs("gf2/cuba/local"),
            ["gf2", "gf2/cuba", "gf2/cuba/local"]
        );
        assert!(FolderTree::crumbs("").is_empty());
    }

    #[test]
    fn places_contain_their_streams() {
        let folder = Place::Folder("gf2/cuba".into());
        assert!(folder.contains("gf2/cuba/local"));
        assert!(folder.contains("gf2/cuba/props/crates"));
        assert!(!folder.contains("gf2/cubafoo"));
        assert!(!folder.contains("gf2/cuba"));
        assert!(Place::Folder(String::new()).contains("anything"));
        assert!(Place::Stream("gf2/cuba/local".into()).contains("gf2/cuba/local"));
        assert!(!Place::Stream("gf2/cuba/local".into()).contains("gf2/cuba/localx"));
    }

    #[test]
    fn categories_and_filters() {
        assert_eq!(Category::of(sdbm(b"metamodel")), Category::Model);
        assert_eq!(Category::of(sdbm(b"mesh")), Category::Model);
        assert_eq!(Category::of(sdbm(b"treemodel")), Category::Model);
        assert_eq!(Category::of(sdbm(b"tg4h")), Category::Texture);
        assert_eq!(Category::of(sdbm(b"sbk")), Category::Sound);
        assert_eq!(Category::of(sdbm(b"simgroup")), Category::Logic);
        assert_eq!(Category::of(sdbm(b"lua")), Category::Script);
        assert_eq!(
            Category::of(sdbm(b"trinity_seq_master")),
            Category::Cutscene
        );
        assert_eq!(Category::of(sdbm(b"lh2")), Category::Text);
        assert_eq!(Category::of(0x1234_5678), Category::Other);
        assert!(Category::Model.accepts(sdbm(b"mesh")));
        assert!(!Category::Model.accepts(sdbm(b"meshvolatile")), "companion");
        let all = Filter::default();
        assert!(all.passes(sdbm(b"lua")));
        assert!(!all.passes(sdbm(b"tg4d")));
        let models = Filter {
            categories: vec![Category::Model, Category::Texture],
        };
        assert!(models.passes(sdbm(b"metamodel")) && models.passes(sdbm(b"tg4h")));
        assert!(!models.passes(sdbm(b"lua")));
        for c in Category::ALL {
            assert!(doctor_toolkit_mods::i18n::has_key(c.key()), "{c:?}");
        }
    }
}

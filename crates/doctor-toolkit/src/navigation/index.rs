//! The game index: every resource with its names and references.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::UNIX_EPOCH;

use rayon::prelude::*;

use crate::container::ustream::ResourceHeader;
use crate::game::GameInstance;
use crate::resource::mesh::TYPE_MESH_VOLATILE;
use crate::resource::texture::TYPE_TG4D;
use crate::workspace::ResourceKey;
use crate::workspace::key::normalize_stream;
use crate::{Error, Result};

use super::{cache, harvest};

/// Position of a record in a [`GameIndex`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RecordId(pub u32);

impl RecordId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// What a name read from a resource stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NameKind {
    /// The resource's own name: texture, material, model, rig, font, Flash
    /// movie, script file, parts database.
    Name,
    /// Texture package (`assets_shared_fonts`).
    Package,
    /// A named piece: draw records of a model, metamodel parts and states,
    /// embedded textures of a Flash movie, parts of a parts database.
    Part,
    /// A SimGroup object name (`CMD_SetName`).
    Object,
    /// A SimGroup class, a metamodel part or state type.
    Class,
    /// A sound event or source file of a sound bank.
    Event,
    /// A script variable.
    Variable,
    /// Any other text: attribute strings, texture source paths, strings of
    /// types that are not parsed yet.
    Text,
}

impl NameKind {
    pub const ALL: [Self; 8] = [
        Self::Name,
        Self::Package,
        Self::Part,
        Self::Object,
        Self::Class,
        Self::Event,
        Self::Variable,
        Self::Text,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Package => "package",
            Self::Part => "part",
            Self::Object => "object",
            Self::Class => "class",
            Self::Event => "event",
            Self::Variable => "variable",
            Self::Text => "text",
        }
    }

    pub(crate) fn from_u8(value: u8) -> Option<Self> {
        Self::ALL.get(value as usize).copied()
    }
}

/// What a hash found in resource data identifies; the function behind most
/// of them is not known, see [`super::Dictionary`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HashKind {
    /// Entry hash of a string table (`lh2`).
    StringEntry,
    /// SimGroup class hash (SDBM of the lower-case class name).
    SimClass,
    /// Shader hash of a material slot.
    Shader,
    /// Bone name hash of a model or rig.
    Bone,
    /// Part hash of a parts database.
    Part,
    /// Name hash of a metamodel property, variable or part type.
    Property,
}

impl HashKind {
    pub const ALL: [Self; 6] = [
        Self::StringEntry,
        Self::SimClass,
        Self::Shader,
        Self::Bone,
        Self::Part,
        Self::Property,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::StringEntry => "string entry",
            Self::SimClass => "SimGroup class",
            Self::Shader => "shader",
            Self::Bone => "bone",
            Self::Part => "parts database part",
            Self::Property => "metamodel name",
        }
    }

    pub(crate) fn from_u8(value: u8) -> Option<Self> {
        Self::ALL.get(value as usize).copied()
    }
}

/// What a reference points at.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Target {
    /// A resource by its last id (`ids[3]`); `type_id` 0 means any type.
    Resource { type_id: u32, id: u32 },
    /// A material by the CRC-32 of its lower-case name.
    Material(u32),
    /// A SimGroup object or a graph by its GUID.
    Guid([u8; 16]),
    /// The skeleton of a rig whose bones match a model's bone table
    /// `(bone hash, skeleton index)`.
    Bones(Box<[(u32, u32)]>),
    /// A resource of a type by its name, e.g. the atlas texture of a font.
    Named { type_id: u32, name: String },
}

impl Target {
    /// Short description of the target for listings.
    pub fn describe(&self) -> String {
        use crate::resource::names::type_tags;
        match self {
            Self::Resource { type_id: 0, id } => format!("resource {id:08x}"),
            Self::Resource { type_id, id } => format!("{} {id:08x}", type_tags().display(*type_id)),
            Self::Material(hash) => format!("material {hash:08x}"),
            Self::Guid(guid) => {
                let hex: String = guid.iter().map(|b| format!("{b:02x}")).collect();
                format!("guid {hex}")
            }
            Self::Bones(bones) => format!("skeleton with {} bone(s)", bones.len()),
            Self::Named { type_id, name } => format!("{} `{name}`", type_tags().display(*type_id)),
        }
    }

    /// The 32-bit value a hash search matches.
    pub fn hash(&self) -> Option<u32> {
        match self {
            Self::Resource { id, .. } => Some(*id),
            Self::Material(hash) => Some(*hash),
            _ => None,
        }
    }
}

/// An outgoing reference of a resource.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Ref {
    /// Where the reference comes from: a sampler name, a property
    /// (`ModelPart.Model`), a draw record, an attribute (`Entity.CMD_SetModel`).
    pub label: String,
    pub target: Target,
}

/// Something other resources can point at, besides the resource's type and id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Anchor {
    /// A material's name hash.
    Material(u32),
    /// A SimGroup object GUID or a graph GUID.
    Guid([u8; 16]),
    /// The bone hashes of a rig skeleton, by skeleton index.
    Skeleton(Box<[u32]>),
}

/// One string of a string table (`lh2`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEntry {
    pub hash: u32,
    /// Index into [`Record::languages`].
    pub language: u16,
    pub text: String,
}

/// Everything the index knows about one resource.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Record {
    /// Index of the resource in its stream.
    pub resource: u32,
    pub header: ResourceHeader,
    /// Bytes the data occupies in the stream file.
    pub stored_size: u32,
    pub compressed: bool,
    pub names: Vec<(NameKind, String)>,
    pub refs: Vec<Ref>,
    pub anchors: Vec<Anchor>,
    /// Hashes found in the data whose names are wanted.
    pub hashes: Vec<(HashKind, u32)>,
    /// Hashes stored next to their name in the data (a texture's name hash,
    /// a material's, a metamodel property's), for the dictionary.
    pub pairs: Vec<(u32, String)>,
    /// Column names of a string table: language codes or `text`.
    pub languages: Vec<String>,
    pub texts: Vec<TextEntry>,
    /// Why the data could not be read, when it could not.
    pub error: Option<String>,
}

impl Record {
    /// The first name of kind [`NameKind::Name`].
    pub fn name(&self) -> Option<&str> {
        self.names
            .iter()
            .find(|(kind, _)| *kind == NameKind::Name)
            .map(|(_, name)| name.as_str())
    }
}

/// Identifies a version of a stream file without reading it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Stamp {
    pub size: u64,
    pub mtime_ns: u64,
    pub file_id: u64,
}

impl Stamp {
    pub fn of(path: &Path) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        let mtime_ns = metadata
            .modified()
            .ok()
            .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos() as u64);
        #[cfg(unix)]
        let file_id = std::os::unix::fs::MetadataExt::ino(&metadata);
        #[cfg(not(unix))]
        let file_id = 0;
        Some(Self {
            size: metadata.len(),
            mtime_ns,
            file_id,
        })
    }
}

/// The records of one stream, the unit of caching.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Chunk {
    /// Path relative to the game directory, as in [`crate::game::GameFile`].
    pub rel_path: String,
    pub stamp: Stamp,
    /// Why the stream could not be read, when it could not.
    pub error: Option<String>,
    pub records: Vec<Record>,
}

/// Reference resolution per resource type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResolveStats {
    pub type_id: u32,
    /// Resources of the type.
    pub records: usize,
    /// Outgoing references of those resources.
    pub refs: usize,
    /// References that point at at least one resource of the game.
    pub resolved: usize,
}

/// What [`GameIndex::open`] and [`GameIndex::refresh`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OpenReport {
    /// Streams taken from the cache.
    pub reused: usize,
    /// Streams read and parsed.
    pub rebuilt: usize,
    /// Cached streams that are no longer in the game.
    pub removed: usize,
    /// Where the cache was written, when it was.
    pub saved: Option<PathBuf>,
    /// Why the cache could not be read or written, if so (the index is still complete).
    pub cache_error: Option<String>,
}

/// Lookup tables built from the records.
#[derive(Debug, Default)]
struct Links {
    /// First global reference number of each record, and the total at the end.
    ref_starts: Vec<u32>,
    /// Targets of each reference, by global reference number.
    resolved: Vec<Box<[RecordId]>>,
    /// Per record: `(source record, reference index in the source)`.
    used_by: Vec<Vec<(RecordId, u32)>>,
    by_key: HashMap<(u32, [u32; 4]), Vec<RecordId>>,
}

/// Every resource of a game with its names and references.
#[derive(Debug, Default)]
pub struct GameIndex {
    root: PathBuf,
    chunks: Vec<Chunk>,
    /// Normalised stream name of each chunk (`gf2/frontend`).
    stream_names: Vec<String>,
    /// First record id of each chunk, and the total at the end.
    starts: Vec<u32>,
    /// Names of the tables of contents: `(hash of the asset GUID, stream path)`.
    toc_names: Vec<(u32, String)>,
    links: Links,
}

/// Version of the harvested data; bump when [`harvest`] changes what it reads.
pub(crate) const HARVEST_VERSION: u32 = 4;

/// CRC-32 of the name lists the harvested labels and names come from (type
/// tags, bones, SimGroup classes and attributes, shaders): a cache written
/// with other lists is rebuilt, so a name added to `data/gf2/` shows up.
pub(crate) fn names_stamp() -> u32 {
    use std::sync::OnceLock;
    static STAMP: OnceLock<u32> = OnceLock::new();
    *STAMP.get_or_init(|| {
        let lists = [
            include_str!("../../../../data/gf2/type_tags.txt"),
            include_str!("../../../../data/gf2/mem_policies.txt"),
            include_str!("../../../../data/gf2/bone_names.txt"),
            include_str!("../../../../data/gf2/sim_classes.txt"),
            include_str!("../../../../data/gf2/simgroup_schema.txt"),
            include_str!("../../../../data/gf2/shaders.txt"),
        ];
        crate::hash::crc32(lists.concat().as_bytes())
    })
}

impl GameIndex {
    /// Builds the index of `game` from scratch, without a cache.
    ///
    /// `progress(done, total)` is called from worker threads as streams finish.
    pub fn build(game: &GameInstance, progress: &(dyn Fn(usize, usize) + Sync)) -> Self {
        let (index, _) = Self::assemble(game, Vec::new(), progress);
        index
    }

    /// Loads the index of `game` from `cache` (a file), re-reads the streams
    /// that changed since it was written and saves it again if anything
    /// changed. Without a cache file this is [`GameIndex::build`].
    pub fn open(
        game: &GameInstance,
        cache: Option<&Path>,
        progress: &(dyn Fn(usize, usize) + Sync),
    ) -> (Self, OpenReport) {
        let mut cache_error = None;
        let cached = match cache {
            Some(path) if path.exists() => match cache::read(path, game.root()) {
                Ok(chunks) => chunks,
                Err(error) => {
                    cache_error = Some(format!("cache {} ignored: {error}", path.display()));
                    Vec::new()
                }
            },
            _ => Vec::new(),
        };
        let (index, mut report) = Self::assemble(game, cached, progress);
        report.cache_error = cache_error;
        if let Some(path) = cache
            && (report.rebuilt > 0 || report.removed > 0 || !path.exists())
        {
            match index.save(path) {
                Ok(()) => report.saved = Some(path.to_owned()),
                Err(error) => report.cache_error = Some(error.to_string()),
            }
        }
        (index, report)
    }

    /// Re-reads the streams whose files changed (after `workspace apply`, for
    /// example) and updates `cache` if given.
    pub fn refresh(
        &mut self,
        game: &GameInstance,
        cache: Option<&Path>,
        progress: &(dyn Fn(usize, usize) + Sync),
    ) -> OpenReport {
        let chunks = std::mem::take(&mut self.chunks);
        let (index, mut report) = Self::assemble(game, chunks, progress);
        *self = index;
        if let Some(path) = cache
            && (report.rebuilt > 0 || report.removed > 0)
        {
            match self.save(path) {
                Ok(()) => report.saved = Some(path.to_owned()),
                Err(error) => report.cache_error = Some(error.to_string()),
            }
        }
        report
    }

    /// Writes the index to `path` (atomically).
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
        }
        let bytes = cache::write(&self.root, &self.chunks);
        crate::atomic::write(path, &bytes)
    }

    /// Uses the cached chunks whose stamps match and harvests the others.
    fn assemble(
        game: &GameInstance,
        cached: Vec<Chunk>,
        progress: &(dyn Fn(usize, usize) + Sync),
    ) -> (Self, OpenReport) {
        let mut cached: HashMap<String, Chunk> = cached
            .into_iter()
            .map(|c| (c.rel_path.clone(), c))
            .collect();
        let total = game.streams().len();
        let done = AtomicUsize::new(0);
        let mut report = OpenReport::default();
        let mut slots: Vec<Option<Chunk>> = Vec::with_capacity(total);
        for file in game.streams() {
            let stamp = Stamp::of(&file.path);
            match cached.remove(&file.rel_path) {
                Some(chunk) if Some(chunk.stamp) == stamp => {
                    report.reused += 1;
                    slots.push(Some(chunk));
                    progress(done.fetch_add(1, Ordering::Relaxed) + 1, total);
                }
                _ => slots.push(None),
            }
        }
        report.removed = cached.len();
        let missing: Vec<usize> = (0..total).filter(|&i| slots[i].is_none()).collect();
        report.rebuilt = missing.len();
        let built: Vec<(usize, Chunk)> = missing
            .into_par_iter()
            .map(|i| {
                let chunk = harvest::stream_chunk(&game.streams()[i]);
                progress(done.fetch_add(1, Ordering::Relaxed) + 1, total);
                (i, chunk)
            })
            .collect();
        for (i, chunk) in built {
            slots[i] = Some(chunk);
        }
        let chunks: Vec<Chunk> = slots.into_iter().map(|c| c.expect("filled")).collect();
        let mut index = Self {
            root: game.root().to_owned(),
            stream_names: chunks
                .iter()
                .map(|c| normalize_stream(&c.rel_path))
                .collect(),
            chunks,
            toc_names: toc_names(game),
            ..Self::default()
        };
        index.link();
        (index, report)
    }

    // ------------------------------------------------------------- accessors

    /// The game directory the index was built for.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Number of records (resources).
    pub fn len(&self) -> usize {
        *self.starts.last().unwrap_or(&0) as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Number of streams; stream numbers match [`GameInstance::streams`].
    pub fn stream_count(&self) -> usize {
        self.chunks.len()
    }

    /// Normalised name of stream `stream` (`gf2/frontend`).
    pub fn stream_name(&self, stream: usize) -> &str {
        &self.stream_names[stream]
    }

    /// Path of stream `stream` relative to the game directory.
    pub fn stream_path(&self, stream: usize) -> &str {
        &self.chunks[stream].rel_path
    }

    /// Why stream `stream` could not be read, if so.
    pub fn stream_error(&self, stream: usize) -> Option<&str> {
        self.chunks[stream].error.as_deref()
    }

    /// All record ids.
    pub fn ids(&self) -> impl Iterator<Item = RecordId> + use<> {
        (0..self.len() as u32).map(RecordId)
    }

    /// The stream a record belongs to.
    pub fn stream_of(&self, id: RecordId) -> usize {
        self.starts.partition_point(|&s| s <= id.0) - 1
    }

    pub fn record(&self, id: RecordId) -> &Record {
        let stream = self.stream_of(id);
        &self.chunks[stream].records[(id.0 - self.starts[stream]) as usize]
    }

    /// Records of stream `stream`, in stream order.
    pub fn stream_records(&self, stream: usize) -> impl Iterator<Item = RecordId> + use<> {
        (self.starts[stream]..self.starts[stream + 1]).map(RecordId)
    }

    /// The record of resource `resource` of stream `stream`.
    pub fn locate(&self, stream: usize, resource: usize) -> Option<RecordId> {
        let records = &self.chunks.get(stream)?.records;
        let position = match records.get(resource) {
            Some(r) if r.resource as usize == resource => resource,
            _ => records
                .iter()
                .position(|r| r.resource as usize == resource)?,
        };
        Some(RecordId(self.starts[stream] + position as u32))
    }

    /// The key of a record.
    pub fn key(&self, id: RecordId) -> ResourceKey {
        let stream = self.stream_of(id);
        let header = &self.record(id).header;
        ResourceKey {
            stream: self.stream_names[stream].clone(),
            type_id: header.type_id,
            ids: header.ids,
        }
    }

    /// The record with `key`.
    pub fn find_key(&self, key: &ResourceKey) -> Option<RecordId> {
        self.links
            .by_key
            .get(&(key.type_id, key.ids))?
            .iter()
            .copied()
            .find(|&id| self.stream_names[self.stream_of(id)] == key.stream)
    }

    /// The same resource (type and ids) in other streams.
    pub fn copies(&self, id: RecordId) -> Vec<RecordId> {
        let header = &self.record(id).header;
        self.links
            .by_key
            .get(&(header.type_id, header.ids))
            .map(|list| list.iter().copied().filter(|&other| other != id).collect())
            .unwrap_or_default()
    }

    /// Outgoing references of a record with the records they resolve to.
    pub fn uses(&self, id: RecordId) -> impl Iterator<Item = (&Ref, &[RecordId])> {
        let start = self.links.ref_starts[id.index()] as usize;
        self.record(id)
            .refs
            .iter()
            .enumerate()
            .map(move |(i, r)| (r, &*self.links.resolved[start + i]))
    }

    /// Records that reference a record: `(source, index into the source's refs)`.
    pub fn used_by(&self, id: RecordId) -> &[(RecordId, u32)] {
        &self.links.used_by[id.index()]
    }

    /// A short display name: the resource name, else its first name, else its id.
    pub fn label(&self, id: RecordId) -> String {
        let record = self.record(id);
        match record
            .name()
            .or(record.names.first().map(|(_, n)| n.as_str()))
        {
            Some(name) => name.to_owned(),
            None => format!("{:08x}", record.header.ids[3]),
        }
    }

    /// `(hash, stream path)` of every name in the tables of contents.
    pub fn toc_names(&self) -> &[(u32, String)] {
        &self.toc_names
    }

    /// Number of records whose data could not be read or parsed.
    pub fn record_errors(&self) -> usize {
        self.chunks
            .iter()
            .flat_map(|c| &c.records)
            .filter(|r| r.error.is_some())
            .count()
    }

    /// Reference resolution per resource type, most references first.
    pub fn resolve_stats(&self) -> Vec<ResolveStats> {
        let mut by_type: HashMap<u32, ResolveStats> = HashMap::new();
        for id in self.ids() {
            let record = self.record(id);
            let stats = by_type
                .entry(record.header.type_id)
                .or_insert(ResolveStats {
                    type_id: record.header.type_id,
                    ..ResolveStats::default()
                });
            stats.records += 1;
            for (_, targets) in self.uses(id) {
                stats.refs += 1;
                stats.resolved += usize::from(!targets.is_empty());
            }
        }
        let mut list: Vec<_> = by_type.into_values().collect();
        list.sort_by(|a, b| b.refs.cmp(&a.refs).then(a.type_id.cmp(&b.type_id)));
        list
    }

    // ------------------------------------------------------------ resolution

    /// Builds the lookup tables and resolves every reference.
    fn link(&mut self) {
        self.starts = Vec::with_capacity(self.chunks.len() + 1);
        let mut total = 0u32;
        for chunk in &self.chunks {
            self.starts.push(total);
            total += chunk.records.len() as u32;
        }
        self.starts.push(total);

        let mut by_id: HashMap<u32, Vec<RecordId>> = HashMap::new();
        let mut by_key: HashMap<(u32, [u32; 4]), Vec<RecordId>> = HashMap::new();
        let mut materials: HashMap<u32, Vec<RecordId>> = HashMap::new();
        let mut guids: HashMap<[u8; 16], Vec<RecordId>> = HashMap::new();
        let mut named: HashMap<(u32, String), Vec<RecordId>> = HashMap::new();
        let mut skeletons: Vec<(RecordId, &[u32])> = Vec::new();
        let mut ref_starts = Vec::with_capacity(total as usize + 1);
        let mut ref_total = 0u32;
        for (stream, chunk) in self.chunks.iter().enumerate() {
            for (i, record) in chunk.records.iter().enumerate() {
                let id = RecordId(self.starts[stream] + i as u32);
                let header = &record.header;
                by_id.entry(header.ids[3]).or_default().push(id);
                by_key
                    .entry((header.type_id, header.ids))
                    .or_default()
                    .push(id);
                for anchor in &record.anchors {
                    match anchor {
                        Anchor::Material(hash) => materials.entry(*hash).or_default().push(id),
                        Anchor::Guid(guid) => guids.entry(*guid).or_default().push(id),
                        Anchor::Skeleton(bones) => skeletons.push((id, bones)),
                    }
                }
                if let Some(name) = record.name() {
                    named
                        .entry((header.type_id, name.to_ascii_lowercase()))
                        .or_default()
                        .push(id);
                }
                ref_starts.push(ref_total);
                ref_total += record.refs.len() as u32;
            }
        }
        ref_starts.push(ref_total);

        let this: &Self = self;
        let starts = &this.starts;
        let stream_of = |id: RecordId| starts.partition_point(|&s| s <= id.0) - 1;
        let resolved: Vec<Box<[RecordId]>> = this
            .chunks
            .par_iter()
            .enumerate()
            .flat_map_iter(|(stream, chunk)| {
                let (by_id, materials, guids, named, skeletons) =
                    (&by_id, &materials, &guids, &named, &skeletons);
                chunk
                    .records
                    .iter()
                    .enumerate()
                    .flat_map(move |(i, record)| {
                        let own = RecordId(starts[stream] + i as u32);
                        record.refs.iter().map(move |r| {
                            let found: Vec<RecordId> = match &r.target {
                                Target::Resource { type_id, id } => {
                                    let all = by_id.get(id).map_or(&[][..], Vec::as_slice);
                                    let typed: Vec<RecordId> = all
                                        .iter()
                                        .copied()
                                        .filter(|&c| this.type_of(c) == *type_id)
                                        .collect();
                                    // Metamodels name Havok collision with type id
                                    // 0x33A1671E while the resources have `hkx`
                                    // (0x33A16735): when the type has no match, the
                                    // id alone decides.
                                    if !typed.is_empty() {
                                        typed
                                    } else {
                                        // Untyped: a texture is its header, a model its mesh.
                                        let main: Vec<RecordId> = all
                                            .iter()
                                            .copied()
                                            .filter(|&c| {
                                                !matches!(
                                                    this.type_of(c),
                                                    TYPE_TG4D | TYPE_MESH_VOLATILE
                                                )
                                            })
                                            .collect();
                                        if main.is_empty() { all.to_vec() } else { main }
                                    }
                                }
                                Target::Material(hash) => {
                                    materials.get(hash).cloned().unwrap_or_default()
                                }
                                Target::Guid(guid) => guids.get(guid).cloned().unwrap_or_default(),
                                Target::Named { type_id, name } => named
                                    .get(&(*type_id, name.to_ascii_lowercase()))
                                    .cloned()
                                    .unwrap_or_default(),
                                Target::Bones(bones) => skeletons
                                    .iter()
                                    .filter(|(_, skeleton)| bones_match(skeleton, bones))
                                    .map(|(id, _)| *id)
                                    .collect(),
                            };
                            prefer_stream(found, own, stream, stream_of)
                        })
                    })
            })
            .collect();

        let mut used_by = vec![Vec::new(); total as usize];
        for source in 0..total as usize {
            let (start, end) = (ref_starts[source] as usize, ref_starts[source + 1] as usize);
            for (k, targets) in resolved[start..end].iter().enumerate() {
                for target in targets.iter() {
                    used_by[target.index()].push((RecordId(source as u32), k as u32));
                }
            }
        }
        self.links = Links {
            ref_starts,
            resolved,
            used_by,
            by_key,
        };
    }

    fn type_of(&self, id: RecordId) -> u32 {
        self.record(id).header.type_id
    }
}

/// Whether every `(hash, index)` pair of a model's bone table names the same
/// bone in a skeleton; see [`crate::resource::rcb::Skeleton::matches`].
fn bones_match(skeleton: &[u32], bones: &[(u32, u32)]) -> bool {
    !bones.is_empty()
        && bones
            .iter()
            .all(|&(hash, index)| skeleton.get(index as usize) == Some(&hash))
}

/// Drops the record itself; keeps candidates of the same stream when there are any.
fn prefer_stream(
    mut found: Vec<RecordId>,
    own: RecordId,
    stream: usize,
    stream_of: impl Fn(RecordId) -> usize,
) -> Box<[RecordId]> {
    found.retain(|&c| c != own);
    found.dedup();
    if found.iter().any(|&c| stream_of(c) == stream) {
        found.retain(|&c| stream_of(c) == stream);
    }
    found.into_boxed_slice()
}

/// `(hash, stream path)` of the names of every table of contents.
fn toc_names(game: &GameInstance) -> Vec<(u32, String)> {
    let mut names: Vec<(u32, String)> = (0..game.tocs().len())
        .filter_map(|i| game.load_toc(i).ok())
        .flat_map(|toc| {
            toc.names
                .iter()
                .map(|n| (n.hash, n.name_lossy()))
                .collect::<Vec<_>>()
        })
        .filter(|(hash, _)| *hash != 0)
        .collect();
    names.sort();
    names.dedup();
    names
}

/// The default cache file of the index of the game in `root`:
/// `<cache dir>/doctor-toolkit/index-<crc32 of the path>.bin`, where the cache
/// dir is `$DOCTOR_TOOLKIT_CACHE_DIR`, `$XDG_CACHE_HOME`, `~/.cache` or
/// `%LOCALAPPDATA%`. The game directory itself is not written to.
pub fn default_cache_file(root: &Path) -> Option<PathBuf> {
    let base = if let Some(dir) = std::env::var_os("DOCTOR_TOOLKIT_CACHE_DIR") {
        PathBuf::from(dir)
    } else {
        let cache = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
            .or_else(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from))?;
        cache.join("doctor-toolkit")
    };
    let root = root.canonicalize().unwrap_or_else(|_| root.to_owned());
    let hash = crate::hash::crc32(root.to_string_lossy().as_bytes());
    Some(base.join(format!("index-{hash:08x}.bin")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bones_match_requires_every_pair() {
        let skeleton = [10, 20, 30];
        assert!(bones_match(&skeleton, &[(20, 1), (30, 2)]));
        assert!(!bones_match(&skeleton, &[(20, 1), (31, 2)]));
        assert!(!bones_match(&skeleton, &[(20, 5)]));
        assert!(!bones_match(&skeleton, &[]));
    }

    #[test]
    fn prefer_stream_keeps_local_candidates() {
        let stream_of = |id: RecordId| (id.0 / 10) as usize;
        let found = vec![RecordId(3), RecordId(12), RecordId(15), RecordId(25)];
        assert_eq!(
            &*prefer_stream(found.clone(), RecordId(15), 1, stream_of),
            &[RecordId(12)]
        );
        assert_eq!(
            &*prefer_stream(found, RecordId(99), 9, stream_of),
            &[RecordId(3), RecordId(12), RecordId(15), RecordId(25)]
        );
    }
}

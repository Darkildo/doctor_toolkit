//! Previews of the Content Browser: thumbnails of models and textures made
//! on worker threads, kept in memory up to a budget and on disk.
//!
//! - The browser asks for the previews of the tiles it draws
//!   ([`Previews::get`]); the requests of a frame go to the queue at its end
//!   ([`Previews::end_frame`]). Workers take the newest request first and
//!   drop the ones nobody asked for in [`STALE`]: scrolling past thousands
//!   of assets makes only the previews that stay on the screen.
//! - A worker hashes the stored bytes of the resource (or takes the SHA-256
//!   of the workspace's version), looks for `<hash>.png` in the disk cache
//!   and makes the image only when it is not there ([`super::thumb`]).
//! - Images become textures of the window; the least recently drawn go
//!   when they take more than [`MEMORY_BUDGET`].
//!
//! The disk cache is `<cache dir>/doctor-toolkit/previews/v<RENDER_VERSION>/`
//! (the cache dir of the game index: `$DOCTOR_TOOLKIT_CACHE_DIR`,
//! `$XDG_CACHE_HOME`, `~/.cache`, `%LOCALAPPDATA%`). A new
//! [`RENDER_VERSION`] leaves the old images behind; they are removed with
//! the oldest files when the cache grows over [`DISK_BUDGET`].

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use doctor_toolkit::game::GameAssets;
use doctor_toolkit::game::{GameInstance, ResourceIndex, StreamFile};
use doctor_toolkit::level::{LevelSource, Lookup, Overrides};
use doctor_toolkit::workspace::{ResourceKey, Workspace};

use super::thumb::{self, GpuSlot, Thumbnail};
use crate::nav::Location;

/// Changes when the look of the previews changes: older images are not used.
pub const RENDER_VERSION: u32 = 2;
/// Previews in memory, in bytes of their pixels.
pub const MEMORY_BUDGET: usize = 256 << 20;
/// Previews on disk; over it, the oldest go until three quarters are left.
pub const DISK_BUDGET: u64 = 1 << 30;
/// A request nobody repeated for this long is dropped.
pub const STALE: Duration = Duration::from_millis(1500);
/// Stream contexts (scope, materials) kept for model previews.
const LEVEL_CONTEXTS: usize = 6;

/// What a preview shows: a resource in one version.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PreviewId {
    pub key: ResourceKey,
    /// SHA-256 of the workspace's data, when the workspace changes it.
    pub version: Option<String>,
}

/// How a preview is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Texture,
    Model,
}

/// A preview to make.
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewJob {
    pub id: PreviewId,
    /// The resource in the game files; `None` for one the workspace adds.
    pub at: Option<Location>,
    pub kind: Kind,
}

/// The state of a preview, for drawing a tile.
pub enum Shown<'a> {
    Ready(&'a egui::TextureHandle),
    Pending,
    Failed(&'a str),
}

/// Counters for the status line and the log.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Stats {
    /// Previews made by the renderer.
    pub made: usize,
    /// Time spent making them.
    pub made_time: Duration,
    /// Of them, models and the time spent on models.
    pub models: usize,
    pub models_time: Duration,
    /// Previews read from the disk cache.
    pub from_disk: usize,
    pub failed: usize,
}

/// A disk cache of preview images, one PNG per hash (and the bounds of a
/// model next to it).
#[derive(Debug, Clone)]
pub struct DiskCache {
    root: PathBuf,
    version: u32,
}

impl DiskCache {
    /// The cache under `root` (`…/previews`) for images of `version`.
    pub fn new(root: impl Into<PathBuf>, version: u32) -> Self {
        Self {
            root: root.into(),
            version,
        }
    }

    /// `<cache dir>/doctor-toolkit/previews`, next to the index cache.
    pub fn default_root() -> Option<PathBuf> {
        // The index cache file of any folder: its folder is the cache dir.
        let index = doctor_toolkit::navigation::index::default_cache_file(Path::new("/"))?;
        Some(index.parent()?.join("previews"))
    }

    fn dir(&self) -> PathBuf {
        self.root.join(format!("v{}", self.version))
    }

    pub fn path(&self, key: u64) -> PathBuf {
        let name = format!("{key:016x}");
        self.dir().join(&name[..2]).join(format!("{name}.png"))
    }

    /// The image of `key`, if cached; reading it marks it as used.
    pub fn get(&self, key: u64) -> Option<Thumbnail> {
        let path = self.path(key);
        let file = std::fs::read(&path).ok()?;
        let (width, height, rgba) = doctor_toolkit::resource::pixels::decode_png(&file).ok()?;
        let bounds = std::fs::read(path.with_extension("box"))
            .ok()
            .and_then(|b| decode_bounds(&b));
        if let Ok(f) = std::fs::File::options().write(true).open(&path) {
            let _ = f.set_modified(std::time::SystemTime::now());
        }
        Some(Thumbnail {
            width: width as u32,
            height: height as u32,
            rgba,
            bounds,
        })
    }

    /// Stores the image of `key`.
    pub fn put(&self, key: u64, image: &Thumbnail) -> std::io::Result<()> {
        let path = self.path(key);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let png = doctor_toolkit::resource::pixels::encode_png(
            image.width as usize,
            image.height as usize,
            &image.rgba,
        )
        .map_err(std::io::Error::other)?;
        if let Some(bounds) = image.bounds {
            crate::settings::write_atomic(&path.with_extension("box"), &encode_bounds(&bounds))?;
        }
        crate::settings::write_atomic(&path, &png)
    }

    /// Removes the images of other versions, then the least recently used
    /// ones while the cache is larger than `budget`, down to three quarters
    /// of it. Returns the files removed and the bytes left.
    pub fn prune(&self, budget: u64) -> (usize, u64) {
        let mut removed = 0;
        let current = self.dir();
        if let Ok(entries) = std::fs::read_dir(&self.root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() && path != current {
                    removed += files(&path).len();
                    let _ = std::fs::remove_dir_all(&path);
                }
            }
        }
        let mut list: Vec<(std::time::SystemTime, u64, PathBuf)> = files(&current)
            .into_iter()
            .filter_map(|p| {
                let meta = std::fs::metadata(&p).ok()?;
                Some((meta.modified().ok()?, meta.len(), p))
            })
            .collect();
        let mut total: u64 = list.iter().map(|(_, len, _)| len).sum();
        if total > budget {
            list.sort();
            for (_, len, path) in list {
                if total <= budget / 4 * 3 {
                    break;
                }
                if std::fs::remove_file(&path).is_ok() {
                    total -= len;
                    removed += 1;
                }
            }
        }
        (removed, total)
    }
}

/// Every file below `dir`.
fn files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}

fn encode_bounds(b: &[[f32; 3]; 2]) -> Vec<u8> {
    b.iter().flatten().flat_map(|v| v.to_le_bytes()).collect()
}

fn decode_bounds(bytes: &[u8]) -> Option<[[f32; 3]; 2]> {
    let v: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes(c.try_into().expect("four bytes")))
        .collect();
    (v.len() == 6).then(|| [[v[0], v[1], v[2]], [v[3], v[4], v[5]]])
}

/// A 64-bit hash of a byte stream, eight bytes at a time; stable across
/// builds (it names files of the disk cache).
#[derive(Debug, Clone, Copy)]
pub struct DataHash(u64, u64);

impl Default for DataHash {
    fn default() -> Self {
        Self(0x9e37_79b9_7f4a_7c15, 0)
    }
}

impl DataHash {
    pub fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for c in &mut chunks {
            self.word(u64::from_le_bytes(c.try_into().expect("eight bytes")));
        }
        let rest = chunks.remainder();
        if !rest.is_empty() {
            let mut last = [0u8; 8];
            last[..rest.len()].copy_from_slice(rest);
            self.word(u64::from_le_bytes(last));
        }
        self.1 += bytes.len() as u64;
    }

    pub fn word(&mut self, w: u64) {
        self.0 = (self.0 ^ w)
            .wrapping_mul(0xff51_afd7_ed55_8ccd)
            .rotate_left(29);
    }

    pub fn finish(mut self) -> u64 {
        self.word(self.1);
        let mut h = self.0;
        h ^= h >> 33;
        h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
        h ^= h >> 33;
        h
    }
}

/// The cache key of a preview: what is drawn, how, and the data.
pub fn cache_key(id: &PreviewId, kind: Kind, data: u64, version: u32) -> u64 {
    let mut h = DataHash::default();
    h.word(version as u64);
    h.word(kind as u64);
    h.word(id.key.type_id as u64);
    for i in id.key.ids {
        h.word(i as u64);
    }
    h.word(data);
    h.finish()
}

/// What the workers read: the game, its index, the workspace's versions
/// of the resources previews show.
pub struct PreviewSource {
    pub game: Arc<GameInstance>,
    pub index: Arc<ResourceIndex>,
    /// Workspace data of models, meshes, collision and textures.
    pub ws: HashMap<ResourceKey, Arc<Vec<u8>>>,
    pub overrides: Overrides,
    pub cache: Option<DiskCache>,
    lookup: OnceLock<Arc<Lookup>>,
    parents: OnceLock<Arc<HashMap<String, Vec<String>>>>,
    streams: Mutex<HashMap<usize, Arc<StreamFile>>>,
    levels: Mutex<Vec<(usize, Arc<LevelContext>)>>,
    making: Mutex<()>,
    assets: Mutex<GameAssets>,
}

/// A stream's scope, for model previews: the meshes of a metamodel are
/// found as the level finds them.
pub struct LevelContext {
    pub source: LevelSource,
}

impl PreviewSource {
    pub fn new(
        game: Arc<GameInstance>,
        index: Arc<ResourceIndex>,
        ws: Option<&Workspace>,
        cache: Option<DiskCache>,
    ) -> Self {
        let mut data = HashMap::new();
        if let Some(ws) = ws {
            use doctor_toolkit::hash::sdbm;
            let wanted = [
                sdbm(b"metamodel"),
                sdbm(b"mesh"),
                sdbm(b"meshvolatile"),
                sdbm(b"treemodel"),
                sdbm(b"hkx"),
                sdbm(b"tg4h"),
                sdbm(b"tg4d"),
            ];
            for change in ws.changes().filter(|c| wanted.contains(&c.key.type_id)) {
                if let Ok(Some(bytes)) = ws.data(&change.key) {
                    data.insert(change.key.clone(), Arc::new(bytes));
                }
            }
        }
        let mut overrides = Overrides::default();
        for (key, bytes) in &data {
            if !super::catalog::Category::Model.accepts(key.type_id)
                && key.type_id != doctor_toolkit::hash::sdbm(b"meshvolatile")
                && key.type_id != doctor_toolkit::hash::sdbm(b"hkx")
            {
                continue;
            }
            if let Some(stream) = game.find_stream(&key.stream) {
                overrides
                    .by_id
                    .insert((key.type_id, key.ids[3]), (stream, bytes.clone()));
            }
        }
        let assets = Mutex::new(GameAssets::new(game.clone(), &index));
        Self {
            game,
            index,
            ws: data,
            overrides,
            cache,
            lookup: OnceLock::new(),
            parents: OnceLock::new(),
            streams: Mutex::default(),
            levels: Mutex::default(),
            making: Mutex::default(),
            assets,
        }
    }

    /// A stream file, opened once.
    pub fn stream(&self, index: usize) -> Result<Arc<StreamFile>, String> {
        let mut open = lock(&self.streams);
        if let Some(s) = open.get(&index) {
            return Ok(s.clone());
        }
        let stream = Arc::new(self.game.open_stream(index).map_err(|e| e.to_string())?);
        if open.len() > 32 {
            open.clear();
        }
        open.insert(index, stream.clone());
        Ok(stream)
    }

    /// Closes the stream files (before the game files are replaced).
    pub fn close(&self) {
        lock(&self.streams).clear();
        lock(&self.levels).clear();
    }

    /// The scope of a stream (made by one worker at a time).
    pub fn level(&self, stream: usize) -> Arc<LevelContext> {
        let _making = lock(&self.making);
        if let Some((_, l)) = lock(&self.levels).iter().find(|(s, _)| *s == stream) {
            return l.clone();
        }
        let lookup = self
            .lookup
            .get_or_init(|| Arc::new(Lookup::new(&self.index)))
            .clone();
        let parents = self
            .parents
            .get_or_init(|| Arc::new(self.game.stream_parents().unwrap_or_default()));
        let scope = doctor_toolkit::level::Scope::new(&self.game, parents, &[stream]);
        let source = LevelSource::new(self.game.clone(), lookup, self.overrides.clone(), scope);
        let context = Arc::new(LevelContext { source });
        let mut levels = lock(&self.levels);
        if levels.len() >= LEVEL_CONTEXTS {
            levels.remove(0);
        }
        levels.push((stream, context.clone()));
        context
    }

    /// Materials and textures across the game, as the model viewer finds
    /// them (the materials are read on first use).
    pub fn assets(&self) -> MutexGuard<'_, GameAssets> {
        lock(&self.assets)
    }

    /// The workspace's data of a resource.
    pub fn ws_data(&self, key: &ResourceKey) -> Option<Arc<Vec<u8>>> {
        self.ws.get(key).cloned()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A finished preview.
struct Done {
    generation: u64,
    id: PreviewId,
    result: Result<(Thumbnail, bool), String>,
    elapsed: Duration,
}

#[derive(Default)]
struct Queue {
    waiting: HashMap<PreviewId, (PreviewJob, Instant)>,
    running: HashSet<PreviewId>,
    source: Option<Arc<PreviewSource>>,
    generation: u64,
    closed: bool,
}

struct Shared {
    queue: Mutex<Queue>,
    wake: Condvar,
    gpu: Mutex<GpuSlot>,
}

enum State {
    Ready {
        texture: egui::TextureHandle,
        bounds: Option<[[f32; 3]; 2]>,
    },
    Pending,
    Failed(String),
}

struct Entry {
    state: State,
    bytes: usize,
    used: u64,
}

/// The preview service of the editor (one for every Content Browser).
pub struct Previews {
    ctx: egui::Context,
    shared: Arc<Shared>,
    tx: Sender<Done>,
    rx: Receiver<Done>,
    workers: usize,
    entries: HashMap<PreviewId, Entry>,
    /// Requests of this frame.
    wanted: Vec<PreviewJob>,
    bytes: usize,
    frame: u64,
    cache_root: Option<PathBuf>,
    pruned: bool,
    pub stats: Stats,
}

impl Drop for Previews {
    fn drop(&mut self) {
        lock(&self.shared.queue).closed = true;
        self.shared.wake.notify_all();
    }
}

impl Previews {
    pub fn new(ctx: &egui::Context) -> Self {
        let (tx, rx) = channel();
        Self {
            ctx: ctx.clone(),
            shared: Arc::new(Shared {
                queue: Mutex::default(),
                wake: Condvar::new(),
                gpu: Mutex::default(),
            }),
            tx,
            rx,
            workers: 0,
            entries: HashMap::new(),
            wanted: Vec::new(),
            bytes: 0,
            frame: 0,
            // Tests keep the user's cache as it is.
            cache_root: if cfg!(test) {
                None
            } else {
                DiskCache::default_root()
            },
            pruned: false,
            stats: Stats::default(),
        }
    }

    /// Where the disk cache is; `None` for none.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_cache_root(&mut self, root: Option<PathBuf>) {
        self.cache_root = root;
        self.pruned = false;
    }

    /// The renderer's device, for model previews (or a device of its own,
    /// made when the first model is drawn, when the window has none).
    pub fn set_gpu(&mut self, context: Option<crate::render::GpuContext>) {
        let mut gpu = lock(&self.shared.gpu);
        *gpu = GpuSlot::new(context, !cfg!(test));
    }

    /// A game was opened, or the workspace changed: what the workers read.
    pub fn set_source(
        &mut self,
        game: Arc<GameInstance>,
        index: Arc<ResourceIndex>,
        ws: Option<&Workspace>,
    ) {
        let cache = self
            .cache_root
            .clone()
            .map(|root| DiskCache::new(root, RENDER_VERSION));
        if !self.pruned
            && let Some(cache) = cache.clone()
        {
            self.pruned = true;
            std::thread::spawn(move || {
                let (removed, left) = cache.prune(DISK_BUDGET);
                if removed > 0 {
                    eprintln!(
                        "previews: {removed} cached image(s) removed, {} MiB left",
                        left >> 20
                    );
                }
            });
        }
        let source = Arc::new(PreviewSource::new(game, index, ws, cache));
        let mut queue = lock(&self.shared.queue);
        if let Some(old) = &queue.source
            && !Arc::ptr_eq(&old.game, &source.game)
        {
            queue.generation += 1;
            queue.waiting.clear();
            drop(queue);
            self.forget();
            queue = lock(&self.shared.queue);
        }
        queue.source = Some(source);
    }

    /// Closes the files the workers have open, before the game files are
    /// replaced.
    pub fn release(&mut self) {
        if let Some(source) = &lock(&self.shared.queue).source {
            source.close();
        }
    }

    /// The game files changed: previews are made again.
    pub fn files_changed(&mut self) {
        let mut queue = lock(&self.shared.queue);
        queue.generation += 1;
        queue.waiting.clear();
        if let Some(source) = &queue.source {
            source.close();
        }
        drop(queue);
        self.forget();
    }

    fn forget(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    /// The preview of `job` for a tile drawn this frame.
    pub fn get(&mut self, job: &PreviewJob) -> Shown<'_> {
        let frame = self.frame;
        let entry = self.entries.entry(job.id.clone()).or_insert(Entry {
            state: State::Pending,
            bytes: 0,
            used: frame,
        });
        entry.used = frame;
        if matches!(entry.state, State::Pending) {
            self.wanted.push(job.clone());
        }
        match &self.entries[&job.id].state {
            State::Ready { texture, .. } => Shown::Ready(texture),
            State::Pending => Shown::Pending,
            State::Failed(why) => Shown::Failed(why),
        }
    }

    /// The preview of a resource (in any version) that is ready.
    pub fn ready(&self, key: &ResourceKey) -> Option<&egui::TextureHandle> {
        self.entries
            .iter()
            .filter(|(id, _)| id.key == *key)
            .find_map(|(_, e)| match &e.state {
                State::Ready { texture, .. } => Some(texture),
                _ => None,
            })
    }

    /// The bounds of a model whose preview is ready.
    pub fn bounds_of(&self, key: &ResourceKey) -> Option<[[f32; 3]; 2]> {
        self.entries
            .iter()
            .filter(|(id, _)| id.key == *key)
            .find_map(|(_, e)| match &e.state {
                State::Ready { bounds, .. } => *bounds,
                _ => None,
            })
    }

    /// Whether previews are being made.
    pub fn busy(&self) -> bool {
        let queue = lock(&self.shared.queue);
        !queue.waiting.is_empty() || !queue.running.is_empty() || !self.wanted.is_empty()
    }

    /// Takes the finished previews, queues the requests of the frame and
    /// forgets the previews drawn least recently when over the budget.
    pub fn end_frame(&mut self) {
        self.frame += 1;
        // Finished previews first: a request of this frame for one of them
        // is not queued again.
        let generation = lock(&self.shared.queue).generation;
        let mut done = Vec::new();
        while let Ok(d) = self.rx.try_recv() {
            done.push(d);
        }
        let finished: Vec<PreviewId> = done.iter().map(|d| d.id.clone()).collect();
        for d in done {
            if d.generation == generation {
                self.finish(d);
            }
        }
        let now = Instant::now();
        let wanted = std::mem::take(&mut self.wanted);
        let mut queue = lock(&self.shared.queue);
        for id in &finished {
            queue.running.remove(id);
        }
        if !wanted.is_empty() && queue.source.is_some() {
            for job in wanted {
                let pending = self
                    .entries
                    .get(&job.id)
                    .is_some_and(|e| matches!(e.state, State::Pending));
                if !pending || queue.running.contains(&job.id) {
                    continue;
                }
                queue.waiting.insert(job.id.clone(), (job, now));
            }
            self.shared.wake.notify_all();
        }
        let waiting = !queue.waiting.is_empty();
        drop(queue);
        if waiting {
            self.start_workers();
        }
        self.evict();
    }

    fn finish(&mut self, d: Done) {
        let Some(entry) = self.entries.get_mut(&d.id) else {
            return;
        };
        match d.result {
            Ok((image, from_disk)) => {
                if from_disk {
                    self.stats.from_disk += 1;
                } else {
                    self.stats.made += 1;
                    self.stats.made_time += d.elapsed;
                    if image.bounds.is_some() {
                        self.stats.models += 1;
                        self.stats.models_time += d.elapsed;
                    }
                }
                let size = [image.width as usize, image.height as usize];
                let color = egui::ColorImage::from_rgba_unmultiplied(size, &image.rgba);
                let options = egui::TextureOptions::LINEAR
                    .with_mipmap_mode(Some(egui::TextureFilter::Linear));
                let texture =
                    self.ctx
                        .load_texture(format!("preview {}", d.id.key), color, options);
                entry.bytes = image.rgba.len() * 4 / 3;
                self.bytes += entry.bytes;
                entry.state = State::Ready {
                    texture,
                    bounds: image.bounds,
                };
            }
            Err(error) => {
                self.stats.failed += 1;
                entry.state = State::Failed(error);
            }
        }
    }

    fn evict(&mut self) {
        if self.bytes <= MEMORY_BUDGET {
            return;
        }
        let mut ready: Vec<(u64, PreviewId)> = self
            .entries
            .iter()
            .filter(|(_, e)| e.bytes > 0 && e.used + 1 < self.frame)
            .map(|(id, e)| (e.used, id.clone()))
            .collect();
        ready.sort_by_key(|(used, _)| *used);
        for (_, id) in ready {
            if self.bytes <= MEMORY_BUDGET / 4 * 3 {
                break;
            }
            if let Some(e) = self.entries.remove(&id) {
                self.bytes -= e.bytes;
            }
        }
    }

    /// Bytes of the previews in memory.
    pub fn memory(&self) -> usize {
        self.bytes
    }

    fn start_workers(&mut self) {
        let count = std::thread::available_parallelism()
            .map_or(2, |n| n.get() / 4)
            .clamp(2, 4);
        while self.workers < count {
            self.workers += 1;
            let (shared, tx, ctx) = (self.shared.clone(), self.tx.clone(), self.ctx.clone());
            let spawned = std::thread::Builder::new()
                .name(format!("preview {}", self.workers))
                .spawn(move || worker(&shared, &tx, &ctx));
            if spawned.is_err() {
                break;
            }
        }
    }
}

fn worker(shared: &Shared, tx: &Sender<Done>, ctx: &egui::Context) {
    loop {
        let (job, source, generation) = {
            let mut queue = lock(&shared.queue);
            loop {
                if queue.closed {
                    return;
                }
                let now = Instant::now();
                queue
                    .waiting
                    .retain(|_, (_, at)| now.duration_since(*at) < STALE);
                let newest = queue
                    .waiting
                    .iter()
                    .max_by_key(|(_, (_, at))| *at)
                    .map(|(id, _)| id.clone());
                if let (Some(id), Some(source)) = (newest, queue.source.clone()) {
                    let (job, _) = queue.waiting.remove(&id).expect("listed");
                    queue.running.insert(id);
                    break (job, source, queue.generation);
                }
                queue = shared
                    .wake
                    .wait_timeout(queue, Duration::from_millis(500))
                    .map(|(q, _)| q)
                    .unwrap_or_else(|e| e.into_inner().0);
            }
        };
        let started = Instant::now();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            make(&source, &job, &shared.gpu)
        }))
        .unwrap_or_else(|_| Err("the preview failed".to_owned()));
        let done = Done {
            generation,
            id: job.id,
            result,
            elapsed: started.elapsed(),
        };
        if tx.send(done).is_err() {
            return;
        }
        ctx.request_repaint();
    }
}

/// The preview of a job: from the disk cache, or made and cached. The
/// second value tells whether it came from the cache.
pub fn make(
    source: &PreviewSource,
    job: &PreviewJob,
    gpu: &Mutex<GpuSlot>,
) -> Result<(Thumbnail, bool), String> {
    let started = std::time::Instant::now();
    let data = thumb::data_hash(source, job)?;
    let key = cache_key(&job.id, job.kind, data, RENDER_VERSION);
    if let Some(cache) = &source.cache
        && let Some(image) = cache.get(key)
    {
        crate::profile::record("preview: from the disk cache", started.elapsed());
        return Ok((image, true));
    }
    let image = match job.kind {
        Kind::Texture => thumb::texture(source, job)?,
        Kind::Model => thumb::model(source, job, gpu)?,
    };
    crate::profile::record(
        match job.kind {
            Kind::Texture => "preview: make texture",
            Kind::Model => "preview: make model",
        },
        started.elapsed(),
    );
    if let Some(cache) = &source.cache
        && let Err(error) = cache.put(key, &image)
    {
        eprintln!("previews: cannot write the cache: {error}");
    }
    Ok((image, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(seed: u8) -> Thumbnail {
        Thumbnail {
            width: 4,
            height: 2,
            rgba: (0..32).map(|i| i as u8 ^ seed).collect(),
            bounds: Some([[0.0, 1.0, 2.0], [3.0, 4.0, 5.0]]),
        }
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dt-previews-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn id(ids: u32) -> PreviewId {
        PreviewId {
            key: ResourceKey {
                stream: "gf2/zone".into(),
                type_id: doctor_toolkit::hash::sdbm(b"metamodel"),
                ids: [ids; 4],
            },
            version: None,
        }
    }

    #[test]
    fn the_disk_cache_hits_misses_and_follows_data_and_version() {
        let root = temp("cache");
        let cache = DiskCache::new(&root, RENDER_VERSION);
        let data = |bytes: &[u8]| {
            let mut h = DataHash::default();
            h.write(bytes);
            h.finish()
        };
        let key = cache_key(&id(1), Kind::Model, data(b"model data"), RENDER_VERSION);
        assert!(cache.get(key).is_none(), "a miss first");
        cache.put(key, &image(0)).unwrap();
        let hit = cache.get(key).expect("a hit");
        assert_eq!(hit, image(0), "pixels and bounds come back");
        // Other data, another resource, another way of drawing, another
        // version of the renderer: other keys.
        let others = [
            cache_key(&id(1), Kind::Model, data(b"model datA"), RENDER_VERSION),
            cache_key(&id(2), Kind::Model, data(b"model data"), RENDER_VERSION),
            cache_key(&id(1), Kind::Texture, data(b"model data"), RENDER_VERSION),
            cache_key(&id(1), Kind::Model, data(b"model data"), RENDER_VERSION + 1),
        ];
        for other in others {
            assert_ne!(other, key);
            assert!(cache.get(other).is_none());
        }
        // A new renderer version does not read the old images and removes
        // them when pruning.
        let newer = DiskCache::new(&root, RENDER_VERSION + 1);
        assert!(newer.get(key).is_none());
        let (removed, left) = newer.prune(DISK_BUDGET);
        assert_eq!((removed, left), (2, 0), "the png and the bounds");
        assert!(cache.get(key).is_none());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn pruning_keeps_the_recently_used_images() {
        let root = temp("prune");
        let cache = DiskCache::new(&root, RENDER_VERSION);
        let old = std::time::SystemTime::now() - Duration::from_secs(3600);
        for k in 0..8u64 {
            let mut img = image(0);
            img.bounds = None;
            cache.put(k, &img).unwrap();
            let f = std::fs::File::options()
                .write(true)
                .open(cache.path(k))
                .unwrap();
            f.set_modified(old + Duration::from_secs(k)).unwrap();
        }
        let size = std::fs::metadata(cache.path(0)).unwrap().len();
        // A budget of four images: down to three, the newest.
        let (removed, left) = cache.prune(size * 4);
        assert_eq!(removed, 5);
        assert_eq!(left, size * 3);
        assert!(cache.get(7).is_some() && cache.get(5).is_some());
        assert!(cache.get(4).is_none() && cache.get(0).is_none());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn data_hashes_differ_by_content_and_length() {
        let hash = |bytes: &[u8]| {
            let mut h = DataHash::default();
            h.write(bytes);
            h.finish()
        };
        assert_eq!(hash(b"abc"), hash(b"abc"));
        assert_ne!(hash(b"abc"), hash(b"abd"));
        assert_ne!(hash(b"abc"), hash(b"abc\0"));
        assert_ne!(hash(&[0; 16]), hash(&[0; 24]));
        let mut split = DataHash::default();
        split.write(b"01234567");
        split.write(b"89");
        assert_eq!(split.finish(), hash(b"0123456789"));
    }
}

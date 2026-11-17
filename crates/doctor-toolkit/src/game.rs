//! A game directory: its streams, tables of contents and resource index.
//!
//! This is the only layer that writes into the game directory. Every file is
//! backed up before it is modified for the first time.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use memmap2::Mmap;
use rayon::prelude::*;

use crate::container::ustream::{self, Replacement, ResourceHeader, StreamLayout};
use crate::profile::GameProfile;
use crate::toc::{self, Toc};
use crate::{Error, Result};

/// Directory inside the game directory where originals are kept.
pub const BACKUP_DIR: &str = "_doctor_toolkit/backups";

/// Directories that hold tool data or caches of other programs, not game
/// data. They are skipped when scanning (compared ignoring case). Graphics
/// drivers keep shader caches next to the executable: NVIDIA's `GLCache`
/// holds `*.toc` files of its own.
const SKIPPED_DIRS: &[&str] = &[
    "_toolkit",
    "_doctor_toolkit",
    "GLCache",
    "DXCache",
    "NV_Cache",
    "ShaderCache",
    "shader_cache",
    "DxcCache",
];

/// A memory-mapped stream file together with its parsed layout.
///
/// The file must not be modified while it is open: the mapping would change
/// under the parsed layout.
#[derive(Debug)]
pub struct StreamFile {
    path: PathBuf,
    mmap: Mmap,
    layout: StreamLayout,
}

impl StreamFile {
    /// Maps and parses the stream at `path`.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let file = fs::File::open(&path).map_err(|e| Error::io(&path, e))?;
        // SAFETY: the mapping is read-only. Concurrent modification of the file
        // by another process is outside of what this tool can prevent.
        let mmap = unsafe { Mmap::map(&file) }.map_err(|e| Error::io(&path, e))?;
        let layout = StreamLayout::parse(&mmap)?;
        Ok(Self { path, mmap, layout })
    }

    /// Path the stream was opened from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The raw file contents.
    pub fn data(&self) -> &[u8] {
        &self.mmap
    }

    /// The parsed block structure.
    pub fn layout(&self) -> &StreamLayout {
        &self.layout
    }

    /// Reads and decompresses the data of resource `index`.
    pub fn resource_data(&self, index: usize) -> Result<Vec<u8>> {
        self.layout.resource_data(&self.mmap, index)
    }
}

/// A file found in the game directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameFile {
    /// Path relative to the game directory, with forward slashes.
    pub rel_path: String,
    /// Absolute path.
    pub path: PathBuf,
    /// File size in bytes.
    pub len: u64,
}

impl GameFile {
    /// The stream name as tables of contents spell it: lower case, forward
    /// slashes, no extension.
    pub fn stream_name(&self) -> String {
        let without_ext = self
            .rel_path
            .rsplit_once('.')
            .map_or(&*self.rel_path, |(stem, _)| stem);
        without_ext.to_ascii_lowercase()
    }
}

/// One resource in the game-wide index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexEntry {
    /// Index into [`GameInstance::streams`].
    pub stream: u32,
    /// Index of the resource inside its stream.
    pub resource: u32,
    pub header: ResourceHeader,
    /// Bytes the data blocks occupy on disk.
    pub stored_size: u32,
    pub compressed: bool,
}

/// Totals for one resource type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypeSummary {
    pub type_id: u32,
    pub count: usize,
    /// Sum of decompressed sizes.
    pub data_bytes: u64,
    /// Sum of on-disk sizes.
    pub stored_bytes: u64,
}

/// All resources of a game.
#[derive(Debug, Clone, Default)]
pub struct ResourceIndex {
    /// Entries ordered by stream, then by position in the stream.
    pub entries: Vec<IndexEntry>,
}

impl ResourceIndex {
    /// Totals per resource type, most frequent type first.
    pub fn type_summary(&self) -> Vec<TypeSummary> {
        let mut by_type: std::collections::HashMap<u32, TypeSummary> = Default::default();
        for entry in &self.entries {
            let summary = by_type.entry(entry.header.type_id).or_insert(TypeSummary {
                type_id: entry.header.type_id,
                count: 0,
                data_bytes: 0,
                stored_bytes: 0,
            });
            summary.count += 1;
            summary.data_bytes += entry.header.data_size as u64;
            summary.stored_bytes += entry.stored_size as u64;
        }
        let mut list: Vec<_> = by_type.into_values().collect();
        list.sort_by(|a, b| b.count.cmp(&a.count).then(a.type_id.cmp(&b.type_id)));
        list
    }
}

/// Options for [`GameInstance::replace_resources`].
#[derive(Debug, Clone, Copy)]
pub struct ReplaceOptions {
    /// Update the stream's size in every table of contents that lists it.
    pub update_toc: bool,
}

impl Default for ReplaceOptions {
    fn default() -> Self {
        Self { update_toc: true }
    }
}

/// What [`GameInstance::replace_resources`] changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplaceReport {
    /// Stream size before and after, in bytes.
    pub old_len: u64,
    pub new_len: u64,
    /// Sum of decompressed resource sizes before and after.
    pub old_data_size: u64,
    pub new_data_size: u64,
    /// Relative paths of the files that were written.
    pub written: Vec<String>,
    /// Relative paths of the files that were backed up by this call.
    pub backed_up: Vec<String>,
}

/// An installed game.
#[derive(Debug, Clone)]
pub struct GameInstance {
    root: PathBuf,
    profile: GameProfile,
    streams: Vec<GameFile>,
    tocs: Vec<GameFile>,
    skipped: Vec<GameFile>,
    /// A staged copy whose changed streams are resource patches
    /// ([`Self::stage_resource_patches`]).
    resource_patches: bool,
    /// Streams that are not in the game folder ([`Self::with_staged`]).
    staged: Vec<Staged>,
}

/// A stream that is not in the game folder yet (a stream a workspace
/// creates), given to [`GameInstance::with_staged`]: readers see it as one
/// more stream of the game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedStream {
    /// Stream name: lower case, forward slashes, no extension.
    pub name: String,
    /// The file that holds it, outside the game folder.
    pub path: PathBuf,
    /// The streams loaded before it, as the tables of contents will say.
    pub parents: Vec<String>,
    /// Its hash in the tables of contents.
    pub hash: u32,
}

/// One staged stream of a [`GameInstance`].
#[derive(Debug, Clone)]
struct Staged {
    /// Its index in `streams`.
    index: usize,
    /// The file of the game folder it stands for, when there is one (the
    /// stream was written into the game earlier).
    on_disk: Option<GameFile>,
    name: String,
    parents: Vec<String>,
    hash: u32,
}

impl GameInstance {
    /// Scans the game directory `root` for streams and tables of contents.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        match fs::metadata(&root) {
            Err(error) => return Err(Error::io(root, error)),
            Ok(metadata) if !metadata.is_dir() => return Err(Error::NotAGame { path: root }),
            Ok(_) => {}
        }
        let detected = GameProfile::detect(&root);
        // Do not walk a whole home directory chosen by mistake.
        if detected.is_none() && !has_game_files(&root, 2) {
            return Err(Error::NotAGame { path: root });
        }
        let profile = detected.unwrap_or(GameProfile::GF2_PC);
        let mut found = Found::default();
        scan(&root, "", &mut found)?;
        let Found {
            mut streams,
            mut tocs,
            mut skipped,
        } = found;
        if streams.is_empty() && tocs.is_empty() {
            return Err(Error::NotAGame { path: root });
        }
        streams.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
        tocs.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
        skipped.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
        Ok(Self {
            root,
            profile,
            streams,
            tocs,
            skipped,
            resource_patches: false,
            staged: Vec::new(),
        })
    }

    /// This game with `streams` shown as streams of it: readers
    /// ([`Self::streams`], [`Self::find_stream`], [`Self::open_stream`],
    /// [`Self::build_index`], [`Self::stream_parents`], [`Self::stream_hash`])
    /// see the game as it will be once the streams are written into it.
    ///
    /// The streams of the game folder keep their indices: a staged stream is
    /// listed after them, in the order given, or takes the place of the file
    /// of the same name when the game folder has one. Staged streams of
    /// `self` are dropped first. Nothing of the game folder is touched, and
    /// the functions that write straight into the game refuse a staged
    /// stream.
    pub fn with_staged(&self, streams: &[StagedStream]) -> Result<Self> {
        let mut game = self.on_disk();
        for stream in streams {
            let len = fs::metadata(&stream.path)
                .map_err(|e| Error::io(&stream.path, e))?
                .len();
            let name =
                String::from_utf8_lossy(&toc::normalize_name(stream.name.as_bytes())).into_owned();
            let (index, on_disk) = match game.find_stream(&name) {
                Some(index) => {
                    if game.staged.iter().any(|s| s.index == index) {
                        return Err(Error::InvalidArgument(format!(
                            "stream {name} is staged twice"
                        )));
                    }
                    let file = &mut game.streams[index];
                    let on_disk = file.clone();
                    file.path = stream.path.clone();
                    file.len = len;
                    (index, Some(on_disk))
                }
                None => {
                    game.streams.push(GameFile {
                        rel_path: format!("{name}.str"),
                        path: stream.path.clone(),
                        len,
                    });
                    (game.streams.len() - 1, None)
                }
            };
            game.staged.push(Staged {
                index,
                on_disk,
                name,
                parents: stream.parents.clone(),
                hash: stream.hash,
            });
        }
        Ok(game)
    }

    /// The game as its folder has it: without the staged streams.
    pub fn on_disk(&self) -> Self {
        let mut game = self.clone();
        let staged = std::mem::take(&mut game.staged);
        let appended = staged.iter().filter(|s| s.on_disk.is_none()).count();
        game.streams.truncate(game.streams.len() - appended);
        for s in staged {
            if let Some(file) = s.on_disk {
                game.streams[s.index] = file;
            }
        }
        game
    }

    /// Whether some streams are staged ([`Self::with_staged`]).
    pub fn has_staged(&self) -> bool {
        !self.staged.is_empty()
    }

    /// Whether stream `index` is staged: its file is not the game's.
    pub fn is_staged(&self, index: usize) -> bool {
        self.staged.iter().any(|s| s.index == index)
    }

    /// Indices of the staged streams.
    pub fn staged_streams(&self) -> Vec<usize> {
        self.staged.iter().map(|s| s.index).collect()
    }

    /// The number of streams the game folder has: every index below it is
    /// the same with and without staging.
    pub fn disk_stream_count(&self) -> usize {
        self.streams.len() - self.staged.iter().filter(|s| s.on_disk.is_none()).count()
    }

    /// The index of this game from the index of the game folder
    /// ([`Self::on_disk`]): the entries of the staged streams are read from
    /// their files, the rest is copied.
    pub fn staged_index(&self, on_disk: &ResourceIndex) -> ResourceIndex {
        if self.staged.is_empty() {
            return on_disk.clone();
        }
        let limit = self.disk_stream_count();
        let mut entries: Vec<IndexEntry> = on_disk
            .entries
            .iter()
            .filter(|e| (e.stream as usize) < limit && !self.is_staged(e.stream as usize))
            .copied()
            .collect();
        for s in &self.staged {
            if let Ok(stream) = self.open_stream(s.index) {
                entries.extend(index_entries(s.index, &stream));
            }
        }
        // Stable: the entries of one stream stay in the order of its resources.
        entries.sort_by_key(|e| e.stream);
        ResourceIndex { entries }
    }

    /// The file of stream `index` in the game folder: an error for a staged
    /// stream, which the functions that write must not touch.
    fn disk_file(&self, index: usize) -> Result<&GameFile> {
        let entry = self
            .streams
            .get(index)
            .ok_or_else(|| Error::InvalidArgument(format!("stream index {index} out of range")))?;
        if self.is_staged(index) {
            return Err(Error::InvalidArgument(format!(
                "stream {} is new in the workspace and not in the game files: edit it \
                 through the workspace",
                entry.stream_name()
            )));
        }
        Ok(entry)
    }

    /// Marks this instance as a staged copy of a game (stage 46,
    /// [`crate::play::overlay`]) whose changed streams are written as
    /// **resource patches** (stage 100, [`doctor_toolkit_live::resources`])
    /// instead of rebuilt streams: the live module composes each stream in
    /// memory from the game's own file and the patch. Never set for a real
    /// game folder — the game cannot read a patch.
    pub(crate) fn stage_resource_patches(&mut self) {
        self.resource_patches = true;
    }

    /// Whether changed streams are written as resource patches.
    pub fn resource_patches(&self) -> bool {
        self.resource_patches
    }

    /// The game directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Format parameters of this game.
    pub fn profile(&self) -> &GameProfile {
        &self.profile
    }

    /// All `.str` files, sorted by relative path.
    pub fn streams(&self) -> &[GameFile] {
        &self.streams
    }

    /// All `.toc` files, sorted by relative path.
    pub fn tocs(&self) -> &[GameFile] {
        &self.tocs
    }

    /// Files named like streams or tables of contents that are not the
    /// game's (they do not start with the format's magic): caches of
    /// graphics drivers and other programs. They are never read or written.
    pub fn skipped(&self) -> &[GameFile] {
        &self.skipped
    }

    /// Finds a stream by name. Accepts a table-of-contents name (`gf2\frontend`),
    /// a relative path (`gf2/frontend.str`) in any case.
    pub fn find_stream(&self, name: &str) -> Option<usize> {
        let mut wanted =
            String::from_utf8_lossy(&toc::normalize_name(name.as_bytes())).into_owned();
        if let Some(stem) = wanted.strip_suffix(".str") {
            wanted = stem.to_owned();
        }
        self.streams.iter().position(|s| s.stream_name() == wanted)
    }

    /// Opens stream `index`.
    pub fn open_stream(&self, index: usize) -> Result<StreamFile> {
        let entry = self
            .streams
            .get(index)
            .ok_or_else(|| Error::InvalidArgument(format!("stream index {index} out of range")))?;
        StreamFile::open(&entry.path)
    }

    /// Reads and parses table of contents `index`.
    pub fn load_toc(&self, index: usize) -> Result<Toc> {
        let entry = self
            .tocs
            .get(index)
            .ok_or_else(|| Error::InvalidArgument(format!("toc index {index} out of range")))?;
        let data = fs::read(&entry.path).map_err(|e| Error::io(&entry.path, e))?;
        Toc::parse(&data)
    }

    /// The prerequisites of every stream in every table of contents: the
    /// streams the game loads before it. Names are normalised like
    /// [`GameFile::stream_name`].
    pub fn stream_parents(&self) -> Result<HashMap<String, Vec<String>>> {
        let normalize =
            |bytes: &[u8]| String::from_utf8_lossy(&toc::normalize_name(bytes)).into_owned();
        let mut parents: HashMap<String, Vec<String>> = HashMap::new();
        for index in 0..self.tocs.len() {
            let table = self.load_toc(index)?;
            for (file_index, file) in table.files.iter().enumerate() {
                let deps = &table.dep_words[file.deps.clone()][..file.prerequisite_count()];
                let list = parents
                    .entry(normalize(&table.file_name(file_index).name))
                    .or_default();
                for &dep in deps {
                    let dep = normalize(&table.names[dep].name);
                    if !list.contains(&dep) {
                        list.push(dep);
                    }
                }
            }
        }
        for staged in &self.staged {
            let list = parents.entry(staged.name.clone()).or_default();
            for parent in &staged.parents {
                if !list.contains(parent) {
                    list.push(parent.clone());
                }
            }
        }
        Ok(parents)
    }

    /// Streams the game loads before stream `name`: its prerequisites in
    /// every table of contents, transitively, nearest first.
    pub fn stream_ancestors(&self, name: &str) -> Result<Vec<String>> {
        Ok(ancestors(&self.stream_parents()?, name))
    }

    /// Hash of stream `name` in the tables of contents: the fold of the
    /// GUID of the stream ([`toc::guid_hash`]); 0 for the root `*_global`
    /// streams. `None` when no table lists the stream.
    pub fn stream_hash(&self, name: &str) -> Result<Option<u32>> {
        for index in 0..self.tocs.len() {
            let toc = self.load_toc(index)?;
            if let Some(found) = toc.find_name(name) {
                return Ok(Some(toc.names[found].hash));
            }
        }
        let wanted = String::from_utf8_lossy(&toc::normalize_name(name.as_bytes())).into_owned();
        if let Some(staged) = self.staged.iter().find(|s| s.name == wanted) {
            return Ok(Some(staged.hash));
        }
        Ok(None)
    }

    /// Every non-zero name hash in the tables of contents.
    pub fn toc_hashes(&self) -> Result<std::collections::HashSet<u32>> {
        let mut out = std::collections::HashSet::new();
        for index in 0..self.tocs.len() {
            out.extend(
                self.load_toc(index)?
                    .names
                    .iter()
                    .map(|n| n.hash)
                    .filter(|&h| h != 0),
            );
        }
        out.extend(self.staged.iter().map(|s| s.hash).filter(|&h| h != 0));
        Ok(out)
    }

    /// Builds the index of all resources, reading the streams in parallel.
    ///
    /// `progress` is called once per finished stream, from worker threads.
    /// Streams that fail to parse are reported in the second element.
    pub fn build_index(
        &self,
        progress: impl Fn(&GameFile) + Sync,
    ) -> (ResourceIndex, Vec<(usize, Error)>) {
        let results: Vec<_> = self
            .streams
            .par_iter()
            .enumerate()
            .map(|(stream_index, file)| {
                let result =
                    StreamFile::open(&file.path).map(|stream| index_entries(stream_index, &stream));
                progress(file);
                (stream_index, result)
            })
            .collect();

        let mut index = ResourceIndex::default();
        let mut errors = Vec::new();
        for (stream_index, result) in results {
            match result {
                Ok(entries) => index.entries.extend(entries),
                Err(error) => errors.push((stream_index, error)),
            }
        }
        (index, errors)
    }

    /// Replaces the data of resources in stream `stream_index` on disk.
    ///
    /// The original stream and every table of contents that is modified are
    /// copied to [`BACKUP_DIR`] first, unless a backup already exists.
    pub fn replace_resources(
        &self,
        stream_index: usize,
        replacements: &[Replacement],
        options: ReplaceOptions,
    ) -> Result<ReplaceReport> {
        let entry = self.disk_file(stream_index)?;

        let (rebuilt, old_data_size) = {
            let stream = StreamFile::open(&entry.path)?;
            let rebuilt = ustream::rebuild(stream.data(), stream.layout(), replacements)?;
            (rebuilt, stream.layout().total_data_size())
            // The mapping is dropped here, before the file is replaced.
        };
        let new_layout = StreamLayout::parse(&rebuilt)?;
        let new_data_size = new_layout.total_data_size();
        let toc_size = u32::try_from(new_data_size)
            .map_err(|_| Error::InvalidArgument("stream data exceeds 4 GiB".into()))?;

        // Prepare every table of contents before touching the disk.
        let mut toc_updates = Vec::new();
        if options.update_toc {
            let name = entry.stream_name();
            for toc_file in &self.tocs {
                let data = fs::read(&toc_file.path).map_err(|e| Error::io(&toc_file.path, e))?;
                let mut table = Toc::parse(&data)?;
                if let Some(file_index) = table.find_file(&name)
                    && table.files[file_index].data_size != toc_size
                {
                    table.files[file_index].data_size = toc_size;
                    toc_updates.push((toc_file, table.to_bytes()?));
                }
            }
        }

        let mut report = ReplaceReport {
            old_len: entry.len,
            new_len: rebuilt.len() as u64,
            old_data_size,
            new_data_size,
            written: Vec::new(),
            backed_up: Vec::new(),
        };
        let mut write = self.begin_write();
        write.replace(entry, &rebuilt)?;
        for (toc_file, bytes) in toc_updates {
            write.replace(toc_file, &bytes)?;
        }
        let done = write.commit().map_err(|failure| failure.error)?;
        report.written = done.written;
        report.backed_up = done.backed_up;
        Ok(report)
    }

    /// Replaces a texture of stream `stream_index` with a DDS or PNG image.
    ///
    /// `pair` locates the texture; the header keeps its identity, see
    /// [`crate::resource::texture::from_dds`] and [`crate::resource::texture::from_png`].
    /// Both resources keep their storage (raw or compressed).
    pub fn import_texture(
        &self,
        stream_index: usize,
        pair: crate::resource::texture::TexturePair,
        image: &[u8],
        is_png: bool,
    ) -> Result<(crate::resource::texture::Texture, ReplaceReport)> {
        use crate::container::ustream::Storage;
        use crate::resource::texture::{self, Texture};

        let stream = self.open_stream(stream_index)?;
        let layout = stream.layout();
        let original = Texture::load(stream.data(), layout, pair)?;
        let replacement = if is_png {
            texture::from_png(&original.header, image)?
        } else {
            texture::from_dds(&original.header, image)?
        };
        let header_bytes = replacement.header.to_bytes()?;
        let storage = |index: usize| {
            if layout.is_compressed(index) {
                Storage::RefPack
            } else {
                Storage::Raw
            }
        };
        let replacements = [
            Replacement {
                index: pair.header,
                data: &header_bytes,
                storage: storage(pair.header),
            },
            Replacement {
                index: pair.data,
                data: &replacement.data,
                storage: storage(pair.data),
            },
        ];
        let replacements = replacements.to_vec();
        drop(stream);
        let report =
            self.replace_resources(stream_index, &replacements, ReplaceOptions::default())?;
        Ok((replacement, report))
    }

    /// Replaces string table (`lh2`) resource `index` of stream `stream_index`.
    pub fn import_string_table(
        &self,
        stream_index: usize,
        index: usize,
        table: &crate::resource::lh2::StringTable,
    ) -> Result<ReplaceReport> {
        use crate::container::ustream::Storage;
        let stream = self.open_stream(stream_index)?;
        let storage = if stream.layout().is_compressed(index) {
            Storage::RefPack
        } else {
            Storage::Raw
        };
        drop(stream);
        let bytes = table.to_bytes()?;
        self.replace_resources(
            stream_index,
            &[Replacement {
                index,
                data: &bytes,
                storage,
            }],
            ReplaceOptions::default(),
        )
    }

    /// Restores every file that has a backup and removes the backups.
    /// Returns the relative paths of the restored files.
    pub fn restore_backups(&self) -> Result<Vec<String>> {
        let backup_root = self.root.join(BACKUP_DIR);
        let mut restored = Vec::new();
        if !backup_root.is_dir() {
            return Ok(restored);
        }
        let mut files = Vec::new();
        collect_files(&backup_root, "", &mut files)?;
        files.sort();
        for rel_path in files {
            let backup = backup_root.join(&rel_path);
            let target = self.root.join(&rel_path);
            crate::atomic::copy(&backup, &target)?;
            fs::remove_file(&backup).map_err(|e| Error::io(&backup, e))?;
            restored.push(rel_path);
        }
        // Remove the now empty backup directories, up to and including the tool directory.
        remove_empty_dirs(&backup_root)?;
        if let Some(tool_dir) = backup_root.parent() {
            let _ = fs::remove_dir(tool_dir);
        }
        Ok(restored)
    }

    /// Whether `file` has a backup of its original contents.
    pub fn has_backup(&self, file: &GameFile) -> bool {
        self.root.join(BACKUP_DIR).join(&file.rel_path).is_file()
    }

    /// Path of the backup of `file`, whether it exists or not.
    pub fn backup_path(&self, file: &GameFile) -> PathBuf {
        self.root.join(BACKUP_DIR).join(&file.rel_path)
    }

    /// Path of the original contents of `file`: its backup when one exists,
    /// otherwise the file itself.
    pub fn original_path(&self, file: &GameFile) -> PathBuf {
        let backup = self.backup_path(file);
        if backup.is_file() {
            backup
        } else {
            file.path.clone()
        }
    }

    /// Backs `file` up unless a backup exists. Returns whether a backup was made.
    ///
    /// The backup is written atomically: a torn backup would later be trusted
    /// as the original.
    pub fn ensure_backup(&self, file: &GameFile) -> Result<bool> {
        let backup = self.backup_path(file);
        if backup.exists() {
            return Ok(false);
        }
        if let Some(parent) = backup.parent() {
            fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
        // A staged copy with resource patches keeps no copies of streams:
        // the "backup" is a link to the game's own file (the file here is
        // itself a link to it).
        if self.resource_patches
            && let Ok(target) = fs::canonicalize(&file.path)
            && link_file(&target, &backup).is_ok()
        {
            return Ok(true);
        }
        crate::atomic::copy(&file.path, &backup)?;
        Ok(true)
    }

    /// Starts a write of several game files as one operation; see [`GameWrite`].
    pub fn begin_write(&self) -> GameWrite<'_> {
        GameWrite {
            game: self,
            steps: Vec::new(),
        }
    }
}

fn link_file(target: &Path, at: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, at)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(target, at)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, at);
        Err(std::io::Error::other("no symbolic links here"))
    }
}

/// Tool directory where a [`GameWrite`] keeps the files it replaces until
/// it is done.
pub const UNDO_DIR: &str = "_doctor_toolkit/undo";

/// Several game files changed as one operation.
///
/// Everything that can fail is done first: the new contents are written and
/// flushed next to their files ([`crate::atomic::Staged`]). [`Self::commit`]
/// then backs up every replaced file that has no backup yet, and only after
/// that puts the new files in place, one rename each, keeping every replaced
/// file in [`UNDO_DIR`]. When a step fails, the files already put in place
/// get their previous contents back and the backups made by this write are
/// removed: the game is as before. Dropped without a commit, it changes
/// nothing.
#[derive(Debug)]
pub struct GameWrite<'g> {
    game: &'g GameInstance,
    steps: Vec<WriteStep>,
}

#[derive(Debug)]
struct WriteStep {
    rel_path: String,
    path: PathBuf,
    /// The new contents; `None` deletes the file.
    staged: Option<crate::atomic::Staged>,
    /// Back the file up before it changes the first time.
    backup: bool,
}

/// What [`GameWrite::commit`] changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WriteDone {
    /// Relative paths of the files written or deleted.
    pub written: Vec<String>,
    /// Relative paths of the files backed up by this write.
    pub backed_up: Vec<String>,
}

/// A failed [`GameWrite::commit`].
#[derive(Debug)]
pub struct WriteFailure {
    pub error: Error,
    /// Whether the game files are as they were before the write. When not,
    /// the replaced files are in [`UNDO_DIR`] and the originals in
    /// [`BACKUP_DIR`].
    pub unchanged: bool,
}

#[cfg(test)]
thread_local! {
    /// Tests: fail a [`GameWrite::commit`] of this thread after this many
    /// files are in place.
    pub(crate) static FAIL_WRITE_AFTER: std::cell::Cell<Option<usize>> =
        const { std::cell::Cell::new(None) };
}

impl GameWrite<'_> {
    /// Whether nothing is to be written.
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// Replaces the game file `file` with `contents`; it is backed up first.
    pub fn replace(&mut self, file: &GameFile, contents: &[u8]) -> Result<()> {
        self.push(&file.rel_path, Some(contents), true)
    }

    /// Writes the file `rel_path` the tool owns (a created stream), without a backup.
    pub fn create(&mut self, rel_path: &str, contents: &[u8]) -> Result<()> {
        self.push(rel_path, Some(contents), false)
    }

    /// Deletes the file `rel_path` the tool owns (a created stream).
    pub fn delete(&mut self, rel_path: &str) -> Result<()> {
        self.push(rel_path, None, false)
    }

    fn push(&mut self, rel_path: &str, contents: Option<&[u8]>, backup: bool) -> Result<()> {
        let path = self.game.root.join(rel_path);
        if self.steps.iter().any(|s| s.path == path) {
            return Err(Error::InvalidArgument(format!(
                "{rel_path} is written twice in one operation"
            )));
        }
        let staged = match contents {
            Some(contents) => {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
                }
                Some(crate::atomic::stage(&path, contents)?)
            }
            None => None,
        };
        self.steps.push(WriteStep {
            rel_path: rel_path.to_owned(),
            path,
            staged,
            backup,
        });
        Ok(())
    }

    /// Puts every file in place; see [`GameWrite`].
    pub fn commit(mut self) -> std::result::Result<WriteDone, WriteFailure> {
        let game = self.game;
        let undo_root = game.root.join(UNDO_DIR);
        // A leftover of an earlier write that could not finish.
        if undo_root.exists() {
            fs::remove_dir_all(&undo_root).map_err(|e| WriteFailure {
                error: Error::io(&undo_root, e),
                unchanged: true,
            })?;
        }
        let mut done = WriteDone::default();
        // Backups first: until every one is made, no game file changes.
        for step in &self.steps {
            if !step.backup || fs::symlink_metadata(&step.path).is_err() {
                continue;
            }
            let file = GameFile {
                rel_path: step.rel_path.clone(),
                path: step.path.clone(),
                len: 0,
            };
            match game.ensure_backup(&file) {
                Ok(true) => done.backed_up.push(step.rel_path.clone()),
                Ok(false) => {}
                Err(error) => {
                    game.remove_backups(&done.backed_up);
                    return Err(WriteFailure {
                        error,
                        unchanged: true,
                    });
                }
            }
        }
        // `placed[i]`: whether step `i` kept a replaced file in the undo folder.
        let mut placed: Vec<bool> = Vec::new();
        let mut failure = None;
        for step in &mut self.steps {
            #[cfg(test)]
            if FAIL_WRITE_AFTER.get() == Some(placed.len()) {
                failure = Some(Error::io(
                    &step.path,
                    std::io::Error::other("simulated failure"),
                ));
                break;
            }
            match place(step, &undo_root) {
                Ok(kept) => placed.push(kept),
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            }
        }
        let Some(error) = failure else {
            let _ = fs::remove_dir_all(&undo_root);
            done.written = self.steps.iter().map(|s| s.rel_path.clone()).collect();
            return Ok(done);
        };
        // Give the files already in place their previous contents, last first.
        let mut put_back = Ok(());
        for (step, kept) in self.steps.iter().zip(&placed).rev() {
            let result = if *kept {
                fs::rename(undo_root.join(&step.rel_path), &step.path)
            } else {
                fs::remove_file(&step.path)
            };
            if let Err(e) = result {
                put_back = Err(Error::io(&step.path, e));
                break;
            }
        }
        match put_back {
            Ok(()) => {
                game.remove_backups(&done.backed_up);
                let _ = fs::remove_dir_all(&undo_root);
                Err(WriteFailure {
                    error: Error::mod_error(format!(
                        "{error}. The files written so far were put back: nothing is changed \
                         in the game"
                    )),
                    unchanged: true,
                })
            }
            Err(again) => Err(WriteFailure {
                error: Error::mod_error(format!(
                    "{error}. Putting the written files back failed too ({again}); the \
                     replaced files are in {UNDO_DIR}. Use Restore originals to put the \
                     game's originals back"
                )),
                unchanged: false,
            }),
        }
    }
}

/// Puts the new contents of `step` in place, keeping the file it replaces
/// (a hard link, or a copy where links are not supported) under `undo_root`.
/// Returns whether there was a file to keep.
fn place(step: &mut WriteStep, undo_root: &Path) -> Result<bool> {
    let kept = fs::symlink_metadata(&step.path).is_ok();
    if kept {
        let undo = undo_root.join(&step.rel_path);
        if let Some(parent) = undo.parent() {
            fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
        if fs::hard_link(&step.path, &undo).is_err() {
            fs::copy(&step.path, &undo).map_err(|e| Error::io(&undo, e))?;
        }
    }
    match step.staged.take() {
        Some(staged) => staged.commit()?,
        None if kept => fs::remove_file(&step.path).map_err(|e| Error::io(&step.path, e))?,
        None => {}
    }
    Ok(kept)
}

impl GameInstance {
    /// Removes the backups `rel_paths`, made by a write that did not happen.
    fn remove_backups(&self, rel_paths: &[String]) {
        for rel_path in rel_paths {
            let _ = fs::remove_file(self.root.join(BACKUP_DIR).join(rel_path));
        }
    }
}

/// The index entries of the resources of stream `stream_index`.
fn index_entries(stream_index: usize, stream: &StreamFile) -> Vec<IndexEntry> {
    let layout = stream.layout();
    layout
        .resources
        .iter()
        .enumerate()
        .map(|(resource_index, resource)| IndexEntry {
            stream: stream_index as u32,
            resource: resource_index as u32,
            header: resource.header,
            stored_size: layout.stored_size(resource_index) as u32,
            compressed: layout.is_compressed(resource_index),
        })
        .collect()
}

/// The ancestors of `name` in a map from [`GameInstance::stream_parents`],
/// nearest first.
pub fn ancestors(parents: &HashMap<String, Vec<String>>, name: &str) -> Vec<String> {
    let start = String::from_utf8_lossy(&toc::normalize_name(name.as_bytes())).into_owned();
    let mut out: Vec<String> = Vec::new();
    let mut todo = std::collections::VecDeque::from([start.trim_end_matches(".str").to_owned()]);
    while let Some(current) = todo.pop_front() {
        for parent in parents.get(&current).into_iter().flatten() {
            if !out.contains(parent) {
                out.push(parent.clone());
                todo.push_back(parent.clone());
            }
        }
    }
    out
}

/// Finds materials and textures anywhere in the game for model export.
///
/// Textures are found through the resource index (their last id is the name
/// hash). Materials are identified by the name hash inside their data, so the
/// first material lookup reads every material of the game once.
pub struct GameAssets {
    game: std::sync::Arc<GameInstance>,
    textures: HashMap<u32, (usize, usize)>,
    materials: Option<HashMap<u32, (usize, usize)>>,
    /// `rcb` resources as `(stream, resource)`, parsed on first use.
    rig_locations: Vec<(usize, usize)>,
    rigs: Option<Vec<crate::resource::rcb::Rig>>,
    open: HashMap<usize, StreamFile>,
}

const TYPE_MATERIAL: u32 = crate::hash::sdbm(b"material");

impl GameAssets {
    pub fn new(game: std::sync::Arc<GameInstance>, index: &ResourceIndex) -> Self {
        let mut textures = HashMap::new();
        let mut rig_locations = Vec::new();
        for entry in &index.entries {
            let location = (entry.stream as usize, entry.resource as usize);
            if entry.header.type_id == crate::resource::texture::TYPE_TG4H {
                textures.entry(entry.header.ids[3]).or_insert(location);
            } else if entry.header.type_id == crate::resource::rcb::TYPE_RCB {
                rig_locations.push(location);
            }
        }
        Self {
            game,
            textures,
            materials: None,
            rig_locations,
            rigs: None,
            open: HashMap::new(),
        }
    }

    fn stream(&mut self, index: usize) -> Option<&StreamFile> {
        if !self.open.contains_key(&index) {
            let stream = self.game.open_stream(index).ok()?;
            self.open.insert(index, stream);
        }
        self.open.get(&index)
    }

    /// `(stream, resource)` of the material whose name hash is `hash`.
    pub fn material_location(&mut self, hash: u32) -> Option<(usize, usize)> {
        self.material_map().get(&hash).copied()
    }

    /// `(stream, resource)` of the header (`tg4h`) of the texture `hash`.
    pub fn texture_location(&self, hash: u32) -> Option<(usize, usize)> {
        self.textures.get(&hash).copied()
    }

    /// Every rig of the game, parsed on first use.
    pub fn rigs(&mut self) -> &[crate::resource::rcb::Rig] {
        if self.rigs.is_none() {
            let mut rigs = Vec::new();
            for (s, r) in self.rig_locations.clone() {
                let id = self
                    .stream(s)
                    .and_then(|stream| stream.layout().resources.get(r))
                    .map_or(0, |res| res.header.ids[3]);
                if let Some(mut rig) = self
                    .stream(s)
                    .and_then(|stream| stream.resource_data(r).ok())
                    .and_then(|data| crate::resource::rcb::Rig::parse(&data).ok())
                {
                    rig.id = id;
                    rigs.push(rig);
                }
            }
            self.rigs = Some(rigs);
        }
        self.rigs.as_deref().unwrap_or_default()
    }

    fn material_map(&mut self) -> &HashMap<u32, (usize, usize)> {
        let game = &self.game;
        self.materials.get_or_insert_with(|| {
            let found: Vec<(u32, usize, usize)> = (0..game.streams().len())
                .into_par_iter()
                .flat_map_iter(|s| {
                    let Ok(stream) = game.open_stream(s) else {
                        return Vec::new();
                    };
                    let layout = stream.layout();
                    (0..layout.resources.len())
                        .filter(|&r| layout.resources[r].header.type_id == TYPE_MATERIAL)
                        .filter_map(|r| {
                            let data = stream.resource_data(r).ok()?;
                            let hash =
                                crate::io::read_u32(&data, 8, crate::io::Endian::Little).ok()?;
                            Some((hash, s, r))
                        })
                        .collect::<Vec<_>>()
                })
                .collect();
            let mut map = HashMap::new();
            for (hash, s, r) in found {
                map.entry(hash).or_insert((s, r));
            }
            map
        })
    }
}

impl crate::export::gltf::AssetSource for GameAssets {
    fn rigs(&mut self) -> &[crate::resource::rcb::Rig] {
        GameAssets::rigs(self)
    }

    fn material(&mut self, hash: u32) -> Option<crate::resource::material::Material> {
        let &(s, r) = self.material_map().get(&hash)?;
        let data = self.stream(s)?.resource_data(r).ok()?;
        crate::resource::material::Material::parse(&data).ok()
    }

    fn texture(&mut self, hash: u32) -> Option<crate::resource::texture::Texture> {
        let &(s, r) = self.textures.get(&hash)?;
        let stream = self.stream(s)?;
        let pair = crate::resource::texture::pair_of(stream.layout(), r)?;
        crate::resource::texture::Texture::load(stream.data(), stream.layout(), pair).ok()
    }

    fn skeleton(&mut self, bones: &[(u32, u32)]) -> Option<crate::resource::rcb::Skeleton> {
        crate::resource::rcb::skeleton_for(self.rigs(), None, bones).cloned()
    }
}

/// Whether `dir` or a folder up to `depth` levels below it has a `.str` or `.toc` file.
/// The profile of the game installed in `dir`, when `dir` looks like a game
/// directory: the executable of a known game and a folder of game files
/// (`gf2/` with `.str` or `.toc` files). Quick: reads two directory listings.
pub fn looks_like_game(dir: &Path) -> Option<GameProfile> {
    let profile = GameProfile::detect(dir)?;
    let data = fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case("gf2"))?
        .path();
    has_game_files(&data, 0).then_some(profile)
}

fn has_game_files(dir: &Path, depth: usize) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    let mut subdirs = Vec::new();
    for entry in entries.filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if name.ends_with(".str") || name.ends_with(".toc") {
            return true;
        }
        if depth > 0 && entry.file_type().is_ok_and(|t| t.is_dir()) {
            subdirs.push(entry.path());
        }
    }
    subdirs.iter().any(|sub| has_game_files(sub, depth - 1))
}

/// What [`scan`] finds.
#[derive(Default)]
struct Found {
    streams: Vec<GameFile>,
    tocs: Vec<GameFile>,
    skipped: Vec<GameFile>,
}

/// What a file named `*.str` or `*.toc` holds, by its first four bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Stream,
    Toc,
    Other,
}

/// Whether `bytes` start with `magic` in either byte order.
fn starts_with_magic(bytes: &[u8], magic: u32) -> bool {
    bytes.len() >= 4 && (bytes[..4] == magic.to_le_bytes() || bytes[..4] == magic.to_be_bytes())
}

/// The kind of the file at `path` named `lower` (lower case): a stream or a
/// table of contents only when it starts with the format's magic. Other
/// programs use the same extensions: NVIDIA's shader cache writes `*.toc`
/// files that start with `CDVN`.
fn kind_of(path: &Path, lower: &str) -> std::io::Result<Kind> {
    use std::io::Read;
    let (kind, magic) = if lower.ends_with(".str") {
        (Kind::Stream, ustream::MAGIC)
    } else if lower.ends_with(".toc") {
        (Kind::Toc, toc::MAGIC)
    } else {
        return Ok(Kind::Other);
    };
    let mut head = [0u8; 4];
    let mut read = 0;
    let mut file = fs::File::open(path)?;
    while read < head.len() {
        match file.read(&mut head[read..])? {
            0 => break,
            n => read += n,
        }
    }
    Ok(if starts_with_magic(&head[..read], magic) {
        kind
    } else {
        Kind::Other
    })
}

/// Recursively collects stream and table-of-contents files below `dir`. A
/// file is taken by its name and its magic: a `.str` or `.toc` file with
/// other contents goes to `found.skipped`.
fn scan(dir: &Path, rel: &str, found: &mut Found) -> Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        // A folder the user cannot read holds no game files; this happens when
        // a directory above the game is chosen.
        Err(e) if !rel.is_empty() && e.kind() == std::io::ErrorKind::PermissionDenied => {
            return Ok(());
        }
        Err(e) => return Err(Error::io(dir, e)),
    };
    for entry in entries {
        let entry = entry.map_err(|e| Error::io(dir, e))?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let path = entry.path();
        let rel_path = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        let lower = name.to_ascii_lowercase();
        let game_file = lower.ends_with(".str") || lower.ends_with(".toc");
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            // A broken link or a socket of another program: not a game file.
            Err(_) if !game_file => continue,
            Err(e) => return Err(Error::io(&path, e)),
        };
        if metadata.is_dir() {
            if !SKIPPED_DIRS.iter().any(|d| d.eq_ignore_ascii_case(&name)) {
                scan(&path, &rel_path, found)?;
            }
            continue;
        }
        if !game_file {
            continue;
        }
        let kind = match kind_of(&path, &lower) {
            Ok(kind) => kind,
            // Held open without sharing by its program (a driver's cache
            // while the game runs): the game's own files are never locked so.
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Kind::Other,
            Err(e) => return Err(Error::io(&path, e)),
        };
        let file = GameFile {
            rel_path,
            path,
            len: metadata.len(),
        };
        match kind {
            Kind::Stream => found.streams.push(file),
            Kind::Toc => found.tocs.push(file),
            Kind::Other => found.skipped.push(file),
        }
    }
    Ok(())
}

/// Removes `dir` and its subdirectories if they contain no files.
fn remove_empty_dirs(dir: &Path) -> Result<()> {
    for entry in fs::read_dir(dir).map_err(|e| Error::io(dir, e))? {
        let path = entry.map_err(|e| Error::io(dir, e))?.path();
        if path.is_dir() {
            remove_empty_dirs(&path)?;
        }
    }
    // Fails harmlessly when the directory still holds files.
    let _ = fs::remove_dir(dir);
    Ok(())
}

/// Recursively collects the relative paths of all files below `dir`.
fn collect_files(dir: &Path, rel: &str, out: &mut Vec<String>) -> Result<()> {
    for entry in fs::read_dir(dir).map_err(|e| Error::io(dir, e))? {
        let entry = entry.map_err(|e| Error::io(dir, e))?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let path = entry.path();
        let rel_path = if rel.is_empty() {
            name
        } else {
            format!("{rel}/{name}")
        };
        if path.is_dir() {
            collect_files(&path, &rel_path, out)?;
        } else {
            out.push(rel_path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::container::ustream::{FileHeader, Storage, StreamWriter};
    use crate::hash::sdbm;
    use crate::io::Endian;

    fn resource_header(tag: &[u8], id: u32) -> ResourceHeader {
        ResourceHeader {
            mem_policy: sdbm(b"default"),
            alignment: 0x10,
            type_id: sdbm(tag),
            ids: [id; 4],
            data_size: 0,
        }
    }

    fn stream_bytes(resources: &[(&[u8], u32, Vec<u8>)]) -> Vec<u8> {
        let mut writer = StreamWriter::new(&FileHeader::GF2_PC, Endian::Little);
        for (tag, id, data) in resources {
            writer
                .push_resource(&resource_header(tag, *id), data, Storage::Raw)
                .unwrap();
        }
        writer.finish()
    }

    /// Builds a miniature game directory with two streams and twin tables of contents.
    fn fake_game() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("gf2/Text")).unwrap();
        fs::create_dir_all(root.join("_toolkit/backups/gf2")).unwrap();
        fs::write(root.join("godfather2.exe"), b"MZ").unwrap();
        fs::write(
            root.join("gf2/frontend.str"),
            stream_bytes(&[(b"tg4h", 1, vec![1; 100]), (b"tg4d", 1, vec![2; 5000])]),
        )
        .unwrap();
        fs::write(
            root.join("gf2/Text/ABCD.STR"),
            stream_bytes(&[(b"lh2", 2, vec![3; 40])]),
        )
        .unwrap();
        // A stream in the old toolkit's backup directory must be ignored.
        fs::write(
            root.join("_toolkit/backups/gf2/frontend.str"),
            stream_bytes(&[]),
        )
        .unwrap();

        let table = Toc::parse(&sample_toc_bytes()).unwrap();
        fs::write(root.join("gf2/frontend.toc"), table.to_bytes().unwrap()).unwrap();
        fs::write(
            root.join("gf2/frontend_global.toc"),
            table.to_bytes().unwrap(),
        )
        .unwrap();
        dir
    }

    /// A table listing `gf2\frontend` (5100 bytes) and `gf2\text\abcd` (40 bytes).
    fn sample_toc_bytes() -> Vec<u8> {
        let mut out = Vec::new();
        let names: [(&[u8], u32); 2] = [(b"gf2\\frontend", 0x11), (b"gf2\\text\\abcd", 0x22)];
        let names_start = 0x10 + 2 * 20;
        let deps_start = names_start + 2 * 8;
        let pool_start = deps_start + 4;
        out.extend_from_slice(&toc::MAGIC.to_le_bytes());
        out.extend_from_slice(&10u32.to_le_bytes());
        for value in [2u16, 2, 1, 0] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        // frontend: no dependencies; text: depends on frontend.
        for (i, (size, flags, deps)) in [
            (5100u32, [0u8; 4], 0u32),
            (40, [0, 0, 1, 0], deps_start as u32),
        ]
        .into_iter()
        .enumerate()
        {
            out.extend_from_slice(&((names_start + i * 8) as u32).to_le_bytes());
            out.extend_from_slice(&size.to_le_bytes());
            out.extend_from_slice(&flags);
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&deps.to_le_bytes());
        }
        let mut string_offset = pool_start + 1;
        for (name, hash) in names {
            out.extend_from_slice(&hash.to_le_bytes());
            out.extend_from_slice(&(string_offset as u32).to_le_bytes());
            string_offset += name.len() + 1;
        }
        out.extend_from_slice(&(names_start as u32).to_le_bytes());
        out.push(0);
        for (name, _) in names {
            out.extend_from_slice(name);
            out.push(0);
        }
        out.resize(toc::PADDING, 0);
        out
    }

    #[test]
    fn scans_streams_and_tocs() {
        let dir = fake_game();
        let game = GameInstance::open(dir.path()).unwrap();
        assert_eq!(game.profile().game, crate::profile::Game::Godfather2);
        let names: Vec<_> = game.streams().iter().map(|s| s.rel_path.as_str()).collect();
        assert_eq!(names, ["gf2/Text/ABCD.STR", "gf2/frontend.str"]);
        assert_eq!(game.tocs().len(), 2);
        assert_eq!(game.find_stream("gf2\\frontend"), Some(1));
        assert_eq!(game.find_stream("GF2/TEXT/abcd.str"), Some(0));
        assert_eq!(game.find_stream("nope"), None);
        assert!(GameInstance::open(dir.path().join("missing")).is_err());
        // A directory without game files, e.g. the parent of the game folder.
        let empty = tempfile::tempdir().unwrap();
        std::fs::create_dir(empty.path().join("docs")).unwrap();
        assert!(matches!(
            GameInstance::open(empty.path()),
            Err(Error::NotAGame { .. })
        ));
    }

    #[test]
    fn builds_index_and_summary() {
        let dir = fake_game();
        let game = GameInstance::open(dir.path()).unwrap();
        let seen = std::sync::atomic::AtomicUsize::new(0);
        let (index, errors) = game.build_index(|_| {
            seen.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        });
        assert!(errors.is_empty());
        assert_eq!(seen.into_inner(), 2);
        assert_eq!(index.entries.len(), 3);
        assert_eq!(index.entries[0].header.type_id, sdbm(b"lh2"));
        let summary = index.type_summary();
        assert_eq!(summary.len(), 3);
        let tg4d = summary.iter().find(|s| s.type_id == sdbm(b"tg4d")).unwrap();
        assert_eq!((tg4d.count, tg4d.data_bytes), (1, 5000));
    }

    #[test]
    fn index_reports_broken_streams() {
        let dir = fake_game();
        fs::write(dir.path().join("gf2/broken.str"), b"3slo and then nonsense").unwrap();
        fs::write(dir.path().join("gf2/notes.str"), b"not a stream").unwrap();
        let game = GameInstance::open(dir.path()).unwrap();
        let (index, errors) = game.build_index(|_| {});
        assert_eq!(index.entries.len(), 3);
        assert_eq!(errors.len(), 1);
        assert_eq!(game.streams()[errors[0].0].rel_path, "gf2/broken.str");
        // Without the magic it is not taken for a stream at all.
        let skipped: Vec<_> = game.skipped().iter().map(|f| &f.rel_path).collect();
        assert_eq!(skipped, ["gf2/notes.str"]);
    }

    /// NVIDIA's shader cache next to the executable: `GLCache/<hash>/<hash>/*.toc`
    /// starting with `CDVN`. Neither the folder nor such a file elsewhere is a
    /// table of contents.
    #[test]
    fn foreign_toc_files_are_skipped() {
        let dir = fake_game();
        let root = dir.path();
        let cache = root.join("GLCache/a5006eb268eeda1b/1271864ec7dbc5c8");
        fs::create_dir_all(&cache).unwrap();
        let cdvn = [&b"CDVN"[..], &[0, 0, 3, 0], &[0xC8; 24]].concat();
        fs::write(cache.join("483ab422c6120620.toc"), &cdvn).unwrap();
        fs::write(cache.join("483ab422c6120620.bin"), [0u8; 32]).unwrap();
        fs::create_dir_all(root.join("Cache")).unwrap();
        fs::write(root.join("Cache/x.toc"), &cdvn).unwrap();
        fs::write(root.join("gf2/empty.toc"), b"").unwrap();
        let game = GameInstance::open(root).unwrap();
        let tocs: Vec<_> = game.tocs().iter().map(|f| f.rel_path.as_str()).collect();
        assert_eq!(tocs, ["gf2/frontend.toc", "gf2/frontend_global.toc"]);
        let skipped: Vec<_> = game.skipped().iter().map(|f| f.rel_path.as_str()).collect();
        assert_eq!(skipped, ["Cache/x.toc", "gf2/empty.toc"]);
        assert_eq!(game.toc_hashes().unwrap().len(), 2);
        assert!(game.stream_parents().is_ok());
        let (_, errors) = game.build_index(|_| {});
        assert!(errors.is_empty());
    }

    #[test]
    fn replace_backs_up_updates_toc_and_restores() {
        let dir = fake_game();
        let root = dir.path();
        let game = GameInstance::open(root).unwrap();
        let stream_index = game.find_stream("gf2/frontend").unwrap();
        let original_stream = fs::read(root.join("gf2/frontend.str")).unwrap();
        let original_toc = fs::read(root.join("gf2/frontend.toc")).unwrap();

        let new_data = vec![9u8; 200_000];
        let replacement = Replacement {
            index: 1,
            data: &new_data,
            storage: Storage::RefPack,
        };
        let report = game
            .replace_resources(
                stream_index,
                std::slice::from_ref(&replacement),
                ReplaceOptions::default(),
            )
            .unwrap();
        assert_eq!(report.old_data_size, 5100);
        assert_eq!(report.new_data_size, 200_100);
        assert_eq!(
            report.written,
            [
                "gf2/frontend.str",
                "gf2/frontend.toc",
                "gf2/frontend_global.toc"
            ]
        );
        assert_eq!(report.backed_up, report.written);

        let stream = game.open_stream(stream_index).unwrap();
        assert_eq!(stream.resource_data(0).unwrap(), vec![1; 100]);
        assert_eq!(stream.resource_data(1).unwrap(), new_data);
        drop(stream);
        for toc_index in 0..2 {
            let table = game.load_toc(toc_index).unwrap();
            assert_eq!(
                table.files[table.find_file("gf2/frontend").unwrap()].data_size,
                200_100
            );
            assert_eq!(
                table.files[table.find_file("gf2/text/abcd").unwrap()].data_size,
                40
            );
        }
        assert!(game.has_backup(&game.streams()[stream_index]));

        // A second replacement keeps the first backup: it holds the true original.
        let report = game
            .replace_resources(
                stream_index,
                &[replacement],
                ReplaceOptions { update_toc: false },
            )
            .unwrap();
        assert!(report.backed_up.is_empty());
        assert_eq!(report.written, ["gf2/frontend.str"]);
        assert_eq!(
            fs::read(root.join(BACKUP_DIR).join("gf2/frontend.str")).unwrap(),
            original_stream
        );

        let restored = game.restore_backups().unwrap();
        assert_eq!(restored.len(), 3);
        assert_eq!(
            fs::read(root.join("gf2/frontend.str")).unwrap(),
            original_stream
        );
        assert_eq!(
            fs::read(root.join("gf2/frontend.toc")).unwrap(),
            original_toc
        );
        assert!(game.restore_backups().unwrap().is_empty());
        assert!(
            !root.join("_doctor_toolkit").exists(),
            "backup directories are removed"
        );
    }

    #[test]
    fn replace_without_size_change_leaves_toc_alone() {
        let dir = fake_game();
        let game = GameInstance::open(dir.path()).unwrap();
        let stream_index = game.find_stream("gf2/frontend").unwrap();
        let new_data = vec![7u8; 5000];
        let report = game
            .replace_resources(
                stream_index,
                &[Replacement {
                    index: 1,
                    data: &new_data,
                    storage: Storage::Raw,
                }],
                ReplaceOptions::default(),
            )
            .unwrap();
        assert_eq!(report.written, ["gf2/frontend.str"]);
        assert_eq!(report.new_data_size, report.old_data_size);
    }

    /// A miniature game with one stream holding a 4×4 DXT1 texture and a string table.
    fn editable_game() -> (tempfile::TempDir, GameInstance) {
        use crate::resource::lh2::StringTable;
        use crate::resource::tg4::{PARAM_DATA_SIZE, PARAM_FORMAT, ParamValue, Tg4Header};

        let mut header = Tg4Header {
            version: 3,
            budget_hash: 0,
            package: "pkg".into(),
            name_hash: 0x55,
            name: "wall".into(),
            data_size: 8,
            width: 4,
            height: 4,
            padding: 0,
            surface_count: 1,
            params: Vec::new(),
        };
        header.set_param(PARAM_FORMAT, ParamValue::Str("DXT1c".into()));
        header.set_param(PARAM_DATA_SIZE, ParamValue::Int(8));
        let table = StringTable {
            version: 1,
            flags: 0x4000,
            languages: Vec::new(),
            hashes: vec![1, 2],
            strings: vec![vec!["Hello".into(), "World".into()]],
        };
        let mut writer = StreamWriter::new(&FileHeader::GF2_PC, Endian::Little);
        let mut tex_header = resource_header(b"tg4h", 0x55);
        tex_header.ids = [0, sdbm(b"tg4h"), 7, 0x55];
        writer
            .push_resource(&tex_header, &header.to_bytes().unwrap(), Storage::Raw)
            .unwrap();
        let mut tex_data = resource_header(b"tg4d", 0x55);
        tex_data.ids = [0, sdbm(b"tg4d"), 7, 0x55];
        writer
            .push_resource(&tex_data, &[0u8; 8], Storage::RefPack)
            .unwrap();
        writer
            .push_resource(
                &resource_header(b"lh2", 9),
                &table.to_bytes().unwrap(),
                Storage::Raw,
            )
            .unwrap();

        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("gf2")).unwrap();
        fs::write(dir.path().join("gf2/level.str"), writer.finish()).unwrap();
        let game = GameInstance::open(dir.path()).unwrap();
        (dir, game)
    }

    #[test]
    fn import_texture_from_png_and_dds() {
        use crate::resource::{dds, pixels, texture};
        let (_dir, game) = editable_game();
        let stream = game.find_stream("gf2/level").unwrap();
        let pair = texture::TexturePair { header: 0, data: 1 };

        let rgba: Vec<u8> = (0..16 * 8)
            .flat_map(|i| [i as u8, 0, 255 - i as u8, 255])
            .collect();
        let png = pixels::encode_png(16, 8, &rgba).unwrap();
        let (texture, report) = game.import_texture(stream, pair, &png, true).unwrap();
        assert_eq!(
            (
                texture.header.width,
                texture.header.height,
                texture.header.mip_count()
            ),
            (16, 8, 5)
        );
        assert_eq!(report.written, ["gf2/level.str"]);

        let file = game.open_stream(stream).unwrap();
        let loaded = texture::Texture::load(file.data(), file.layout(), pair).unwrap();
        assert_eq!(loaded.header, texture.header);
        assert_eq!(loaded.data, texture.data);
        assert!(
            file.layout().is_compressed(1),
            "data keeps its RefPack storage"
        );
        assert_eq!(
            file.resource_data(2).unwrap().len(),
            file.layout().resources[2].header.data_size as usize
        );
        drop(file);

        let dds_file = dds::write(&texture.header, &texture.data).unwrap();
        let (again, _) = game.import_texture(stream, pair, &dds_file, false).unwrap();
        assert_eq!(again.data, texture.data);
        assert!(game.import_texture(stream, pair, b"garbage", true).is_err());
    }

    #[test]
    fn import_string_table_keeps_other_resources() {
        use crate::resource::lh2::StringTable;
        let (_dir, game) = editable_game();
        let stream = game.find_stream("gf2/level").unwrap();
        let file = game.open_stream(stream).unwrap();
        let mut table = StringTable::parse(&file.resource_data(2).unwrap()).unwrap();
        let texture_before = file.resource_data(1).unwrap();
        drop(file);
        table.strings[0][1] = "Мир".into();
        game.import_string_table(stream, 2, &table).unwrap();
        let file = game.open_stream(stream).unwrap();
        assert_eq!(
            StringTable::parse(&file.resource_data(2).unwrap()).unwrap(),
            table
        );
        assert_eq!(file.resource_data(1).unwrap(), texture_before);
        drop(file);
        assert_eq!(game.restore_backups().unwrap(), ["gf2/level.str"]);
    }
}

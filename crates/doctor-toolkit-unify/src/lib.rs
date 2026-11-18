//! The unified game: one game folder, the same for everybody, made from
//! whichever disc edition a player owns.
//!
//! The retail discs are two builds of the same game: PAL (27.01.2009,
//! 1.0.764.0: speech en, ru; text en, es, ru, cs, nl, pl) and NTSC
//! (19.02.2009, 1.0.766.0: speech en, es; text en, es). The unified game is
//! the NTSC data — the reference of doctor_toolkit — with the executable
//! 1.0.789.0 and every language of both discs (`tools/unified-build`).
//!
//! A **pack** describes that game and carries what a disc lacks:
//!
//! ```text
//! unified.json            the manifest: every file of the unified game with
//!                         its size and SHA-256, and where each edition has it
//! common/…                files neither disc has (the executable, patched streams)
//! for-ntsc/…              files the NTSC disc lacks (Russian speech, movies)
//! for-pal/…               files the PAL disc lacks (the NTSC streams, Spanish speech)
//! ```
//!
//! [`build_pack`] makes a pack from the three trees; [`apply`] makes the
//! unified game from a pack and a **source**: the folder of a disc (a
//! mounted image: cabinets and plain files) or an installed copy, of either
//! edition. Whatever is taken from the source is checked against the
//! manifest, so a changed copy cannot leak into the result; the sound
//! archives are rebuilt entry by entry and come out byte-identical.

pub mod big;
pub mod cab;
pub mod locale;
pub mod support;

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::big::BigArchive;
use crate::support::atomic;
pub use crate::support::{Error, Result};

/// File name of the manifest inside a pack.
pub const MANIFEST: &str = "unified.json";
pub const FORMAT: &str = "doctor-toolkit-unified";
pub const VERSION: u32 = 1;

/// Pack part with files no edition has.
pub const PART_COMMON: &str = "common";
/// Files up to this size are in the pack even when every edition has them:
/// icons and settings that installers and repacks tend to drop.
pub const ALWAYS_PACKED: u64 = 64 << 10;

/// A file of the unified game.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// Path from the game folder, with `/`.
    pub path: String,
    pub size: u64,
    pub sha256: String,
    /// Edition id → path of an identical file in an installed copy of it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub from: BTreeMap<String, String>,
    /// Pack part that holds the file for the editions that lack it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<String>,
}

/// A sound archive of the unified game.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveEntry {
    pub path: String,
    pub size: u64,
    pub sha256: String,
    /// Entries in archive order: name and size.
    pub entries: Vec<(String, u64)>,
    /// Edition id → pack archive (path from the pack) with the entries the
    /// archives of that edition lack. Absent: the edition has them all.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub lacking: BTreeMap<String, String>,
}

/// A disc edition a unified game can be made from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditionSource {
    /// `ntsc` or `pal`.
    pub id: String,
    pub name: String,
    /// Third line of `text/localetable.txt`: `ntsc_en`, `pal_en`.
    pub tag: String,
    /// Pack part with what this edition lacks.
    pub part: String,
    /// Installed path → cabinet of the disc and the name inside it, for the
    /// files the disc keeps in cabinets. Other files lie on the disc under
    /// their installed path.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub disc: BTreeMap<String, (String, String)>,
}

/// The description of a unified game.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: String,
    pub version: u32,
    /// For people, e.g. `1.0.789.0, en es ru cs nl pl`.
    pub name: String,
    pub editions: Vec<EditionSource>,
    pub files: Vec<FileEntry>,
    pub archives: Vec<ArchiveEntry>,
}

impl Manifest {
    pub fn load(pack: &Path) -> Result<Self> {
        let path = pack.join(MANIFEST);
        let text = fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
        let manifest: Manifest = serde_json::from_str(&text)
            .map_err(|e| Error::InvalidArgument(format!("{}: {e}", path.display())))?;
        if manifest.format != FORMAT || manifest.version != VERSION {
            return Err(Error::InvalidArgument(format!(
                "{} is not a unified game pack of format {VERSION}",
                path.display()
            )));
        }
        Ok(manifest)
    }

    pub fn edition(&self, id: &str) -> Option<&EditionSource> {
        self.editions.iter().find(|e| e.id == id)
    }

    /// Bytes of the unified game.
    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum::<u64>()
            + self.archives.iter().map(|a| a.size).sum::<u64>()
    }
}

/// A pack published with this program.
#[derive(Debug, PartialEq, Eq)]
pub struct KnownPack {
    /// Number of the pack release.
    pub release: u32,
    /// SHA-256 of its `unified.json`. The manifest names the hash of every
    /// file of the game, so it vouches for the whole pack.
    pub manifest_sha256: &'static str,
    /// Where it is downloaded from.
    pub magnet: &'static str,
}

/// The published packs, oldest first.
pub const KNOWN_PACKS: &[KnownPack] = &[KnownPack {
    release: 1,
    manifest_sha256: "0870136baa8dc473c7a99ce9ae8074e7e314da977a98114baf31f724c3a9517e",
    magnet: "magnet:?xt=urn:btih:91a01d0d021e6f84dd7932ab8f6c744ef1afb677&dn=pack\
             &tr=udp%3A%2F%2Ftracker.opentrackr.org%3A1337%2Fannounce\
             &tr=udp%3A%2F%2Fopen.stealth.si%3A80%2Fannounce",
}];

/// The newest published pack.
pub fn latest_pack() -> &'static KnownPack {
    KNOWN_PACKS.last().expect("a pack is published")
}

/// The published pack this folder holds; `None` for a pack somebody else
/// made (its files, the executable among them, are not vouched for).
pub fn known_pack(pack: &Path) -> Result<Option<&'static KnownPack>> {
    let sha256 = file_sha256(&pack.join(MANIFEST))?;
    Ok(KNOWN_PACKS.iter().find(|k| k.manifest_sha256 == sha256))
}

/// Pack files a copy of this edition cannot do without that are absent or
/// of another size (a download with that part left out, or not finished):
/// paths from the pack folder.
pub fn pack_gaps(manifest: &Manifest, pack: &Path, edition: &EditionSource) -> Vec<String> {
    let parts: Vec<&str> = std::iter::once(PART_COMMON)
        .chain(manifest.editions.iter().map(|e| e.part.as_str()))
        .collect();
    let mut gaps = Vec::new();
    for file in &manifest.files {
        if file.from.contains_key(&edition.id) {
            continue;
        }
        let found = file
            .part
            .as_deref()
            .into_iter()
            .chain(parts.iter().copied())
            .any(|part| {
                fs::metadata(pack.join(part).join(&file.path))
                    .is_ok_and(|m| m.is_file() && m.len() == file.size)
            });
        if !found {
            let part = file.part.as_deref().unwrap_or(&edition.part);
            gaps.push(format!("{part}/{}", file.path));
        }
    }
    for archive in &manifest.archives {
        if let Some(rel) = archive.lacking.get(&edition.id)
            && !pack.join(rel).is_file()
        {
            gaps.push(rel.clone());
        }
    }
    gaps
}

fn is_archive(path: &str) -> bool {
    big::archive_number(Path::new(path)).is_some()
}

/// SHA-256 of a file, in hexadecimal.
pub fn file_sha256(path: &Path) -> Result<String> {
    let mut file = File::open(path).map_err(|e| Error::io(path, e))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).map_err(|e| Error::io(path, e))?;
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Files of a tree as paths with `/`, sorted; the toolkit's own folder and
/// temporary files are left out. Symbolic links are followed.
fn tree_files(root: &Path) -> Result<Vec<String>> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
        for entry in fs::read_dir(dir).map_err(|e| Error::io(dir, e))? {
            let path = entry.map_err(|e| Error::io(dir, e))?.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == "_doctor_toolkit" || name.ends_with(atomic::TEMP_SUFFIX) {
                continue;
            }
            if path.is_dir() {
                walk(root, &path, out)?;
            } else if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(root, root, &mut out)?;
    out.sort();
    Ok(out)
}

/// `root/rel`, matching the names of `rel` without regard to case when the
/// exact path does not exist (copies made on Windows, discs).
pub fn resolve(root: &Path, rel: &str) -> Option<PathBuf> {
    let exact = root.join(rel);
    if exact.exists() {
        return Some(exact);
    }
    let mut at = root.to_path_buf();
    for part in rel.split('/') {
        let next = at.join(part);
        if next.exists() {
            at = next;
            continue;
        }
        at = fs::read_dir(&at)
            .ok()?
            .filter_map(|e| e.ok())
            .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(part))?
            .path();
    }
    Some(at)
}

// ---------------------------------------------------------------- build

/// One edition for [`build_pack`]: a clean installed tree of it.
#[derive(Debug, Clone)]
pub struct EditionTree {
    pub id: String,
    pub name: String,
    pub part: String,
    pub tree: PathBuf,
    /// Lines `installed path <TAB> cabinet or - <TAB> name in the cabinet
    /// <TAB> size` (`tools/unified-build/disc_map.py`).
    pub disc_map: Option<PathBuf>,
}

fn locale_tag(table: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(table);
    text.lines().nth(2).map(|l| l.trim().to_string())
}

/// Path → (size, SHA-256) of the files of a tree, hashed in parallel.
fn hash_tree(root: &Path, mut note: impl FnMut(&str)) -> Result<BTreeMap<String, (u64, String)>> {
    use rayon::prelude::*;
    let files = tree_files(root)?;
    note(&format!(
        "hashing {} file(s) of {}",
        files.len(),
        root.display()
    ));
    files
        .par_iter()
        .map(|rel| {
            let path = root.join(rel);
            let size = fs::metadata(&path).map_err(|e| Error::io(&path, e))?.len();
            Ok((rel.clone(), (size, file_sha256(&path)?)))
        })
        .collect()
}

fn copy_into_pack(from: &Path, pack: &Path, part: &str, rel: &str) -> Result<()> {
    let to = pack.join(part).join(rel);
    if let Some(dir) = to.parent() {
        fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    }
    atomic::copy(from, &to)
}

/// Name → (archive index, entry index) over the archives of a game folder,
/// the first archive that has a name winning, as in the game.
struct ArchiveSet {
    archives: Vec<BigArchive>,
    by_name: HashMap<String, (usize, usize)>,
}

impl ArchiveSet {
    fn open(paths: &[PathBuf]) -> Result<Self> {
        let mut set = Self {
            archives: Vec::new(),
            by_name: HashMap::new(),
        };
        for path in paths {
            let archive = BigArchive::open(path)?;
            let index = set.archives.len();
            for (i, entry) in archive.entries.iter().enumerate() {
                set.by_name
                    .entry(big::normalize_name(&entry.name))
                    .or_insert((index, i));
            }
            set.archives.push(archive);
        }
        Ok(set)
    }

    fn find(&self, name: &str, size: u64) -> Option<(usize, u64)> {
        let &(archive, entry) = self.by_name.get(&big::normalize_name(name))?;
        let entry = &self.archives[archive].entries[entry];
        (entry.size == size).then_some((archive, entry.offset))
    }
}

/// Writes a `BIGF` archive with the given entries, taking the data of each
/// from `fetch(name, size, out)`; returns the size and SHA-256.
fn write_archive(
    path: &Path,
    entries: &[(String, u64)],
    mut fetch: impl FnMut(&str, u64, &mut dyn Write) -> Result<()>,
) -> Result<(u64, String)> {
    let sizes: Vec<(&str, u64)> = entries.iter().map(|(n, s)| (n.as_str(), *s)).collect();
    let (header, offsets) = big::archive_header(&sizes)?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    }
    let temp = atomic::temp_path(path)?;
    let result = (|| {
        let file = File::create(&temp).map_err(|e| Error::io(&temp, e))?;
        let mut out = HashWriter::new(BufWriter::with_capacity(1 << 20, file));
        out.write_all(&header).map_err(|e| Error::io(&temp, e))?;
        for ((name, size), &offset) in entries.iter().zip(&offsets) {
            let gap = offset - out.written;
            std::io::copy(&mut std::io::repeat(0).take(gap), &mut out)
                .map_err(|e| Error::io(&temp, e))?;
            fetch(name, *size, &mut out)?;
            if out.written != offset + size {
                return Err(Error::InvalidArgument(format!(
                    "archive entry {name}: {} byte(s) instead of {size}",
                    out.written - offset
                )));
            }
        }
        let (mut inner, size, sha256) = out.finish();
        inner.flush().map_err(|e| Error::io(&temp, e))?;
        Ok((size, sha256))
    })();
    match result {
        Ok(done) => {
            fs::rename(&temp, path).map_err(|e| Error::io(path, e))?;
            Ok(done)
        }
        Err(error) => {
            let _ = fs::remove_file(&temp);
            Err(error)
        }
    }
}

/// A writer that counts and hashes what passes through it.
struct HashWriter<W> {
    inner: W,
    hasher: Sha256,
    written: u64,
}

impl<W: Write> HashWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
            written: 0,
        }
    }

    fn finish(self) -> (W, u64, String) {
        (self.inner, self.written, hex(&self.hasher.finalize()))
    }
}

impl<W: Write> Write for HashWriter<W> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(data)?;
        self.hasher.update(&data[..n]);
        self.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn copy_range(
    file: &mut File,
    path: &Path,
    offset: u64,
    size: u64,
    out: &mut dyn Write,
) -> Result<()> {
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| Error::io(path, e))?;
    let copied = std::io::copy(&mut file.take(size), out).map_err(|e| Error::io(path, e))?;
    if copied != size {
        return Err(Error::io(
            path,
            std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "archive entry cut short"),
        ));
    }
    Ok(())
}

/// Makes a pack in `pack` from the unified tree and a clean installed tree
/// of every edition. `note` receives progress lines.
pub fn build_pack(
    name: &str,
    unified: &Path,
    editions: &[EditionTree],
    pack: &Path,
    mut note: impl FnMut(&str),
) -> Result<Manifest> {
    fs::create_dir_all(pack).map_err(|e| Error::io(pack, e))?;
    let target = hash_tree(unified, &mut note)?;
    // Per edition: its files by hash, and its archives.
    let mut by_hash: Vec<HashMap<String, String>> = Vec::new();
    let mut hashes: Vec<BTreeMap<String, (u64, String)>> = Vec::new();
    let mut sources = Vec::new();
    for edition in editions {
        let tree = hash_tree(&edition.tree, &mut note)?;
        let mut map = HashMap::new();
        for (path, (_, sha)) in &tree {
            // The same path as in the unified game is preferred below.
            map.entry(sha.clone()).or_insert_with(|| path.clone());
        }
        let table = edition.tree.join("text/localetable.txt");
        let tag = fs::read(&table)
            .ok()
            .and_then(|t| locale_tag(&t))
            .ok_or_else(|| {
                Error::InvalidArgument(format!("{} has no build tag", table.display()))
            })?;
        let mut disc = BTreeMap::new();
        if let Some(map_path) = &edition.disc_map {
            let text = fs::read_to_string(map_path).map_err(|e| Error::io(map_path, e))?;
            for line in text.lines() {
                let mut cells = line.split('\t');
                if let (Some(path), Some(cabinet), Some(inner)) =
                    (cells.next(), cells.next(), cells.next())
                    && cabinet != "-"
                    && !cabinet.starts_with('#')
                {
                    disc.insert(path.to_string(), (cabinet.to_string(), inner.to_string()));
                }
            }
        }
        sources.push(EditionSource {
            id: edition.id.clone(),
            name: edition.name.clone(),
            tag,
            part: edition.part.clone(),
            disc,
        });
        by_hash.push(map);
        hashes.push(tree);
    }

    let mut files = Vec::new();
    let mut archives = Vec::new();
    for (path, (size, sha256)) in &target {
        if is_archive(path) {
            continue;
        }
        let mut from = BTreeMap::new();
        let mut lacking = Vec::new();
        for (i, edition) in editions.iter().enumerate() {
            let same_path = hashes[i].get(path).is_some_and(|(_, sha)| sha == sha256);
            match (same_path, by_hash[i].get(sha256)) {
                (true, _) => drop(from.insert(edition.id.clone(), path.clone())),
                (false, Some(other)) => drop(from.insert(edition.id.clone(), other.clone())),
                (false, None) => lacking.push(i),
            }
        }
        // One edition lacks it: the part of that edition; more: the common part.
        let part = match lacking.as_slice() {
            [] if *size <= ALWAYS_PACKED => Some(PART_COMMON.to_string()),
            [] => None,
            [one] if editions.len() > 1 => Some(editions[*one].part.clone()),
            _ => Some(PART_COMMON.to_string()),
        };
        if let Some(part) = &part {
            copy_into_pack(&unified.join(path), pack, part, path)?;
        }
        files.push(FileEntry {
            path: path.clone(),
            size: *size,
            sha256: sha256.clone(),
            from,
            part,
        });
    }
    // Used cabinet entries only.
    for (i, source) in sources.iter_mut().enumerate() {
        let used: std::collections::HashSet<&String> = files
            .iter()
            .filter_map(|f| f.from.get(&editions[i].id))
            .collect();
        source.disc.retain(|path, _| used.contains(path));
    }

    for (path, (size, sha256)) in target.iter().filter(|(p, _)| is_archive(p)) {
        note(&format!("archive {path}"));
        let archive = BigArchive::open(unified.join(path))?;
        let entries: Vec<(String, u64)> = archive
            .entries
            .iter()
            .map(|e| (e.name.clone(), e.size))
            .collect();
        // The manifest promises the layout of `write_archive`.
        let sizes: Vec<(&str, u64)> = entries.iter().map(|(n, s)| (n.as_str(), *s)).collect();
        let (_, offsets) = big::archive_header(&sizes)?;
        if archive
            .entries
            .iter()
            .zip(&offsets)
            .any(|(e, &o)| e.offset != o)
        {
            return Err(Error::InvalidArgument(format!(
                "{path} is not laid out like the archives of the game"
            )));
        }
        let mut lacking = BTreeMap::new();
        for edition in editions {
            let set = ArchiveSet::open(&big::find_archives(&edition.tree)?)?;
            let missing: Vec<(String, u64)> = entries
                .iter()
                .filter(|(name, size)| set.find(name, *size).is_none())
                .cloned()
                .collect();
            if missing.is_empty() {
                continue;
            }
            let rel = format!("{}/{path}", edition.part);
            let source_path = unified.join(path);
            let mut source = File::open(&source_path).map_err(|e| Error::io(&source_path, e))?;
            let offsets: HashMap<&str, u64> = archive
                .entries
                .iter()
                .map(|e| (e.name.as_str(), e.offset))
                .collect();
            write_archive(&pack.join(&rel), &missing, |name, size, out| {
                copy_range(&mut source, &source_path, offsets[name], size, out)
            })?;
            lacking.insert(edition.id.clone(), rel);
        }
        archives.push(ArchiveEntry {
            path: path.clone(),
            size: *size,
            sha256: sha256.clone(),
            entries,
            lacking,
        });
    }

    let manifest = Manifest {
        format: FORMAT.into(),
        version: VERSION,
        name: name.to_string(),
        editions: sources,
        files,
        archives,
    };
    let json = serde_json::to_vec(&manifest).map_err(|e| Error::InvalidArgument(e.to_string()))?;
    atomic::write(&pack.join(MANIFEST), &json)?;
    Ok(manifest)
}

// ---------------------------------------------------------------- apply

/// Where the files of an edition come from.
#[derive(Debug)]
pub struct Source {
    pub root: PathBuf,
    /// Index into [`Manifest::editions`].
    pub edition: usize,
    /// The folder of a disc (cabinets) rather than an installed copy.
    pub disc: bool,
}

impl Source {
    /// Looks at a folder: an installed copy has `text/localetable.txt`, a
    /// disc has it in `data_t14.cab`; its third line names the build.
    pub fn open(manifest: &Manifest, root: &Path) -> Result<Self> {
        let (table, disc) = if let Some(path) = resolve(root, "text/localetable.txt") {
            (fs::read(&path).map_err(|e| Error::io(&path, e))?, false)
        } else if let Some(path) = resolve(root, "data_t14.cab") {
            (cab::read_file(&path, "textLocaletable_txt")?, true)
        } else {
            return Err(Error::InvalidArgument(format!(
                "{} is neither a disc of the game (no data_t14.cab) nor an installed copy \
                 (no text/localetable.txt)",
                root.display()
            )));
        };
        let tag = locale_tag(&table).unwrap_or_default();
        let edition = manifest
            .editions
            .iter()
            .position(|e| e.tag == tag)
            .ok_or_else(|| {
                Error::InvalidArgument(format!(
                    "{}: the build `{tag}` is not one the pack knows ({})",
                    root.display(),
                    manifest
                        .editions
                        .iter()
                        .map(|e| e.tag.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })?;
        Ok(Self {
            root: root.to_path_buf(),
            edition,
            disc,
        })
    }
}

/// What [`apply`] did.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    /// Files already in place with the right contents.
    pub kept: usize,
    pub from_source: usize,
    pub from_pack: usize,
    pub archives_built: usize,
    /// Files of the source that differ from the disc and were taken from the pack.
    pub changed_in_source: Vec<String>,
}

/// Progress of [`apply`]: bytes done of the total, and what is being written.
pub type Progress<'a> = &'a mut dyn FnMut(u64, u64, &str);

fn up_to_date(path: &Path, size: u64, sha256: &str) -> bool {
    fs::metadata(path).is_ok_and(|m| m.len() == size)
        && file_sha256(path).is_ok_and(|s| s == sha256)
}

/// Copies `from` to `to` and returns the SHA-256 of what was written; the
/// file appears under its name only when the hash is the expected one.
fn copy_checked(from: &mut dyn Read, to: &Path, sha256: &str) -> Result<bool> {
    if let Some(dir) = to.parent() {
        fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    }
    let temp = atomic::temp_path(to)?;
    let file = File::create(&temp).map_err(|e| Error::io(&temp, e))?;
    let mut out = HashWriter::new(BufWriter::with_capacity(1 << 20, file));
    let copied = std::io::copy(from, &mut out).map_err(|e| Error::io(&temp, e));
    let (mut inner, _, found) = out.finish();
    let flushed = inner.flush().map_err(|e| Error::io(&temp, e));
    drop(inner);
    if copied.is_ok() && flushed.is_ok() && found == sha256 {
        fs::rename(&temp, to).map_err(|e| Error::io(to, e))?;
        return Ok(true);
    }
    let _ = fs::remove_file(&temp);
    copied?;
    flushed?;
    Ok(false)
}

/// Makes the unified game in `out` from the pack and a source folder.
///
/// Files already in `out` with the right contents are kept, so an
/// interrupted run continues. A file of the source that differs from the
/// disc is taken from the pack when the pack has it, and is an error
/// otherwise. `out` must not be the source.
pub fn apply(pack: &Path, source_root: &Path, out: &Path, progress: Progress) -> Result<Report> {
    let manifest = Manifest::load(pack)?;
    let source = Source::open(&manifest, source_root)?;
    let edition = &manifest.editions[source.edition];
    fs::create_dir_all(out).map_err(|e| Error::io(out, e))?;
    let same = |a: &Path, b: &Path| matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b);
    if same(out, source_root) {
        return Err(Error::InvalidArgument(
            "the unified game is written to a new folder: choose another one than the source"
                .into(),
        ));
    }
    let total = manifest.total_size();
    let mut done = 0u64;
    let mut report = Report::default();
    let mut missing = Vec::new();

    // The pack file of an entry: its own part, or any part that has it (a
    // pack with every part repairs a changed copy).
    let parts: Vec<&str> = std::iter::once(PART_COMMON)
        .chain(manifest.editions.iter().map(|e| e.part.as_str()))
        .collect();
    let in_pack = |rel: &str, preferred: Option<&str>| -> Option<PathBuf> {
        preferred
            .into_iter()
            .chain(parts.iter().copied())
            .map(|part| pack.join(part).join(rel))
            .find(|p| p.is_file())
    };

    // Files kept in the cabinets of a disc, grouped by cabinet.
    let mut pending: BTreeMap<String, Vec<&FileEntry>> = BTreeMap::new();
    let mut later = Vec::new();
    for file in &manifest.files {
        let target = out.join(&file.path);
        if up_to_date(&target, file.size, &file.sha256) {
            report.kept += 1;
            done += file.size;
            continue;
        }
        match file.from.get(&edition.id) {
            Some(path) if source.disc && edition.disc.contains_key(path) => {
                pending
                    .entry(edition.disc[path].0.clone())
                    .or_default()
                    .push(file);
            }
            Some(path) => later.push((file, Some(path.as_str()))),
            None => later.push((file, None)),
        }
    }

    let from_pack = |file: &FileEntry,
                     done: &mut u64,
                     report: &mut Report,
                     missing: &mut Vec<String>|
     -> Result<()> {
        let Some(path) = in_pack(&file.path, file.part.as_deref()) else {
            missing.push(file.path.clone());
            return Ok(());
        };
        let mut input = File::open(&path).map_err(|e| Error::io(&path, e))?;
        if !copy_checked(&mut input, &out.join(&file.path), &file.sha256)? {
            return Err(Error::InvalidArgument(format!(
                "{} of the pack is damaged or not downloaded to the end",
                path.display()
            )));
        }
        report.from_pack += 1;
        *done += file.size;
        Ok(())
    };

    for (cabinet_name, wanted) in &pending {
        let Some(cabinet_path) = resolve(&source.root, cabinet_name) else {
            for file in wanted {
                from_pack(file, &mut done, &mut report, &mut missing)?;
            }
            continue;
        };
        let cabinet = cab::Cabinet::open(&cabinet_path)?;
        let by_inner: HashMap<String, &FileEntry> = wanted
            .iter()
            .map(|f| {
                (
                    edition.disc[&f.from[&edition.id]].1.to_ascii_lowercase(),
                    *f,
                )
            })
            .collect();
        let mut open: HashMap<String, (PathBuf, HashWriter<BufWriter<File>>)> = HashMap::new();
        let mut finished: Vec<(&FileEntry, PathBuf, String)> = Vec::new();
        cabinet.extract(
            |f| by_inner.contains_key(&f.name.to_ascii_lowercase()),
            |f, piece| {
                let key = f.name.to_ascii_lowercase();
                let entry = by_inner[&key];
                if !open.contains_key(&key) {
                    let target = out.join(&entry.path);
                    if let Some(dir) = target.parent() {
                        fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
                    }
                    let temp = atomic::temp_path(&target)?;
                    let file = File::create(&temp).map_err(|e| Error::io(&temp, e))?;
                    open.insert(key.clone(), (temp, HashWriter::new(BufWriter::new(file))));
                }
                if piece.is_empty() {
                    let (temp, writer) = open.remove(&key).expect("opened above");
                    let (mut inner, _, sha) = writer.finish();
                    inner.flush().map_err(|e| Error::io(&temp, e))?;
                    finished.push((entry, temp, sha));
                } else {
                    let (temp, writer) = open.get_mut(&key).expect("opened above");
                    writer
                        .write_all(piece)
                        .map_err(|e| Error::io(temp.as_path(), e))?;
                }
                Ok(())
            },
        )?;
        let mut got: std::collections::HashSet<&str> = Default::default();
        for (entry, temp, sha) in finished {
            got.insert(entry.path.as_str());
            if sha == entry.sha256 {
                let target = out.join(&entry.path);
                fs::rename(&temp, &target).map_err(|e| Error::io(&target, e))?;
                report.from_source += 1;
                done += entry.size;
                progress(done, total, &entry.path);
            } else {
                let _ = fs::remove_file(&temp);
                report.changed_in_source.push(entry.path.clone());
                from_pack(entry, &mut done, &mut report, &mut missing)?;
            }
        }
        for file in wanted.iter().filter(|f| !got.contains(f.path.as_str())) {
            from_pack(file, &mut done, &mut report, &mut missing)?;
        }
    }

    for (file, from) in later {
        progress(done, total, &file.path);
        let taken = match from.and_then(|rel| resolve(&source.root, rel)) {
            Some(path) => {
                let mut input = File::open(&path).map_err(|e| Error::io(&path, e))?;
                copy_checked(&mut input, &out.join(&file.path), &file.sha256)?
            }
            None => false,
        };
        if taken {
            report.from_source += 1;
            done += file.size;
        } else {
            if from.is_some() {
                report.changed_in_source.push(file.path.clone());
            }
            from_pack(file, &mut done, &mut report, &mut missing)?;
        }
    }

    // Sound archives: entry by entry from the archives of the source and
    // the pack archive of the edition.
    let source_archives = {
        let mut paths = big::find_archives(&source.root).unwrap_or_default();
        paths.sort_by_key(|p| big::archive_number(p));
        ArchiveSet::open(&paths)?
    };
    for archive in &manifest.archives {
        let target = out.join(&archive.path);
        progress(done, total, &archive.path);
        if up_to_date(&target, archive.size, &archive.sha256) {
            report.kept += 1;
            done += archive.size;
            continue;
        }
        let mut pack_paths: Vec<PathBuf> = archive
            .lacking
            .get(&edition.id)
            .map(|rel| pack.join(rel))
            .into_iter()
            .chain(archive.lacking.values().map(|rel| pack.join(rel)))
            .filter(|p| p.is_file())
            .collect();
        pack_paths.dedup();
        let pack_archives = ArchiveSet::open(&pack_paths)?;
        // Some repacks unpack the archives into the game folder.
        let loose = |name: &str, size: u64| -> Option<PathBuf> {
            resolve(&source.root, &name.replace('\\', "/"))
                .filter(|p| fs::metadata(p).is_ok_and(|m| m.is_file() && m.len() == size))
        };
        let absent: Vec<&str> = archive
            .entries
            .iter()
            .filter(|(n, s)| {
                source_archives.find(n, *s).is_none()
                    && pack_archives.find(n, *s).is_none()
                    && loose(n, *s).is_none()
            })
            .map(|(n, _)| n.as_str())
            .collect();
        if !absent.is_empty() {
            missing.push(format!(
                "{} ({} sound(s), e.g. {})",
                archive.path,
                absent.len(),
                absent[0]
            ));
            continue;
        }
        let mut handles: HashMap<(bool, usize), File> = HashMap::new();
        let base = done;
        let mut written = 0u64;
        let (_, sha256) = write_archive(&target, &archive.entries, |name, size, out| {
            // The pack first: it holds what the source lacks or has changed.
            let (from_pack, (index, offset)) = match (
                pack_archives.find(name, size),
                source_archives.find(name, size),
            ) {
                (Some(found), _) => (true, found),
                (None, Some(found)) => (false, found),
                (None, None) => {
                    let path = loose(name, size).expect("checked above");
                    let mut file = File::open(&path).map_err(|e| Error::io(&path, e))?;
                    std::io::copy(&mut file, out).map_err(|e| Error::io(&path, e))?;
                    written += size;
                    return Ok(());
                }
            };
            let set = if from_pack {
                &pack_archives
            } else {
                &source_archives
            };
            let path = &set.archives[index].path;
            let file = match handles.entry((from_pack, index)) {
                std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
                std::collections::hash_map::Entry::Vacant(e) => {
                    e.insert(File::open(path).map_err(|e| Error::io(path, e))?)
                }
            };
            let mut reader = BufReader::with_capacity(1 << 16, &mut *file);
            reader
                .seek(SeekFrom::Start(offset))
                .map_err(|e| Error::io(path, e))?;
            let copied =
                std::io::copy(&mut reader.take(size), out).map_err(|e| Error::io(path, e))?;
            if copied != size {
                return Err(Error::malformed("BIGF", offset as usize, "entry cut short"));
            }
            written += size;
            if written % (64 << 20) < size {
                progress(base + written, total, &archive.path);
            }
            Ok(())
        })?;
        if sha256 != archive.sha256 {
            let _ = fs::remove_file(&target);
            return Err(Error::InvalidArgument(format!(
                "{} came out different from the unified game: the sound archives of {} are not \
                 those of the disc",
                archive.path,
                source.root.display()
            )));
        }
        report.archives_built += 1;
        done += archive.size;
    }

    if !missing.is_empty() {
        missing.sort();
        return Err(Error::InvalidArgument(format!(
            "{} file(s) are neither in {} (as on the disc of {}) nor in the pack; the pack lacks \
             its part `{}`, or the copy is changed: {}{}",
            missing.len(),
            source.root.display(),
            edition.name,
            edition.part,
            missing
                .iter()
                .take(8)
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
            if missing.len() > 8 { ", …" } else { "" }
        )));
    }
    progress(total, total, "");
    Ok(report)
}

/// Checks a game folder against the manifest: paths that are absent or differ.
pub fn verify(pack: &Path, game: &Path) -> Result<Vec<String>> {
    use rayon::prelude::*;
    let manifest = Manifest::load(pack)?;
    let all: Vec<(&str, u64, &str)> = manifest
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.size, f.sha256.as_str()))
        .chain(
            manifest
                .archives
                .iter()
                .map(|a| (a.path.as_str(), a.size, a.sha256.as_str())),
        )
        .collect();
    Ok(all
        .par_iter()
        .filter(|(path, size, sha)| {
            !resolve(game, path).is_some_and(|p| up_to_date(&p, *size, sha))
        })
        .map(|(path, _, _)| path.to_string())
        .collect())
}

#[cfg(test)]
mod tests;

//! Content-addressed payload files and file fingerprints.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Error, Result};

/// SHA-256 of no data.
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// A piece of data stored in the workspace, named by its SHA-256.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Blob {
    /// SHA-256 of the data, lower-case hexadecimal.
    pub sha256: String,
    /// Length in bytes.
    pub size: u64,
}

impl Blob {
    /// Describes `data` without storing it.
    pub fn of(data: &[u8]) -> Self {
        Self {
            sha256: sha256_hex(data),
            size: data.len() as u64,
        }
    }
}

/// SHA-256 of `data` in lower-case hexadecimal.
pub fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

/// Payloads kept in memory by hash, see [`PayloadStore::staged`].
type Staged = Arc<Mutex<HashMap<String, Vec<u8>>>>;

/// The `payloads/` directory: one file per distinct piece of data, at
/// `payloads/<first two hex digits>/<sha256>`. Identical data is stored once.
///
/// The store of a workspace that is not on disk yet keeps its data in
/// memory ([`Self::staged`]) until [`Self::flush`] writes it.
#[derive(Debug, Clone)]
pub struct PayloadStore {
    dir: PathBuf,
    staged: Option<Staged>,
}

impl PayloadStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            staged: None,
        }
    }

    /// A store that keeps its data in memory until [`Self::flush`].
    pub fn staged(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            staged: Some(Arc::default()),
        }
    }

    /// Writes the data kept in memory to the directory; afterwards the store
    /// writes directly.
    pub fn flush(&mut self) -> Result<()> {
        let Some(staged) = self.staged.take() else {
            return Ok(());
        };
        let staged = std::mem::take(&mut *staged.lock().unwrap_or_else(|e| e.into_inner()));
        for data in staged.values() {
            self.put(data)?;
        }
        Ok(())
    }

    fn staged_map(&self) -> Option<std::sync::MutexGuard<'_, HashMap<String, Vec<u8>>>> {
        self.staged
            .as_ref()
            .map(|m| m.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Path of the file holding `sha256`.
    pub fn path(&self, sha256: &str) -> PathBuf {
        let prefix = sha256.get(..2).unwrap_or("xx");
        self.dir.join(prefix).join(sha256)
    }

    /// Stores `data` and returns its description.
    pub fn put(&self, data: &[u8]) -> Result<Blob> {
        let blob = Blob::of(data);
        if let Some(mut staged) = self.staged_map() {
            staged
                .entry(blob.sha256.clone())
                .or_insert_with(|| data.to_vec());
            return Ok(blob);
        }
        let path = self.path(&blob.sha256);
        if !path.is_file() {
            let parent = path.parent().expect("payload paths have a parent");
            fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
            crate::atomic::write(&path, data)?;
        }
        Ok(blob)
    }

    /// Whether the data of `blob` is stored.
    pub fn contains(&self, blob: &Blob) -> bool {
        if let Some(staged) = self.staged_map() {
            return staged.contains_key(&blob.sha256);
        }
        self.path(&blob.sha256).is_file()
    }

    /// Reads the data of `blob` and checks it against the hash.
    pub fn get(&self, blob: &Blob) -> Result<Vec<u8>> {
        if blob.sha256.len() != 64 || !blob.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::InvalidArgument(format!(
                "bad payload hash `{}`",
                blob.sha256
            )));
        }
        // No data needs no file: the "original" of an added resource is
        // empty and was never stored.
        if blob.size == 0 && blob.sha256 == EMPTY_SHA256 {
            return Ok(Vec::new());
        }
        let path = self.path(&blob.sha256);
        let data = match self.staged_map() {
            Some(staged) => staged.get(&blob.sha256).cloned().ok_or_else(|| {
                Error::InvalidArgument(format!("payload {} is not stored", blob.sha256))
            })?,
            None => fs::read(&path).map_err(|e| Error::io(&path, e))?,
        };
        if data.len() as u64 != blob.size || sha256_hex(&data) != blob.sha256 {
            return Err(Error::InvalidArgument(format!(
                "payload {} is damaged",
                path.display()
            )));
        }
        Ok(data)
    }

    /// Removes every payload whose hash is not in `keep`. Returns how many were removed.
    pub fn retain(&self, keep: &std::collections::HashSet<&str>) -> Result<usize> {
        if let Some(mut staged) = self.staged_map() {
            let before = staged.len();
            staged.retain(|sha, _| keep.contains(sha.as_str()));
            return Ok(before - staged.len());
        }
        let mut removed = 0;
        let Ok(prefixes) = fs::read_dir(&self.dir) else {
            return Ok(0);
        };
        for prefix in prefixes {
            let prefix = prefix.map_err(|e| Error::io(&self.dir, e))?.path();
            if !prefix.is_dir() {
                continue;
            }
            for entry in fs::read_dir(&prefix).map_err(|e| Error::io(&prefix, e))? {
                let path = entry.map_err(|e| Error::io(&prefix, e))?.path();
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name.len() == 64 && !keep.contains(name) {
                    fs::remove_file(&path).map_err(|e| Error::io(&path, e))?;
                    removed += 1;
                }
            }
            let _ = fs::remove_dir(&prefix);
        }
        Ok(removed)
    }
}

/// What a file looked like at some moment: size and SHA-256, plus the
/// modification time and file id to skip hashing when nothing changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStamp {
    pub size: u64,
    pub sha256: String,
    /// Modification time in nanoseconds since the Unix epoch; a hint only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtime_ns: Option<u128>,
    /// Inode number on Unix; a hint only. A file replaced by renaming gets a new one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_id: Option<u64>,
}

impl FileStamp {
    /// Reads and hashes the file at `path` (following symbolic links).
    pub fn of_file(path: &Path) -> Result<Self> {
        let (mtime_ns, file_id) = hints(path);
        let mut file = fs::File::open(path).map_err(|e| Error::io(path, e))?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; 1 << 20];
        let mut size = 0u64;
        loop {
            let n = file.read(&mut buffer).map_err(|e| Error::io(path, e))?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
            size += n as u64;
        }
        Ok(Self {
            size,
            sha256: hex(&hasher.finalize()),
            mtime_ns,
            file_id,
        })
    }

    /// Stamp of contents without a file: no hints, always compared by hash.
    pub fn of_data(data: &[u8]) -> Self {
        Self {
            size: data.len() as u64,
            sha256: sha256_hex(data),
            mtime_ns: None,
            file_id: None,
        }
    }

    /// Stamp of `data` that was just written to `path`, with the file's hints.
    pub fn of_written(path: &Path, data: &[u8]) -> Self {
        let (mtime_ns, file_id) = hints(path);
        Self {
            mtime_ns,
            file_id,
            ..Self::of_data(data)
        }
    }

    /// Whether the file at `path` has this stamp's size, modification time
    /// and file id, so that it can be assumed unchanged without hashing.
    pub fn quick_match(&self, path: &Path) -> bool {
        let Ok(metadata) = fs::metadata(path) else {
            return false;
        };
        let (mtime_ns, file_id) = hints(path);
        metadata.len() == self.size
            && self.mtime_ns.is_some()
            && mtime_ns == self.mtime_ns
            && file_id == self.file_id
    }

    /// Whether the file at `path` still has the contents of this stamp. The
    /// file is hashed unless [`Self::quick_match`] holds.
    pub fn matches_file(&self, path: &Path) -> Result<bool> {
        match fs::metadata(path) {
            Ok(metadata) if metadata.len() == self.size => {}
            _ => return Ok(false),
        }
        if self.quick_match(path) {
            return Ok(true);
        }
        Ok(FileStamp::of_file(path)?.same_contents(self))
    }

    /// Whether two stamps describe the same contents.
    pub fn same_contents(&self, other: &FileStamp) -> bool {
        self.size == other.size && self.sha256 == other.sha256
    }
}

/// Modification time and file id of `path`.
fn hints(path: &Path) -> (Option<u128>, Option<u64>) {
    let Ok(metadata) = fs::metadata(path) else {
        return (None, None);
    };
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos());
    #[cfg(unix)]
    let file_id = Some(std::os::unix::fs::MetadataExt::ino(&metadata));
    #[cfg(not(unix))]
    let file_id = None;
    (mtime, file_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_data_is_read_without_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = PayloadStore::new(dir.path().join("payloads"));
        let empty = Blob::of(&[]);
        assert_eq!(empty.sha256, EMPTY_SHA256);
        assert_eq!(store.get(&empty).unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn stores_once_and_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let store = PayloadStore::new(dir.path().join("payloads"));
        let blob = store.put(b"hello").unwrap();
        assert_eq!(
            blob.sha256,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        assert_eq!(store.put(b"hello").unwrap(), blob);
        assert_eq!(store.get(&blob).unwrap(), b"hello");
        fs::write(store.path(&blob.sha256), b"jello").unwrap();
        assert!(store.get(&blob).is_err(), "damaged payloads are detected");

        let other = store.put(b"other").unwrap();
        let keep = std::collections::HashSet::from([other.sha256.as_str()]);
        assert_eq!(store.retain(&keep).unwrap(), 1);
        assert!(!store.contains(&blob));
        assert!(store.contains(&other));
    }

    #[test]
    fn stamps_detect_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        fs::write(&path, b"abc").unwrap();
        let stamp = FileStamp::of_file(&path).unwrap();
        assert_eq!(stamp, FileStamp::of_written(&path, b"abc"));
        assert!(stamp.quick_match(&path));
        assert!(!FileStamp::of_data(b"abc").quick_match(&path));
        assert!(stamp.matches_file(&path).unwrap());
        // Same size, different contents and time.
        let stale = FileStamp {
            mtime_ns: Some(1),
            ..stamp.clone()
        };
        fs::write(&path, b"abd").unwrap();
        assert!(!stale.matches_file(&path).unwrap());
        assert!(!stamp.matches_file(&dir.path().join("missing")).unwrap());
    }
}

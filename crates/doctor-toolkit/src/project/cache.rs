//! What the tool keeps in `.doctor/`: the build cache (`build.json`) and
//! the resource data that strings and logic are applied to (`base/`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::workspace::store::sha256_hex;
use crate::{Error, Result};

/// Folder of the tool's data in the project.
pub const TOOL_DIR: &str = ".doctor";
/// The build cache in [`TOOL_DIR`]; deleting it only costs time.
pub const CACHE_FILE: &str = "build.json";
/// Resource data by SHA-256 in [`TOOL_DIR`]; needed to build strings and logic.
pub const BASE_DIR: &str = "base";

/// A file changed less than this before it was hashed may change again
/// within the same time stamp; it is hashed again next time.
const RACY_NS: u64 = 2_000_000_000;

/// State of a resource in the workspace: SHA-256 of the recorded data, or
/// `None` when the workspace does not change it.
pub type Outputs = BTreeMap<String, Option<String>>;

/// The state of an entry after its last build (or when the project was made).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Built {
    /// Digest of the files (and patch) that were built.
    pub inputs: String,
    /// What the workspace had for the entry's resources and the resources
    /// the build added or changed besides them, by key.
    pub outputs: Outputs,
}

/// A stream the project creates, as the last build or export left it in
/// the workspace.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuiltStream {
    pub stream: String,
    /// Digest of its description in `stream.toml` at that time
    /// ([`super::manifest::NewStreamSource::digest`]): a description that
    /// differs was changed by hand and is not built yet.
    #[serde(default)]
    pub described: String,
}

/// Size and time of a source file and its SHA-256.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStamp {
    pub size: u64,
    pub mtime_ns: u64,
    /// When the file was hashed.
    pub hashed_ns: u64,
    pub sha256: String,
}

/// `.doctor/build.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildCache {
    pub format: u32,
    /// Entry id (path of its main file from the project root) → last build.
    pub entries: BTreeMap<String, Built>,
    /// Source files (path from the project root) → stamp and hash.
    #[serde(default)]
    pub files: BTreeMap<String, FileStamp>,
    /// Folders of streams the project creates (path from the project root)
    /// → the stream, as the last build or export left them in the
    /// workspace. A folder that is gone names a stream to remove.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub streams: BTreeMap<String, BuiltStream>,
}

impl BuildCache {
    pub fn path(root: &Path) -> PathBuf {
        root.join(TOOL_DIR).join(CACHE_FILE)
    }

    /// Reads the cache; a missing or unreadable cache is empty.
    pub fn read(root: &Path) -> Self {
        std::fs::read(Self::path(root))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Self>(&bytes).ok())
            .filter(|c| c.format <= super::config::FORMAT)
            .unwrap_or_default()
    }

    pub fn write(&self, root: &Path) -> Result<()> {
        let path = Self::path(root);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
        }
        let mut copy = self.clone();
        copy.format = super::config::FORMAT;
        let text = serde_json::to_string_pretty(&copy)
            .map_err(|e| Error::InvalidArgument(e.to_string()))?;
        crate::atomic::write(&path, (text + "\n").as_bytes())
    }

    /// SHA-256 of `root/rel`; read only when its size or time changed since
    /// it was last hashed. `None` when the file does not exist.
    pub fn file_sha(&mut self, root: &Path, rel: &str) -> Result<Option<String>> {
        let path = root.join(rel);
        let metadata = match std::fs::metadata(&path) {
            Ok(m) if m.is_file() => m,
            Ok(_) => return Ok(None),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(Error::io(&path, e)),
        };
        let mtime_ns = metadata
            .modified()
            .ok()
            .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos() as u64);
        if let Some(stamp) = self.files.get(rel)
            && stamp.size == metadata.len()
            && stamp.mtime_ns == mtime_ns
            && mtime_ns != 0
            && stamp.hashed_ns > mtime_ns.saturating_add(RACY_NS)
        {
            return Ok(Some(stamp.sha256.clone()));
        }
        let bytes = std::fs::read(&path).map_err(|e| Error::io(&path, e))?;
        let sha256 = sha256_hex(&bytes);
        self.files.insert(
            rel.to_owned(),
            FileStamp {
                size: bytes.len() as u64,
                mtime_ns,
                hashed_ns: now_ns(),
                sha256: sha256.clone(),
            },
        );
        Ok(Some(sha256))
    }
}

fn now_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

/// Digest of the inputs of an entry: each file (path relative to its
/// manifest and SHA-256) and the SHA-256 of its patch, if any.
pub fn digest(files: &[(&str, &str)], patch: Option<&str>) -> String {
    let mut text = String::new();
    for (path, sha) in files {
        text.push_str(path);
        text.push('\0');
        text.push_str(sha);
        text.push('\n');
    }
    if let Some(patch) = patch {
        text.push_str("patch\0");
        text.push_str(patch);
        text.push('\n');
    }
    sha256_hex(text.as_bytes())
}

/// Path of the base data with SHA-256 `sha`.
fn base_path(root: &Path, sha: &str) -> PathBuf {
    root.join(TOOL_DIR)
        .join(BASE_DIR)
        .join(&sha[..2.min(sha.len())])
        .join(sha)
}

/// Keeps `data` in `.doctor/base/` and returns its SHA-256.
pub fn put_base(root: &Path, data: &[u8]) -> Result<String> {
    let sha = sha256_hex(data);
    let path = base_path(root, &sha);
    if !path.exists() {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
        }
        crate::atomic::write(&path, data)?;
    }
    Ok(sha)
}

/// The base data with SHA-256 `sha`, checked against it.
pub fn get_base(root: &Path, sha: &str) -> Result<Vec<u8>> {
    let path = base_path(root, sha);
    let data = std::fs::read(&path).map_err(|e| {
        Error::InvalidArgument(format!(
            "{}: {e}; the data the file is applied to is missing (make the project again)",
            path.display()
        ))
    })?;
    if sha256_hex(&data) != sha {
        return Err(Error::InvalidArgument(format!(
            "{} is damaged (its SHA-256 does not match)",
            path.display()
        )));
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digests_change_with_any_file_or_the_patch() {
        let a = digest(&[("a.png", "11"), ("b.png", "22")], None);
        assert_ne!(a, digest(&[("a.png", "11"), ("b.png", "23")], None));
        assert_ne!(a, digest(&[("a.png", "11")], None));
        assert_ne!(a, digest(&[("a.png", "11"), ("b.png", "22")], Some("33")));
        assert_eq!(a, digest(&[("a.png", "11"), ("b.png", "22")], None));
    }

    #[test]
    fn files_are_hashed_again_only_when_they_change() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("x"), b"one").unwrap();
        let mut cache = BuildCache::default();
        let first = cache.file_sha(dir.path(), "x").unwrap().unwrap();
        assert_eq!(first, sha256_hex(b"one"));
        std::fs::write(dir.path().join("x"), b"two!").unwrap();
        assert_eq!(
            cache.file_sha(dir.path(), "x").unwrap().unwrap(),
            sha256_hex(b"two!")
        );
        assert_eq!(cache.file_sha(dir.path(), "missing").unwrap(), None);
        let sha = put_base(dir.path(), b"base").unwrap();
        assert_eq!(get_base(dir.path(), &sha).unwrap(), b"base");
        cache.write(dir.path()).unwrap();
        assert_eq!(BuildCache::read(dir.path()).files.len(), 1);
    }
}

//! Reading and writing mod packages (ZIP files).
//!
//! A package is untrusted input. Reading it:
//!
//! - refuses more than [`MAX_ENTRIES`] entries, duplicate names, names that
//!   are not plain relative paths (`..`, absolute paths, backslashes, control
//!   characters), symbolic links, encrypted entries and compression methods
//!   other than *stored* and *deflate*;
//! - never extracts by entry name: only `mod.json`, the preview named in it
//!   and `payloads/<sha256>` are read, into memory or into a payload store;
//! - reads every entry through a limit: the size the manifest declares, which
//!   is itself bounded ([`MAX_PAYLOAD_SIZE`], [`MAX_TOTAL_SIZE`]). A ZIP bomb
//!   stops at the limit; the data is then checked against its SHA-256.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};

use zip::read::ZipArchive;
use zip::write::{SimpleFileOptions, ZipWriter};
use zip::{CompressionMethod, DateTime};

use super::{
    MANIFEST_NAME, MAX_ENTRIES, MAX_MANIFEST_SIZE, MAX_PAYLOAD_SIZE, MAX_PREVIEW_SIZE, ModManifest,
    PAYLOAD_DIR,
};
use crate::workspace::{Blob, store::sha256_hex};
use crate::{Error, Result};

/// An opened, validated mod package. Payloads are read on demand.
pub struct ModArchive {
    path: PathBuf,
    archive: ZipArchive<BufReader<File>>,
    manifest: ModManifest,
    /// Entry index of every payload, by SHA-256.
    payloads: HashMap<String, usize>,
    preview: Option<usize>,
    warnings: Vec<String>,
}

impl std::fmt::Debug for ModArchive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModArchive")
            .field("path", &self.path)
            .field("id", &self.manifest.id)
            .finish_non_exhaustive()
    }
}

/// What an entry of the package is.
enum EntryKind {
    Manifest,
    Payload(String),
    Directory,
    Other(String),
}

impl ModArchive {
    /// Opens the package at `path` and checks its structure and manifest.
    /// Payload data is not read yet; see [`Self::verify`].
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_inner(path).map_err(|e| match e {
            Error::Mod(message) => Error::mod_error(format!("{}: {message}", path.display())),
            other => other,
        })
    }

    fn open_inner(path: &Path) -> Result<Self> {
        let file = File::open(path).map_err(|e| Error::io(path, e))?;
        let mut archive = ZipArchive::new(BufReader::new(file))
            .map_err(|e| Error::mod_error(format!("not a valid ZIP file: {e}")))?;
        if archive.len() > MAX_ENTRIES {
            return Err(Error::mod_error(format!(
                "{} entries; a mod has at most {MAX_ENTRIES}",
                archive.len()
            )));
        }
        if archive.has_duplicate_names() {
            return Err(Error::mod_error(
                "the ZIP file has several entries with the same name",
            ));
        }
        archive
            .validate_header_consistency()
            .map_err(|e| Error::mod_error(format!("inconsistent ZIP file: {e}")))?;

        let mut manifest_entry = None;
        let mut payloads = HashMap::new();
        let mut others = Vec::new();
        for index in 0..archive.len() {
            let entry = archive
                .by_index_raw(index)
                .map_err(|e| Error::mod_error(format!("ZIP entry {index}: {e}")))?;
            let name = entry
                .name()
                .map_err(|_| Error::mod_error(format!("ZIP entry {index} has a bad name")))?
                .into_owned();
            if entry.encrypted() {
                return Err(Error::mod_error(format!(
                    "entry {} is encrypted",
                    name.escape_debug()
                )));
            }
            if entry.is_symlink() {
                return Err(Error::mod_error(format!(
                    "entry {} is a symbolic link",
                    name.escape_debug()
                )));
            }
            if !matches!(
                entry.compression(),
                CompressionMethod::Stored | CompressionMethod::Deflated
            ) {
                return Err(Error::mod_error(format!(
                    "entry {} uses compression {}; only stored and deflate are supported",
                    name.escape_debug(),
                    entry.compression()
                )));
            }
            let kind = classify(&name, entry.is_dir())?;
            match kind {
                EntryKind::Manifest => manifest_entry = Some(index),
                EntryKind::Payload(sha) => {
                    payloads.insert(sha, index);
                }
                EntryKind::Directory => {}
                EntryKind::Other(name) => others.push((name, index)),
            }
        }
        let manifest_entry = manifest_entry
            .ok_or_else(|| Error::mod_error("no mod.json: this is not a doctor_toolkit mod"))?;
        let json = read_entry(&mut archive, manifest_entry, None, MAX_MANIFEST_SIZE)?;
        let manifest = ModManifest::parse(&json)?;

        let mut preview = None;
        let mut warnings = Vec::new();
        for (name, index) in others {
            if manifest.preview.as_deref() == Some(name.as_str()) {
                preview = Some(index);
            } else {
                warnings.push(format!("ignored unknown entry {}", name.escape_debug()));
            }
        }
        if let Some(name) = &manifest.preview
            && preview.is_none()
        {
            return Err(Error::mod_error(format!(
                "mod.json names the preview {name}, which is not in the package"
            )));
        }
        for blob in manifest.payloads() {
            let index = *payloads
                .get(&blob.sha256)
                .ok_or_else(|| Error::mod_error(format!("payload {} is missing", blob.sha256)))?;
            let size = archive
                .by_index_raw(index)
                .map_err(|e| Error::mod_error(format!("payload {}: {e}", blob.sha256)))?
                .size();
            if size != blob.size {
                return Err(Error::mod_error(format!(
                    "payload {} has {size} bytes; mod.json says {}",
                    blob.sha256, blob.size
                )));
            }
        }
        let used: std::collections::HashSet<&str> = manifest
            .resources
            .iter()
            .map(|r| r.payload.sha256.as_str())
            .collect();
        let unused = payloads
            .keys()
            .filter(|k| !used.contains(k.as_str()))
            .count();
        if unused > 0 {
            warnings.push(format!("{unused} payload(s) are not used by any resource"));
        }
        Ok(Self {
            path: path.to_owned(),
            archive,
            manifest,
            payloads,
            preview,
            warnings,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn manifest(&self) -> &ModManifest {
        &self.manifest
    }

    /// Harmless oddities found while opening, e.g. unknown entries.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// The preview picture, if the mod has one.
    pub fn read_preview(&mut self) -> Result<Option<Vec<u8>>> {
        let Some(index) = self.preview else {
            return Ok(None);
        };
        read_entry(&mut self.archive, index, None, MAX_PREVIEW_SIZE).map(Some)
    }

    /// Reads the payload `blob` and checks its size and hash.
    pub fn read_payload(&mut self, blob: &Blob) -> Result<Vec<u8>> {
        let index = *self
            .payloads
            .get(&blob.sha256)
            .ok_or_else(|| Error::mod_error(format!("payload {} is missing", blob.sha256)))?;
        let data = read_entry(&mut self.archive, index, Some(blob.size), MAX_PAYLOAD_SIZE)?;
        if sha256_hex(&data) != blob.sha256 {
            return Err(Error::mod_error(format!(
                "{}: payload {} is damaged (its contents do not match the hash)",
                self.path.display(),
                blob.sha256
            )));
        }
        Ok(data)
    }

    /// Reads and checks every payload.
    pub fn verify(&mut self) -> Result<()> {
        let blobs: Vec<Blob> = self.manifest.payloads().into_iter().cloned().collect();
        for blob in &blobs {
            self.read_payload(blob)?;
        }
        Ok(())
    }
}

/// Sorts an entry name into what it is, refusing unsafe names.
fn classify(name: &str, is_dir: bool) -> Result<EntryKind> {
    let unsafe_name = name.is_empty()
        || name.len() > 512
        || name.starts_with('/')
        || name.contains('\\')
        || name.contains(':')
        || name.chars().any(char::is_control)
        || name
            .trim_end_matches('/')
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..");
    if unsafe_name {
        return Err(Error::mod_error(format!(
            "entry {} has an unsafe name",
            name.escape_debug()
        )));
    }
    if is_dir || name.ends_with('/') {
        return Ok(EntryKind::Directory);
    }
    if name == MANIFEST_NAME {
        return Ok(EntryKind::Manifest);
    }
    if let Some(sha) = name
        .strip_prefix(PAYLOAD_DIR)
        .and_then(|rest| rest.strip_prefix('/'))
    {
        if sha.len() == 64
            && sha
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Ok(EntryKind::Payload(sha.to_owned()));
        }
        return Err(Error::mod_error(format!(
            "entry {} in payloads/ is not named by a SHA-256",
            name.escape_debug()
        )));
    }
    Ok(EntryKind::Other(name.to_owned()))
}

/// Reads entry `index` completely, at most `limit` bytes, and exactly
/// `expected` bytes when given. The declared size is not trusted: the read
/// stops one byte past the limit.
fn read_entry(
    archive: &mut ZipArchive<BufReader<File>>,
    index: usize,
    expected: Option<u64>,
    limit: u64,
) -> Result<Vec<u8>> {
    let entry = archive
        .by_index(index)
        .map_err(|e| Error::mod_error(format!("ZIP entry {index}: {e}")))?;
    let name = entry.name().map(|n| n.into_owned()).unwrap_or_default();
    let declared = entry.size();
    let limit = expected.unwrap_or(limit).min(limit);
    if declared > limit {
        return Err(Error::mod_error(format!(
            "entry {} is {declared} bytes; at most {limit} are allowed here",
            name.escape_debug()
        )));
    }
    let mut data = Vec::with_capacity(declared as usize);
    entry
        .take(limit + 1)
        .read_to_end(&mut data)
        .map_err(|e| Error::mod_error(format!("cannot read {}: {e}", name.escape_debug())))?;
    let wrong_size = match expected {
        Some(size) => data.len() as u64 != size,
        None => data.len() as u64 != declared || data.len() as u64 > limit,
    };
    if wrong_size {
        return Err(Error::mod_error(format!(
            "entry {} does not have the size it declares",
            name.escape_debug()
        )));
    }
    Ok(data)
}

/// Writes a mod package to `out`, replacing the file atomically. `payload`
/// returns the data of each distinct payload; it is checked against its hash.
/// Returns the size of the written file.
pub fn write_package(
    out: &Path,
    manifest: &ModManifest,
    preview: Option<&[u8]>,
    mut payload: impl FnMut(&Blob) -> Result<Vec<u8>>,
) -> Result<u64> {
    manifest.validate()?;
    if manifest.preview.is_some() != preview.is_some() {
        return Err(Error::InvalidArgument(
            "the preview picture and its name in the manifest go together".into(),
        ));
    }
    if let Some(picture) = preview
        && picture.len() as u64 > super::MAX_PREVIEW_SIZE
    {
        return Err(Error::mod_error(format!(
            "the preview picture is larger than {} bytes",
            super::MAX_PREVIEW_SIZE
        )));
    }
    let json = manifest.to_json()?;
    let mut failure: Option<Error> = None;
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .last_modified_time(DateTime::default());
    let result = crate::atomic::write_with(out, |file| {
        let mut zip = ZipWriter::new(file);
        let to_io = |e: zip::result::ZipError| std::io::Error::other(e.to_string());
        zip.start_file(MANIFEST_NAME, options).map_err(to_io)?;
        zip.write_all(&json)?;
        if let (Some(name), Some(picture)) = (&manifest.preview, preview) {
            let stored = options.compression_method(CompressionMethod::Stored);
            zip.start_file(name.as_str(), stored).map_err(to_io)?;
            zip.write_all(picture)?;
        }
        for blob in manifest.payloads() {
            let data = match payload(blob) {
                Ok(data) if Blob::of(&data) == *blob => data,
                Ok(_) => {
                    failure = Some(Error::mod_error(format!(
                        "payload {} does not match its hash",
                        blob.sha256
                    )));
                    return Err(std::io::Error::other("payload mismatch"));
                }
                Err(error) => {
                    failure = Some(error);
                    return Err(std::io::Error::other("payload unavailable"));
                }
            };
            let entry = format!("{PAYLOAD_DIR}/{}", blob.sha256);
            zip.start_file(entry, options.large_file(blob.size >= u32::MAX as u64))
                .map_err(to_io)?;
            zip.write_all(&data)?;
        }
        zip.finish().map_err(to_io)?;
        Ok(())
    });
    if let Some(error) = failure {
        return Err(error);
    }
    result?;
    Ok(std::fs::metadata(out).map_err(|e| Error::io(out, e))?.len())
}

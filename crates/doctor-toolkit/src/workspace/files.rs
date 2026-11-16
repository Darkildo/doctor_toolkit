//! Changes of game files outside the streams: entries of the `.viv`
//! archives (streamed audio) and loose files (movies).
//!
//! The archives are 4.7 GB and are never rewritten. The game mounts
//! `GODFATH0.VIV` .. `GODFATH12.VIV` in this order (`0x0068bd30`) and opens a
//! name from the **first** archive that has it (`BigHandler` open,
//! `0x00b90fb0`). Changed archive entries therefore go into one small
//! **overlay archive** that takes number 0, and the game's archives are
//! renamed one number up (`GODFATH0.viv` → `GODFATH1.viv`, …): renames only,
//! no data is copied, and undoing them restores the original names. The
//! overlay's first entry, [`OVERLAY_MARKER`], names the owner; archives with
//! the marker are never taken for originals. Loose files are backed up to
//! `_doctor_toolkit/backups/` and replaced atomically, like streams.
//!
//! What was written is kept in `applied.json` ([`AppliedFile`]): the
//! original file (none for the overlay), the written file and which payload of
//! which change it holds. Applying writes every file that differs from the
//! wanted state and puts back every file that no change wants any more.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use super::deploy::Deployment;
use super::{ApplyReport, Blob, ChangeState, FileStamp, StreamAction};
use crate::container::big::{self, BigArchive, archive_number, find_archives, normalize_name};
use crate::game::{BACKUP_DIR, GameInstance};
use crate::{Error, Result};

/// Owner name the mod manager gives to [`sync`].
pub const MODS_OWNER: &str = "mods";

/// First entry of an overlay archive written by the toolkit.
pub const OVERLAY_MARKER: &str = "_doctor_toolkit\\overlay.txt";

/// A game file outside the streams.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum FileKey {
    /// An entry of the `.viv` archives: lower case, `\` separators
    /// (`audiostreams\e_screen\title_screen_01.exa.snu`).
    Archive(String),
    /// A loose file: path relative to the game directory, `/` separators, as
    /// on disk (`movies/common/mainmenu.vp6`).
    Loose(String),
}

impl FileKey {
    /// An archive entry; `/` and `\` are accepted, case is ignored.
    pub fn archive_entry(name: &str) -> Self {
        Self::Archive(normalize_name(name).trim_start_matches('\\').to_owned())
    }

    /// A loose file, relative to the game directory.
    pub fn loose(rel_path: &str) -> Self {
        Self::Loose(
            rel_path
                .replace('\\', "/")
                .trim_start_matches('/')
                .to_owned(),
        )
    }

    /// The entry name or relative path.
    pub fn name(&self) -> &str {
        match self {
            Self::Archive(name) | Self::Loose(name) => name,
        }
    }
}

impl fmt::Display for FileKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Archive(name) => write!(f, "viv:{name}"),
            Self::Loose(path) => write!(f, "file:{path}"),
        }
    }
}

impl FromStr for FileKey {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        let key = if let Some(name) = s.strip_prefix("viv:") {
            Self::archive_entry(name)
        } else if let Some(path) = s.strip_prefix("file:") {
            Self::loose(path)
        } else {
            return Err(Error::InvalidArgument(format!(
                "{s:?} is not a file key (viv:<archive entry> or file:<path>)"
            )));
        };
        if key.name().is_empty() || key.name().split(['/', '\\']).any(|p| p == "..") {
            return Err(Error::InvalidArgument(format!("bad file key {s:?}")));
        }
        Ok(key)
    }
}

impl TryFrom<String> for FileKey {
    type Error = Error;
    fn try_from(s: String) -> Result<Self> {
        s.parse()
    }
}

impl From<FileKey> for String {
    fn from(key: FileKey) -> String {
        key.to_string()
    }
}

/// A replaced game file outside the streams.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    pub key: FileKey,
    /// The new contents (in `payloads/`).
    pub payload: Blob,
    /// The contents in the unmodified game. Not stored: the original archives
    /// are never written, and loose originals stay in the game or its backups.
    pub original: Blob,
    pub label: String,
}

/// The change of one file made by an operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEdit {
    pub key: FileKey,
    pub before: Option<FileChange>,
    pub after: Option<FileChange>,
}

/// A file outside the streams written by an owner, in `applied.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedFile {
    /// The unmodified file; `None` when the owner created it (the overlay archive).
    pub original: Option<FileStamp>,
    pub written: FileStamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_flight: Option<FileStamp>,
    /// Payload hash of every change in the written file.
    pub changes: BTreeMap<FileKey, String>,
    /// For the overlay: the game's archives renamed to make room for it, as
    /// (original name, current name), in the order the renames were done.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shifted: Vec<(String, String)>,
}

/// Where a file change is written.
fn target_path(key: &FileKey, overlay: &str) -> String {
    match key {
        FileKey::Archive(_) => overlay.to_owned(),
        FileKey::Loose(path) => path.clone(),
    }
}

/// Whether the archive at `path` is an overlay written by the toolkit.
pub fn is_overlay(path: &Path) -> bool {
    path.is_file()
        && BigArchive::open(path).is_ok_and(|a| {
            a.entries
                .first()
                .is_some_and(|e| normalize_name(&e.name) == OVERLAY_MARKER)
        })
}

/// The game's own archives, in the order the game mounts them.
pub fn original_archives(game_root: &Path) -> Result<Vec<PathBuf>> {
    let mut archives: Vec<(usize, PathBuf)> = find_archives(game_root)?
        .into_iter()
        .filter(|p| !is_overlay(p))
        .map(|p| (archive_number(&p).unwrap_or(usize::MAX), p))
        .collect();
    archives.sort();
    Ok(archives.into_iter().map(|(_, p)| p).collect())
}

/// Extension of the game's archive names as written on disk (`viv`).
fn archive_extension(game_root: &Path) -> Result<String> {
    let first = original_archives(game_root)?
        .into_iter()
        .find(|p| archive_number(p).is_some())
        .ok_or_else(|| Error::InvalidArgument("the game has no GODFATHn.VIV archives".into()))?;
    Ok(first
        .extension()
        .map_or("VIV".into(), |e| e.to_string_lossy().into_owned()))
}

/// File name of the overlay archive: number 0, which the game opens first.
pub fn overlay_name(game_root: &Path) -> Result<String> {
    Ok(format!("GODFATH0.{}", archive_extension(game_root)?))
}

/// Renames that move the game's archives one number up, highest first.
fn shift_plan(game_root: &Path) -> Result<Vec<(String, String)>> {
    let ext = archive_extension(game_root)?;
    let mut numbered: Vec<(usize, String)> = original_archives(game_root)?
        .iter()
        .filter_map(|p| Some((archive_number(p)?, p.file_name()?.to_str()?.to_owned())))
        .collect();
    numbered.sort();
    let existing: Vec<String> = find_archives(game_root)?
        .iter()
        .filter_map(|p| Some(p.file_name()?.to_str()?.to_owned()))
        .collect();
    let mut plan: Vec<(String, String)> = Vec::new();
    for (n, name) in numbered.into_iter().rev() {
        if n + 1 >= big::MAX_ARCHIVES {
            return Err(Error::InvalidArgument(format!(
                "cannot rename {name}: the game opens GODFATH0..{}.VIV only",
                big::MAX_ARCHIVES - 1
            )));
        }
        let to = format!("GODFATH{}.{ext}", n + 1);
        // The target is free, or is itself renamed before (highest first).
        let taken = existing.iter().any(|e| e.eq_ignore_ascii_case(&to))
            && !plan.iter().any(|(from, _)| from.eq_ignore_ascii_case(&to));
        if taken {
            return Err(Error::InvalidArgument(format!(
                "cannot rename {name} to {to}: {to} exists"
            )));
        }
        plan.push((name, to));
    }
    Ok(plan)
}

/// Does the renames of `plan` that are not done yet.
fn do_renames(root: &Path, plan: &[(String, String)]) -> Result<()> {
    for (from, to) in plan {
        let (from, to) = (root.join(from), root.join(to));
        if from.symlink_metadata().is_ok() && to.symlink_metadata().is_err() {
            std::fs::rename(&from, &to).map_err(|e| Error::io(&from, e))?;
        }
    }
    Ok(())
}

/// Undoes the renames of `plan`, lowest first.
fn undo_renames(root: &Path, plan: &[(String, String)]) -> Result<()> {
    for (from, to) in plan.iter().rev() {
        let (from, to) = (root.join(from), root.join(to));
        if to.symlink_metadata().is_ok() && from.symlink_metadata().is_err() {
            std::fs::rename(&to, &from).map_err(|e| Error::io(&to, e))?;
        }
    }
    Ok(())
}

/// The unmodified contents of a file: the archive entry from the game's own
/// archives, or the loose file (its backup when there is one).
pub fn original_data(game_root: &Path, key: &FileKey) -> Result<Vec<u8>> {
    match key {
        FileKey::Archive(name) => {
            for path in original_archives(game_root)? {
                let archive = BigArchive::open(&path)?;
                if let Some(entry) = archive
                    .entries
                    .iter()
                    .find(|e| normalize_name(&e.name) == *name)
                {
                    return archive.read(entry);
                }
            }
            Err(Error::InvalidArgument(format!(
                "no archive of the game has {name}"
            )))
        }
        FileKey::Loose(rel) => {
            let backup = game_root.join(BACKUP_DIR).join(rel);
            let path = if backup.is_file() {
                backup
            } else {
                game_root.join(rel)
            };
            std::fs::read(&path).map_err(|e| Error::io(&path, e))
        }
    }
}

/// The overlay archive with the given changes, the marker entry first.
fn build_overlay(changes: &[(&FileKey, Vec<u8>)], owner: &str) -> Result<Vec<u8>> {
    let mut listing = format!("doctor_toolkit overlay archive\nowner: {owner}\n");
    for (key, _) in changes {
        listing.push_str(key.name());
        listing.push('\n');
    }
    let mut entries = vec![(OVERLAY_MARKER.to_owned(), listing.into_bytes())];
    entries.extend(
        changes
            .iter()
            .map(|(k, d)| (k.name().to_owned(), d.clone())),
    );
    big::build_archive(&entries)
}

/// State of one written file relative to the record.
fn file_state(root: &Path, rel: &str, record: Option<&AppliedFile>) -> Result<FileState> {
    let path = root.join(rel);
    let exists = path.is_file();
    let matches = |stamp: Option<&FileStamp>| -> Result<bool> {
        match stamp {
            Some(s) if exists => s.matches_file(&path),
            _ => Ok(false),
        }
    };
    Ok(match record {
        Some(r) if matches(Some(&r.written))? || matches(r.in_flight.as_ref())? => {
            FileState::Written
        }
        Some(r) => match &r.original {
            None if !exists => FileState::Original,
            Some(o) if matches(Some(o))? => FileState::Original,
            _ => FileState::Changed,
        },
        None if exists => FileState::Original,
        None => FileState::Missing,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileState {
    /// Holds what the record says was written.
    Written,
    /// The unmodified file (for the overlay: no overlay).
    Original,
    /// Changed by someone else.
    Changed,
    /// A loose file that does not exist.
    Missing,
}

/// The overlay path in a record, if one was written; else where it would go
/// when `needed`.
fn overlay_path(game_root: &Path, applied: &Deployment, needed: bool) -> Result<String> {
    match applied
        .files
        .iter()
        .find(|(_, r)| r.original.is_none())
        .map(|(p, _)| p.clone())
    {
        Some(path) => Ok(path),
        None if needed => overlay_name(game_root),
        None => Ok(String::new()),
    }
}

/// Status of every file change: pending, applied or in conflict.
pub fn status(
    game: &GameInstance,
    applied: &Deployment,
    changes: &BTreeMap<FileKey, FileChange>,
) -> Result<Vec<(FileKey, ChangeState)>> {
    let root = game.root();
    let needed = changes.keys().any(|k| matches!(k, FileKey::Archive(_)));
    let overlay = overlay_path(root, applied, needed)?;
    let mut out = Vec::new();
    for (key, change) in changes {
        let rel = target_path(key, &overlay);
        let record = applied.files.get(&rel);
        let state = match record.map(|_| file_state(root, &rel, record)).transpose()? {
            Some(FileState::Written)
                if record.and_then(|r| r.changes.get(key)) == Some(&change.payload.sha256) =>
            {
                ChangeState::Applied
            }
            Some(FileState::Changed) => ChangeState::Conflict,
            _ => ChangeState::Pending,
        };
        out.push((key.clone(), state));
    }
    Ok(out)
}

/// Writes `changes` into the game (or, with no changes, puts every file back).
/// `owner` is written into the overlay's marker. Saves `applied` to
/// `record_path` around every write.
pub fn sync(
    game: &GameInstance,
    applied: &mut Deployment,
    record_path: &Path,
    changes: &BTreeMap<FileKey, FileChange>,
    read: &dyn Fn(&FileChange) -> Result<Vec<u8>>,
    owner: &str,
    force: bool,
) -> Result<ApplyReport> {
    let root = game.root();
    let mut report = ApplyReport::default();
    let needed = changes.keys().any(|k| matches!(k, FileKey::Archive(_)));
    let overlay = overlay_path(root, applied, needed)?;
    // Wanted contents per target path.
    let mut wanted: BTreeMap<String, Vec<&FileChange>> = BTreeMap::new();
    for change in changes.values() {
        wanted
            .entry(target_path(&change.key, &overlay))
            .or_default()
            .push(change);
    }
    let paths: BTreeSet<String> = wanted.keys().chain(applied.files.keys()).cloned().collect();
    // Check everything before the first write.
    let mut plan = Vec::new();
    for rel in &paths {
        let at_overlay = *rel == overlay;
        let record = applied.files.get(rel);
        // Before the first apply, the overlay's name is the game's first archive.
        let state = if at_overlay && record.is_none() {
            FileState::Original
        } else {
            file_state(root, rel, record)?
        };
        let want = wanted.get(rel);
        let want_map: BTreeMap<FileKey, String> = want
            .into_iter()
            .flatten()
            .map(|c| (c.key.clone(), c.payload.sha256.clone()))
            .collect();
        let up_to_date = match (state, record) {
            (FileState::Written, Some(r)) => !want_map.is_empty() && r.changes == want_map,
            (FileState::Original | FileState::Missing, None) => want_map.is_empty(),
            (FileState::Original, Some(_)) => want_map.is_empty() && !at_overlay,
            _ => false,
        };
        if up_to_date {
            continue;
        }
        if state == FileState::Changed && !force {
            return Err(Error::InvalidArgument(format!(
                "{rel} was changed outside the toolkit; use force to overwrite it"
            )));
        }
        if at_overlay && record.is_none() && is_overlay(&root.join(rel)) {
            return Err(Error::InvalidArgument(format!(
                "{rel} is an overlay archive of another owner (the mod manager?)"
            )));
        }
        if !at_overlay && state == FileState::Missing {
            return Err(Error::InvalidArgument(format!(
                "{rel} is not a file of the game"
            )));
        }
        plan.push((
            rel.clone(),
            want_map,
            want.cloned().unwrap_or_default(),
            state,
        ));
    }
    if plan.iter().any(|(_, m, _, _)| !m.is_empty()) {
        // The workspace and the mod manager do not write at the same time.
        if owner == MODS_OWNER {
            crate::mods::ensure_workspace_not_in_game(game)?;
        } else {
            crate::mods::ensure_mods_not_in_game(game)?;
        }
    }
    for (rel, want_map, want, state) in plan {
        let path = root.join(&rel);
        let at_overlay = rel == overlay;
        if want.is_empty() {
            // Put the original back.
            if at_overlay {
                if is_overlay(&path) {
                    std::fs::remove_file(&path).map_err(|e| Error::io(&path, e))?;
                }
                let shifted = applied.files[&rel].shifted.clone();
                undo_renames(root, &shifted)?;
                report
                    .written
                    .extend(shifted.into_iter().map(|(from, _)| from));
            } else if state != FileState::Original {
                let backup = root.join(BACKUP_DIR).join(&rel);
                if !backup.is_file() {
                    return Err(Error::InvalidArgument(format!(
                        "no backup of {rel} to restore"
                    )));
                }
                crate::atomic::copy(&backup, &path)?;
                report.written.push(rel.clone());
            }
            applied.files.remove(&rel);
            applied.save(record_path)?;
            report.streams.push((rel, StreamAction::Restored));
            continue;
        }
        let data = if at_overlay {
            let mut entries = Vec::with_capacity(want.len());
            for change in &want {
                entries.push((&change.key, read(change)?));
            }
            build_overlay(&entries, owner)?
        } else {
            read(want[0])?
        };
        let previous = applied.files.get(&rel).cloned();
        let original = match &previous {
            Some(r) => r.original.clone(),
            None if at_overlay => None,
            None => {
                let backup = root.join(BACKUP_DIR).join(&rel);
                if !backup.exists() {
                    if let Some(parent) = backup.parent() {
                        std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
                    }
                    crate::atomic::copy(&path, &backup)?;
                    report.backed_up.push(rel.clone());
                }
                Some(FileStamp::of_file(&backup)?)
            }
        };
        let shifted = match &previous {
            Some(r) => r.shifted.clone(),
            None if at_overlay => shift_plan(root)?,
            None => Vec::new(),
        };
        let in_flight = FileStamp::of_data(&data);
        applied.files.insert(
            rel.clone(),
            AppliedFile {
                original: original.clone(),
                written: previous
                    .as_ref()
                    .map_or_else(|| in_flight.clone(), |r| r.written.clone()),
                in_flight: Some(in_flight),
                changes: previous.map(|r| r.changes).unwrap_or_default(),
                shifted: shifted.clone(),
            },
        );
        applied.save(record_path)?;
        if at_overlay {
            do_renames(root, &shifted)?;
        }
        crate::atomic::write(&path, &data)?;
        applied.files.insert(
            rel.clone(),
            AppliedFile {
                original,
                written: FileStamp::of_written(&path, &data),
                in_flight: None,
                changes: want_map.clone(),
                shifted,
            },
        );
        applied.save(record_path)?;
        report.written.push(rel.clone());
        report
            .streams
            .push((rel, StreamAction::Rebuilt(want_map.len())));
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_round_trip() {
        let key: FileKey = "viv:AudioStreams/e_screen/Title_Screen_01.exa.snu"
            .parse()
            .unwrap();
        assert_eq!(
            key,
            FileKey::Archive("audiostreams\\e_screen\\title_screen_01.exa.snu".into())
        );
        assert_eq!(key.to_string().parse::<FileKey>().unwrap(), key);
        let loose: FileKey = "file:movies\\common\\mainmenu.vp6".parse().unwrap();
        assert_eq!(loose, FileKey::Loose("movies/common/mainmenu.vp6".into()));
        assert!("movies/x".parse::<FileKey>().is_err());
        assert!("file:../x".parse::<FileKey>().is_err());
        let json = serde_json::to_string(&loose).unwrap();
        assert_eq!(json, "\"file:movies/common/mainmenu.vp6\"");
        assert_eq!(serde_json::from_str::<FileKey>(&json).unwrap(), loose);
    }

    #[test]
    fn archive_numbers() {
        assert_eq!(archive_number(Path::new("/g/GODFATH12.viv")), Some(12));
        assert_eq!(archive_number(Path::new("/g/other.viv")), None);
    }
}

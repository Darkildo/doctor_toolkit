//! Integrity of the game files: which streams and tables of contents differ
//! from their originals, and who changed them.
//!
//! The originals come from three places, in this order:
//!
//! 1. the backups of this toolkit, `_doctor_toolkit/backups/`;
//! 2. the backups of the old closed toolkit, `_toolkit/backups/`;
//! 3. a reference list of hashes, `_doctor_toolkit/reference.json`, made with
//!    [`save_reference`] while the game is known to be unmodified.
//!
//! A file with none of these cannot be verified. A file that differs from
//! its backup is attributed to the mod manager or the workspace when their
//! records say they wrote it, to the old toolkit when its backup is the
//! original, and to another program otherwise.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::game::{GameFile, GameInstance};
use crate::workspace::{Deployment, FileStamp, Workspace};
use crate::{Error, Result};

/// Directory of the old closed toolkit inside the game directory.
pub const OLD_TOOLKIT_DIR: &str = "_toolkit";
/// Its backups of the files it patched.
pub const OLD_BACKUP_DIR: &str = "_toolkit/backups";
/// The reference list of original hashes, relative to the game directory.
pub const REFERENCE_FILE: &str = "_doctor_toolkit/reference.json";
const REFERENCE_FORMAT: &str = "doctor-toolkit-reference";

/// Path of the old toolkit's backup of `file`, whether it exists or not.
pub fn old_backup_path(game: &GameInstance, file: &GameFile) -> PathBuf {
    game.root().join(OLD_BACKUP_DIR).join(&file.rel_path)
}

/// The best known unmodified contents of `file`: this toolkit's backup, else
/// the old toolkit's backup, else the file itself.
pub fn pristine_path(game: &GameInstance, file: &GameFile) -> PathBuf {
    let ours = game.backup_path(file);
    if ours.is_file() {
        return ours;
    }
    let old = old_backup_path(game, file);
    if old.is_file() {
        old
    } else {
        file.path.clone()
    }
}

/// What the old toolkit left in the game directory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OldToolkit {
    pub dir: PathBuf,
    /// Installed patches in the order the old toolkit applied them
    /// (`_toolkit/game_instance.xml`).
    pub installed_patches: Vec<String>,
    /// Patch files present in `_toolkit/installed_patches/`, in that order.
    pub patch_files: Vec<PathBuf>,
    /// Relative paths of the game files it backed up.
    pub backups: Vec<String>,
}

/// Looks for the old toolkit's directory in the game directory.
pub fn detect_old_toolkit(root: &Path) -> Option<OldToolkit> {
    let dir = root.join(OLD_TOOLKIT_DIR);
    if !dir.is_dir() {
        return None;
    }
    let installed_patches = fs::read_to_string(dir.join("game_instance.xml"))
        .map(|xml| installed_patch_names(&xml))
        .unwrap_or_default();
    let patch_dir = dir.join("installed_patches");
    let mut patch_files: Vec<PathBuf> = installed_patches
        .iter()
        .map(|name| patch_dir.join(name))
        .filter(|p| p.is_file())
        .collect();
    // Patches present but not listed go last, sorted by name.
    if let Ok(entries) = fs::read_dir(&patch_dir) {
        let mut extra: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("vpat"))
                    && !patch_files.contains(p)
            })
            .collect();
        extra.sort();
        patch_files.extend(extra);
    }
    let mut backups = Vec::new();
    collect_files(&dir.join("backups"), "", &mut backups);
    backups.sort();
    Some(OldToolkit {
        dir,
        installed_patches,
        patch_files,
        backups,
    })
}

/// Names inside `<installed_patches><patch>…</patch></installed_patches>`.
fn installed_patch_names(xml: &str) -> Vec<String> {
    let section = match (
        xml.find("<installed_patches>"),
        xml.find("</installed_patches>"),
    ) {
        (Some(start), Some(end)) if start < end => &xml[start..end],
        _ => return Vec::new(),
    };
    let mut names = Vec::new();
    let mut rest = section;
    while let Some(start) = rest.find("<patch>") {
        rest = &rest[start + "<patch>".len()..];
        let Some(end) = rest.find("</patch>") else {
            break;
        };
        let name = rest[..end]
            .trim()
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&amp;", "&");
        // Only plain file names: the list must not point outside the patch directory.
        if !name.is_empty() && !name.contains(['/', '\\']) && name != ".." {
            names.push(name);
        }
        rest = &rest[end..];
    }
    names
}

fn collect_files(dir: &Path, rel: &str, out: &mut Vec<String>) {
    collect_files_to_depth(dir, rel, out, 16);
}

fn collect_files_to_depth(dir: &Path, rel: &str, out: &mut Vec<String>, depth: usize) {
    if depth == 0 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = if rel.is_empty() {
            name
        } else {
            format!("{rel}/{name}")
        };
        // Follows symbolic links: a shadow copy of the game is made of them.
        let Ok(metadata) = fs::metadata(entry.path()) else {
            continue;
        };
        if metadata.is_dir() {
            // Bounded depth: a link to a parent directory must not loop forever.
            collect_files_to_depth(&entry.path(), &rel, out, depth - 1);
        } else if metadata.is_file() {
            out.push(rel);
        }
    }
}

/// `_doctor_toolkit/reference.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reference {
    pub format: String,
    pub version: u32,
    /// Seconds since the Unix epoch.
    pub created: u64,
    /// Size and SHA-256 of every original stream and table of contents, by relative path.
    pub files: BTreeMap<String, FileStamp>,
}

impl Reference {
    pub fn load(root: &Path) -> Result<Option<Self>> {
        let path = root.join(REFERENCE_FILE);
        if !path.is_file() {
            return Ok(None);
        }
        let text = fs::read(&path).map_err(|e| Error::io(&path, e))?;
        let reference: Self = serde_json::from_slice(&text)
            .map_err(|e| Error::mod_error(format!("{}: {e}", path.display())))?;
        if reference.format != REFERENCE_FORMAT || reference.version != 1 {
            return Err(Error::mod_error(format!(
                "{} is not a reference list this program reads",
                path.display()
            )));
        }
        Ok(Some(reference))
    }
}

/// Hashes the original of every stream and table of contents (see
/// [`pristine_path`]) and saves the list to [`REFERENCE_FILE`]. Do this while
/// the game is unmodified; later checks compare the files with it.
/// `progress(done, total)` is called from worker threads.
pub fn save_reference(
    game: &GameInstance,
    progress: impl Fn(usize, usize) + Sync,
) -> Result<usize> {
    let files: Vec<&GameFile> = game.streams().iter().chain(game.tocs()).collect();
    let done = AtomicUsize::new(0);
    let stamps: Vec<(String, FileStamp)> = files
        .par_iter()
        .map(|file| {
            let mut stamp = FileStamp::of_file(&pristine_path(game, file))?;
            stamp.mtime_ns = None;
            stamp.file_id = None;
            progress(done.fetch_add(1, Ordering::Relaxed) + 1, files.len());
            Ok((file.rel_path.clone(), stamp))
        })
        .collect::<Result<_>>()?;
    let reference = Reference {
        format: REFERENCE_FORMAT.to_owned(),
        version: 1,
        created: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
        files: stamps.into_iter().collect(),
    };
    let path = game.root().join(REFERENCE_FILE);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
    }
    let mut text = serde_json::to_vec_pretty(&reference)
        .map_err(|e| Error::mod_error(format!("reference list: {e}")))?;
    text.push(b'\n');
    crate::atomic::write(&path, &text)?;
    Ok(reference.files.len())
}

/// Copies the old toolkit's backups into this toolkit's backup directory
/// where this toolkit has no backup yet, so that its originals become the
/// originals the workspace and the mod manager rebuild from. Game files are
/// not touched. Returns the relative paths adopted.
pub fn adopt_old_backups(game: &GameInstance) -> Result<Vec<String>> {
    let mut adopted = Vec::new();
    for file in game.streams().iter().chain(game.tocs()) {
        let old = old_backup_path(game, file);
        let ours = game.backup_path(file);
        if !old.is_file() || ours.exists() {
            continue;
        }
        if let Some(parent) = ours.parent() {
            fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
        crate::atomic::copy(&old, &ours)?;
        adopted.push(file.rel_path.clone());
    }
    Ok(adopted)
}

/// Who changed a game file, or whether it is original.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Condition {
    /// Equal to its backup or to the reference list.
    Original,
    /// Written by the mod manager.
    Mods,
    /// Written by the editor's workspace.
    Workspace,
    /// Changed by the old toolkit: differs from the original it backed up.
    OldToolkit,
    /// Differs from the original in this toolkit's backup, and neither the mod
    /// manager nor the workspace wrote it.
    OtherProgram,
    /// Differs from the reference list.
    DiffersFromReference,
    /// No backup and no reference: cannot be checked.
    Unverified,
}

impl Condition {
    pub fn describe(self) -> &'static str {
        match self {
            Self::Original => "original",
            Self::Mods => "changed by the mod manager",
            Self::Workspace => "changed by the editor's workspace",
            Self::OldToolkit => "changed by the old toolkit",
            Self::OtherProgram => "changed by another program",
            Self::DiffersFromReference => "differs from the reference list",
            Self::Unverified => "not verified",
        }
    }

    /// Whether the file is changed.
    pub fn is_changed(self) -> bool {
        !matches!(self, Self::Original | Self::Unverified)
    }
}

/// One checked file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileReport {
    pub rel_path: String,
    pub condition: Condition,
}

/// Result of [`check`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IntegrityReport {
    /// Every stream and table of contents, sorted by path.
    pub files: Vec<FileReport>,
    pub old_toolkit: Option<OldToolkit>,
    /// Whether a reference list was used.
    pub reference: bool,
    /// Leftovers of interrupted writes (harmless; the game ignores them).
    pub temp_files: Vec<String>,
}

impl IntegrityReport {
    pub fn count(&self, condition: Condition) -> usize {
        self.files
            .iter()
            .filter(|f| f.condition == condition)
            .count()
    }

    /// Files that are changed, with who changed them.
    pub fn changed(&self) -> impl Iterator<Item = &FileReport> {
        self.files.iter().filter(|f| f.condition.is_changed())
    }
}

/// Options for [`check`].
#[derive(Debug, Clone, Copy, Default)]
pub struct IntegrityOptions {
    /// Hash the files that have no backup and compare them with the reference
    /// list (slow: reads the whole game).
    pub full: bool,
}

/// Checks the streams and tables of contents of the game against their
/// originals. `progress(done, total)` is called from worker threads.
pub fn check(
    game: &GameInstance,
    options: IntegrityOptions,
    progress: impl Fn(usize, usize) + Sync,
) -> Result<IntegrityReport> {
    let root = game.root();
    let reference = Reference::load(root)?;
    let mods = Deployment::load(&super::manager::applied_path(root))?;
    let workspace = Deployment::load(&Workspace::default_dir(root).join("applied.json"))?;
    let mods_held = mods.held_streams(game)?;
    let workspace_held = workspace.held_streams(game)?;

    let files: Vec<(&GameFile, bool)> = game
        .streams()
        .iter()
        .map(|f| (f, true))
        .chain(game.tocs().iter().map(|f| (f, false)))
        .collect();
    let done = AtomicUsize::new(0);
    let mut reports: Vec<FileReport> = files
        .par_iter()
        .map(|&(file, is_stream)| {
            let condition = condition_of(
                game,
                file,
                is_stream,
                reference.as_ref(),
                options,
                (&mods_held, &workspace_held),
            )?;
            progress(done.fetch_add(1, Ordering::Relaxed) + 1, files.len());
            Ok(FileReport {
                rel_path: file.rel_path.clone(),
                condition,
            })
        })
        .collect::<Result<_>>()?;
    reports.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));

    let mut all = Vec::new();
    collect_files(root, "", &mut all);
    let temp_files = all
        .into_iter()
        .filter(|p| p.ends_with(crate::atomic::TEMP_SUFFIX))
        .collect();
    Ok(IntegrityReport {
        files: reports,
        old_toolkit: detect_old_toolkit(root),
        reference: reference.is_some(),
        temp_files,
    })
}

fn condition_of(
    game: &GameInstance,
    file: &GameFile,
    is_stream: bool,
    reference: Option<&Reference>,
    options: IntegrityOptions,
    (mods_held, workspace_held): (&[String], &[String]),
) -> Result<Condition> {
    let ours = game.backup_path(file);
    let old = old_backup_path(game, file);
    if ours.is_file() {
        let backup = FileStamp::of_file(&ours)?;
        if backup.matches_file(&file.path)? {
            return Ok(Condition::Original);
        }
        let name = file.stream_name();
        let condition = if is_stream {
            if mods_held.contains(&name) {
                Condition::Mods
            } else if workspace_held.contains(&name) {
                Condition::Workspace
            } else if old.is_file() && FileStamp::of_file(&old)?.same_contents(&backup) {
                Condition::OldToolkit
            } else {
                Condition::OtherProgram
            }
        } else if !mods_held.is_empty() {
            // Tables of contents carry the stream sizes the owner wrote.
            Condition::Mods
        } else if !workspace_held.is_empty() {
            Condition::Workspace
        } else {
            Condition::OtherProgram
        };
        return Ok(condition);
    }
    if is_stream {
        // A stream created by the mod manager or the workspace has no backup:
        // its original is that the file does not exist.
        let name = file.stream_name();
        if mods_held.contains(&name) {
            return Ok(Condition::Mods);
        }
        if workspace_held.contains(&name) {
            return Ok(Condition::Workspace);
        }
    }
    if old.is_file() {
        return Ok(if FileStamp::of_file(&old)?.matches_file(&file.path)? {
            Condition::Original
        } else {
            Condition::OldToolkit
        });
    }
    match reference.and_then(|r| r.files.get(&file.rel_path)) {
        Some(stamp) if options.full => Ok(if stamp.matches_file(&file.path)? {
            Condition::Original
        } else {
            Condition::DiffersFromReference
        }),
        Some(stamp) => {
            // Without hashing: a different size is still a certain difference.
            let size = fs::metadata(&file.path)
                .map_err(|e| Error::io(&file.path, e))?
                .len();
            Ok(if size == stamp.size {
                Condition::Unverified
            } else {
                Condition::DiffersFromReference
            })
        }
        None => Ok(Condition::Unverified),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_installed_patches() {
        let xml = r#"<instance id="1" name="g" type="2">
    <installed_patches>
        <patch>Beta Weapon Icons.vpat</patch>
        <patch> A &amp; B.vpat </patch>
        <patch>../evil.vpat</patch>
    </installed_patches>
    <my_patches/>
</instance>"#;
        assert_eq!(
            installed_patch_names(xml),
            ["Beta Weapon Icons.vpat", "A & B.vpat"]
        );
        assert!(installed_patch_names("<x/>").is_empty());
    }
}

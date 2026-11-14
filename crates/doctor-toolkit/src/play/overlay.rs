//! Playing a workspace or the enabled mods without writing into the game
//! folder (stage 46).
//!
//! [`Overlay::build`] makes a **staged copy** of the game in a folder of the
//! toolkit: real directories, and for every file a symbolic link to the
//! game's file. The workspace (or the mod manager) is applied to that copy
//! with the same code that writes into the game ([`crate::workspace`]): its
//! writes replace links by files of their own (`atomic::write` renames over
//! the link, never writing through it), and the overlay archive of `.viv`
//! entries renames the links of the archives one number up. What differs
//! from the game afterwards — files that are no longer links, links that
//! point at another file of the game — becomes the module's list of
//! redirects ([`doctor_toolkit_live::files`]): when the game opens one of
//! those paths of its folder, the module gives it the staged file. The
//! game's own files are never written, so there is nothing to put back; the
//! staged copy is removed after the game exits.
//!
//! The module and its configuration still go into `scripts/` of the game
//! for the launch (as with `play --live`) and are taken away afterwards.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use doctor_toolkit_live::files::{Redirect, line_safe};

use crate::game::GameInstance;
use crate::mods::ModManager;
use crate::mods::manager::ModApplyOptions;
use crate::workspace::{ApplyOptions, ApplyReport, Workspace};
use crate::{Error, Result};

/// Whether "Play" writes into the game or lets the live module redirect the
/// files ([`Overlay`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlayMode {
    /// Through the module when it can be used ([`availability`]), else
    /// written into the game and put back afterwards.
    #[default]
    Auto,
    /// Only through the module: refused when it cannot be used.
    On,
    /// Written into the game, as before stage 46.
    Off,
}

impl OverlayMode {
    pub fn is_default(&self) -> bool {
        *self == Self::Auto
    }

    /// `auto`, `on`, `off`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::On => "on",
            Self::Off => "off",
        }
    }
}

/// What the staged copy gets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The workspace in this folder.
    Workspace(PathBuf),
    /// The enabled mods of the game's mod manager.
    Mods,
}

/// Folders of the toolkit in the game folder: mirrored for the workspace
/// and the mod manager, never redirected (the game does not read them).
const TOOLKIT_DIRS: &[&str] = &["_doctor_toolkit", "_toolkit"];
/// The state of the live module's installation: not mirrored.
const LIVE_STATE: &str = "_doctor_toolkit/live";

/// Why the module cannot redirect the files of this game, if it cannot: it
/// is not built, the game has no ASI loader, or the executable is not the
/// build the module knows (it would install nothing and the game would run
/// without the mod).
pub fn availability(root: &Path) -> std::result::Result<(), String> {
    if crate::live::module_binary().is_none() {
        return Err("the live module is not built (tools/build-live-module.sh)".into());
    }
    if crate::live::loader::find(root).is_none() {
        return Err("no ASI loader (dinput8.dll) in the game folder".into());
    }
    let exe = crate::play::find_executable(root)
        .ok_or_else(|| "no godfather2.exe in the game folder".to_owned())?;
    let data = std::fs::read(&exe).map_err(|e| format!("{}: {e}", exe.display()))?;
    doctor_toolkit_live::edition::check_file(&data).map_err(|e| e.to_string())?;
    if !cfg!(windows) && !wine_sees_root() {
        return Err("Wine's drive Z: is not the root of the file system".into());
    }
    Ok(())
}

/// Under Wine the module opens the staged files as `Z:\…`: the default
/// prefix maps `Z:` to `/`. Checked when `WINEPREFIX` names a prefix (the
/// launch methods without one use a prefix of their own, where it is the
/// default too).
fn wine_sees_root() -> bool {
    let Some(prefix) = std::env::var_os("WINEPREFIX") else {
        return true;
    };
    match std::fs::read_link(Path::new(&prefix).join("dosdevices/z:")) {
        Ok(target) => target == Path::new("/"),
        // No such link: Wine makes it when the prefix is first used.
        Err(_) => true,
    }
}

/// How a launch brings the changes into the game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    /// Through the live module ([`Overlay`]).
    Overlay,
    /// Written into the game (and put back afterwards); with `Auto`, why
    /// the module could not be used.
    Write(Option<String>),
}

/// Chooses for the game in `root`: [`OverlayMode::On`] fails when the
/// module cannot be used, [`OverlayMode::Auto`] falls back to writing.
pub fn choose(root: &Path, mode: OverlayMode) -> Result<Choice> {
    match mode {
        OverlayMode::Off => Ok(Choice::Write(None)),
        OverlayMode::On => match availability(root) {
            Ok(()) => Ok(Choice::Overlay),
            Err(why) => Err(Error::InvalidArgument(format!(
                "cannot play without writing into the game: {why}"
            ))),
        },
        OverlayMode::Auto => Ok(match availability(root) {
            Ok(()) => Choice::Overlay,
            Err(why) => Choice::Write(Some(why)),
        }),
    }
}

/// Removes the staged copy of an earlier launch of the game in `root`
/// ([`default_dir`]) that was not removed (the toolkit stopped, or `live
/// install` made it). Returns whether there was one.
pub fn remove_stale(root: &Path) -> Result<bool> {
    let Some(dir) = default_dir(root) else {
        return Ok(false);
    };
    let found = std::fs::symlink_metadata(&dir).is_ok();
    remove_dir(&dir)?;
    Ok(found)
}

/// The folder of the staged copy of the game in `root`:
/// `<cache dir>/doctor-toolkit/overlay-<crc32 of the path>` (the cache dir
/// as for the navigation index: `$DOCTOR_TOOLKIT_CACHE_DIR`,
/// `$XDG_CACHE_HOME`, `~/.cache` or `%LOCALAPPDATA%`), outside the game.
pub fn default_dir(root: &Path) -> Option<PathBuf> {
    let index = crate::navigation::index::default_cache_file(root)?;
    let name = index
        .file_name()?
        .to_string_lossy()
        .replace("index-", "overlay-");
    Some(index.with_file_name(name.trim_end_matches(".bin")))
}

/// A path of this computer as the game sees it: the path itself on
/// Windows, `Z:\…` under Wine.
pub fn game_path(path: &Path) -> Option<String> {
    let text = path.to_str()?;
    if cfg!(windows) {
        return Some(text.to_owned());
    }
    text.starts_with('/')
        .then(|| format!("Z:{}", text.replace('/', "\\")))
}

/// The staged copy of a game for one launch, and the redirects that give
/// the game its changed files.
#[derive(Debug)]
pub struct Overlay {
    dir: PathBuf,
    redirects: Vec<Redirect>,
    report: ApplyReport,
    removed: bool,
}

impl Overlay {
    /// Stages `source` for the game in `game` into `dir` (made anew) and
    /// adds `cmd_txt` (the game's launch options) as the game's `cmd.txt`.
    pub fn build(
        game: &GameInstance,
        source: Option<&Source>,
        cmd_txt: Option<&str>,
        dir: &Path,
    ) -> Result<Self> {
        let root = std::path::absolute(game.root()).map_err(|e| Error::io(game.root(), e))?;
        remove_dir(dir)?;
        let staged_root = dir.join("game");
        std::fs::create_dir_all(&staged_root).map_err(|e| Error::io(&staged_root, e))?;
        let mut overlay = Self {
            dir: dir.to_path_buf(),
            redirects: Vec::new(),
            report: ApplyReport::default(),
            removed: false,
        };
        mirror(&root, &staged_root, "")?;
        if let Some(source) = source {
            let staged = GameInstance::open(&staged_root)?;
            overlay.report = match source {
                Source::Workspace(ws_dir) => {
                    let ws_dir = std::path::absolute(ws_dir).map_err(|e| Error::io(ws_dir, e))?;
                    let staged_ws = match ws_dir.strip_prefix(&root) {
                        Ok(rel) => staged_root.join(rel),
                        Err(_) => {
                            let to = dir.join("workspace");
                            std::fs::create_dir_all(&to).map_err(|e| Error::io(&to, e))?;
                            mirror(&ws_dir, &to, "")?;
                            to
                        }
                    };
                    if Workspace::exists(&staged_ws) {
                        Workspace::open_read_only(&staged_ws)?
                            .apply(&staged, ApplyOptions::default())?
                    } else {
                        ApplyReport::default()
                    }
                }
                Source::Mods => {
                    ModManager::open(&staged)?.apply(&staged, ModApplyOptions::default())?
                }
            };
        }
        let mut redirects = Vec::new();
        collect(&root, &staged_root, "", &mut redirects)?;
        if let Some(text) = cmd_txt {
            let path = dir.join(crate::play::start::CMD_FILE);
            crate::atomic::write(&path, text.as_bytes())?;
            redirects.push(redirect_to(crate::play::start::CMD_FILE, &path)?);
        }
        overlay.redirects = redirects;
        Ok(overlay)
    }

    /// The redirects for the module's configuration.
    pub fn redirects(&self) -> &[Redirect] {
        &self.redirects
    }

    /// What the workspace or the mod manager wrote into the staged copy.
    pub fn report(&self) -> &ApplyReport {
        &self.report
    }

    /// The folder of the staged copy.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Keeps the staged copy after this value is gone (`live install`:
    /// `live uninstall` removes it, [`remove_stale`]).
    pub fn keep(mut self) {
        self.removed = true;
    }

    /// Removes the staged copy (after the game exited).
    pub fn remove(mut self) -> Result<()> {
        self.removed = true;
        remove_dir(&self.dir)
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        if !self.removed {
            let _ = remove_dir(&self.dir);
        }
    }
}

/// Removes a staged copy: its links, never what they point to.
fn remove_dir(dir: &Path) -> Result<()> {
    match std::fs::symlink_metadata(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::io(dir, e)),
        // `remove_dir_all` removes symbolic links themselves.
        Ok(_) => std::fs::remove_dir_all(dir).map_err(|e| Error::io(dir, e)),
    }
}

fn link(target: &Path, at: &Path) -> std::io::Result<()> {
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

/// Mirrors the folder `from` into `to`: real directories, a symbolic link
/// for every file.
fn mirror(from: &Path, to: &Path, rel: &str) -> Result<()> {
    let entries = std::fs::read_dir(from).map_err(|e| Error::io(from, e))?;
    for entry in entries {
        let entry = entry.map_err(|e| Error::io(from, e))?;
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        let rel_path = if rel.is_empty() {
            name_str.to_owned()
        } else {
            format!("{rel}/{name_str}")
        };
        if rel_path == LIVE_STATE {
            continue;
        }
        let path = entry.path();
        let target = to.join(&name);
        // Follows links: a linked folder of the game is a folder here.
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            std::fs::create_dir_all(&target).map_err(|e| Error::io(&target, e))?;
            mirror(&path, &target, &rel_path)?;
        } else {
            link(&path, &target).map_err(|e| Error::io(&target, e))?;
        }
    }
    Ok(())
}

/// A redirect of the game path `rel` to a file outside the game.
fn redirect_to(rel: &str, file: &Path) -> Result<Redirect> {
    let to = game_path(file).ok_or_else(|| {
        Error::InvalidArgument(format!("{}: not a path the game can open", file.display()))
    })?;
    redirect(rel, to)
}

fn redirect(rel: &str, to: String) -> Result<Redirect> {
    let from = doctor_toolkit_live::files::key(rel)
        .ok_or_else(|| Error::InvalidArgument(format!("{rel}: not a path in the game folder")))?;
    if !line_safe(&from) || !line_safe(&to) {
        return Err(Error::InvalidArgument(format!(
            "{rel} -> {to}: the module's configuration cannot hold `#`, `|` or control characters"
        )));
    }
    Ok(Redirect { from, to })
}

/// The redirects of the staged copy `staged` of the game `root`: files that
/// are no longer links to the same file of the game.
fn collect(root: &Path, staged: &Path, rel: &str, out: &mut Vec<Redirect>) -> Result<()> {
    let entries = std::fs::read_dir(staged).map_err(|e| Error::io(staged, e))?;
    let mut names: BTreeSet<String> = BTreeSet::new();
    for entry in entries {
        let entry = entry.map_err(|e| Error::io(staged, e))?;
        if let Some(name) = entry.file_name().to_str() {
            names.insert(name.to_owned());
        }
    }
    for name in names {
        let rel_path = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        if rel.is_empty() && TOOLKIT_DIRS.contains(&name.as_str()) {
            continue;
        }
        let path = staged.join(&name);
        let meta = std::fs::symlink_metadata(&path).map_err(|e| Error::io(&path, e))?;
        if meta.is_dir() {
            collect(root, &path, &rel_path, out)?;
        } else if meta.file_type().is_symlink() {
            let target = std::fs::read_link(&path).map_err(|e| Error::io(&path, e))?;
            if target == root.join(&rel_path) {
                continue;
            }
            // Another file of the game under this name (the archives one
            // number up), or something outside it.
            match target.strip_prefix(root) {
                Ok(other) => {
                    let other = other.to_string_lossy().replace('\\', "/");
                    out.push(redirect(&rel_path, other)?);
                }
                Err(_) => out.push(redirect_to(&rel_path, &target)?),
            }
        } else {
            out.push(redirect_to(&rel_path, &path)?);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;

//! Who changes the mods of a game: one program at a time (stage 107).
//!
//! The mod manager of a game is shown by several programs at once: Open Mod
//! Manager, the editor's Mods window, a command line. Since a project is
//! put into the game as a mod, the editor also changes the mods on its own
//! (Install into Mods) and reads them for every launch (Play). Each of
//! them keeps the list of mods in memory and writes all of it back, so two
//! writing at the same moment — or one writing a list it read before the
//! other changed it — would lose an installed mod or an answer.
//!
//! [`RegistryLock`] is the lock of the mod manager's folder
//! (`<game>/_doctor_toolkit/mods/lock`), made like the lock of a workspace
//! ([`crate::workspace::WorkspaceLock`]): an advisory lock of the operating
//! system that goes with the process, with the holder named next to it in
//! `lock.holder`. Unlike a workspace, nobody holds it for as long as a
//! window is open: [`super::ModManager`] takes it for the time of one
//! change (an installation, a write into the game, a change of the order)
//! and first reads the mods again when another program changed them
//! meanwhile. A launch that stages the mods outside the game
//! ([`crate::play::overlay`]) holds it while it reads them.

use std::path::Path;

use crate::workspace::LockHolder;
use crate::workspace::lock::FolderLock;
use crate::{Error, Result};

/// The lock of the mod manager of one game, held until dropped.
#[derive(Debug)]
pub struct RegistryLock(FolderLock);

impl RegistryLock {
    /// Takes the lock of the mod manager folder `dir`
    /// ([`super::mods_dir`]; made when it does not exist yet) for
    /// `program`. Fails at once, naming the holder, when another program —
    /// or another handle of this one — holds it.
    pub fn acquire(dir: &Path, program: &str) -> Result<Self> {
        FolderLock::acquire(dir, program, has_registry, |who| {
            format!(
                "the mods in {} are being changed by {who}; wait until it has finished and try \
                 again",
                dir.display()
            )
        })
        .map(Self)
        .map_err(|error| match error {
            Error::InvalidArgument(text) => Error::mod_error(text),
            other => other,
        })
    }

    /// The mod manager folder.
    pub fn dir(&self) -> &Path {
        self.0.dir()
    }

    /// Who holds the lock of the mod manager folder `dir`, when someone does.
    pub fn holder(dir: &Path) -> Option<LockHolder> {
        FolderLock::holder(dir)
    }
}

/// A folder the lock made is taken away again unless a registry was written.
fn has_registry(dir: &Path) -> bool {
    dir.join(super::manager::REGISTRY_FILE).is_file()
}

/// The name the running program holds the lock under: the name of its
/// executable (`open-mod-manager`, `doctor-toolkit-gui`).
pub fn program_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_stem()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "another program".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_program_changes_the_mods_at_a_time() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("_doctor_toolkit/mods");
        assert_eq!(RegistryLock::holder(&dir), None);
        let lock = RegistryLock::acquire(&dir, "the editor").unwrap();
        assert_eq!(RegistryLock::holder(&dir).unwrap().program, "the editor");
        let error = RegistryLock::acquire(&dir, "open-mod-manager")
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("are being changed by the editor (process"),
            "{error}"
        );
        drop(lock);
        // Nothing was installed: the folder made for the lock is gone.
        assert!(!temp.path().join("_doctor_toolkit").exists());
        // With a registry in it, the folder and the lock file stay.
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("registry.json"), b"{}").unwrap();
        drop(RegistryLock::acquire(&dir, "x").unwrap());
        assert!(dir.join("lock").is_file());
        assert!(!dir.join("lock.holder").exists());
        assert_eq!(RegistryLock::holder(&dir), None);
    }
}

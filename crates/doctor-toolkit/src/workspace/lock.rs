//! Who writes a workspace: one program at a time.
//!
//! The editor keeps its workspace in memory while it is open and writes it
//! back after every edit; a command line or an AI client (the MCP server,
//! `docs/mcp.md`) writing the same workspace meanwhile would make the
//! editor's copy stale. The revision of the manifest catches that late
//! ("changed by another program; reopen it"); this lock says it early and
//! names the holder.
//!
//! The lock is an advisory lock of the operating system on `<workspace>/lock`
//! ([`std::fs::File::try_lock`]): it goes with the process, also when the
//! process is killed. Who holds it is written next to it, in `lock.holder`
//! (a lock on Windows keeps other processes from reading the locked file).
//! The lock file of a workspace is not deleted: deleting it while another
//! process opens it could let two processes lock two different files. Only
//! a lock that made the folder of a workspace not written yet takes the
//! folder away again (a command that records nothing leaves nothing
//! behind); two programs creating the same new workspace at once are still
//! caught when the workspace is written ([`super::Workspace`]).

use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::{Error, Result};

/// Name of the lock file in the workspace directory.
pub const LOCK_FILE: &str = "lock";
/// Name of the file that says who holds the lock.
pub const HOLDER_FILE: &str = "lock.holder";

/// Who holds the lock of a workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockHolder {
    /// The program, e.g. `the editor`, `the MCP server`.
    pub program: String,
    pub pid: u32,
}

impl std::fmt::Display for LockHolder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (process {})", self.program, self.pid)
    }
}

/// The lock of a workspace, held until dropped.
#[derive(Debug)]
pub struct WorkspaceLock {
    file: File,
    dir: PathBuf,
    /// The folders this lock made, the workspace's first: taken away on
    /// drop when no workspace was written in them.
    made: Vec<PathBuf>,
}

impl WorkspaceLock {
    /// Takes the lock of the workspace in `dir` (the directory is made when
    /// it does not exist yet) for `program`. Fails at once, naming the
    /// holder, when another program — or another handle of this one —
    /// holds it.
    pub fn acquire(dir: &Path, program: &str) -> Result<Self> {
        let made: Vec<PathBuf> = dir
            .ancestors()
            .take_while(|d| !d.as_os_str().is_empty() && !d.exists())
            .map(Path::to_path_buf)
            .collect();
        fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
        let path = dir.join(LOCK_FILE);
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|e| Error::io(&path, e))?;
        match file.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(Error::InvalidArgument(busy_message(dir)));
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(Error::io(&path, e)),
        }
        let holder = dir.join(HOLDER_FILE);
        let text = format!("{}\n{}\n", std::process::id(), program);
        // Best effort: the lock holds without it, only the message is poorer.
        let _ = File::create(&holder).and_then(|mut f| f.write_all(text.as_bytes()));
        Ok(Self {
            file,
            dir: dir.to_path_buf(),
            made,
        })
    }

    /// The workspace directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Who holds the lock of the workspace in `dir`, when someone does.
    pub fn holder(dir: &Path) -> Option<LockHolder> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(dir.join(LOCK_FILE))
            .ok()?;
        match file.try_lock() {
            Ok(()) => {
                let _ = file.unlock();
                None
            }
            Err(_) => Some(read_holder(dir).unwrap_or(LockHolder {
                program: "another program".into(),
                pid: 0,
            })),
        }
    }
}

impl Drop for WorkspaceLock {
    fn drop(&mut self) {
        // The holder file goes first: once unlocked, the next holder writes its own.
        let _ = fs::remove_file(self.dir.join(HOLDER_FILE));
        let _ = self.file.unlock();
        if !self.made.is_empty() && !super::Workspace::exists(&self.dir) {
            let _ = fs::remove_file(self.dir.join(LOCK_FILE));
            // Only empty folders go.
            for d in &self.made {
                if fs::remove_dir(d).is_err() {
                    break;
                }
            }
        }
    }
}

fn read_holder(dir: &Path) -> Option<LockHolder> {
    let text = fs::read_to_string(dir.join(HOLDER_FILE)).ok()?;
    let mut lines = text.lines();
    let pid = lines.next()?.trim().parse().ok()?;
    let program = lines.next()?.trim().to_owned();
    Some(LockHolder { program, pid })
}

fn busy_message(dir: &Path) -> String {
    let who = read_holder(dir).map_or_else(|| "another program".to_owned(), |h| h.to_string());
    format!(
        "the workspace in {} is open for writing in {who}; close it there (or finish its \
         command) and try again",
        dir.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_holder_at_a_time_and_the_next_after_it() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        assert_eq!(WorkspaceLock::holder(&ws), None);
        let lock = WorkspaceLock::acquire(&ws, "the editor").unwrap();
        let holder = WorkspaceLock::holder(&ws).unwrap();
        assert_eq!(holder.program, "the editor");
        assert_eq!(holder.pid, std::process::id());
        // A second handle, even of the same process, is refused with the holder named.
        let error = WorkspaceLock::acquire(&ws, "the MCP server")
            .unwrap_err()
            .to_string();
        assert!(error.contains("the editor (process"), "{error}");
        drop(lock);
        assert_eq!(WorkspaceLock::holder(&ws), None);
        let again = WorkspaceLock::acquire(&ws, "the MCP server").unwrap();
        assert_eq!(
            WorkspaceLock::holder(&ws).unwrap().program,
            "the MCP server"
        );
        drop(again);
        // The folder it made for a workspace never written goes again.
        assert!(!ws.exists());
        let nested = dir.path().join("a/b/ws");
        drop(WorkspaceLock::acquire(&nested, "x").unwrap());
        assert!(!dir.path().join("a").exists());
        // With a workspace in it, the folder and the lock file stay.
        std::fs::create_dir_all(&ws).unwrap();
        let lock = WorkspaceLock::acquire(&ws, "x").unwrap();
        drop(lock);
        assert!(ws.join(LOCK_FILE).is_file());
    }
}

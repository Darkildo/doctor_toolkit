//! Crash-safe file replacement.
//!
//! A file is replaced by writing a temporary file in the same directory,
//! flushing it to disk and renaming it over the target. A reader, or the game,
//! sees either the old or the new contents, never a torn file: an interrupted
//! write leaves at most a stray temporary file next to the target.
//!
//! The rename replaces the *path*. When the target is a symbolic link (a
//! shadow copy of the game made of links to the real files), the link itself
//! is replaced by a regular file and the file it pointed to stays untouched.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::{Error, Result};

/// Suffix of temporary files. They never end in `.str` or `.toc`, so a game
/// scan ignores leftovers of an interrupted write.
pub const TEMP_SUFFIX: &str = ".doctor-toolkit-tmp";

/// Replaces `path` with `contents`.
pub fn write(path: &Path, contents: &[u8]) -> Result<()> {
    write_with(path, |file| file.write_all(contents))
}

/// Replaces `to` with a copy of `from`. `from` may be a symbolic link; its
/// target is read.
pub fn copy(from: &Path, to: &Path) -> Result<()> {
    let mut source = File::open(from).map_err(|e| Error::io(from, e))?;
    write_with(to, |file| std::io::copy(&mut source, file).map(drop))
}

/// Replaces `path` with whatever `fill` writes into the temporary file.
///
/// When `fill` fails, the temporary file is removed and `path` is left as it was.
pub fn write_with(path: &Path, fill: impl FnOnce(&mut File) -> std::io::Result<()>) -> Result<()> {
    let temp = temp_path(path)?;
    // A leftover of an earlier interrupted write; removing a link removes only the link.
    if fs::symlink_metadata(&temp).is_ok() {
        fs::remove_file(&temp).map_err(|e| Error::io(&temp, e))?;
    }
    // `create_new` refuses to follow a symbolic link planted at the temporary path.
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|e| Error::io(&temp, e))?;
    let result = (|| {
        fill(&mut file)?;
        file.sync_all()?;
        // Keep the permissions of the file being replaced (of a link's target).
        if let Ok(metadata) = fs::metadata(path) {
            file.set_permissions(metadata.permissions())?;
        }
        Ok(())
    })();
    drop(file);
    if let Err(error) = result {
        let _ = fs::remove_file(&temp);
        return Err(Error::io(&temp, error));
    }
    if let Err(error) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(Error::io(path, error));
    }
    sync_dir(path.parent().unwrap_or(Path::new(".")));
    Ok(())
}

/// The temporary file used to replace `path`: same directory, hidden, with the
/// process id so that two processes never share it.
pub fn temp_path(path: &Path) -> Result<PathBuf> {
    let name = path
        .file_name()
        .ok_or_else(|| Error::InvalidArgument(format!("{} has no file name", path.display())))?;
    let mut temp = std::ffi::OsString::from(".");
    temp.push(name);
    temp.push(format!(".{}{TEMP_SUFFIX}", std::process::id()));
    Ok(path.with_file_name(temp))
}

/// Flushes a directory entry change (the rename) to disk. Best effort: not
/// every platform or file system supports syncing a directory.
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(dir) = File::open(dir) {
        let _ = dir.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_contents_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.str");
        write(&path, b"one").unwrap();
        write(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["a.str"]);
    }

    #[test]
    fn failed_write_keeps_the_original() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.str");
        fs::write(&path, b"original").unwrap();
        let result = write_with(&path, |file| {
            file.write_all(b"half of the new")?;
            Err(std::io::Error::other("interrupted"))
        });
        assert!(result.is_err());
        assert_eq!(fs::read(&path).unwrap(), b"original");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn leftover_temp_file_is_ignored_and_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.str");
        fs::write(&path, b"original").unwrap();
        // A crash after writing the temporary file but before the rename.
        fs::write(temp_path(&path).unwrap(), b"torn").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"original");
        write(&path, b"new").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn replaces_a_symlink_instead_of_writing_through_it() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.str");
        let link = dir.path().join("link.str");
        fs::write(&real, b"game file").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();

        write(&link, b"edited").unwrap();
        assert_eq!(fs::read(&real).unwrap(), b"game file");
        assert_eq!(fs::read(&link).unwrap(), b"edited");
        assert!(
            !fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );

        // Copying from a link reads its target.
        let other = dir.path().join("other.str");
        std::os::unix::fs::symlink(&real, &other).unwrap();
        copy(&other, &dir.path().join("copy.str")).unwrap();
        assert_eq!(fs::read(dir.path().join("copy.str")).unwrap(), b"game file");
        copy(&link, &other).unwrap();
        assert_eq!(fs::read(&real).unwrap(), b"game file");
        assert_eq!(fs::read(&other).unwrap(), b"edited");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_at_the_temp_path_is_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim");
        fs::write(&victim, b"keep").unwrap();
        let path = dir.path().join("a.str");
        std::os::unix::fs::symlink(&victim, temp_path(&path).unwrap()).unwrap();
        write(&path, b"new").unwrap();
        assert_eq!(fs::read(&victim).unwrap(), b"keep");
        assert_eq!(fs::read(&path).unwrap(), b"new");
    }
}

//! Noticing that a working file was saved.
//!
//! Editors save in different ways: GIMP and Krita write a new file and
//! rename it over the old one, text editors often write in place, sometimes
//! in several pieces, and some save without changes. [`ChangeDetector`]
//! decides when to import, from events and the clock only (tests drive it
//! with simulated saves); [`FileWatcher`] feeds it with file system events
//! (`notify`) and checks the file's size and time as a fallback.
//!
//! - A save is imported after the file has been quiet for
//!   [`ChangeDetector::quiet`], so a burst of writes gives one import.
//! - Content equal to what was exported or imported last is not imported.
//! - A missing file (between an editor's delete and rename) is waited for.
//! - A file cut short by a write in pieces fails to import; the next write
//!   is noticed again and imported, so nothing has to be done by hand.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant, SystemTime};

use sha2::{Digest, Sha256};

use crate::{Error, Result};

/// How long a file must stay unchanged before it is read.
pub const DEFAULT_QUIET: Duration = Duration::from_millis(400);

/// A missing file is waited for this long before it counts as deleted.
pub const MISSING_GRACE: Duration = Duration::from_secs(5);

/// What [`ChangeDetector::poll`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Poll {
    /// Nothing happened since the last poll.
    Idle,
    /// The file changed recently; poll again after this long.
    Waiting(Duration),
    /// The file was saved with the same content as before.
    Unchanged,
    /// The file was saved with new content.
    Changed(Vec<u8>),
    /// The file is gone (deleted and not replaced).
    Missing,
}

/// Size and modification time of a file, to notice writes without events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    pub len: u64,
    pub modified: Option<SystemTime>,
}

impl Stamp {
    /// The stamp of `path`; `None` when the file does not exist.
    pub fn of(path: &Path) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        Some(Self {
            len: meta.len(),
            modified: meta.modified().ok(),
        })
    }
}

/// SHA-256 of `bytes`.
pub fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// Decides when a watched file holds a new save. Pure: the caller reports
/// events and stamps and reads the file when asked.
#[derive(Debug, Clone)]
pub struct ChangeDetector {
    /// Digest of the content exported or imported last.
    known: [u8; 32],
    /// When the last change was noticed; `None` when there is nothing to check.
    dirty_since: Option<Instant>,
    /// When the file was first found missing since it was last seen.
    missing_since: Option<Instant>,
    stamp: Option<Stamp>,
    pub quiet: Duration,
}

impl ChangeDetector {
    /// A detector for a file that now holds `content`, with stamp `stamp`.
    pub fn new(content: &[u8], stamp: Option<Stamp>) -> Self {
        Self {
            known: digest(content),
            dirty_since: None,
            missing_since: None,
            stamp,
            quiet: DEFAULT_QUIET,
        }
    }

    /// An event for the file arrived at `now`.
    pub fn notice(&mut self, now: Instant) {
        self.dirty_since = Some(now);
    }

    /// The current stamp of the file; a different one counts as an event.
    pub fn observe(&mut self, now: Instant, stamp: Option<Stamp>) {
        if stamp != self.stamp {
            self.stamp = stamp;
            self.notice(now);
        }
    }

    /// Whether a change waits to be checked.
    pub fn is_dirty(&self) -> bool {
        self.dirty_since.is_some()
    }

    /// Checks the file when it has been quiet long enough. `read` returns its
    /// content, or `None` when it does not exist.
    pub fn poll(
        &mut self,
        now: Instant,
        read: impl FnOnce() -> std::io::Result<Option<Vec<u8>>>,
    ) -> Result<Poll> {
        let Some(since) = self.dirty_since else {
            return Ok(Poll::Idle);
        };
        let quiet_for = now.saturating_duration_since(since);
        if quiet_for < self.quiet {
            return Ok(Poll::Waiting(self.quiet - quiet_for));
        }
        let content = read().map_err(|e| Error::io("working file", e))?;
        let Some(content) = content else {
            // Between the delete and the rename of an editor's save.
            let first = *self.missing_since.get_or_insert(now);
            if now.saturating_duration_since(first) < MISSING_GRACE {
                return Ok(Poll::Waiting(self.quiet));
            }
            self.dirty_since = None;
            return Ok(Poll::Missing);
        };
        self.missing_since = None;
        self.dirty_since = None;
        let hash = digest(&content);
        if hash == self.known {
            return Ok(Poll::Unchanged);
        }
        self.known = hash;
        Ok(Poll::Changed(content))
    }

    /// The file now holds `content` (written by the link itself).
    pub fn reset(&mut self, content: &[u8], stamp: Option<Stamp>) {
        self.known = digest(content);
        self.stamp = stamp;
        self.dirty_since = None;
        self.missing_since = None;
    }
}

/// Watches one file with file system events and a check of its stamp.
///
/// The folder of the file is watched, not the file, so an editor that saves
/// by renaming a new file over it is still seen.
pub struct FileWatcher {
    path: PathBuf,
    detector: ChangeDetector,
    events: Option<(notify::RecommendedWatcher, Receiver<()>)>,
    /// Stamp of the file after the watcher's own last write: events that
    /// arrive late for that write are not a save.
    written: Option<Stamp>,
    /// Why events are not available; the stamp check still works.
    pub event_error: Option<String>,
}

impl FileWatcher {
    /// Watches `path`, which now holds `content`. `wake` is called from the
    /// watcher thread on every event for the file (a GUI repaints then).
    pub fn new(
        path: &Path,
        content: &[u8],
        wake: Option<Box<dyn Fn() + Send + Sync>>,
    ) -> Result<Self> {
        let path = path.to_owned();
        let dir = path
            .parent()
            .ok_or_else(|| Error::InvalidArgument(format!("{} has no folder", path.display())))?
            .to_owned();
        let name = path.file_name().map(ToOwned::to_owned);
        let (tx, rx) = channel();
        let handler = move |event: notify::Result<notify::Event>| {
            let Ok(event) = event else { return };
            if event.kind.is_access() {
                return;
            }
            let ours = event.paths.is_empty()
                || event
                    .paths
                    .iter()
                    .any(|p| p.file_name().map(ToOwned::to_owned) == name);
            if ours {
                let _ = tx.send(());
                if let Some(wake) = &wake {
                    wake();
                }
            }
        };
        let (events, event_error) = match start(&dir, handler) {
            Ok(watcher) => (Some((watcher, rx)), None),
            Err(error) => (None, Some(error)),
        };
        Ok(Self {
            detector: ChangeDetector::new(content, Stamp::of(&path)),
            path,
            events,
            written: None,
            event_error,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn detector(&self) -> &ChangeDetector {
        &self.detector
    }

    pub fn detector_mut(&mut self) -> &mut ChangeDetector {
        &mut self.detector
    }

    /// Takes the events that arrived and checks the file. Call it regularly
    /// (a few times a second); [`Poll::Waiting`] says when to call again.
    pub fn poll(&mut self, now: Instant) -> Result<Poll> {
        if let Some((_, rx)) = &self.events {
            let mut any = false;
            while rx.try_recv().is_ok() {
                any = true;
            }
            let stamp = Stamp::of(&self.path);
            if stamp.is_some() && stamp == self.written {
                // Still the file this watcher wrote: its events came late.
                any = false;
            } else {
                self.written = None;
            }
            if any {
                self.detector.notice(now);
            }
        }
        self.detector.observe(now, Stamp::of(&self.path));
        let path = &self.path;
        self.detector.poll(now, || match std::fs::read(path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        })
    }

    /// Writes `content` to the file and takes it as the known content.
    pub fn rewrite(&mut self, content: &[u8]) -> Result<()> {
        std::fs::write(&self.path, content).map_err(|e| Error::io(&self.path, e))?;
        // Our own write is not a save.
        if let Some((_, rx)) = &self.events {
            while rx.try_recv().is_ok() {}
        }
        self.written = Stamp::of(&self.path);
        self.detector.reset(content, self.written);
        Ok(())
    }

    /// Whether file system events are delivered.
    pub fn has_events(&self) -> bool {
        self.events.is_some()
    }
}

fn start(
    dir: &Path,
    handler: impl notify::EventHandler,
) -> std::result::Result<notify::RecommendedWatcher, String> {
    use notify::Watcher as _;
    let mut watcher = notify::recommended_watcher(handler).map_err(|e| e.to_string())?;
    watcher
        .watch(dir, notify::RecursiveMode::NonRecursive)
        .map_err(|e| e.to_string())?;
    Ok(watcher)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    fn read(content: &[u8]) -> impl FnOnce() -> std::io::Result<Option<Vec<u8>>> + '_ {
        move || Ok(Some(content.to_vec()))
    }

    #[test]
    fn a_burst_of_writes_is_one_change() {
        let t0 = Instant::now();
        let mut d = ChangeDetector::new(b"v1", None);
        assert_eq!(d.poll(t0, read(b"x")).unwrap(), Poll::Idle);
        d.notice(t0);
        d.notice(t0 + 100 * MS);
        d.notice(t0 + 200 * MS);
        // Not quiet long enough after the last write: not read.
        let poll = d.poll(t0 + 300 * MS, || panic!("read too early")).unwrap();
        assert_eq!(poll, Poll::Waiting(300 * MS));
        assert_eq!(
            d.poll(t0 + 600 * MS, read(b"v2")).unwrap(),
            Poll::Changed(b"v2".to_vec())
        );
        assert_eq!(d.poll(t0 + 700 * MS, read(b"v2")).unwrap(), Poll::Idle);
    }

    #[test]
    fn saving_the_same_content_is_not_a_change() {
        let t0 = Instant::now();
        let mut d = ChangeDetector::new(b"same", None);
        d.notice(t0);
        assert_eq!(
            d.poll(t0 + 500 * MS, read(b"same")).unwrap(),
            Poll::Unchanged
        );
        // Back to an earlier content after a change is a change.
        d.notice(t0 + MS * 600);
        assert_eq!(
            d.poll(t0 + 1100 * MS, read(b"new")).unwrap(),
            Poll::Changed(b"new".to_vec())
        );
        d.notice(t0 + MS * 1200);
        assert_eq!(
            d.poll(t0 + 1700 * MS, read(b"same")).unwrap(),
            Poll::Changed(b"same".to_vec())
        );
    }

    #[test]
    fn a_missing_file_is_waited_for() {
        let t0 = Instant::now();
        let mut d = ChangeDetector::new(b"v1", None);
        d.notice(t0);
        let gone = || Ok(None);
        assert!(matches!(
            d.poll(t0 + 500 * MS, gone).unwrap(),
            Poll::Waiting(_)
        ));
        // The rename arrives.
        d.notice(t0 + 600 * MS);
        assert_eq!(
            d.poll(t0 + 1100 * MS, read(b"v2")).unwrap(),
            Poll::Changed(b"v2".to_vec())
        );
        // Deleted for good.
        d.notice(t0 + 2000 * MS);
        assert!(matches!(
            d.poll(t0 + 2500 * MS, gone).unwrap(),
            Poll::Waiting(_)
        ));
        assert_eq!(
            d.poll(t0 + 2500 * MS + MISSING_GRACE, gone).unwrap(),
            Poll::Missing
        );
    }

    #[test]
    fn a_new_stamp_counts_as_an_event() {
        let t0 = Instant::now();
        let stamp = |len| {
            Some(Stamp {
                len,
                modified: None,
            })
        };
        let mut d = ChangeDetector::new(b"v1", stamp(2));
        d.observe(t0, stamp(2));
        assert!(!d.is_dirty());
        d.observe(t0, stamp(5));
        assert!(d.is_dirty());
        assert_eq!(
            d.poll(t0 + 500 * MS, read(b"v2222")).unwrap(),
            Poll::Changed(b"v2222".to_vec())
        );
    }
}

//! A resource linked to a working file: export, watch, import on save.

use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::{Duration, Instant};

use super::watch::{FileWatcher, Poll};
use super::{Imported, ModelContext, ModelOptions, Target, export, file_name, import, sanitize};
use crate::export::gltf::AssetSource;
use crate::game::GameInstance;
use crate::resource::names::type_tags;
use crate::workspace::Workspace;
use crate::{Error, Result};

/// Saves closer together than this replace each other's operation, so a
/// session of edits is undone with one step.
pub const DEFAULT_COALESCE: Duration = Duration::from_secs(60);

/// What happened to a link at a check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkEvent {
    /// A save was imported. `merged`: it replaced the operation of the
    /// previous save instead of adding one.
    Imported { imported: Imported, merged: bool },
    /// The file was saved without changes; nothing imported.
    Unchanged,
    /// The save could not be imported (a half-written file, a syntax error);
    /// the link keeps watching and the next save is tried again.
    Failed(String),
    /// The working file was deleted.
    Missing,
}

/// A resource being edited in another program.
pub struct EditLink {
    pub target: Target,
    /// For models: replace the model or make a new model set.
    pub options: ModelOptions,
    watcher: FileWatcher,
    /// Sequence number and time of the operation the last import recorded.
    last: Option<(u64, Instant)>,
    /// Saves within this time of the previous import are merged with it.
    pub coalesce: Duration,
    /// Number of saves imported.
    pub imports: usize,
    /// The last thing that happened, for a status line.
    pub last_event: Option<(Instant, LinkEvent)>,
    editor: Option<Child>,
}

impl EditLink {
    /// Exports `target` into a working file in `dir` (created when missing)
    /// and starts watching it. `wake` is called from another thread when
    /// the file changes.
    pub fn create(
        game: &GameInstance,
        ws: Option<&Workspace>,
        target: Target,
        dir: &Path,
        models: Option<ModelContext<'_>>,
        wake: Option<Box<dyn Fn() + Send + Sync>>,
    ) -> Result<Self> {
        let content = export(
            game,
            ws,
            &target,
            models.map(|m| m.assets as &mut dyn AssetSource),
        )?;
        std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
        let path = dir.join(file_name(game, ws, &target));
        crate::atomic::write(&path, &content)?;
        let watcher = FileWatcher::new(&path, &content, wake)?;
        Ok(Self {
            target,
            options: ModelOptions::default(),
            watcher,
            last: None,
            coalesce: DEFAULT_COALESCE,
            imports: 0,
            last_event: None,
            editor: None,
        })
    }

    /// A folder for the working file of `target` below the system's
    /// temporary folder, one per process and resource.
    pub fn default_dir(target: &Target) -> PathBuf {
        let stream = sanitize(&target.key.stream.replace('/', "_")).unwrap_or_default();
        let name = format!(
            "{stream}_{}_{:08x}",
            type_tags().display(target.type_id),
            target.key.ids[3]
        );
        std::env::temp_dir()
            .join("doctor-toolkit-edit")
            .join(std::process::id().to_string())
            .join(name)
    }

    /// The working file.
    pub fn path(&self) -> &Path {
        self.watcher.path()
    }

    /// Why file system events are not delivered; changes are then noticed
    /// by checking the file's time and size.
    pub fn event_error(&self) -> Option<&str> {
        self.watcher.event_error.as_deref()
    }

    /// The quiet time before a changed file is read.
    pub fn set_quiet(&mut self, quiet: Duration) {
        self.watcher.detector_mut().quiet = quiet;
    }

    /// Opens the working file with `command` (empty: the desktop's program).
    pub fn open_editor(&mut self, command: &str) -> Result<()> {
        self.editor = Some(super::launch(command, self.path())?);
        Ok(())
    }

    /// Whether the program started by [`Self::open_editor`] still runs;
    /// `None` when none was started. A desktop opener (`xdg-open`) exits at
    /// once, so this says little for it.
    pub fn editor_running(&mut self) -> Option<bool> {
        let child = self.editor.as_mut()?;
        Some(matches!(child.try_wait(), Ok(None)))
    }

    /// Checks the file; [`Poll::Changed`] carries a save to import with
    /// [`Self::import`].
    pub fn poll(&mut self, now: Instant) -> Result<Poll> {
        self.watcher.poll(now)
    }

    /// Whether a change is waiting for the file to be quiet.
    pub fn is_pending(&self) -> bool {
        self.watcher.detector().is_dirty()
    }

    /// Imports a save. When the operation of the previous save is still the
    /// last one in the workspace and came less than [`Self::coalesce`] ago
    /// (always, for a new model set), it is undone first, so the edits of
    /// a session make one operation (and a new model set is made once).
    pub fn import(
        &mut self,
        game: &GameInstance,
        ws: &mut Workspace,
        bytes: &[u8],
        models: Option<ModelContext<'_>>,
        now: Instant,
    ) -> LinkEvent {
        let event = match self.try_import(game, ws, bytes, models, now) {
            Ok((imported, merged)) => {
                self.imports += 1;
                LinkEvent::Imported { imported, merged }
            }
            Err(error) => LinkEvent::Failed(error.to_string()),
        };
        self.last_event = Some((now, event.clone()));
        event
    }

    fn try_import(
        &mut self,
        game: &GameInstance,
        ws: &mut Workspace,
        bytes: &[u8],
        models: Option<ModelContext<'_>>,
        now: Instant,
    ) -> Result<(Imported, bool)> {
        let head_seq = ws.head().checked_sub(1).map(|i| ws.operations()[i].seq);
        let new_set = self.options.new_name.is_some();
        let merge = self.last.is_some_and(|(seq, at)| {
            Some(seq) == head_seq && (new_set || now.saturating_duration_since(at) <= self.coalesce)
        });
        if new_set && self.last.is_some() && !merge {
            return Err(Error::InvalidArgument(
                "the new model set was made by an earlier save and other edits came \
                 after it; undo them, or link the new set itself"
                    .into(),
            ));
        }
        if merge {
            ws.undo()?;
        }
        match import(game, ws, &self.target, bytes, models, &self.options) {
            Ok(imported) => {
                self.last = imported.seq.map(|seq| (seq, now));
                Ok((imported, merge))
            }
            Err(error) => {
                if merge {
                    ws.redo()?;
                }
                Err(error)
            }
        }
    }

    /// Polls and imports in one step; `None` when there is nothing new.
    pub fn check(
        &mut self,
        game: &GameInstance,
        ws: &mut Workspace,
        models: Option<ModelContext<'_>>,
        now: Instant,
    ) -> Option<LinkEvent> {
        let event = match self.poll(now) {
            Ok(Poll::Idle | Poll::Waiting(_)) => return None,
            Ok(Poll::Unchanged) => LinkEvent::Unchanged,
            Ok(Poll::Missing) => LinkEvent::Missing,
            Ok(Poll::Changed(bytes)) => return Some(self.import(game, ws, &bytes, models, now)),
            Err(error) => LinkEvent::Failed(error.to_string()),
        };
        self.last_event = Some((now, event.clone()));
        Some(event)
    }

    /// Writes the workspace version of the resource into the working file
    /// again (after an undo, say). The editor has to load it again.
    pub fn refresh(
        &mut self,
        game: &GameInstance,
        ws: Option<&Workspace>,
        models: Option<ModelContext<'_>>,
    ) -> Result<()> {
        let content = export(
            game,
            ws,
            &self.target,
            models.map(|m| m.assets as &mut dyn AssetSource),
        )?;
        self.watcher.rewrite(&content)
    }
}

impl Drop for EditLink {
    fn drop(&mut self) {
        // The editor keeps running; only the child handle is reaped.
        if let Some(child) = &mut self.editor {
            let _ = child.try_wait();
        }
    }
}

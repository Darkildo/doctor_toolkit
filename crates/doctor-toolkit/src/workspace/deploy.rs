//! Writing a set of resource changes into the game, and recognising what the
//! game files hold.
//!
//! Both the workspace and the mod manager write into the game this way. Each
//! keeps its own [`Deployment`] record (`applied.json`): for every stream it
//! rewrote, the original file, the written file and which payload of which
//! resource is in it. A stream is always rebuilt from its original with the
//! complete set of changes, so the result does not depend on what was written
//! before.
//!
//! Changes may add resources ([`Change::added`]) and create streams
//! ([`NewStream`]). A created stream has no original: its "original state" is
//! that the file does not exist. Its entries in the tables of contents are
//! added when it is written and removed when it is deleted.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{
    ApplyReport, Blob, Change, ChangeState, FileStamp, NewStream, ResourceKey, StreamAction,
    StreamState, StreamStatus, read_json, write_json,
};
use crate::container::ustream::{
    self, Insertion, Replacement, ResourceHeader, Storage, StreamLayout,
};
use crate::game::{GameInstance, GameWrite};
use crate::toc::Toc;
use crate::{Error, Result};

/// `applied.json`: what one owner (a workspace or the mod manager) has
/// written into the game.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deployment {
    pub streams: BTreeMap<String, AppliedStream>,
    /// Files outside the streams (overlay archive, movies), by relative path.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub files: BTreeMap<String, super::files::AppliedFile>,
}

/// A stream the owner has rewritten or created.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedStream {
    /// Path relative to the game directory.
    pub path: String,
    /// The unmodified file; the stamp of empty data for a created stream.
    pub original: FileStamp,
    /// The file as it was written.
    pub written: FileStamp,
    /// Set while a write is under way: the file about to be written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_flight: Option<FileStamp>,
    /// Payload hash of every replaced or added resource in the written file.
    pub resources: BTreeMap<ResourceKey, String>,
    /// Sum of the decompressed resource sizes of the written file, as the
    /// tables of contents store it. Absent in older records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_size: Option<u64>,
    /// Set when the owner created the file: the stream as created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_stream: Option<NewStream>,
}

/// Comparison of a set of changes with the game files.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeployStatus {
    pub streams: Vec<StreamStatus>,
    pub changes: Vec<(ResourceKey, ChangeState)>,
    /// Resources the game still has changed although the set no longer changes them.
    pub pending_reverts: Vec<ResourceKey>,
}

/// How far [`Deployment::sync_with_progress`] is: `done` of `total`
/// streams are written, `stream` is written next (empty at the end).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncProgress<'a> {
    pub done: usize,
    pub total: usize,
    pub stream: &'a str,
}

/// No new streams.
fn no_streams() -> &'static BTreeMap<String, NewStream> {
    static EMPTY: BTreeMap<String, NewStream> = BTreeMap::new();
    &EMPTY
}

impl Deployment {
    /// Reads the record at `path`; an absent file is an empty record.
    pub fn load(path: &Path) -> Result<Self> {
        if path.is_file() {
            read_json(path)
        } else {
            Ok(Self::default())
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
        write_json(path, self)
    }

    /// State of the game file of stream `name` relative to this record.
    pub fn stream_state(&self, game: &GameInstance, name: &str) -> Result<StreamState> {
        self.state_of(game, name, None)
    }

    /// The created stream `name`: wanted, or written earlier.
    fn created<'a>(&'a self, name: &str, wanted: Option<&'a NewStream>) -> Option<&'a NewStream> {
        wanted.or_else(|| self.streams.get(name).and_then(|r| r.new_stream.as_ref()))
    }

    fn state_of(
        &self,
        game: &GameInstance,
        name: &str,
        wanted_new: Option<&NewStream>,
    ) -> Result<StreamState> {
        let record = self.streams.get(name);
        if let Some(stream) = self.created(name, wanted_new) {
            // "Original" for a created stream means the file does not exist.
            let path = game.root().join(stream.rel_path());
            if !path.exists() {
                return Ok(StreamState::Original);
            }
            let Some(record) = record else {
                return Ok(StreamState::ChangedExternally);
            };
            if record.written.quick_match(&path) {
                return Ok(StreamState::Applied);
            }
            let current = FileStamp::of_file(&path)?;
            let in_flight = record.in_flight.as_ref();
            if current.same_contents(&record.written)
                || in_flight.is_some_and(|s| current.same_contents(s))
            {
                return Ok(StreamState::Applied);
            }
            return Ok(StreamState::ChangedExternally);
        }
        let Some(index) = game.find_stream(name) else {
            return Ok(StreamState::Missing);
        };
        let file = &game.streams()[index];
        if let Some(record) = record {
            if record.written.quick_match(&file.path) {
                return Ok(if record.written.same_contents(&record.original) {
                    StreamState::Original
                } else {
                    StreamState::Applied
                });
            }
            let current = FileStamp::of_file(&file.path)?;
            // The original first: a write interrupted before the rename leaves it in place.
            if current.same_contents(&record.original) {
                return Ok(StreamState::Original);
            }
            let in_flight = record.in_flight.as_ref();
            if current.same_contents(&record.written)
                || in_flight.is_some_and(|s| current.same_contents(s))
            {
                return Ok(StreamState::Applied);
            }
            return Ok(StreamState::ChangedExternally);
        }
        let backup = game.backup_path(file);
        if backup.is_file() && !FileStamp::of_file(&backup)?.matches_file(&file.path)? {
            // Changed by a direct write, by another owner, or by another tool.
            return Ok(StreamState::ChangedExternally);
        }
        Ok(StreamState::Original)
    }

    /// Whether the game file of a stream has exactly the changes `desired`.
    pub fn is_up_to_date(&self, name: &str, state: StreamState, desired: &[&Change]) -> bool {
        self.up_to_date(name, state, desired, None)
    }

    fn up_to_date(
        &self,
        name: &str,
        state: StreamState,
        desired: &[&Change],
        wanted_new: Option<&NewStream>,
    ) -> bool {
        match state {
            StreamState::Original => desired.is_empty() && wanted_new.is_none(),
            // A write that did not finish: the record cannot tell what the
            // file holds, and the tables of contents may not match it.
            StreamState::Applied => self.streams.get(name).is_some_and(|record| {
                record.in_flight.is_none()
                    && record.resources.len() == desired.len()
                    && desired
                        .iter()
                        .all(|c| record.resources.get(&c.key) == Some(&c.payload.sha256))
                    && record.new_stream.as_ref() == wanted_new
            }),
            StreamState::ChangedExternally | StreamState::Missing => {
                desired.is_empty() && wanted_new.is_none()
            }
        }
    }

    /// Streams whose game file is, right now, as this record wrote it with
    /// changes in it: the streams this owner holds in the game.
    pub fn held_streams(&self, game: &GameInstance) -> Result<Vec<String>> {
        let mut held = Vec::new();
        for (name, record) in &self.streams {
            let changed = !record.resources.is_empty()
                || record.new_stream.is_some()
                || record.in_flight.is_some();
            if changed && self.stream_state(game, name)? == StreamState::Applied {
                held.push(name.clone());
            }
        }
        // Files outside the streams (overlay archive, movies) are held while recorded.
        held.extend(self.files.keys().cloned());
        Ok(held)
    }

    /// Compares the changes `desired` (grouped by stream) with the game.
    pub fn status(
        &self,
        game: &GameInstance,
        desired: &BTreeMap<String, Vec<&Change>>,
    ) -> Result<DeployStatus> {
        self.status_with(game, desired, no_streams())
    }

    /// Like [`Self::status`], with streams to create.
    pub fn status_with(
        &self,
        game: &GameInstance,
        desired: &BTreeMap<String, Vec<&Change>>,
        new_streams: &BTreeMap<String, NewStream>,
    ) -> Result<DeployStatus> {
        let names: BTreeSet<&String> = desired
            .keys()
            .chain(self.streams.keys())
            .chain(new_streams.keys())
            .collect();
        let mut status = DeployStatus::default();
        for name in names {
            let wanted_new = new_streams.get(name);
            let state = self.state_of(game, name, wanted_new)?;
            let wanted = desired.get(name).map_or(&[][..], Vec::as_slice);
            let record = self.streams.get(name);
            for change in wanted {
                let applied = state == StreamState::Applied
                    && record.and_then(|r| r.resources.get(&change.key))
                        == Some(&change.payload.sha256);
                let change_state = match state {
                    _ if applied => ChangeState::Applied,
                    StreamState::ChangedExternally => ChangeState::Conflict,
                    _ => ChangeState::Pending,
                };
                status.changes.push((change.key.clone(), change_state));
            }
            if state == StreamState::Applied
                && let Some(record) = record
            {
                status.pending_reverts.extend(
                    record
                        .resources
                        .keys()
                        .filter(|k| !wanted.iter().any(|c| &c.key == *k))
                        .cloned(),
                );
            }
            status.streams.push(StreamStatus {
                stream: name.clone(),
                state,
                up_to_date: self.up_to_date(name, state, wanted, wanted_new),
            });
        }
        Ok(status)
    }

    /// Makes the streams `names` of the game hold exactly the changes
    /// `desired(name)`: each stream that differs is rebuilt from its original
    /// with all of its changes and its size is updated in the tables of
    /// contents; a stream without changes is put back to its original.
    /// `payload` reads the data of a change.
    ///
    /// The whole operation is one [`GameWrite`]: every file is built and
    /// staged before the first game file changes, and when putting them in
    /// place fails, the files already written get their previous contents
    /// back and the record stays as it was. The record is saved to
    /// `record_path` before the files are put in place (with the files about
    /// to be written, see [`Self::interrupted`]) and after.
    ///
    /// Streams changed by someone else are refused unless `force`. Putting
    /// originals back only (no changes, no created streams) does not stop at
    /// a table of contents or an original that cannot be read: the file goes
    /// back and the report warns.
    pub fn sync(
        &mut self,
        record_path: &Path,
        game: &GameInstance,
        names: &BTreeSet<String>,
        force: bool,
        desired: impl Fn(&str) -> Vec<Change>,
        payload: impl Fn(&Change) -> Result<Vec<u8>>,
    ) -> Result<ApplyReport> {
        self.sync_with(
            record_path,
            game,
            names,
            force,
            no_streams(),
            desired,
            payload,
        )
    }

    /// Like [`Self::sync`], with streams to create: a created stream in
    /// `names` that is not in `new_streams` is deleted and leaves the tables
    /// of contents.
    #[allow(clippy::too_many_arguments)]
    pub fn sync_with(
        &mut self,
        record_path: &Path,
        game: &GameInstance,
        names: &BTreeSet<String>,
        force: bool,
        new_streams: &BTreeMap<String, NewStream>,
        desired: impl Fn(&str) -> Vec<Change>,
        payload: impl Fn(&Change) -> Result<Vec<u8>>,
    ) -> Result<ApplyReport> {
        self.sync_with_progress(
            record_path,
            game,
            names,
            force,
            new_streams,
            desired,
            payload,
            &mut |_| {},
        )
    }

    /// Like [`Self::sync_with`], telling `progress` before each stream is
    /// written and once more when all are.
    #[allow(clippy::too_many_arguments)]
    pub fn sync_with_progress(
        &mut self,
        record_path: &Path,
        game: &GameInstance,
        names: &BTreeSet<String>,
        force: bool,
        new_streams: &BTreeMap<String, NewStream>,
        desired: impl Fn(&str) -> Vec<Change>,
        payload: impl Fn(&Change) -> Result<Vec<u8>>,
        progress: &mut dyn FnMut(SyncProgress<'_>),
    ) -> Result<ApplyReport> {
        let mut plan = Vec::new();
        let mut conflicts = Vec::new();
        for name in names {
            let wanted_new = new_streams.get(name);
            let state = self.state_of(game, name, wanted_new)?;
            let wanted = desired(name);
            let refs: Vec<&Change> = wanted.iter().collect();
            if self.up_to_date(name, state, &refs, wanted_new) {
                if state == StreamState::Original && self.streams.contains_key(name) {
                    // Restored by other means (e.g. `restore`): forget the record.
                    plan.push((name.clone(), state, wanted, false));
                }
                continue;
            }
            match state {
                StreamState::Missing => {
                    return Err(Error::InvalidArgument(format!(
                        "stream {name} is not in the game"
                    )));
                }
                StreamState::ChangedExternally if !force => conflicts.push(name.clone()),
                _ => {}
            }
            plan.push((name.clone(), state, wanted, true));
        }
        if !conflicts.is_empty() {
            return Err(Error::mod_error(format!(
                "changed by another program: {}; apply with force to overwrite (the other \
                 program's changes to these files are lost)",
                conflicts.join(", ")
            )));
        }
        // Tables of contents that list a created stream, before or after.
        let mut stream_tocs: BTreeSet<String> = BTreeSet::new();
        for (name, _, _, _) in &plan {
            for stream in [
                new_streams.get(name),
                self.streams.get(name).and_then(|r| r.new_stream.as_ref()),
            ]
            .into_iter()
            .flatten()
            {
                stream_tocs.extend(stream.tocs.iter().map(|t| t.to_ascii_lowercase()));
            }
        }
        // Putting originals back only: a table of contents or an original
        // that cannot be read must not stop it.
        let restoring = new_streams.is_empty() && plan.iter().all(|(_, _, w, _)| w.is_empty());
        let mut report = ApplyReport::default();
        let total = plan.iter().filter(|(_, _, _, write)| *write).count();
        let mut done = 0;
        // Everything that can fail happens before the first game file
        // changes: each new file is built and staged next to its target.
        let mut write = game.begin_write();
        let mut prepared = Vec::with_capacity(plan.len());
        let mut sizes: Vec<(String, u64)> = Vec::new();
        for (name, state, wanted, write_it) in plan {
            if !write_it {
                // Restored by other means (e.g. `restore`): forget the record.
                prepared.push(Prepared::forget(name));
                continue;
            }
            progress(SyncProgress {
                done,
                total,
                stream: &name,
            });
            done += 1;
            let created = self.created(&name, new_streams.get(&name)).cloned();
            prepared.push(match created {
                Some(stream) => self.prepare_new_stream(
                    game,
                    &stream,
                    new_streams.contains_key(&name),
                    &wanted,
                    &payload,
                    &mut write,
                )?,
                None => self.prepare_stream(
                    game,
                    &name,
                    state,
                    &wanted,
                    &payload,
                    restoring,
                    &mut write,
                    &mut sizes,
                    &mut report,
                )?,
            });
        }
        // The records once everything is written: the entries of created
        // streams in the tables of contents come from them.
        let mut after = self.streams.clone();
        for p in &prepared {
            match &p.after {
                Some(record) => after.insert(p.name.clone(), record.clone()),
                None => after.remove(&p.name),
            };
        }
        prepare_tocs(
            game,
            &sizes,
            &stream_tocs,
            &after,
            restoring,
            &mut write,
            &mut report,
        )?;

        // Note the files about to be written, so an interruption is recognised.
        let before = self.clone();
        if !write.is_empty() {
            for p in &prepared {
                if let Some(record) = &p.in_flight {
                    self.streams.insert(p.name.clone(), record.clone());
                }
            }
            if let Err(error) = self.save(record_path) {
                *self = before;
                return Err(error);
            }
        }
        let written = match write.commit() {
            Ok(written) => written,
            Err(failure) => {
                if failure.unchanged {
                    *self = before;
                    self.save(record_path)?;
                }
                return Err(failure.error);
            }
        };
        for p in prepared {
            match p.after {
                Some(mut record) => {
                    record.written = record.written.placed_at(&game.root().join(&record.path));
                    self.streams.insert(p.name.clone(), record);
                }
                None => {
                    self.streams.remove(&p.name);
                }
            }
            if let Some(action) = p.action {
                report.streams.push((p.name, action));
            }
        }
        self.save(record_path)?;
        report.written.extend(written.written);
        report.backed_up.extend(written.backed_up);
        progress(SyncProgress {
            done: total,
            total,
            stream: "",
        });
        Ok(report)
    }

    /// Builds the new file of stream `name` from its original with the
    /// changes `wanted` (the original itself without changes) and stages it.
    /// Its new data size goes to `sizes`.
    #[allow(clippy::too_many_arguments)]
    fn prepare_stream(
        &self,
        game: &GameInstance,
        name: &str,
        state: StreamState,
        wanted: &[Change],
        payload: &impl Fn(&Change) -> Result<Vec<u8>>,
        restoring: bool,
        write: &mut GameWrite<'_>,
        sizes: &mut Vec<(String, u64)>,
        report: &mut ApplyReport,
    ) -> Result<Prepared> {
        let index = game
            .find_stream(name)
            .ok_or_else(|| Error::InvalidArgument(format!("stream {name} is not in the game")))?;
        let file = &game.streams()[index];
        let backup = game.backup_path(file);
        let original_path = match state {
            StreamState::Original => game.original_path(file),
            _ if backup.is_file() => backup,
            _ => {
                return Err(Error::InvalidArgument(format!(
                    "no backup of the original {}; cannot rebuild it",
                    file.rel_path
                )));
            }
        };
        let original = fs::read(&original_path).map_err(|e| Error::io(&original_path, e))?;
        let original_stamp = FileStamp::of_data(&original);

        let (target, action, data_size) = if wanted.is_empty() {
            // The original goes back as it is; only its size in the tables
            // of contents needs it parsed.
            let data_size = match StreamLayout::parse(&original) {
                Ok(layout) => Some(layout.total_data_size()),
                Err(error) if restoring => {
                    report.warnings.push(format!(
                        "{}: {error}; its size in the tables of contents is left as it is",
                        file.rel_path
                    ));
                    None
                }
                Err(error) => return Err(error),
            };
            (original, StreamAction::Restored, data_size)
        } else {
            let original_layout = StreamLayout::parse(&original)?;
            let mut payloads = Vec::with_capacity(wanted.len());
            let mut additions = Vec::new();
            for change in wanted {
                if let Some(added) = &change.added {
                    if change.key.find_in(&original_layout).is_some() {
                        return Err(Error::InvalidArgument(format!(
                            "{} is added, but the stream already has it",
                            change.key
                        )));
                    }
                    let end = original_layout.resources.len();
                    let before = match &added.before {
                        None => end,
                        Some(anchor) => anchor.find_in(&original_layout).unwrap_or_else(|| {
                            report.warnings.push(format!(
                                "{}: {anchor} is not in the stream; added at the end",
                                change.key
                            ));
                            end
                        }),
                    };
                    let data = payload(change)?;
                    let header = added.header(&change.key, data.len() as u64);
                    additions.push((
                        before,
                        added.order,
                        header,
                        data,
                        change.storage.resolve(false),
                    ));
                    continue;
                }
                let index = change.key.find_in(&original_layout).ok_or_else(|| {
                    Error::InvalidArgument(format!("{} is not in the original stream", change.key))
                })?;
                let actual = original_layout.resource_data(&original, index)?;
                if Blob::of(&actual) != change.original {
                    report.warnings.push(format!(
                        "{}: the game's original differs from the one the change was made on",
                        change.key
                    ));
                }
                let storage = change.storage.resolve(original_layout.is_compressed(index));
                payloads.push((index, payload(change)?, storage));
            }
            additions.sort_by_key(|a| (a.0, a.1));
            let replacements: Vec<Replacement> = payloads
                .iter()
                .map(|(index, data, storage)| Replacement {
                    index: *index,
                    data,
                    storage: *storage,
                })
                .collect();
            let insertions: Vec<Insertion> = additions
                .iter()
                .map(|(before, _, header, data, storage)| Insertion {
                    before: *before,
                    header: *header,
                    data,
                    storage: *storage,
                })
                .collect();
            let rebuilt =
                ustream::rebuild_with(&original, &original_layout, &replacements, &insertions)?;
            let data_size = StreamLayout::parse(&rebuilt)?.total_data_size();
            (
                rebuilt,
                StreamAction::Rebuilt(wanted.len()),
                Some(data_size),
            )
        };
        if let Some(size) = data_size {
            sizes.push((file.stream_name(), size));
        }
        let target_stamp = FileStamp::of_data(&target);
        write.replace(file, &target)?;
        drop(target);

        let previous = self.streams.get(name).cloned();
        let in_flight = AppliedStream {
            path: file.rel_path.clone(),
            original: original_stamp.clone(),
            written: previous
                .as_ref()
                .map_or_else(|| original_stamp.clone(), |p| p.written.clone()),
            in_flight: Some(target_stamp.clone()),
            data_size: previous.as_ref().and_then(|p| p.data_size),
            resources: previous.map(|p| p.resources).unwrap_or_default(),
            new_stream: None,
        };
        let after = (!wanted.is_empty()).then(|| AppliedStream {
            path: file.rel_path.clone(),
            original: original_stamp,
            written: target_stamp,
            in_flight: None,
            resources: wanted
                .iter()
                .map(|c| (c.key.clone(), c.payload.sha256.clone()))
                .collect(),
            data_size,
            new_stream: None,
        });
        Ok(Prepared {
            name: name.to_owned(),
            in_flight: Some(in_flight),
            after,
            action: Some(action),
        })
    }

    /// Builds and stages a created stream, or its deletion (`keep` false).
    /// Only added resources can be in it; they are written in their order.
    fn prepare_new_stream(
        &self,
        game: &GameInstance,
        stream: &NewStream,
        keep: bool,
        wanted: &[Change],
        payload: &impl Fn(&Change) -> Result<Vec<u8>>,
        write: &mut GameWrite<'_>,
    ) -> Result<Prepared> {
        let rel_path = stream.rel_path();
        let path = game.root().join(&rel_path);
        let name = stream.name.clone();
        if !keep {
            if fs::symlink_metadata(&path).is_ok() {
                write.delete(&rel_path)?;
            }
            return Ok(Prepared {
                name,
                in_flight: None,
                after: None,
                action: Some(StreamAction::Restored),
            });
        }
        let mut additions: Vec<&Change> = wanted.iter().collect();
        if let Some(change) = additions.iter().find(|c| !c.is_added()) {
            return Err(Error::InvalidArgument(format!(
                "{} replaces a resource of the new stream {name}; only added resources can be in it",
                change.key
            )));
        }
        super::file_order(&mut additions);
        let mut resources: Vec<(ResourceHeader, Vec<u8>, Storage)> = Vec::new();
        for change in &additions {
            let data = payload(change)?;
            let added = change.added.as_ref().expect("checked above");
            resources.push((
                added.header(&change.key, data.len() as u64),
                data,
                change.storage.resolve(false),
            ));
        }
        let borrowed: Vec<(ResourceHeader, &[u8], Storage)> = resources
            .iter()
            .map(|(h, d, s)| (*h, d.as_slice(), *s))
            .collect();
        let target = ustream::build(&borrowed)?;
        let data_size = StreamLayout::parse(&target)?.total_data_size();
        let target_stamp = FileStamp::of_data(&target);
        write.create(&rel_path, &target)?;
        drop(target);
        let empty = FileStamp::of_data(&[]);
        let previous = self.streams.get(&name).cloned();
        let in_flight = AppliedStream {
            path: rel_path.clone(),
            original: empty.clone(),
            written: previous
                .as_ref()
                .map_or_else(|| empty.clone(), |p| p.written.clone()),
            in_flight: Some(target_stamp.clone()),
            data_size: previous.as_ref().and_then(|p| p.data_size),
            resources: previous
                .as_ref()
                .map(|p| p.resources.clone())
                .unwrap_or_default(),
            new_stream: Some(stream.clone()),
        };
        let after = AppliedStream {
            path: rel_path,
            original: empty,
            written: target_stamp,
            in_flight: None,
            resources: wanted
                .iter()
                .map(|c| (c.key.clone(), c.payload.sha256.clone()))
                .collect(),
            data_size: Some(data_size),
            new_stream: Some(stream.clone()),
        };
        Ok(Prepared {
            name,
            in_flight: Some(in_flight),
            after: Some(after),
            action: Some(StreamAction::Rebuilt(wanted.len())),
        })
    }

    /// Streams whose last write did not finish: the record still notes a
    /// file about to be written, and the game file is neither the original
    /// nor the file written before. The tables of contents may not match
    /// such a stream; [`Self::sync`] without changes (putting the originals
    /// back) mends it from the backups.
    pub fn interrupted(&self, game: &GameInstance) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for (name, record) in &self.streams {
            if record.in_flight.is_none() {
                continue;
            }
            let path = game.root().join(&record.path);
            if !path.exists() {
                // A created stream not created yet is as before; a game
                // stream that is gone is not.
                if record.new_stream.is_none() {
                    out.push(name.clone());
                }
                continue;
            }
            if record.original.matches_file(&path)? || record.written.matches_file(&path)? {
                continue;
            }
            out.push(name.clone());
        }
        Ok(out)
    }
}

/// One stream of [`Deployment::sync_with_progress`], staged.
struct Prepared {
    name: String,
    /// The record while the write is under way; `None` keeps the current one.
    in_flight: Option<AppliedStream>,
    /// The record afterwards; `None` forgets the stream.
    after: Option<AppliedStream>,
    /// What is done to the stream, for the report.
    action: Option<StreamAction>,
}

impl Prepared {
    /// A stream already back to its original: only its record goes.
    fn forget(name: String) -> Self {
        Self {
            name,
            in_flight: None,
            after: None,
            action: None,
        }
    }
}

/// Stages the tables of contents that change: the data size of every stream
/// in `sizes` (by stream name), and, for the tables `stream_tocs` (lower-case
/// relative paths), the entries of created streams.
///
/// A table in `stream_tocs` is rebuilt from its original (the backup, or the
/// file when there is none) with the current data size of every stream it
/// lists, plus an entry for every created stream in `records` that names it.
/// Without created streams a table returns to its original structure, byte
/// for byte when the sizes are the original ones.
///
/// When `restoring`, a table that cannot be read is left as it is, with a
/// warning: putting the originals back must not depend on it.
fn prepare_tocs(
    game: &GameInstance,
    sizes: &[(String, u64)],
    stream_tocs: &BTreeSet<String>,
    records: &BTreeMap<String, AppliedStream>,
    restoring: bool,
    write: &mut GameWrite<'_>,
    report: &mut ApplyReport,
) -> Result<()> {
    let mut sizes32 = Vec::with_capacity(sizes.len());
    for (name, size) in sizes {
        let size = u32::try_from(*size)
            .map_err(|_| Error::InvalidArgument(format!("stream {name}: data exceeds 4 GiB")))?;
        sizes32.push((name.as_str(), size));
    }
    for toc_file in game.tocs() {
        let rebuild = stream_tocs.contains(&toc_file.rel_path.to_ascii_lowercase());
        if sizes32.is_empty() && !rebuild {
            continue;
        }
        let read = |path: &Path| -> Result<(Vec<u8>, Toc)> {
            let bytes = fs::read(path).map_err(|e| Error::io(path, e))?;
            let table = Toc::parse(&bytes).map_err(|e| {
                Error::mod_error(format!("table of contents {}: {e}", toc_file.rel_path))
            })?;
            Ok((bytes, table))
        };
        let (current_bytes, mut table) = match read(&toc_file.path) {
            Ok(read) => read,
            Err(error) if restoring => {
                report.warnings.push(format!("{error}; left as it is"));
                continue;
            }
            Err(error) => return Err(error),
        };
        let mut changed = false;
        for &(name, size) in &sizes32 {
            if let Some(index) = table.find_file(name)
                && table.files[index].data_size != size
            {
                table.files[index].data_size = size;
                changed = true;
            }
        }
        if rebuild {
            let original = match read(&game.original_path(toc_file)) {
                Ok((_, original)) => Some(original),
                Err(error) if restoring => {
                    report
                        .warnings
                        .push(format!("{error}; its created streams are left in it"));
                    None
                }
                Err(error) => return Err(error),
            };
            if let Some(mut original) = original {
                // Keep the sizes written since, by anyone.
                for index in 0..original.files.len() {
                    let name = original.file_name(index).name_lossy();
                    if let Some(found) = table.find_file(&name) {
                        original.files[index].data_size = table.files[found].data_size;
                    }
                }
                for record in records.values() {
                    let Some(stream) = &record.new_stream else {
                        continue;
                    };
                    if !stream
                        .tocs
                        .iter()
                        .any(|t| t.eq_ignore_ascii_case(&toc_file.rel_path))
                    {
                        continue;
                    }
                    let parent = stream.parent_in(&toc_file.rel_path);
                    add_stream_entry(&mut original, stream, parent, record.data_size.unwrap_or(0))
                        .map_err(|e| {
                            Error::InvalidArgument(format!("{}: {e}", toc_file.rel_path))
                        })?;
                }
                table = original;
                changed = true;
            }
        }
        if !changed {
            continue;
        }
        let bytes = table.to_bytes()?;
        if bytes != current_bytes {
            write.replace(toc_file, &bytes)?;
        }
    }
    Ok(())
}

/// Adds the entry of a created stream to a table of contents: a name record
/// with the stream's hash, a file entry that depends on the parent and, for
/// a stream loaded with its parent, a child link from the parent's entry.
pub fn add_stream_entry(
    table: &mut Toc,
    stream: &NewStream,
    parent_name: &str,
    data_size: u64,
) -> Result<()> {
    let parent = table
        .find_name(parent_name)
        .ok_or_else(|| Error::InvalidArgument(format!("{parent_name} is not listed")))?;
    let size = u32::try_from(data_size)
        .map_err(|_| Error::InvalidArgument("stream data exceeds 4 GiB".into()))?;
    let entry = table.add_name(stream.hash, &stream.toc_name());
    let flags = if stream.with_parent {
        crate::toc::FLAGS_PARENTED
    } else {
        crate::toc::FLAGS_REQUESTED
    };
    table.add_file(entry, size, flags, &[parent])?;
    if stream.with_parent {
        let parent_file = table
            .find_file(parent_name)
            .ok_or_else(|| Error::InvalidArgument(format!("{parent_name} has no entry")))?;
        table.attach_child(parent_file, entry)?;
    }
    Ok(())
}

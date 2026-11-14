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
use crate::game::{GameInstance, ReplaceReport};
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
            StreamState::Applied => self.streams.get(name).is_some_and(|record| {
                record.resources.len() == desired.len()
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
            if (!record.resources.is_empty() || record.new_stream.is_some())
                && self.stream_state(game, name)? == StreamState::Applied
            {
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
    /// `payload` reads the data of a change. The record is saved to
    /// `record_path` before and after every write.
    ///
    /// Everything is checked before the first write. Streams changed by
    /// someone else are refused unless `force`.
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
        let mut report = ApplyReport::default();
        for (name, state, wanted, write) in plan {
            if !write {
                self.streams.remove(&name);
                self.save(record_path)?;
                continue;
            }
            let created = self.created(&name, new_streams.get(&name)).cloned();
            match created {
                Some(stream) => self.sync_new_stream(
                    record_path,
                    game,
                    &stream,
                    new_streams.contains_key(&name),
                    &wanted,
                    &payload,
                    &mut report,
                )?,
                None => self.sync_stream(
                    record_path,
                    game,
                    &name,
                    state,
                    &wanted,
                    &payload,
                    &mut report,
                )?,
            }
        }
        if !stream_tocs.is_empty() {
            self.sync_new_stream_tocs(game, &stream_tocs, &mut report)?;
        }
        Ok(report)
    }

    #[allow(clippy::too_many_arguments)]
    fn sync_stream(
        &mut self,
        record_path: &Path,
        game: &GameInstance,
        name: &str,
        state: StreamState,
        wanted: &[Change],
        payload: &impl Fn(&Change) -> Result<Vec<u8>>,
        report: &mut ApplyReport,
    ) -> Result<()> {
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
        let original_layout = StreamLayout::parse(&original)?;

        let (target, action) = if wanted.is_empty() {
            (original.clone(), StreamAction::Restored)
        } else {
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
            (rebuilt, StreamAction::Rebuilt(wanted.len()))
        };
        let data_size = StreamLayout::parse(&target)?.total_data_size();

        // Note the file about to be written, so an interruption is recognised.
        let resources: BTreeMap<ResourceKey, String> = wanted
            .iter()
            .map(|c| (c.key.clone(), c.payload.sha256.clone()))
            .collect();
        let previous = self.streams.get(name).cloned();
        self.streams.insert(
            name.to_owned(),
            AppliedStream {
                path: file.rel_path.clone(),
                original: original_stamp.clone(),
                written: previous
                    .as_ref()
                    .map_or_else(|| original_stamp.clone(), |p| p.written.clone()),
                in_flight: Some(FileStamp::of_data(&target)),
                data_size: previous.as_ref().and_then(|p| p.data_size),
                resources: previous.map(|p| p.resources).unwrap_or_default(),
                new_stream: None,
            },
        );
        self.save(record_path)?;

        let mut files = ReplaceReport {
            old_len: file.len,
            new_len: target.len() as u64,
            old_data_size: 0,
            new_data_size: data_size,
            written: Vec::new(),
            backed_up: Vec::new(),
        };
        game.write_with_backup(file, &target, &mut files)?;
        game.update_toc_sizes(index, data_size, &mut files)?;

        if wanted.is_empty() {
            self.streams.remove(name);
        } else {
            self.streams.insert(
                name.to_owned(),
                AppliedStream {
                    path: file.rel_path.clone(),
                    original: original_stamp,
                    written: FileStamp::of_written(&file.path, &target),
                    in_flight: None,
                    resources,
                    data_size: Some(data_size),
                    new_stream: None,
                },
            );
        }
        self.save(record_path)?;
        report.streams.push((name.to_owned(), action));
        report.written.extend(files.written);
        report.backed_up.extend(files.backed_up);
        Ok(())
    }

    /// Creates, rewrites or deletes (`keep` false) a created stream. Only
    /// added resources can be in it; they are written in their order.
    #[allow(clippy::too_many_arguments)]
    fn sync_new_stream(
        &mut self,
        record_path: &Path,
        game: &GameInstance,
        stream: &NewStream,
        keep: bool,
        wanted: &[Change],
        payload: &impl Fn(&Change) -> Result<Vec<u8>>,
        report: &mut ApplyReport,
    ) -> Result<()> {
        let rel_path = stream.rel_path();
        let path = game.root().join(&rel_path);
        let name = stream.name.clone();
        if !keep {
            if path.exists() {
                fs::remove_file(&path).map_err(|e| Error::io(&path, e))?;
                report.written.push(rel_path);
            }
            self.streams.remove(&name);
            self.save(record_path)?;
            report.streams.push((name, StreamAction::Restored));
            return Ok(());
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
        let empty = FileStamp::of_data(&[]);
        let previous = self.streams.get(&name).cloned();
        self.streams.insert(
            name.clone(),
            AppliedStream {
                path: rel_path.clone(),
                original: empty.clone(),
                written: previous
                    .as_ref()
                    .map_or_else(|| empty.clone(), |p| p.written.clone()),
                in_flight: Some(FileStamp::of_data(&target)),
                data_size: previous.as_ref().and_then(|p| p.data_size),
                resources: previous
                    .as_ref()
                    .map(|p| p.resources.clone())
                    .unwrap_or_default(),
                new_stream: Some(stream.clone()),
            },
        );
        self.save(record_path)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
        crate::atomic::write(&path, &target)?;
        self.streams.insert(
            name.clone(),
            AppliedStream {
                path: rel_path.clone(),
                original: empty,
                written: FileStamp::of_written(&path, &target),
                in_flight: None,
                resources: wanted
                    .iter()
                    .map(|c| (c.key.clone(), c.payload.sha256.clone()))
                    .collect(),
                data_size: Some(data_size),
                new_stream: Some(stream.clone()),
            },
        );
        self.save(record_path)?;
        report
            .streams
            .push((name, StreamAction::Rebuilt(wanted.len())));
        report.written.push(rel_path);
        Ok(())
    }

    /// Brings the entries of created streams in the tables of contents
    /// `tocs` (lower-case relative paths) in line with the record.
    ///
    /// A table is rebuilt from its original (the backup, or the file when
    /// there is none) with the current data size of every stream it lists,
    /// plus an entry for every created stream of this record that names it.
    /// Without created streams a table returns to its original structure,
    /// byte for byte when the sizes are the original ones.
    fn sync_new_stream_tocs(
        &self,
        game: &GameInstance,
        tocs: &BTreeSet<String>,
        report: &mut ApplyReport,
    ) -> Result<()> {
        for toc_file in game.tocs() {
            if !tocs.contains(&toc_file.rel_path.to_ascii_lowercase()) {
                continue;
            }
            let original_path = game.original_path(toc_file);
            let original = fs::read(&original_path).map_err(|e| Error::io(&original_path, e))?;
            let current_bytes =
                fs::read(&toc_file.path).map_err(|e| Error::io(&toc_file.path, e))?;
            let current = Toc::parse(&current_bytes)?;
            let mut table = Toc::parse(&original)?;
            // Keep the sizes written since, by anyone.
            for index in 0..table.files.len() {
                let name = table.file_name(index).name_lossy();
                if let Some(found) = current.find_file(&name) {
                    table.files[index].data_size = current.files[found].data_size;
                }
            }
            for record in self.streams.values() {
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
                add_stream_entry(&mut table, stream, parent, record.data_size.unwrap_or(0))
                    .map_err(|e| Error::InvalidArgument(format!("{}: {e}", toc_file.rel_path)))?;
            }
            let bytes = table.to_bytes()?;
            if bytes == current_bytes {
                continue;
            }
            let mut files = ReplaceReport {
                old_len: current_bytes.len() as u64,
                new_len: bytes.len() as u64,
                old_data_size: 0,
                new_data_size: 0,
                written: Vec::new(),
                backed_up: Vec::new(),
            };
            game.write_with_backup(toc_file, &bytes, &mut files)?;
            report.written.extend(files.written);
            report.backed_up.extend(files.backed_up);
        }
        Ok(())
    }
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

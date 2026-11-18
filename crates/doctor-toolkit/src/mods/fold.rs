//! Folding the deltas of the enabled mods onto what the game has (format
//! version 6).
//!
//! A resource several mods change is no longer the data of the last of
//! them. Its data is the game's original — or the last whole resource an
//! enabled mod gives it — and then every delta of the enabled mods above
//! that, in load order ([`crate::resource::delta::apply_resource`]). So two
//! mods that add objects to one SimGroup are both in the game, and a mod
//! made on top of another applies to whatever version of it the player has.
//!
//! The result is new data, of no mod: the mod manager keeps it in a store of
//! its own and names it by its hash in the plan, so that the write, the
//! record of what is written and the comparison with the game work as for
//! any other payload.
//!
//! ```text
//! <game>/_doctor_toolkit/mods/.folded/
//! ├── index.json        (type, data, delta) → the data after the delta, and its notes
//! └── payloads/ab/ab12… the folded resources of the last write
//! ```
//!
//! # Deterministic and cached
//!
//! Applying a delta is a function of the bytes of the data and of the
//! document, so one step is known by three things: the type of the
//! resource, the SHA-256 of the data it is applied to and the SHA-256 of
//! the document. `index.json` remembers what each step gave — the hash of
//! the data after it, how many operations applied, which were skipped,
//! which changed a value their author did not see, or why the delta was
//! refused. A fold whose steps are all known reads no delta and applies
//! nothing; enabling, disabling or reordering mods changes the chain of a
//! resource and only the steps that are new are computed.
//!
//! The cache is never trusted for data: a payload is read by its hash and
//! checked against it, and a chain whose remembered hashes the data does
//! not give is computed anew. Only [`ModManager::apply`] writes the folder
//! (the checks and the comparison with the game keep what they compute in
//! memory), and after a write it keeps what the written plan uses.
//!
//! A SimGroup delta gives its author's bytes only on the data it was made
//! on (its `layout` section, `docs/formats/delta.md` §5.1). On any other
//! data the result is right in what it holds and laid out by the writer, so
//! a fold is never compared with the hash a mod's author had: the plan
//! names the hash of what the fold gave here.
//!
//! [`ModManager::apply`]: super::manager::ModManager::apply

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use super::{ModResource, PAYLOAD_DIR};
use crate::game::{GameInstance, StreamFile};
use crate::resource::delta::{self, Note};
use crate::workspace::{Blob, PayloadStore, ResourceKey};
use crate::{Error, Result};

/// The folder of the folds in the mod manager's directory. The name starts
/// with a dot, which no mod id does.
pub(super) const FOLD_DIR: &str = ".folded";
const INDEX_FILE: &str = "index.json";
const INDEX_FORMAT: &str = "doctor-toolkit-folds";
/// The version of what applying a delta gives. It goes up when a version of
/// this library writes other bytes for the same data and document (the
/// writer of a resource changed): the steps remembered by the version
/// before are then computed anew.
const ENGINE: u32 = 1;

/// What applying one delta to one piece of data gave.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    /// The data after the delta; `None` when the delta was refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Blob>,
    /// Number of operations that were applied.
    #[serde(default)]
    pub applied: usize,
    /// Operations left out because their target is not in the data.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<Note>,
    /// Operations applied over a value their author did not see.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflicts: Vec<Note>,
    /// Why the delta was refused: it adds an element whose address is
    /// taken, or the document or the data does not read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What one mod does in the fold of a resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// It gives the whole resource: whatever is below is replaced.
    Whole,
    /// Its delta is below a whole resource of a later mod and is not applied.
    Replaced,
    /// Its delta has nothing to be applied to: the resource is not in the
    /// game and no mod below gives it whole.
    NoBase,
    /// Its delta was applied, or refused.
    Delta(Outcome),
}

/// The fold of one resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyFold {
    /// One step per entry, in load order.
    pub steps: Vec<Step>,
    /// The data the entries give the resource; `None` when they give none
    /// (deltas without anything to apply them to).
    pub result: Option<Blob>,
    /// Whether a delta is in the result: it is then data of the fold and
    /// not of a mod.
    pub folded: bool,
    /// The game's own data of the resource, when the fold starts from it.
    pub original: Option<Blob>,
    /// The steps of the index this fold stands on.
    pub(super) ids: Vec<String>,
}

/// What a caller needs of the result of a fold beyond its hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Want {
    /// The hash: the checks, the comparison with the game.
    Hash,
    /// The data, readable with [`FoldCache::data`].
    Data,
}

/// One mod's entry for a resource: the entry and where its payload is.
pub(super) struct Entry<'a> {
    pub resource: &'a ModResource,
    pub store: PayloadStore,
}

/// The resources of the unmodified game, for the folds that start from
/// them: read from what a write rebuilds a stream from (the backup, or the
/// file itself), never from a stream the mod manager created.
pub(super) struct Originals<'a> {
    game: &'a GameInstance,
    created: BTreeSet<String>,
    /// The stream opened last: the keys of a plan come stream by stream.
    open: Option<(String, Option<StreamFile>)>,
}

impl<'a> Originals<'a> {
    /// `created`: the streams the mod manager made, which are no part of
    /// the unmodified game.
    pub fn new(game: &'a GameInstance, created: BTreeSet<String>) -> Self {
        Self {
            game,
            created,
            open: None,
        }
    }

    /// The file the stream `name` of the unmodified game is read from.
    fn path(&self, name: &str) -> Option<PathBuf> {
        if self.created.contains(name) {
            return None;
        }
        let file = &self.game.streams()[self.game.find_stream(name)?];
        let path = self.game.original_path(file);
        path.is_file().then_some(path)
    }

    fn stream(&mut self, name: &str) -> Result<Option<&StreamFile>> {
        if self.open.as_ref().is_none_or(|(open, _)| open != name) {
            let stream = self.path(name).map(StreamFile::open).transpose()?;
            self.open = Some((name.to_owned(), stream));
        }
        Ok(self.open.as_ref().and_then(|(_, stream)| stream.as_ref()))
    }

    /// The data of `key` in the unmodified game, if it is there.
    pub fn data(&mut self, key: &ResourceKey) -> Result<Option<Vec<u8>>> {
        let Some(stream) = self.stream(&key.stream)? else {
            return Ok(None);
        };
        key.find_in(stream.layout())
            .map(|index| stream.resource_data(index))
            .transpose()
    }
}

/// How the file of an original looked when a resource of it was hashed.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Seen {
    path: PathBuf,
    len: u64,
    modified: Option<SystemTime>,
}

impl Seen {
    fn of(path: PathBuf) -> Option<Self> {
        let meta = fs::metadata(&path).ok()?;
        Some(Self {
            path,
            len: meta.len(),
            modified: meta.modified().ok(),
        })
    }
}

/// `index.json`.
#[derive(Serialize, Deserialize)]
struct Index {
    format: String,
    /// [`ENGINE`].
    engine: u32,
    steps: BTreeMap<String, Outcome>,
}

#[derive(Debug, Default)]
struct State {
    /// Whether `index.json` has been read.
    loaded: bool,
    /// The steps of the index and those computed since.
    steps: HashMap<String, Outcome>,
    /// Data computed since the last write, by hash: results of steps that
    /// are not in the store (yet).
    data: HashMap<String, Arc<Vec<u8>>>,
    /// The hash of the game's own data of a resource, with the file it was
    /// read from as it was then.
    originals: HashMap<ResourceKey, (Seen, Blob)>,
}

/// The folds of one mod manager: the steps it knows and the data it keeps.
#[derive(Debug)]
pub(super) struct FoldCache {
    dir: PathBuf,
    state: Mutex<State>,
}

/// The name of a step in the index.
fn step_id(type_id: u32, data: &Blob, document: &Blob) -> String {
    format!("{type_id:08x}-{}-{}", data.sha256, document.sha256)
}

impl FoldCache {
    /// The folds of the mod manager in `mods_dir`. Nothing is read yet.
    pub fn new(mods_dir: &Path) -> Self {
        Self {
            dir: mods_dir.join(FOLD_DIR),
            state: Mutex::default(),
        }
    }

    /// The store of the folded resources.
    pub fn store(&self) -> PayloadStore {
        PayloadStore::new(self.dir.join(PAYLOAD_DIR))
    }

    fn state(&self) -> MutexGuard<'_, State> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.loaded {
            state.loaded = true;
            // A cache: one that does not read, or of another engine, is
            // none.
            let index = fs::read(self.dir.join(INDEX_FILE))
                .ok()
                .and_then(|text| serde_json::from_slice::<Index>(&text).ok())
                .filter(|index| index.format == INDEX_FORMAT && index.engine == ENGINE);
            if let Some(index) = index {
                state.steps.extend(index.steps);
            }
        }
        state
    }

    /// Forgets what was read and computed: another program changed the mods.
    pub fn forget(&self) {
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = State::default();
    }

    /// The hash of the game's own data of `key`, if the game has it.
    fn original(&self, originals: &mut Originals<'_>, key: &ResourceKey) -> Result<Option<Blob>> {
        let seen = originals.path(&key.stream).and_then(Seen::of);
        if let Some(seen) = &seen
            && let Some((then, blob)) = self.state().originals.get(key)
            && then == seen
        {
            return Ok(Some(blob.clone()));
        }
        let Some(data) = originals.data(key)? else {
            return Ok(None);
        };
        let blob = Blob::of(&data);
        if let Some(seen) = seen {
            self.state()
                .originals
                .insert(key.clone(), (seen, blob.clone()));
        }
        Ok(Some(blob))
    }

    /// The data `blob` of a fold made with [`Want::Data`].
    pub fn data(&self, blob: &Blob) -> Result<Vec<u8>> {
        if let Some(data) = self.state().data.get(&blob.sha256) {
            return Ok(data.to_vec());
        }
        self.store().get(blob)
    }

    /// The fold of the resource `key` over `entries`, the entries of the
    /// mods for it in load order.
    pub fn fold(
        &self,
        originals: &mut Originals<'_>,
        key: &ResourceKey,
        entries: &[Entry<'_>],
        want: Want,
    ) -> Result<KeyFold> {
        if let Some(fold) = self.fold_with_cache(originals, key, entries, want)? {
            return Ok(fold);
        }
        // The data did not give a hash the index remembers: the index is
        // not of these files. Without it every step is computed, and what
        // is computed is what the data gives.
        self.state().steps.clear();
        self.fold_with_cache(originals, key, entries, want)?
            .ok_or_else(|| {
                Error::mod_error(format!(
                    "{key}: applying the same deltas twice gave different data"
                ))
            })
    }

    /// [`Self::fold`]; `None` when the index does not fit the data.
    fn fold_with_cache(
        &self,
        originals: &mut Originals<'_>,
        key: &ResourceKey,
        entries: &[Entry<'_>],
        want: Want,
    ) -> Result<Option<KeyFold>> {
        let whole = entries.iter().rposition(|e| !e.resource.is_delta());
        let first = whole.map_or(0, |w| w + 1);
        let mut steps: Vec<Step> = entries[..first]
            .iter()
            .map(|e| match e.resource.is_delta() {
                true => Step::Replaced,
                false => Step::Whole,
            })
            .collect();
        let mut original = None;
        let mut current = match whole {
            Some(w) => Some(entries[w].resource.payload.clone()),
            None => {
                original = self.original(originals, key)?;
                original.clone()
            }
        };
        let mut folded = false;
        let mut ids = Vec::new();
        for (at, entry) in entries.iter().enumerate().skip(first) {
            let Some(input) = current.clone() else {
                steps.push(Step::NoBase);
                continue;
            };
            let id = step_id(key.type_id, &input, &entry.resource.payload);
            let known = self.state().steps.get(&id).cloned();
            let outcome = match known {
                Some(outcome) => outcome,
                None => {
                    let Some(base) = self.chain_data(originals, key, entries, at, &input)? else {
                        return Ok(None);
                    };
                    let document = entry.store.get(&entry.resource.payload)?;
                    let outcome = match delta::apply_resource(key.type_id, &document, &base) {
                        Ok(applied) => {
                            let blob = Blob::of(&applied.data);
                            self.state()
                                .data
                                .insert(blob.sha256.clone(), Arc::new(applied.data));
                            Outcome {
                                data: Some(blob),
                                applied: applied.applied,
                                skipped: applied.skipped,
                                conflicts: applied.conflicts,
                                error: None,
                            }
                        }
                        Err(error) => Outcome {
                            data: None,
                            applied: 0,
                            skipped: Vec::new(),
                            conflicts: Vec::new(),
                            error: Some(error.to_string()),
                        },
                    };
                    self.state().steps.insert(id.clone(), outcome.clone());
                    outcome
                }
            };
            ids.push(id);
            if let Some(data) = &outcome.data {
                current = Some(data.clone());
                folded = true;
            }
            steps.push(Step::Delta(outcome));
        }
        if folded
            && want == Want::Data
            && let Some(result) = &current
            && self
                .chain_data(originals, key, entries, entries.len(), result)?
                .is_none()
        {
            return Ok(None);
        }
        Ok(Some(KeyFold {
            steps,
            result: current,
            folded,
            original,
            ids,
        }))
    }

    /// The data after the entries before `upto`, whose hash the steps so
    /// far say is `expected`: from memory, from the store, else computed
    /// from the start of the chain — and then kept in memory. `None` when
    /// the computed data has another hash: the steps were remembered for
    /// other data.
    fn chain_data(
        &self,
        originals: &mut Originals<'_>,
        key: &ResourceKey,
        entries: &[Entry<'_>],
        upto: usize,
        expected: &Blob,
    ) -> Result<Option<Arc<Vec<u8>>>> {
        if let Some(data) = self.state().data.get(&expected.sha256) {
            return Ok(Some(data.clone()));
        }
        let store = self.store();
        if store.contains(expected)
            && let Ok(data) = store.get(expected)
        {
            return Ok(Some(Arc::new(data)));
        }
        let whole = entries[..upto].iter().rposition(|e| !e.resource.is_delta());
        let mut data = match whole {
            Some(w) => entries[w].store.get(&entries[w].resource.payload)?,
            None => originals
                .data(key)?
                .ok_or_else(|| Error::mod_error(format!("{key} is not in the game any more")))?,
        };
        for entry in &entries[whole.map_or(0, |w| w + 1)..upto] {
            let document = entry.store.get(&entry.resource.payload)?;
            // A delta that is refused leaves the data as it is.
            if let Ok(applied) = delta::apply_resource(key.type_id, &document, &data) {
                data = applied.data;
            }
        }
        if Blob::of(&data) != *expected {
            return Ok(None);
        }
        let data = Arc::new(data);
        self.state()
            .data
            .insert(expected.sha256.clone(), data.clone());
        Ok(Some(data))
    }

    /// Puts the folded resources `blobs` (folded with [`Want::Data`]) into
    /// the store: a write reads them from there.
    pub fn persist<'a>(&self, blobs: impl IntoIterator<Item = &'a Blob>) -> Result<()> {
        let store = self.store();
        for blob in blobs {
            if store.contains(blob) {
                continue;
            }
            let data = self
                .state()
                .data
                .get(&blob.sha256)
                .cloned()
                .ok_or_else(|| {
                    Error::mod_error(format!("the folded resource {} is gone", blob.sha256))
                })?;
            store.put(&data)?;
        }
        Ok(())
    }

    /// After a write: keeps the folded resources `blobs` and the steps
    /// `ids` — what the written plan stands on — and drops the rest, in the
    /// folder and in memory. With nothing to keep the folder goes.
    pub fn settle<'a>(
        &self,
        blobs: impl IntoIterator<Item = &'a Blob>,
        ids: &HashSet<String>,
    ) -> Result<()> {
        let keep: HashSet<&str> = blobs.into_iter().map(|b| b.sha256.as_str()).collect();
        let steps: BTreeMap<String, Outcome> = {
            let mut state = self.state();
            state.data.clear();
            state
                .steps
                .iter()
                .filter(|(id, _)| ids.contains(*id))
                .map(|(id, outcome)| (id.clone(), outcome.clone()))
                .collect()
        };
        if keep.is_empty() && steps.is_empty() {
            if fs::symlink_metadata(&self.dir).is_ok() {
                fs::remove_dir_all(&self.dir).map_err(|e| Error::io(&self.dir, e))?;
            }
            return Ok(());
        }
        self.store().retain(&keep)?;
        let index = Index {
            format: INDEX_FORMAT.to_owned(),
            engine: ENGINE,
            steps,
        };
        let mut text = serde_json::to_vec_pretty(&index)
            .map_err(|e| Error::mod_error(format!("the index of the folds: {e}")))?;
        text.push(b'\n');
        fs::create_dir_all(&self.dir).map_err(|e| Error::io(&self.dir, e))?;
        let path = self.dir.join(INDEX_FILE);
        // Written only when it says something new: a staged copy of the
        // game then keeps its link to the game's index.
        if fs::read(&path).is_ok_and(|old| old == text) {
            return Ok(());
        }
        crate::atomic::write(&path, &text)
    }
}

#[cfg(test)]
pub(crate) mod tests;

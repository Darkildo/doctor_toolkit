//! Rebasing (stage 111): changes made on data the game no longer gives.
//!
//! An edit starts from the game as it is, with the mods the mod manager
//! has written into it ([`Workspace::current_data`]), so the data of a
//! change is "what the mods gave, and my edit". When one of those mods is
//! updated, or the mods of the game are arranged otherwise, the change
//! still holds what the mods gave **then**: a mod built of it would take
//! back what they changed since — remove what an update added, put back
//! what it removed.
//!
//! A change therefore records the data it was made on
//! ([`Change::made_on`]), and
//!
//! - [`Workspace::stale`] lists the changes whose resource the game gives
//!   otherwise now;
//! - [`Workspace::rebase`] moves them onto what the game gives now. A
//!   resource of a kind that has deltas ([`delta::kind_of`]: SimGroups,
//!   string tables, the parts database, the world grid) is merged: the
//!   operations that turn the data it was made on into the author's are
//!   applied to the new data, and what does not apply is told by object or
//!   string ([`delta::Applied`]). Any other resource (a texture, a mesh)
//!   has no elements to merge by, and the author decides: keep the change
//!   as it is ([`Workspace::keep_mine`]) or give it up for the game's data
//!   ([`Workspace::take_base`]).
//!
//! All of one call is one undoable operation, and a call that finds nothing
//! stale records nothing.
//!
//! "What the game gives" is the file of the game as it is — what the
//! editor shows and what a new edit would start from. One case apart: the
//! workspace's own mod. An author installs the workspace as a mod and
//! writes the mods to see it in the game; the file then holds the
//! workspace's own change, which is not what that change stands on. For
//! those resources the game's data is what the other mods give
//! ([`Workspace::given`], `crate::mods::game_without_mod`; a project adds
//! the mod of its id, `crate::project::Project::given`).

use std::collections::BTreeMap;

use super::{Blob, Change, ChangeDetail, Edit, ResourceKey, StorageChoice, Workspace, diff};
use crate::game::{GameInstance, StreamFile};
use crate::resource::delta::{self, Kind, Note};
use crate::{Error, Result};

/// The data the game gives some resources without the changes of a
/// workspace, where that is not what its files hold.
pub type Given = BTreeMap<ResourceKey, Vec<u8>>;

/// A change made on other data than the game gives its resource now
/// ([`Workspace::stale`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stale {
    pub key: ResourceKey,
    /// The data the change was made on ([`Change::made_on`]).
    pub made_on: Blob,
    /// The data the game gives the resource now.
    pub now: Blob,
    /// The kind of delta the resource has: its change is rebased by its
    /// operations. `None`: the author decides between the two versions.
    pub kind: Option<Kind>,
}

/// What a rebase is asked to do ([`Workspace::rebase_given`]).
#[derive(Debug, Clone, Copy, Default)]
pub struct RebaseRequest<'a> {
    /// The stale changes to merge by their operations; `None`: every stale
    /// change that is in neither of the other two lists.
    pub keys: Option<&'a [ResourceKey]>,
    /// "Keep mine": these changes stay as they are and count as made on
    /// what the game gives now.
    pub keep: &'a [ResourceKey],
    /// "Take the base's": these changes are removed.
    pub take_base: &'a [ResourceKey],
}

/// What a rebase did with one change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RebaseOutcome {
    /// The operations of the change were applied to the data the game
    /// gives now.
    Merged {
        /// Operations applied.
        applied: usize,
        /// Operations left out because what they address is gone, each
        /// with the reason.
        skipped: Vec<Note>,
        /// Operations applied over a value the game changed as well; the
        /// author's value stands.
        conflicts: Vec<Note>,
        /// Nothing of the change differs from the game's data any more:
        /// the change was removed.
        removed: bool,
    },
    /// Kept as it is; it is made on what the game gives now.
    Kept,
    /// Removed: the resource is what the game gives.
    Taken,
    /// Left as it was: the author has to decide between keeping the change
    /// and taking the game's data.
    Undecided { reason: String },
    /// Named in the request, but made on what the game gives.
    NotStale,
}

/// One change of a [`RebaseReport`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rebased {
    pub key: ResourceKey,
    pub outcome: RebaseOutcome,
}

impl Rebased {
    /// The outcome in words, one line and one more for each operation that
    /// was left out or met another value.
    pub fn describe(&self) -> Vec<String> {
        let key = &self.key;
        match &self.outcome {
            RebaseOutcome::Merged {
                applied,
                skipped,
                conflicts,
                removed,
            } => {
                let mut head = format!("{key}: rebased, {applied} operation(s) applied");
                if !skipped.is_empty() {
                    head += &format!(", {} left out", skipped.len());
                }
                if !conflicts.is_empty() {
                    head += &format!(
                        ", {} over a value the game changed as well",
                        conflicts.len()
                    );
                }
                if *removed {
                    head += "; nothing of the change differs from the game's data any more, \
                             so it is removed";
                }
                let mut lines = vec![head];
                lines.extend(skipped.iter().map(|n| format!("  left out: {n}")));
                lines.extend(conflicts.iter().map(|n| format!("  yours stands: {n}")));
                lines
            }
            RebaseOutcome::Kept => vec![format!(
                "{key}: kept as it is; it counts as made on what the game gives now"
            )],
            RebaseOutcome::Taken => {
                vec![format!(
                    "{key}: the change is removed; the game's data stays"
                )]
            }
            RebaseOutcome::Undecided { reason } => vec![format!(
                "{key}: not rebased, it needs your decision (keep yours or take the game's): \
                 {reason}"
            )],
            RebaseOutcome::NotStale => {
                vec![format!(
                    "{key}: is made on what the game gives; nothing to do"
                )]
            }
        }
    }
}

/// What [`Workspace::rebase`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RebaseReport {
    /// Sequence number of the recorded operation; `None` when nothing
    /// changed.
    pub seq: Option<u64>,
    /// The changes the rebase looked at, by key.
    pub changes: Vec<Rebased>,
}

impl RebaseReport {
    /// The changes that were merged by their operations.
    pub fn merged(&self) -> impl Iterator<Item = &Rebased> {
        self.changes
            .iter()
            .filter(|c| matches!(c.outcome, RebaseOutcome::Merged { .. }))
    }

    /// The changes left for the author to decide.
    pub fn undecided(&self) -> impl Iterator<Item = &Rebased> {
        self.changes
            .iter()
            .filter(|c| matches!(c.outcome, RebaseOutcome::Undecided { .. }))
    }

    /// Operations left out of the merged changes.
    pub fn skipped(&self) -> usize {
        self.changes
            .iter()
            .map(|c| match &c.outcome {
                RebaseOutcome::Merged { skipped, .. } => skipped.len(),
                _ => 0,
            })
            .sum()
    }

    /// Operations of the merged changes applied over a value the game
    /// changed as well.
    pub fn conflicts(&self) -> usize {
        self.changes
            .iter()
            .map(|c| match &c.outcome {
                RebaseOutcome::Merged { conflicts, .. } => conflicts.len(),
                _ => 0,
            })
            .sum()
    }

    /// Every outcome in words ([`Rebased::describe`]).
    pub fn describe(&self) -> Vec<String> {
        self.changes.iter().flat_map(Rebased::describe).collect()
    }
}

/// What a file of the game holds of one stream, for [`GameNow`].
enum Source {
    /// Nothing a change is compared with: a stream the workspace creates
    /// or wrote itself, or one the game does not have.
    Nothing,
    /// The file is the unmodified original.
    Original,
    File(StreamFile),
}

/// The data the game gives a resource now.
enum Now {
    /// The original of the change.
    Original,
    Data(Vec<u8>),
}

impl Now {
    fn blob(&self, change: &Change) -> Blob {
        match self {
            Self::Original => change.original.clone(),
            Self::Data(data) => Blob::of(data),
        }
    }
}

/// Reads what the game gives the resources of a workspace's changes
/// without those changes, stream by stream.
struct GameNow<'a> {
    ws: &'a Workspace,
    game: &'a GameInstance,
    given: &'a Given,
    /// The stream looked at last: changes come sorted by key.
    open: Option<(String, Source)>,
}

impl<'a> GameNow<'a> {
    fn new(ws: &'a Workspace, game: &'a GameInstance, given: &'a Given) -> Self {
        Self {
            ws,
            game,
            given,
            open: None,
        }
    }

    fn source(&self, name: &str) -> Result<Source> {
        // A stream this workspace wrote holds its own changes, and the mods
        // are not in the game then ([`Workspace::current_data`]).
        if self.ws.new_streams.contains_key(name) || self.ws.wrote_stream(name) {
            return Ok(Source::Nothing);
        }
        let Some(index) = self.game.find_stream(name) else {
            return Ok(Source::Nothing);
        };
        let file = &self.game.streams()[index];
        // Without a backup nothing has written the file: no need to read
        // it to know what it holds.
        if self.game.original_path(file) == file.path {
            return Ok(Source::Original);
        }
        Ok(Source::File(StreamFile::open(&file.path)?))
    }

    /// What the game gives the resource of `change`; `None` for a change
    /// that has nothing to be compared with — a resource the workspace
    /// adds, or one that is not in the file.
    fn of(&mut self, change: &Change) -> Result<Option<Now>> {
        if change.is_added() {
            return Ok(None);
        }
        let name = &change.key.stream;
        if self.open.as_ref().is_none_or(|(open, _)| open != name) {
            self.open = Some((name.clone(), self.source(name)?));
        }
        let Some((_, source)) = &self.open else {
            return Ok(None);
        };
        if matches!(source, Source::Nothing) {
            return Ok(None);
        }
        if let Some(data) = self.given.get(&change.key) {
            return Ok(Some(Now::Data(data.clone())));
        }
        Ok(match source {
            Source::Nothing => None,
            Source::Original => Some(Now::Original),
            Source::File(stream) => match change.key.find_in(stream.layout()) {
                Some(index) => Some(Now::Data(stream.resource_data(index)?)),
                None => None,
            },
        })
    }
}

impl Workspace {
    /// The changes made on other data than the game gives their resources
    /// now: a mod below them was updated, or the mods of the game changed.
    /// A change a mod is built of as it is would take back what changed
    /// since ([`Self::rebase`]).
    ///
    /// The game's data is the file of the game as it is, for a stream this
    /// workspace did not write. A resource or a stream the workspace adds
    /// is never stale: nothing else gives it. A change recorded before
    /// stage 111 counts as made on the unmodified original.
    ///
    /// Reads only the streams something has written (those with a backup),
    /// one resource for each change in them.
    pub fn stale(&self, game: &GameInstance) -> Result<Vec<Stale>> {
        self.stale_given(game, &self.given(game)?)
    }

    /// What the game gives the resources of the workspace where its files
    /// hold a mod installed from this very workspace
    /// ([`crate::mods::game_without_mod`]): the data without that mod.
    pub fn given(&self, game: &GameInstance) -> Result<Given> {
        crate::mods::game_without_mod(game, self, &[])
    }

    /// [`Self::stale`], where the game gives the resources of `given`
    /// that data and not what its files hold.
    pub fn stale_given(&self, game: &GameInstance, given: &Given) -> Result<Vec<Stale>> {
        let on_disk;
        let game = if game.has_staged() {
            on_disk = game.on_disk();
            &on_disk
        } else {
            game
        };
        let mut now = GameNow::new(self, game, given);
        let mut stale = Vec::new();
        for change in self.state.values() {
            let Some(found) = now.of(change)? else {
                continue;
            };
            let blob = found.blob(change);
            if blob != *change.made_on() {
                stale.push(Stale {
                    key: change.key.clone(),
                    made_on: change.made_on().clone(),
                    now: blob,
                    kind: delta::kind_of(change.key.type_id),
                });
            }
        }
        Ok(stale)
    }

    /// Moves stale changes ([`Self::stale`]; all, or those in `keys`) onto
    /// the data the game gives now, as one undoable operation.
    ///
    /// A change of a kind that has deltas is merged: the operations that
    /// turn the data it was made on into the author's
    /// ([`delta::operations`]) are applied to the game's data, and the
    /// result is the new data of the change, made on the game's. An
    /// operation whose target is gone is left out, one that meets another
    /// value than its author saw wins; both are in the report by object or
    /// string. A change that cannot be merged — of a kind without deltas,
    /// one that is more than operations can say, one that adds what the
    /// game has now — is left as it was and reported as
    /// [`RebaseOutcome::Undecided`]: [`Self::keep_mine`] or
    /// [`Self::take_base`] settle it.
    ///
    /// Nothing is recorded when nothing is stale.
    pub fn rebase(
        &mut self,
        game: &GameInstance,
        keys: Option<&[ResourceKey]>,
    ) -> Result<RebaseReport> {
        let request = RebaseRequest {
            keys,
            ..Default::default()
        };
        self.rebase_with(game, &request)
    }

    /// [`Self::rebase`], [`Self::keep_mine`] and [`Self::take_base`]
    /// together, as `request` says, as one undoable operation.
    pub fn rebase_with(
        &mut self,
        game: &GameInstance,
        request: &RebaseRequest<'_>,
    ) -> Result<RebaseReport> {
        let given = self.given(game)?;
        self.rebase_given(game, &given, request)
    }

    /// "Keep mine" for the stale changes `keys`: each stays as it is and
    /// counts as made on what the game gives now, so it is stale no more —
    /// the author has seen the other version and wants this one. One
    /// undoable operation.
    pub fn keep_mine(&mut self, game: &GameInstance, keys: &[ResourceKey]) -> Result<RebaseReport> {
        let request = RebaseRequest {
            keys: Some(&[]),
            keep: keys,
            take_base: &[],
        };
        self.rebase_with(game, &request)
    }

    /// "Take the base's" for the stale changes `keys`: each is removed, and
    /// the resource is what the game gives. One undoable operation.
    pub fn take_base(&mut self, game: &GameInstance, keys: &[ResourceKey]) -> Result<RebaseReport> {
        let request = RebaseRequest {
            keys: Some(&[]),
            keep: &[],
            take_base: keys,
        };
        self.rebase_with(game, &request)
    }

    /// [`Self::rebase_with`], where the game gives the resources of `given`
    /// that data and not what its files hold.
    pub fn rebase_given(
        &mut self,
        game: &GameInstance,
        given: &Given,
        request: &RebaseRequest<'_>,
    ) -> Result<RebaseReport> {
        let named = || {
            request
                .keys
                .unwrap_or_default()
                .iter()
                .chain(request.keep)
                .chain(request.take_base)
        };
        for key in named() {
            if !self.state.contains_key(key) {
                return Err(Error::InvalidArgument(format!(
                    "the workspace does not change {key}"
                )));
            }
        }
        if let Some(key) = request.keep.iter().find(|k| request.take_base.contains(k)) {
            return Err(Error::InvalidArgument(format!(
                "{key} cannot be both kept and given up"
            )));
        }
        let on_disk;
        let game = if game.has_staged() {
            on_disk = game.on_disk();
            &on_disk
        } else {
            game
        };

        let mut report = RebaseReport::default();
        let mut edits = Vec::new();
        let mut now = GameNow::new(self, game, given);
        for change in self.state.values() {
            let key = &change.key;
            let take = request.take_base.contains(key);
            let keep = request.keep.contains(key);
            let merge = !take && !keep && request.keys.is_none_or(|keys| keys.contains(key));
            let is_named = take || keep || request.keys.is_some_and(|keys| keys.contains(key));
            if !take && !keep && !merge {
                continue;
            }
            let found = now.of(change)?.map(|found| (found.blob(change), found));
            let Some((blob, found)) = found.filter(|(blob, _)| blob != change.made_on()) else {
                if is_named {
                    report.changes.push(Rebased {
                        key: key.clone(),
                        outcome: RebaseOutcome::NotStale,
                    });
                }
                continue;
            };
            let data = match found {
                Now::Original => self.store.get(&change.original)?,
                Now::Data(data) => data,
            };
            // The data the change is made on from now on; the original is
            // not written down.
            let made_on = |ws: &Self| -> Result<Option<Blob>> {
                Ok(match blob == change.original {
                    true => None,
                    false => Some(ws.store.put(&data)?),
                })
            };
            let (outcome, after) = if take {
                (RebaseOutcome::Taken, None)
            } else if keep {
                let after = Change {
                    made_on: made_on(self)?,
                    ..change.clone()
                };
                (RebaseOutcome::Kept, Some(after))
            } else {
                match self.merged(change, &data)? {
                    Ok(applied) => {
                        let payload = Blob::of(&applied.data);
                        // Nothing of the author's is left, or the game has
                        // it all now: no change, as when an edit gives the
                        // game's data ([`Self::set_resources`]).
                        let removed = payload == blob && change.storage == StorageChoice::Keep;
                        let after = match removed {
                            true => None,
                            false => Some(Change {
                                payload: self.store.put(&applied.data)?,
                                made_on: made_on(self)?,
                                ..change.clone()
                            }),
                        };
                        let outcome = RebaseOutcome::Merged {
                            applied: applied.applied,
                            skipped: applied.skipped,
                            conflicts: applied.conflicts,
                            removed,
                        };
                        (outcome, after)
                    }
                    Err(reason) => {
                        report.changes.push(Rebased {
                            key: key.clone(),
                            outcome: RebaseOutcome::Undecided { reason },
                        });
                        continue;
                    }
                }
            };
            report.changes.push(Rebased {
                key: key.clone(),
                outcome,
            });
            edits.push(Edit {
                key: key.clone(),
                before: Some(change.clone()),
                after,
            });
        }
        if edits.is_empty() {
            return Ok(report);
        }
        let label = match &edits[..] {
            [edit] => format!("Rebase {}", edit.key),
            edits => format!("Rebase {} changes", edits.len()),
        };
        report.seq = Some(self.commit(&label, edits, Vec::new())?);
        Ok(report)
    }

    /// The change `change` on the data `now` the game gives its resource:
    /// what its operations give there, or why it cannot be merged.
    fn merged(
        &self,
        change: &Change,
        now: &[u8],
    ) -> Result<std::result::Result<delta::Applied, String>> {
        let type_id = change.key.type_id;
        if delta::kind_of(type_id).is_none() {
            return Ok(Err(format!(
                "a resource of type {} has no elements two versions can be merged by",
                change.key.type_name()
            )));
        }
        let made_on = self.store.get(change.made_on())?;
        let mine = self.store.get(&change.payload)?;
        let operations = match delta::operations(type_id, &made_on, &mine) {
            Ok(Some(operations)) => operations,
            Ok(None) => {
                return Ok(Err(
                    "the change is more than operations on the elements of the resource can say"
                        .to_owned(),
                ));
            }
            Err(error) => return Ok(Err(format!("the change does not read: {error}"))),
        };
        // An element the change adds that the game has now is refused: the
        // two cannot both be right, and which one is, the author knows.
        Ok(operations.apply(now).map_err(|error| error.to_string()))
    }

    /// What the game changed under the change of `key`: the comparison of
    /// the data the change was made on with the data the game gives now
    /// (with `given` as in [`Self::stale_given`]), as [`Self::detail`]
    /// compares the original with the change. `None` when the workspace
    /// does not change `key` or nothing gives the resource but the
    /// workspace.
    pub fn base_detail(
        &self,
        game: &GameInstance,
        given: &Given,
        key: &ResourceKey,
    ) -> Result<Option<ChangeDetail>> {
        let Some(change) = self.state.get(key) else {
            return Ok(None);
        };
        let on_disk;
        let game = if game.has_staged() {
            on_disk = game.on_disk();
            &on_disk
        } else {
            game
        };
        let Some(found) = GameNow::new(self, game, given).of(change)? else {
            return Ok(None);
        };
        let now = match found {
            Now::Original => self.store.get(&change.original)?,
            Now::Data(data) => data,
        };
        let made_on = self.store.get(change.made_on())?;
        Ok(Some(ChangeDetail {
            bytes: diff::byte_diff(&made_on, &now),
            semantic: diff::semantic_diff(key.type_id, &made_on, &now),
        }))
    }
}

#[cfg(test)]
mod tests;

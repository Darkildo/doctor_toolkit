//! The game as the workspace will leave it, for readers: the streams the
//! workspace creates as files of their own, before anything is written into
//! the game.
//!
//! Everything that reads a level addresses a resource by the number of its
//! stream in the list of game files and by its position in that file
//! ([`crate::level::Location`]). A stream made with
//! [`Workspace::add_stream`] has neither until [`Workspace::apply`].
//! [`Workspace::stage`] gives it both: the stream is built from the
//! resources the workspace added to it, in the order `apply` writes them,
//! into `<workspace>/staged/`, and the returned [`GameInstance`] lists it
//! after the game's own streams ([`GameInstance::with_staged`]) with the
//! parents and the hash its tables of contents will give it. The level
//! catalog, the resource index, [`Workspace::current_data`] and
//! [`Workspace::set_resources`] then work on it as on a stream that was
//! applied, and nothing in the game folder changes.
//!
//! A staged file is named by its contents and never rewritten, so a file
//! that is memory-mapped by a reader stays as it was. The files of the game
//! given to [`Workspace::stage`] are kept; those of states before it are
//! removed.

use std::fs;
use std::path::PathBuf;

use sha2::{Digest, Sha256};

use super::{ResourceKey, Workspace, file_order};
use crate::container::ustream::{self, ResourceHeader, Storage};
use crate::game::{GameInstance, StagedStream};
use crate::{Error, Result};

/// Directory of the staged streams inside the workspace directory.
pub const STAGED_DIR: &str = "staged";

impl Workspace {
    /// The streams the workspace creates with the keys of their resources in
    /// file order: what [`Self::stage`] builds the list of streams and the
    /// positions of resources from. Two workspaces with equal layouts stage
    /// games in which every [`crate::level::Location`] means the same
    /// resource.
    pub fn staged_layout(&self) -> Vec<(String, Vec<ResourceKey>)> {
        self.new_streams
            .keys()
            .map(|name| {
                let keys = self
                    .new_stream_resources(name)
                    .iter()
                    .map(|c| c.key.clone())
                    .collect();
                (name.clone(), keys)
            })
            .collect()
    }

    /// `game` with every stream this workspace creates staged as a file of
    /// the workspace directory (see the module documentation). Without new
    /// streams this is the game folder as it is. Staged streams of `game`
    /// are replaced.
    ///
    /// The data in a staged file is the workspace's at the time of the call;
    /// later edits are read through the workspace, as edits of game streams
    /// are, until the next call.
    pub fn stage(&self, game: &GameInstance) -> Result<GameInstance> {
        // The files `game` was staged with stay for one more staging: who
        // still holds that game (a level being loaded) can read them.
        let mut keep: Vec<PathBuf> = game
            .staged_streams()
            .into_iter()
            .map(|s| game.streams()[s].path.clone())
            .collect();
        let game = game.on_disk();
        let dir = self.dir.join(STAGED_DIR);
        if self.new_streams.is_empty() {
            if dir.is_dir() {
                remove_stale(&dir, &keep);
            }
            return Ok(game);
        }
        fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;
        let mut staged = Vec::new();
        for stream in self.new_streams.values() {
            let mut changes: Vec<&super::Change> = self
                .state
                .values()
                .filter(|c| c.key.stream == stream.name && c.is_added())
                .collect();
            file_order(&mut changes);
            let mut hasher = Sha256::new();
            hasher.update(stream.name.as_bytes());
            for change in &changes {
                let added = change.added.as_ref().expect("filtered above");
                hasher.update([0]);
                hasher.update(change.key.to_string().as_bytes());
                hasher.update(change.payload.sha256.as_bytes());
                hasher.update(added.mem_policy.to_le_bytes());
                hasher.update(added.alignment.to_le_bytes());
            }
            let digest = hasher.finalize();
            let tag: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
            let flat: String = stream
                .name
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                .collect();
            // Not `.str`: a scan of a folder must never take it for a game file.
            let path = dir.join(format!("{flat}.{tag}.staged"));
            if !path.is_file() {
                let mut resources: Vec<(ResourceHeader, Vec<u8>)> = Vec::new();
                for change in &changes {
                    let data = self.store.get(&change.payload)?;
                    let added = change.added.as_ref().expect("filtered above");
                    resources.push((added.header(&change.key, data.len() as u64), data));
                }
                // Stored raw: the file is read here, not by the game.
                let borrowed: Vec<(ResourceHeader, &[u8], Storage)> = resources
                    .iter()
                    .map(|(h, d)| (*h, d.as_slice(), Storage::Raw))
                    .collect();
                let built = ustream::build(&borrowed)?;
                crate::atomic::write(&path, &built)?;
            }
            let mut parents = vec![stream.parent.clone()];
            for parent in stream.toc_parents.values() {
                if !parents.contains(parent) {
                    parents.push(parent.clone());
                }
            }
            keep.push(path.clone());
            staged.push(StagedStream {
                name: stream.name.clone(),
                path,
                parents,
                hash: stream.hash,
            });
        }
        remove_stale(&dir, &keep);
        game.with_staged(&staged)
    }
}

/// Removes the staged files of earlier states. A file another program still
/// maps may refuse to go; it is removed the next time.
fn remove_stale(dir: &std::path::Path, keep: &[PathBuf]) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "staged") && !keep.contains(&path) {
            let _ = fs::remove_file(&path);
        }
    }
}

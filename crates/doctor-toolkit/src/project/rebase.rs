//! Rebasing a project (stage 111): its edits against the mods it is made
//! on as they are now.
//!
//! The base of a project ([`super::base`]) names the mods; the workspace
//! records, for each edit, the data those mods gave when it was made
//! ([`crate::workspace::Change::made_on`]). When a mod of the base is
//! updated — the difference [`super::BaseDifference::Changed`] — or the
//! mods of the game are others, some edits stand on data the game no longer
//! gives: [`Project::stale`] finds them and [`Project::rebase`] moves them
//! ([`crate::workspace::rebase`]).
//!
//! What the project adds here to its workspace is one thing: its own mod.
//! A project installed into the mods and written into the game is in the
//! files the editor reads, and a file that holds the project's own change
//! is not what that change stands on. For those resources the game's data
//! is taken as the other enabled mods give it ([`Project::given`]).

use crate::Result;
use crate::game::GameInstance;
use crate::workspace::{Given, RebaseReport, RebaseRequest, ResourceKey, Stale, Workspace};

use super::Project;

impl Project {
    /// What the game gives the resources the project changes where its
    /// files hold the project's own mod — the mod of its id, or one
    /// installed from its workspace ([`crate::mods::game_without_mod`]).
    pub fn given(&self, game: &GameInstance, ws: &Workspace) -> Result<Given> {
        crate::mods::game_without_mod(game, ws, &[&self.config.project.id])
    }

    /// The changes of the project in `ws` that were made on other data
    /// than the game gives now ([`Workspace::stale`]): what
    /// [`Self::rebase`] moves, and what a mod of the project would take
    /// back of its base as long as it is not moved.
    pub fn stale(&self, game: &GameInstance, ws: &Workspace) -> Result<Vec<Stale>> {
        let mut stale = ws.stale_given(game, &self.given(game, ws)?)?;
        if !self.has_own_workspace() {
            // The workspace of the game holds other edits as well.
            let keys = self.changed_keys(ws);
            stale.retain(|s| keys.contains(&s.key));
        }
        Ok(stale)
    }

    /// Rebases the stale changes of the project in `ws`
    /// ([`Workspace::rebase_given`]) as one undoable operation: merges
    /// those of `request.keys` (`None`: all of the project), keeps those of
    /// `request.keep` and gives up those of `request.take_base`.
    ///
    /// A project with source files has the rebased data in its workspace
    /// only: its files follow with [`Project::pull`].
    pub fn rebase(
        &self,
        game: &GameInstance,
        ws: &mut Workspace,
        request: &RebaseRequest<'_>,
    ) -> Result<RebaseReport> {
        let given = self.given(game, ws)?;
        if self.has_own_workspace() || request.keys.is_some() {
            return ws.rebase_given(game, &given, request);
        }
        // Of the workspace of the game, only what is the project's.
        let own = self.changed_keys(ws);
        let keys: Vec<ResourceKey> = ws
            .stale_given(game, &given)?
            .into_iter()
            .map(|s| s.key)
            .filter(|key| own.contains(key))
            .filter(|key| !request.keep.contains(key) && !request.take_base.contains(key))
            .collect();
        let request = RebaseRequest {
            keys: Some(&keys),
            ..*request
        };
        ws.rebase_given(game, &given, &request)
    }
}

/// The stale changes `stale` in words, for a warning: how many, the first
/// few by key, and what to do.
pub fn stale_warning(stale: &[Stale]) -> Option<String> {
    const NAMED: usize = 4;
    if stale.is_empty() {
        return None;
    }
    let mut names: Vec<String> = stale
        .iter()
        .take(NAMED)
        .map(|s| s.key.to_string())
        .collect();
    if stale.len() > NAMED {
        names.push(format!("{} more", stale.len() - NAMED));
    }
    Some(format!(
        "{} edit(s) were made on other data than the game gives now ({}): a mod of them would \
         take back what the mods below changed since; rebase them (`doctor-toolkit project \
         rebase`, Rebase in the editor)",
        stale.len(),
        names.join(", ")
    ))
}

#[cfg(test)]
mod tests;

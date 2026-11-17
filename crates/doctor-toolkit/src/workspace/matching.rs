//! Changes of a workspace picked by text: an exact key, or a part of keys.
//! What `workspace revert` of the command line and the tool `revert` of the
//! MCP server take changes back by.

use super::{FileKey, ResourceKey, Workspace};
use crate::{Error, Result};

/// What [`Workspace::revert_matching`] took back.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reverted {
    pub resources: Vec<ResourceKey>,
    pub files: Vec<FileKey>,
    /// Label of the operation; `None` when nothing changed.
    pub operation: Option<String>,
}

impl Workspace {
    /// Keys of the changed resources matching `queries`: a query is a whole
    /// key of a change, or text that keys contain. A query that matches
    /// nothing is an error.
    pub fn matching_keys(&self, queries: &[String]) -> Result<Vec<ResourceKey>> {
        let mut keys = Vec::new();
        for query in queries {
            if let Ok(key) = query.parse::<ResourceKey>()
                && self.change(&key).is_some()
            {
                keys.push(key);
                continue;
            }
            let found: Vec<_> = self
                .changes()
                .filter(|c| c.key.to_string().contains(query.as_str()))
                .map(|c| c.key.clone())
                .collect();
            if found.is_empty() {
                return Err(Error::InvalidArgument(format!(
                    "no change matches `{query}`"
                )));
            }
            keys.extend(found);
        }
        keys.sort();
        keys.dedup();
        Ok(keys)
    }

    /// Keys of the changed files (movies, sounds of the archives) matching
    /// `queries`, as [`Self::matching_keys`] but without the error.
    pub fn matching_files(&self, queries: &[String]) -> Vec<FileKey> {
        self.file_changes()
            .map(|c| c.key.clone())
            .filter(|k| {
                let text = k.to_string();
                queries
                    .iter()
                    .any(|q| text == *q || text.contains(q.as_str()))
            })
            .collect()
    }

    /// Takes the changes matching `queries` out of the workspace (resources
    /// and files), as one undoable operation each kind.
    pub fn revert_matching(&mut self, queries: &[String]) -> Result<Reverted> {
        let files = self.matching_files(queries);
        let resources = if files.is_empty() {
            self.matching_keys(queries)?
        } else {
            self.matching_keys(queries).unwrap_or_default()
        };
        let label = format!("Revert {} change(s)", resources.len() + files.len());
        let mut any = false;
        if !resources.is_empty() {
            any |= self.revert(&resources, &label)?;
        }
        if !files.is_empty() {
            any |= self.revert_files(&files, &label)?;
        }
        Ok(Reverted {
            resources,
            files,
            operation: any.then_some(label),
        })
    }
}

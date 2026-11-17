//! The check of the workspace's edits before they are written into the
//! game (stage 103): every changed resource is read by its parser, objects
//! can show their models, references an edit adds resolve, conflicts with
//! the game files, messages of the changed SimGroups, the project.
//!
//! One function for every surface: the MCP tool `validate`, the command
//! `doctor-toolkit check-edits` and the editor's "Check edits".

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::create::{self, Provided, ReferenceChecker, TYPE_METAMODEL, TYPE_SIMGROUP, Target};
use crate::error::Result;
use crate::game::{GameInstance, ResourceIndex};
use crate::logic_graph::check::{ProblemKind, Severity};
use crate::project::{EntryState, Project};
use crate::resource::graph::{Graph, TYPE_GRAPH};
use crate::resource::lh2::StringTable;
use crate::resource::metamodel::MetaModel;
use crate::resource::simgroup::SimGroup;
use crate::text::TYPE_LH2;
use crate::workspace::{ChangeState, ResourceKey, StreamState, Workspace};

/// What the check does, in words (part of the report).
pub const CHECKS: [&str; 7] = [
    "every changed resource is read by its parser (SimGroup, string table, navigation graph, metamodel, Lua, dialogue)",
    "SimGroup objects can show their models (the game crashes otherwise)",
    "navigation graphs: what the file cannot hold or the game cannot use",
    "references an edit adds resolve in the stream or the streams loaded before it",
    "conflicts with the game files, streams changed outside the workspace",
    "messages of changed SimGroups: a new name with the hash of a game message (error), heard but never sent and sent but never heard (warnings)",
    "the project: entries to build, missing files",
];

/// What to check.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Params {
    /// Only changes whose key contains this text (case does not matter).
    #[serde(default)]
    pub filter: Option<String>,
    /// The mod project whose state is checked too.
    #[serde(default)]
    pub project: Option<PathBuf>,
}

/// The project a finding is about: its folder, or just "the project".
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum ProjectRef {
    Dir(PathBuf),
    Any(bool),
}

/// One finding: what is wrong (`what`) and where.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Finding {
    /// The changed resource (`stream:type:ids`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<ProjectRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_entry: Option<String>,
    /// The message of a logic problem, its kind and the SimGroups using it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<ProblemKind>,
    pub what: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub groups: Option<Vec<String>>,
}

impl Finding {
    fn key(key: &ResourceKey, what: impl Into<String>) -> Self {
        Self {
            key: Some(key.to_string()),
            what: what.into(),
            ..Self::default()
        }
    }

    fn stream(stream: &str, what: impl Into<String>) -> Self {
        Self {
            stream: Some(stream.to_owned()),
            what: what.into(),
            ..Self::default()
        }
    }

    /// Where the finding is, in a few words (for lists).
    pub fn place(&self) -> String {
        if let Some(key) = &self.key {
            key.clone()
        } else if let Some(stream) = &self.stream {
            stream.clone()
        } else if let Some(entry) = &self.project_entry {
            entry.clone()
        } else if let Some(message) = &self.message {
            message.clone()
        } else if let Some(ProjectRef::Dir(dir)) = &self.project {
            dir.display().to_string()
        } else {
            String::new()
        }
    }
}

/// Result of [`validate`].
#[derive(Debug, Clone, Default, Serialize)]
pub struct Report {
    /// No errors (warnings do not count).
    pub ok: bool,
    /// Message names the edits introduce.
    pub new_messages: Vec<String>,
    pub checked_changes: usize,
    pub errors: Vec<Finding>,
    pub warnings: Vec<Finding>,
    pub pending: usize,
    pub applied: usize,
    pub game_running: bool,
    pub checks: Vec<&'static str>,
}

fn hex(v: u32) -> String {
    format!("{v:08x}")
}

/// A reference target in words: `material 0000abcd`.
pub fn target_text(t: &Target) -> String {
    match t {
        Target::Guid(g) => format!("resource {}", g.map(hex).join(".")),
        Target::Material(h) => format!("material {}", hex(*h)),
        Target::Texture(h) => format!("texture {}", hex(*h)),
    }
}

/// What the data of a changed resource says about itself: it is read by
/// its parser, and the parsers that know problems tell them.
fn check_data(
    game: &GameInstance,
    index: &ResourceIndex,
    ws: &Workspace,
    key: &ResourceKey,
    data: &[u8],
    original: Option<&[u8]>,
) -> Vec<String> {
    let tag = crate::resource::names::type_tags().display(key.type_id);
    let mut problems = Vec::new();
    let unreadable = |e: &dyn std::fmt::Display| format!("{tag} cannot be read: {e}");
    match key.type_id {
        TYPE_SIMGROUP => match SimGroup::parse(data) {
            Ok(after) => {
                let before = original.and_then(|o| SimGroup::parse(o).ok());
                match crate::level::compat::check_edit(
                    game,
                    index,
                    Some(ws),
                    before.as_ref(),
                    &after,
                ) {
                    Ok(list) => problems.extend(list),
                    Err(e) => problems.push(format!("models cannot be checked: {e}")),
                }
            }
            Err(e) => problems.push(unreadable(&e)),
        },
        TYPE_LH2 => match StringTable::parse(data) {
            Ok(t) => {
                if let Some(o) = original.and_then(|o| StringTable::parse(o).ok())
                    && o.strings.len() != t.strings.len()
                {
                    problems.push(format!(
                        "{} language column(s) instead of {}",
                        t.strings.len(),
                        o.strings.len()
                    ));
                }
            }
            Err(e) => problems.push(unreadable(&e)),
        },
        TYPE_GRAPH => match Graph::parse(data) {
            Ok(g) => problems.extend(g.problems()),
            Err(e) => problems.push(unreadable(&e)),
        },
        TYPE_METAMODEL => {
            if let Err(e) = MetaModel::parse(data) {
                problems.push(unreadable(&e));
            }
        }
        t if t == crate::hash::sdbm(b"lua") => {
            if let Err(e) = crate::resource::lua::Chunk::parse(data) {
                problems.push(unreadable(&e));
            }
        }
        t if t == crate::hash::sdbm(b"dlg") => {
            if let Err(e) = crate::resource::dlg::Dialogue::parse(data) {
                problems.push(unreadable(&e));
            }
        }
        _ => {}
    }
    problems
}

/// Checks the edits of `ws` against `game` (see the module).
pub fn validate(
    game: &GameInstance,
    index: &ResourceIndex,
    ws: &Workspace,
    params: &Params,
) -> Result<Report> {
    let status = ws.status(game)?;
    let mut errors: Vec<Finding> = Vec::new();
    let mut warnings: Vec<Finding> = Vec::new();
    if !status.fingerprint_matches {
        warnings.push(Finding {
            what: "the workspace was made for another build of the game".to_owned(),
            ..Finding::default()
        });
    }
    for (key, state) in &status.changes {
        if *state == ChangeState::Conflict {
            errors.push(Finding::key(
                key,
                "conflict: the game file changed since the workspace was applied",
            ));
        }
    }
    for s in &status.streams {
        if s.state == StreamState::ChangedExternally {
            warnings.push(Finding::stream(
                &s.stream,
                "changed outside the workspace: `apply` would need force and lose that change",
            ));
        }
        if s.state == StreamState::Missing {
            errors.push(Finding::stream(&s.stream, "missing from the game"));
        }
    }
    let filter = params.filter.as_deref().map(str::to_ascii_lowercase);
    // What the workspace adds to each stream also resolves references.
    let mut added: HashMap<String, Provided> = HashMap::new();
    for c in ws.changes().filter(|c| c.is_added()) {
        let data = ws.data(&c.key).ok().flatten();
        let header = c
            .added
            .as_ref()
            .expect("added")
            .header(&c.key, c.payload.size);
        added
            .entry(c.key.stream.clone())
            .or_default()
            .add(&header, data.as_deref());
    }
    let mut checker = ReferenceChecker::new(game)?;
    let game_textures: HashSet<u32> = index
        .entries
        .iter()
        .filter(|e| e.header.type_id == crate::resource::texture::TYPE_TG4H)
        .map(|e| e.header.ids[3])
        .collect();
    let parents = game.stream_parents().unwrap_or_default();
    let mut checked = 0usize;
    for change in ws.changes() {
        let key = &change.key;
        if filter
            .as_deref()
            .is_some_and(|f| !key.to_string().to_ascii_lowercase().contains(f))
        {
            continue;
        }
        checked += 1;
        let data = match ws.data(key) {
            Ok(Some(d)) => d,
            Ok(None) => continue,
            Err(e) => {
                errors.push(Finding::key(key, format!("payload unreadable: {e}")));
                continue;
            }
        };
        let original = ws.original_data(key).ok().flatten();
        for problem in check_data(game, index, ws, key, &data, original.as_deref()) {
            errors.push(Finding::key(key, problem));
        }
        // References resolve in the stream or a stream loaded before it: a
        // stream the workspace creates is loaded after its parents.
        let scope = ws.load_scope(&parents, &key.stream);
        let game_streams: Vec<usize> = scope.iter().filter_map(|s| game.find_stream(s)).collect();
        if game_streams.is_empty() {
            continue;
        }
        let Ok(targets) = create::references(key.type_id, &data) else {
            continue;
        };
        // Only what the edit adds: the game's own SimGroups name resources
        // of streams loaded apart from theirs, and that works in the game.
        let had: Vec<Target> = original
            .as_deref()
            .filter(|o| !o.is_empty())
            .and_then(|o| create::references(key.type_id, o).ok())
            .unwrap_or_default();
        for t in targets.into_iter().filter(|t| !had.contains(t)) {
            let by_workspace = scope
                .iter()
                .any(|s| added.get(s).is_some_and(|p| p.provides(&t)));
            let by_game = game_streams
                .iter()
                .any(|&s| checker.provides(s, &t).unwrap_or(true));
            // Textures of no stream are the engine's own: the game has them.
            let by_engine = matches!(t, Target::Texture(h) if !game_textures.contains(&h));
            if by_workspace || by_game || by_engine {
                continue;
            }
            errors.push(Finding::key(
                key,
                format!(
                    "unresolved reference: {} is in no stream loaded with it",
                    target_text(&t)
                ),
            ));
        }
    }
    let mut project_notes = Vec::new();
    if let Some(dir) = &params.project
        && let Ok(project) = Project::open(dir)
    {
        let pstatus = project.status_read_only(ws)?;
        let to_build = pstatus.to_build();
        if to_build > 0 {
            warnings.push(Finding {
                project: Some(ProjectRef::Dir(dir.clone())),
                what: format!(
                    "{to_build} entr(y/ies) changed since the last build: project_build records them"
                ),
                ..Finding::default()
            });
        }
        for e in &pstatus.entries {
            if let EntryState::Missing(file) = &e.state {
                errors.push(Finding {
                    project_entry: Some(e.id.clone()),
                    what: format!("missing file {file}"),
                    ..Finding::default()
                });
            }
        }
        project_notes.extend(pstatus.warnings.iter().cloned());
        project_notes.extend(project.game_warnings(game));
    }
    for w in project_notes {
        warnings.push(Finding {
            project: Some(ProjectRef::Any(true)),
            what: w,
            ..Finding::default()
        });
    }
    // Messages of the changed SimGroups: a new name with the hash of a game
    // message, messages left with one end.
    let messages = crate::logic_graph::check::check_workspace(ws);
    for p in &messages.problems {
        let item = Finding {
            message: Some(p.message.clone()),
            kind: Some(p.kind),
            what: p.what.clone(),
            groups: Some(p.groups.clone()),
            ..Finding::default()
        };
        match p.severity {
            Severity::Error => errors.push(item),
            Severity::Warning => warnings.push(item),
        }
    }
    let running = crate::play::game_processes();
    Ok(Report {
        ok: errors.is_empty(),
        new_messages: messages.custom,
        checked_changes: checked,
        errors,
        warnings,
        pending: status.count(ChangeState::Pending),
        applied: status.count(ChangeState::Applied),
        game_running: !running.is_empty(),
        checks: CHECKS.to_vec(),
    })
}

//! The check of the workspace's edits before they are written into the
//! game (stage 103): every changed resource is read by its parser, objects
//! can show their models, references an edit adds resolve (by the rule of
//! `navigation::placement`, with the streams a copy is needed in when not),
//! materials come before the meshes of their stream, copies of a changed
//! resource left with other data (`docs/analysis/resource-placement.md`
//! §6.1), conflicts with the game files, messages of the changed
//! SimGroups, the project.
//!
//! One function for every surface: the MCP tool `validate`, the command
//! `doctor-toolkit check-edits` and the editor's "Check edits".

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::create::{
    self, OrderItem, Provided, ReferenceChecker, TYPE_MATERIAL, TYPE_MESH,
    TYPE_MESHVOLATILE as TYPE_MESH_VOLATILE, TYPE_METAMODEL, TYPE_SIMGROUP, Target,
};
use crate::error::Result;
use crate::game::{GameInstance, ResourceIndex, StreamFile};
use crate::logic_graph::check::{ProblemKind, Severity};
use crate::navigation::placement;
use crate::project::{EntryState, Project};
use crate::resource::graph::{Graph, TYPE_GRAPH};
use crate::resource::lh2::StringTable;
use crate::resource::metamodel::MetaModel;
use crate::resource::simgroup::SimGroup;
use crate::text::TYPE_LH2;
use crate::workspace::store::Blob;
use crate::workspace::{ChangeState, ResourceKey, StreamState, Workspace};

/// What the check does, in words (part of the report).
pub const CHECKS: [&str; 9] = [
    "every changed resource is read by its parser (SimGroup, string table, navigation graph, metamodel, Lua, dialogue)",
    "SimGroup objects can show their models (the game crashes otherwise)",
    "navigation graphs: what the file cannot hold or the game cannot use",
    "references an edit adds resolve in the stream or the streams loaded with it (its prerequisites and the streams their SimGroup data loads); for one that does not, where the game has it and the streams a copy is needed in",
    "the materials of a mesh in a stream the workspace changes come before the mesh when only that stream holds them (the game binds materials when the mesh loads)",
    "copies of a changed resource (a material: the same name hash; else the same type and ids) left with other data in other streams (warnings: the game takes the copy it loads first)",
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
    t.text()
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
    let mut unresolved: Vec<(ResourceKey, Target)> = Vec::new();
    let mut checked = 0usize;
    for change in ws.changes() {
        let key = &change.key;
        if !passes(filter.as_deref(), key) {
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
        // References resolve in the stream or a stream loaded with it: its
        // prerequisites (a stream the workspace creates is loaded after its
        // parents) and the streams their data loads.
        let prerequisites = ws.load_scope(&parents, &key.stream);
        let game_streams: Vec<usize> = prerequisites
            .iter()
            .filter_map(|s| game.find_stream(s))
            .collect();
        if game_streams.is_empty() {
            continue;
        }
        let Ok(targets) = create::references(key.type_id, &data) else {
            continue;
        };
        let scope = load_scope(game, &mut checker, prerequisites, &game_streams);
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
            unresolved.push((key.clone(), t));
        }
    }
    errors.extend(unresolved_findings(game, index, ws, &parents, &unresolved));
    errors.extend(order_findings(
        game,
        ws,
        &mut checker,
        &parents,
        &added,
        filter.as_deref(),
    )?);
    warnings.extend(copy_findings(game, index, ws, filter.as_deref())?);
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

/// Whether `key` passes the filter of [`Params::filter`] (lower case).
fn passes(filter: Option<&str>, key: &ResourceKey) -> bool {
    filter.is_none_or(|f| key.to_string().to_ascii_lowercase().contains(f))
}

/// The streams loaded with a stream: its `prerequisites`
/// ([`Workspace::load_scope`], the stream first) and what the game streams
/// among them (`game_streams`) reach ([`ReferenceChecker::reach`]: the
/// streams their data loads).
fn load_scope(
    game: &GameInstance,
    checker: &mut ReferenceChecker<'_>,
    mut scope: Vec<String>,
    game_streams: &[usize],
) -> Vec<String> {
    for &s in game_streams {
        for r in checker.reach(s).unwrap_or_default() {
            let name = game.streams()[r].stream_name();
            if !scope.contains(&name) {
                scope.push(name);
            }
        }
    }
    scope
}

/// A few stream names: the first three and how many more.
fn some_streams(streams: &[String]) -> String {
    match streams.len() {
        0..=3 => streams.join(", "),
        n => format!("{} and {} more", streams[..3].join(", "), n - 3),
    }
}

/// The findings of references an edit adds that no stream loaded with
/// their stream satisfies: where the game has the target, and the streams
/// a copy is needed in — the rule of [`placement::place`]: every stream
/// that refers to it, except one under another such stream.
fn unresolved_findings(
    game: &GameInstance,
    index: &ResourceIndex,
    ws: &Workspace,
    parents: &HashMap<String, Vec<String>>,
    unresolved: &[(ResourceKey, Target)],
) -> Vec<Finding> {
    if unresolved.is_empty() {
        return Vec::new();
    }
    let mut users: BTreeMap<Target, Vec<String>> = BTreeMap::new();
    for (key, t) in unresolved {
        let list = users.entry(*t).or_default();
        if !list.contains(&key.stream) {
            list.push(key.stream.clone());
        }
    }
    let ancestors: HashMap<String, Vec<String>> = users
        .values()
        .flatten()
        .map(|u| {
            let mut a = ws.load_scope(parents, u);
            a.retain(|s| s != u);
            (u.clone(), a)
        })
        .collect();
    let materials = users
        .keys()
        .any(|t| matches!(t, Target::Material(_)))
        .then(|| crate::resource::material::game_copies(game));
    let name = |s: u32| game.streams()[s as usize].stream_name();
    let mut out = Vec::new();
    for (t, list) in &users {
        let mut holders: Vec<String> = match t {
            Target::Guid(ids) => index
                .entries
                .iter()
                .filter(|e| e.header.ids == *ids && e.header.type_id != TYPE_MESH_VOLATILE)
                .map(|e| name(e.stream))
                .collect(),
            Target::Texture(h) => index
                .entries
                .iter()
                .filter(|e| {
                    e.header.type_id == crate::resource::texture::TYPE_TG4H && e.header.ids[3] == *h
                })
                .map(|e| name(e.stream))
                .collect(),
            Target::Material(h) => materials
                .as_ref()
                .and_then(|m| m.get(h))
                .map(|l| l.iter().map(|&(s, _)| name(s as u32)).collect())
                .unwrap_or_default(),
        };
        for c in ws.changes().filter(|c| c.is_added()) {
            let header = c
                .added
                .as_ref()
                .expect("added")
                .header(&c.key, c.payload.size);
            let data = ws.data(&c.key).ok().flatten();
            let mut provided = Provided::default();
            provided.add(&header, data.as_deref());
            if provided.provides(t) {
                holders.push(c.key.stream.clone());
            }
        }
        holders.dedup();
        let copies = placement::place_by(list, |u| ancestors[u].as_slice());
        let what = if holders.is_empty() {
            format!(
                "unresolved reference: {} is in no stream loaded with it, nor in any stream of the game or the workspace",
                target_text(t)
            )
        } else {
            format!(
                "unresolved reference: {} is in no stream loaded with it; it is in {}: a copy is needed in {}",
                target_text(t),
                some_streams(&holders),
                copies.join(", ")
            )
        };
        for (key, _) in unresolved.iter().filter(|(_, u)| u == t) {
            out.push(Finding::key(key, what.clone()));
        }
    }
    out
}

/// The resources of stream `name` in the order the workspace writes them:
/// the original stream with the added resources at their places (a stream
/// the workspace creates: its resources in file order), each with its
/// data as the workspace leaves it when it is a mesh or a material.
fn written_order(
    game: &GameInstance,
    ws: &Workspace,
    name: &str,
) -> Result<Vec<(ResourceKey, Option<Vec<u8>>)>> {
    let wanted = |t: u32| t == TYPE_MESH || t == TYPE_MATERIAL;
    let Some(stream) = game
        .find_stream(name)
        .filter(|_| ws.new_stream(name).is_none())
    else {
        return ws
            .new_stream_resources(name)
            .into_iter()
            .map(|c| {
                let data = if wanted(c.key.type_id) {
                    ws.data(&c.key)?
                } else {
                    None
                };
                Ok((c.key.clone(), data))
            })
            .collect();
    };
    let original = StreamFile::open(game.original_path(&game.streams()[stream]))?;
    let layout = original.layout();
    // (place, after the additions there, order among them)
    let mut list: Vec<((usize, bool, u64), ResourceKey)> = layout
        .resources
        .iter()
        .enumerate()
        .map(|(i, r)| ((i, true, 0), ResourceKey::new(name, &r.header)))
        .collect();
    for c in ws.changes().filter(|c| c.key.stream == name) {
        let Some(added) = &c.added else { continue };
        let at = added
            .before
            .as_ref()
            .and_then(|b| b.find_in(layout))
            .unwrap_or(layout.resources.len());
        list.push(((at, false, added.order), c.key.clone()));
    }
    list.sort_by_key(|(place, _)| *place);
    list.into_iter()
        .map(|((at, original_resource, _), key)| {
            let data = if !wanted(key.type_id) {
                None
            } else if ws.change(&key).is_some() {
                ws.data(&key)?
            } else if original_resource {
                Some(original.resource_data(at)?)
            } else {
                None
            };
            Ok((key, data))
        })
        .collect()
}

/// Meshes the workspace leaves before their material in streams it changes
/// (added resources, their places, changed meshes and materials): the game
/// binds the materials of a mesh when the mesh loads, so a material only
/// its own stream holds must come before the mesh
/// ([`create::materials_after_meshes`]).
fn order_findings(
    game: &GameInstance,
    ws: &Workspace,
    checker: &mut ReferenceChecker<'_>,
    parents: &HashMap<String, Vec<String>>,
    added: &HashMap<String, Provided>,
    filter: Option<&str>,
) -> Result<Vec<Finding>> {
    let streams: BTreeSet<String> = ws
        .changes()
        .filter(|c| passes(filter, &c.key))
        .filter(|c| c.is_added() || matches!(c.key.type_id, TYPE_MESH | TYPE_MATERIAL))
        .map(|c| c.key.stream.clone())
        .collect();
    let mut out = Vec::new();
    for name in streams {
        let order = written_order(game, ws, &name)?;
        let items: Vec<OrderItem> = order
            .iter()
            .map(|(key, data)| match (key.type_id, data) {
                (TYPE_MATERIAL, Some(d)) => crate::io::read_u32(d, 8, crate::io::Endian::Little)
                    .map_or(OrderItem::Other, OrderItem::Material),
                (TYPE_MESH, Some(d)) => create::references(TYPE_MESH, d)
                    .map_or(OrderItem::Other, |t| {
                        OrderItem::Mesh(create::material_targets(&t))
                    }),
                _ => OrderItem::Other,
            })
            .collect();
        if !items.iter().any(|i| matches!(i, OrderItem::Mesh(_))) {
            continue;
        }
        // Streams loaded with this one, without it.
        let prerequisites = ws.load_scope(parents, &name);
        let own = game
            .find_stream(&name)
            .filter(|_| ws.new_stream(&name).is_none());
        let others: Vec<usize> = prerequisites
            .iter()
            .filter(|s| **s != name)
            .filter_map(|s| game.find_stream(s))
            .collect();
        let mut game_streams = others.clone();
        game_streams.extend(own);
        let scope = load_scope(game, checker, prerequisites, &game_streams);
        let found = create::materials_after_meshes(&items, |hash| {
            let t = Target::Material(hash);
            scope
                .iter()
                .filter(|s| **s != name)
                .any(|s| added.get(s).is_some_and(|p| p.provides(&t)))
                || own.is_some_and(|s| checker.provided_elsewhere(s, &t).unwrap_or(true))
                || others
                    .iter()
                    .any(|&s| checker.provides(s, &t).unwrap_or(true))
        });
        for f in found {
            out.push(Finding::key(
                &order[f.mesh].0,
                format!(
                    "material {:08x} comes after the mesh in the stream ({}): the game binds \
                     materials when the mesh loads; the material must come before the mesh",
                    f.material, order[f.at].0
                ),
            ));
        }
    }
    Ok(out)
}

/// `(stream, resource)` of resources of the game.
type Places = Vec<(usize, usize)>;

/// How copies of a resource are found: a material by the name hash in its
/// data (the game keeps materials by it), anything else by type and ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum CopyGroup {
    Material(u32),
    Key(u32, [u32; 4]),
}

/// Copies of a resource the workspace changes or adds that it leaves with
/// other data in other streams (warnings): the game uses the copy it loads
/// first, so which data shows depends on the order the streams load.
/// Copies whose data differed in the game already are left out.
fn copy_findings(
    game: &GameInstance,
    index: &ResourceIndex,
    ws: &Workspace,
    filter: Option<&str>,
) -> Result<Vec<Finding>> {
    let group_of = |c: &crate::workspace::Change| -> Option<CopyGroup> {
        if c.key.type_id == TYPE_MATERIAL {
            let data = ws.data(&c.key).ok().flatten()?;
            crate::io::read_u32(&data, 8, crate::io::Endian::Little)
                .ok()
                .map(CopyGroup::Material)
        } else {
            // SimGroups are not copies of each other; a texture's data and a
            // mesh's vertices go with the header and the mesh: one warning
            // for the pair.
            let paired = [
                TYPE_SIMGROUP,
                crate::resource::texture::TYPE_TG4D,
                TYPE_MESH_VOLATILE,
            ];
            (c.key.ids != [0; 4] && !paired.contains(&c.key.type_id))
                .then_some(CopyGroup::Key(c.key.type_id, c.key.ids))
        }
    };
    // Changes by group, in the order of the workspace.
    let mut groups: BTreeMap<CopyGroup, Vec<&crate::workspace::Change>> = BTreeMap::new();
    for c in ws.changes() {
        if let Some(g) = group_of(c) {
            groups.entry(g).or_default().push(c);
        }
    }
    groups.retain(|_, list| list.iter().any(|c| passes(filter, &c.key)));
    if groups.is_empty() {
        return Ok(Vec::new());
    }
    let materials = groups
        .keys()
        .any(|g| matches!(g, CopyGroup::Material(_)))
        .then(|| crate::resource::material::game_copies(game));
    // `(stream, resource)` of the game's copies of each key.
    let mut keys: HashMap<(u32, [u32; 4]), Places> = groups
        .keys()
        .filter_map(|g| match g {
            CopyGroup::Key(t, ids) => Some(((*t, *ids), Vec::new())),
            CopyGroup::Material(_) => None,
        })
        .collect();
    for e in &index.entries {
        if let Some(list) = keys.get_mut(&(e.header.type_id, e.header.ids)) {
            list.push((e.stream as usize, e.resource as usize));
        }
    }
    let mut out = Vec::new();
    for (group, changes) in &groups {
        let in_game: &[(usize, usize)] = match group {
            CopyGroup::Material(h) => materials
                .as_ref()
                .and_then(|m| m.get(h))
                .map_or(&[], Vec::as_slice),
            CopyGroup::Key(t, ids) => keys.get(&(*t, *ids)).map_or(&[], Vec::as_slice),
        };
        // Every copy: key, data now, data in the game (`None`: added).
        let mut copies: Vec<(ResourceKey, Blob, Option<Blob>)> = Vec::new();
        for &(s, r) in in_game {
            let file = game.open_stream(s)?;
            let Some(header) = file.layout().resources.get(r).map(|r| r.header) else {
                continue;
            };
            let key = ResourceKey::new(&game.streams()[s].stream_name(), &header);
            if let Some(c) = ws.change(&key) {
                if !c.is_added() && group_of(c) == Some(*group) {
                    copies.push((key, c.payload.clone(), Some(c.made_on().clone())));
                }
                continue;
            }
            let data = Blob::of(&ws.current_data(game, s, r)?);
            copies.push((key, data.clone(), Some(data)));
        }
        for c in changes.iter().filter(|c| c.is_added()) {
            copies.push((c.key.clone(), c.payload.clone(), None));
        }
        let Some(first) = changes.iter().find(|c| passes(filter, &c.key)) else {
            continue;
        };
        let Some((_, now, base)) = copies.iter().find(|(k, ..)| k == &first.key).cloned() else {
            continue;
        };
        let mut differ: Vec<String> = copies
            .iter()
            .filter(|(k, n, b)| {
                k != &first.key && *n != now && (base.is_none() || b.is_none() || *b == base)
            })
            .map(|(k, ..)| k.stream.clone())
            .collect();
        differ.dedup();
        if differ.is_empty() {
            continue;
        }
        let what = match group {
            CopyGroup::Material(h) => {
                let name = ws
                    .data(&first.key)
                    .ok()
                    .flatten()
                    .and_then(|d| crate::resource::material::Material::parse(&d).ok())
                    .map_or_else(|| format!("{h:08x}"), |m| m.name);
                format!("material {name}")
            }
            CopyGroup::Key(t, ids) => format!(
                "{} {}",
                crate::resource::names::type_tags().display(*t),
                ids.map(hex).join(".")
            ),
        };
        out.push(Finding::key(
            &first.key,
            format!(
                "copies of {what} are left with other data in {} of {} stream(s): {}; the game \
                 takes whichever copy it loads first",
                differ.len(),
                copies.len(),
                some_streams(&differ)
            ),
        ));
    }
    Ok(out)
}

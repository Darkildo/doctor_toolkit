//! Checks of the messages that edits add to SimGroups.
//!
//! A wire is nothing but one name at a sender and a listener, and on the bus
//! the name is `sdbm(lower(name))`. So an edit can go wrong in ways the game
//! never reports:
//!
//! - a new name has the hash of a message of the game: the game cannot tell
//!   them apart, listeners of one fire on the other — an error;
//! - after the edit somebody listens to a message nobody sends, or sends one
//!   nobody hears — a warning (a cut-scene, a Lua script or the engine may
//!   still be the other end; the text says what is known).
//!
//! The state of the game before the edit comes from the catalogue of
//! [`crate::game_types::MessageCatalog`] (the files of PC 1.0.789.0), the
//! change from comparing each edited SimGroup with its original.

use std::collections::BTreeMap;

use serde::Serialize;

use super::schema::{LogicSchema, PinKind};
use crate::game_types::message::{self, MessageCatalog, MessageId};
use crate::hash::string_id;
use crate::resource::simgroup::{SimGroup, Value};

/// How bad a problem is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

/// What is wrong with a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProblemKind {
    /// A new name with the hash of a game message.
    HashCollision,
    /// Two new names of the edit with one hash.
    HashCollisionInEdit,
    /// Heard after the edit, sent by nobody.
    HeardNeverSent,
    /// Sent after the edit, heard by nobody.
    SentNeverHeard,
}

/// A problem with one message of an edit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MessageProblem {
    pub severity: Severity,
    pub kind: ProblemKind,
    /// The name as the edit spells it.
    pub message: String,
    pub hash: u32,
    /// What is wrong, in words.
    pub what: String,
    /// Labels of the edited SimGroups that use the message.
    pub groups: Vec<String>,
}

/// A SimGroup before and after an edit.
pub struct EditedGroup<'a> {
    /// How the group is called in reports (its key or stream).
    pub label: String,
    /// The group as the game has it; `None` for a group the edit creates.
    pub original: Option<&'a SimGroup>,
    pub edited: &'a SimGroup,
}

#[derive(Default)]
struct Use {
    /// Spellings in the edited groups.
    names: Vec<String>,
    sends: i64,
    hears: i64,
    /// Sends and subscriptions the originals of the edited groups had.
    sends_before: i64,
    hears_before: i64,
    groups: Vec<String>,
}

fn count(schema: &LogicSchema, group: &SimGroup, mut each: impl FnMut(&str, PinKind)) {
    for o in &group.objects {
        for op in schema.object_pins(group, o) {
            if let (PinKind::In | PinKind::Out, Some(Value::String(s))) = (op.attr.pin, &op.value)
                && !s.is_empty()
            {
                each(s, op.attr.pin);
            }
        }
    }
}

fn collect(schema: &LogicSchema, groups: &[EditedGroup<'_>]) -> BTreeMap<u32, Use> {
    let mut uses: BTreeMap<u32, Use> = BTreeMap::new();
    for g in groups {
        count(schema, g.edited, |name, pin| {
            let u = uses.entry(string_id(name)).or_default();
            if !u.names.iter().any(|n| n == name) {
                u.names.push(name.to_owned());
            }
            if !u.groups.contains(&g.label) {
                u.groups.push(g.label.clone());
            }
            if pin == PinKind::Out {
                u.sends += 1;
            } else {
                u.hears += 1;
            }
        });
        if let Some(original) = g.original {
            count(schema, original, |name, pin| {
                let u = uses.entry(string_id(name)).or_default();
                if pin == PinKind::Out {
                    u.sends_before += 1;
                } else {
                    u.hears_before += 1;
                }
            });
        }
    }
    uses
}

/// Names the edited groups use that the game does not have: the messages a
/// mod adds, sorted without regard to letter case.
pub fn custom_messages(schema: &LogicSchema, groups: &[EditedGroup<'_>]) -> Vec<String> {
    let mut out: Vec<String> = collect(schema, groups)
        .into_values()
        .filter(|u| u.sends + u.hears > 0)
        .filter_map(|u| u.names.into_iter().next())
        .filter(|n| matches!(MessageId::from_name(n), MessageId::Custom { .. }))
        .collect();
    out.sort_by_key(|n| n.to_ascii_lowercase());
    out
}

/// Checks the messages of edited SimGroups against the game.
pub fn check_messages(schema: &LogicSchema, groups: &[EditedGroup<'_>]) -> Vec<MessageProblem> {
    let catalog = MessageCatalog::gf2();
    let mut problems = Vec::new();
    for (hash, u) in collect(schema, groups) {
        let Some(name) = u.names.first().cloned() else {
            // Only the originals used it: the edit removed every use here.
            // What is left elsewhere in the game is checked below by counts.
            let Some(m) = catalog.get(hash) else { continue };
            dangling(&mut problems, hash, m.name, &u, catalog);
            continue;
        };
        // Two different new names with one hash.
        let mut lower: Vec<String> = u.names.iter().map(|n| n.to_ascii_lowercase()).collect();
        lower.sort();
        lower.dedup();
        if lower.len() > 1 {
            problems.push(MessageProblem {
                severity: Severity::Error,
                kind: ProblemKind::HashCollisionInEdit,
                message: name.clone(),
                hash,
                what: format!(
                    "{} have one hash {hash:#010x}: the game cannot tell them apart",
                    u.names
                        .iter()
                        .map(|n| format!("`{n}`"))
                        .collect::<Vec<_>>()
                        .join(" and ")
                ),
                groups: u.groups.clone(),
            });
        }
        for n in &u.names {
            if let Some(other) = message::collision(n) {
                problems.push(MessageProblem {
                    severity: Severity::Error,
                    kind: ProblemKind::HashCollision,
                    message: n.clone(),
                    hash,
                    what: format!(
                        "`{n}` has the hash {hash:#010x} of the game message `{other}`: the game cannot tell them apart — rename it"
                    ),
                    groups: u.groups.clone(),
                });
            }
        }
        dangling(&mut problems, hash, &name, &u, catalog);
    }
    problems
}

/// Adds a warning when the edit leaves the message with one end only.
fn dangling(
    problems: &mut Vec<MessageProblem>,
    hash: u32,
    name: &str,
    u: &Use,
    catalog: &MessageCatalog,
) {
    // The engine sends and hears its own messages.
    if message::engine_name(hash).is_some() {
        return;
    }
    let game = catalog.get(hash);
    let total = |uses: Vec<crate::game_types::PinUse>| -> i64 {
        uses.iter().map(|p| i64::from(p.count)).sum()
    };
    let (game_sends, game_hears) =
        game.map_or((0, 0), |m| (total(m.senders()), total(m.listeners())));
    let (d_sends, d_hears) = (u.sends - u.sends_before, u.hears - u.hears_before);
    if d_sends == 0 && d_hears == 0 {
        return;
    }
    let (sends, hears) = (game_sends + d_sends, game_hears + d_hears);
    let mentions = game.map(|m| m.mentions()).unwrap_or_default();
    let elsewhere = if mentions.is_empty() {
        String::new()
    } else {
        format!(
            "; named by {} (it may be the other end)",
            mentions
                .iter()
                .map(|(label, n)| format!(
                    "{n} {}",
                    match *label {
                        "lua" => "Lua script(s)",
                        "seq" => "cut-scene(s)",
                        "score" => "score table(s)",
                        other => other,
                    }
                ))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    // Only what the edit makes worse: a new listener of a silent message, or
    // the last sender taken away.
    if hears > 0 && sends <= 0 && (d_hears > 0 || game_sends > 0) {
        problems.push(MessageProblem {
            severity: Severity::Warning,
            kind: ProblemKind::HeardNeverSent,
            message: name.to_owned(),
            hash,
            what: format!(
                "`{name}` is heard by {hears} object(s), but no SimGroup object sends it{elsewhere}"
            ),
            groups: u.groups.clone(),
        });
    }
    if sends > 0 && hears <= 0 && (d_sends > 0 || game_hears > 0) {
        problems.push(MessageProblem {
            severity: Severity::Warning,
            kind: ProblemKind::SentNeverHeard,
            message: name.to_owned(),
            hash,
            what: format!(
                "`{name}` is sent by {sends} object(s), but no SimGroup object hears it{elsewhere}"
            ),
            groups: u.groups.clone(),
        });
    }
}

/// What the checks say about the SimGroups a workspace changes or adds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct WorkspaceMessages {
    /// SimGroups compared with their originals.
    pub groups: usize,
    /// Changed SimGroups that could not be read: key and error.
    pub unreadable: Vec<(String, String)>,
    /// Messages the workspace adds to the game, sorted.
    pub custom: Vec<String>,
    pub problems: Vec<MessageProblem>,
}

impl WorkspaceMessages {
    pub fn errors(&self) -> impl Iterator<Item = &MessageProblem> {
        self.problems
            .iter()
            .filter(|p| p.severity == Severity::Error)
    }

    pub fn warnings(&self) -> impl Iterator<Item = &MessageProblem> {
        self.problems
            .iter()
            .filter(|p| p.severity == Severity::Warning)
    }
}

/// Checks the messages of every SimGroup the workspace changes or adds
/// against the game: new names with the hash of a game message, messages
/// left with one end. Only reads.
pub fn check_workspace(ws: &crate::workspace::Workspace) -> WorkspaceMessages {
    check_workspace_keys(ws, |_| true)
}

/// [`check_workspace`] for the changes whose key `selected` accepts (the
/// changes that go into a mod).
pub fn check_workspace_keys(
    ws: &crate::workspace::Workspace,
    selected: impl Fn(&crate::workspace::ResourceKey) -> bool,
) -> WorkspaceMessages {
    let schema = LogicSchema::gf2();
    let mut out = WorkspaceMessages::default();
    let mut loaded: Vec<(String, Option<SimGroup>, SimGroup)> = Vec::new();
    for change in ws.changes() {
        let key = &change.key;
        if key.type_id != crate::create::TYPE_SIMGROUP || !selected(key) {
            continue;
        }
        let label = key.to_string();
        let edited = match ws.data(key) {
            Ok(Some(data)) => match SimGroup::parse(&data) {
                Ok(g) => g,
                Err(e) => {
                    out.unreadable.push((label, e.to_string()));
                    continue;
                }
            },
            // A removed group: nothing of it is left to check.
            Ok(None) => continue,
            Err(e) => {
                out.unreadable.push((label, e.to_string()));
                continue;
            }
        };
        let original = if change.is_added() {
            None
        } else {
            ws.original_data(key)
                .ok()
                .flatten()
                .filter(|d| !d.is_empty())
                .and_then(|d| SimGroup::parse(&d).ok())
        };
        loaded.push((label, original, edited));
    }
    let groups: Vec<EditedGroup<'_>> = loaded
        .iter()
        .map(|(label, original, edited)| EditedGroup {
            label: label.clone(),
            original: original.as_ref(),
            edited,
        })
        .collect();
    out.groups = groups.len();
    out.custom = custom_messages(schema, &groups);
    out.problems = check_messages(schema, &groups);
    out
}

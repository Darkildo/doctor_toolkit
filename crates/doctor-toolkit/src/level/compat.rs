//! Whether a SimGroup object can show a metamodel.
//!
//! The game crashes when it creates an object whose class expects
//! something of its model that the model lacks: a `Destructible` copied
//! from a chandelier (it breaks: `CMD_SetThrowableFlags` 64, interior
//! lighting) that shows an ashtray (one plain `State`) reads a null pointer
//! at `0x008CAE17` while the level starts (`docs/analysis/game-bench.md`
//! §10.3, runs C and D).
//!
//! Not every `Destructible` needs destructible states: 925 of the 16 811
//! `Destructible` objects of the game show metamodels with only a `State`.
//! None of those 925 carries the attributes in [`BREAKING_ATTRIBUTES`] or
//! throwable flags other than 0 and 128, while thousands of objects with
//! destructible states do. [`model_problem`] refuses exactly that pairing:
//! an object set up to break with a model that has nothing to break into.
//! New rules go into [`model_problem`] with the corpus counts that justify
//! them.

use crate::Result;
use crate::game::{GameInstance, ResourceIndex};
use crate::resource::metamodel::MetaModel;
use crate::resource::simgroup::{SimGroup, Value};
use crate::workspace::Workspace;

use super::TYPE_METAMODEL;

/// State types that let an object break.
pub const DESTRUCTIBLE_STATES: &[&str] = &["DestructibleState", "BuildingDestructibleState"];

/// `(class, attribute)` pairs that, in the game, only objects whose
/// metamodel has [`DESTRUCTIBLE_STATES`] carry (any non-zero value).
pub const BREAKING_ATTRIBUTES: &[(&str, &str)] = &[
    ("Destructible", "CMD_SetDestroyMetamodelOnDeath"),
    ("Destructible", "CMD_SetTargetingOffset"),
    ("Destructible", "CMD_SetTranslationX"),
    ("Destructible", "CMD_SetTranslationY"),
    ("Destructible", "CMD_SetTranslationZ"),
    ("Entity", "CMD_SetInteriorLit"),
];

/// Throwable flags the game's `Destructible` objects without destructible
/// states have (0 or 128); other bits (1, 64, 256) come only with them.
const PLAIN_THROWABLE_FLAGS: i32 = 128;

/// Why object `object` of `group` must not show `model`, or `None` when it may.
pub fn model_problem(group: &SimGroup, object: usize, model: &MetaModel) -> Option<String> {
    let o = group.objects.get(object)?;
    let destructible = group
        .object_packets(o)
        .any(|p| p.class_name == Some("Destructible"));
    let breakable_model = model
        .states
        .iter()
        .any(|s| DESTRUCTIBLE_STATES.contains(&s.type_name.as_str()));
    if !destructible || breakable_model {
        return None;
    }
    let mut reasons = Vec::new();
    for packet in group.object_packets(o) {
        let class = packet.class_name.unwrap_or("");
        for attr in &packet.attrs {
            let name = attr.name.unwrap_or("");
            let set = !matches!(attr.value, Value::Int(0));
            if set && BREAKING_ATTRIBUTES.contains(&(class, name)) {
                reasons.push(format!("{class}.{name}"));
            }
            if class == "Destructible"
                && name == "CMD_SetThrowableFlags"
                && let Value::Int(flags) = attr.value
                && flags & !PLAIN_THROWABLE_FLAGS != 0
            {
                reasons.push(format!("{class}.{name}={flags}"));
            }
        }
    }
    if reasons.is_empty() {
        return None;
    }
    let states: Vec<&str> = model.states.iter().map(|s| s.type_name.as_str()).collect();
    Some(format!(
        "object {object} ({}) is set up to break ({}), but metamodel {:08x} has no \
         destructible states ({}); the game crashes on such objects. Pick a model with \
         DestructibleState states or a template object that does not break",
        o.class_name().unwrap_or("Destructible"),
        reasons.join(", "),
        model.ids[3],
        if states.is_empty() {
            "no states".to_owned()
        } else {
            states.join(", ")
        }
    ))
}

/// The metamodel with id `id` as the workspace has it: a resource the
/// workspace added, or one of the game (with the workspace's changes).
/// `None` when neither has it.
pub fn find_metamodel(
    game: &GameInstance,
    index: &ResourceIndex,
    ws: Option<&Workspace>,
    id: u32,
) -> Result<Option<MetaModel>> {
    if let Some(ws) = ws {
        for change in ws.changes() {
            if change.key.type_id == TYPE_METAMODEL
                && change.key.ids[3] == id
                && let Some(data) = ws.data(&change.key)?
            {
                return MetaModel::parse(&data).map(Some);
            }
        }
    }
    let Some(entry) = index
        .entries
        .iter()
        .find(|e| e.header.type_id == TYPE_METAMODEL && e.header.ids[3] == id)
    else {
        return Ok(None);
    };
    let (stream, resource) = (entry.stream as usize, entry.resource as usize);
    let data = match ws {
        Some(ws) => ws.current_data(game, stream, resource)?,
        None => game.open_stream(stream)?.resource_data(resource)?,
    };
    MetaModel::parse(&data).map(Some)
}

/// Problems of the objects of `after` whose model or attributes differ
/// from `before` (every object when `before` is `None`), or that are new.
pub fn check_edit(
    game: &GameInstance,
    index: &ResourceIndex,
    ws: Option<&Workspace>,
    before: Option<&SimGroup>,
    after: &SimGroup,
) -> Result<Vec<String>> {
    let mut problems = Vec::new();
    for (i, o) in after.objects.iter().enumerate() {
        let Some(model) = after.model(o) else {
            continue;
        };
        let unchanged = before.is_some_and(|b| {
            b.objects
                .iter()
                .find(|old| old.guid == o.guid)
                .is_some_and(|old| same_object(b, old, after, o))
        });
        if unchanged {
            continue;
        }
        if let Some(mm) = find_metamodel(game, index, ws, model)?
            && let Some(problem) = model_problem(after, i, &mm)
        {
            problems.push(problem);
        }
    }
    Ok(problems)
}

fn same_object(
    a: &SimGroup,
    x: &crate::resource::simgroup::Object,
    b: &SimGroup,
    y: &crate::resource::simgroup::Object,
) -> bool {
    a.model(x) == b.model(y)
        && a.object_packets(x)
            .map(|p| (p.class, &p.attrs))
            .eq(b.object_packets(y).map(|p| (p.class, &p.attrs)))
}

/// [`check_edit`] for new data of resource `resource` of stream `stream`:
/// nothing to say when it is not a SimGroup.
pub fn check_resource_data(
    game: &GameInstance,
    index: &ResourceIndex,
    ws: Option<&Workspace>,
    (stream, resource): (usize, usize),
    data: &[u8],
) -> Result<Vec<String>> {
    let is_group = game
        .open_stream(stream)?
        .layout()
        .resources
        .get(resource)
        .is_some_and(|r| r.header.type_id == super::TYPE_SIMGROUP);
    if !is_group {
        return Ok(Vec::new());
    }
    let before = match ws {
        Some(ws) => ws.current_data(game, stream, resource)?,
        None => game.open_stream(stream)?.resource_data(resource)?,
    };
    let before = SimGroup::parse(&before)?;
    check_edit(game, index, ws, Some(&before), &SimGroup::parse(data)?)
}

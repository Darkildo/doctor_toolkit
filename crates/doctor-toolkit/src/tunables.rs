//! Game parameters ("tunables"): weapon damage, health, police heat and the
//! like, stored as attributes of SimGroup objects.
//!
//! A curated list with descriptions is `data/gf2/tunables.toml` ([`gf2`]):
//! each entry names a resource, a class, an attribute index and the objects
//! that carry the value (`docs/analysis/tunables.md` tells how the list was
//! built). [`find_attribute`] is the fallback for anything not on the list:
//! the values of one class attribute across the SimGroups of some streams.
//!
//! Reading and writing go through the workspace version of each resource;
//! [`apply`] records every changed value as one undoable operation.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::game::GameInstance;
use crate::resource::names::sim_classes;
use crate::resource::simgroup::{Kind, Object, Schema, SimGroup, Value};
use crate::workspace::{ResourceEdit, ResourceKey, StorageChoice, Workspace};
use crate::{Error, Result};

static GF2_TUNABLES: &str = include_str!("../../../data/gf2/tunables.toml");

const TYPE_SIMGROUP: u32 = crate::hash::sdbm(b"simgroup");

/// Categories of the curated list, in the order they are shown.
pub const CATEGORIES: [&str; 8] = [
    "weapons", "health", "player", "crew", "economy", "police", "vehicles", "world",
];

/// How a value is entered and stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ValueKind {
    Int,
    Float,
    /// An int that is 0 or 1.
    Bool,
}

/// One parameter: an attribute of some objects of a SimGroup.
#[derive(Debug, Clone, PartialEq)]
pub struct Tunable {
    /// Unique id, `<category>.<thing>.<what>`.
    pub id: String,
    pub category: String,
    pub label: String,
    pub description: String,
    /// The SimGroup resource.
    pub resource: ResourceKey,
    /// Class hash of the packet that holds the attribute.
    pub class: u32,
    pub attribute: u16,
    /// Object GUIDs (32 hexadecimal digits) or names; empty for every object
    /// of the resource with a packet of `class`.
    pub objects: Vec<String>,
    /// Names to show for [`Self::objects`], in the same order (optional).
    pub object_names: Vec<String>,
    pub kind: ValueKind,
    pub min: Option<f64>,
    pub max: Option<f64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    version: u32,
    #[serde(default)]
    tunable: Vec<Row>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    id: String,
    category: String,
    label: String,
    #[serde(default)]
    description: String,
    source: String,
    resource: String,
    class: Option<String>,
    attribute: Option<u16>,
    #[serde(default)]
    objects: Vec<String>,
    #[serde(default)]
    object_names: Vec<String>,
    kind: ValueKind,
    min: Option<f64>,
    max: Option<f64>,
}

/// Class hash from a class name of `data/gf2/sim_classes.txt` or eight
/// hexadecimal digits.
pub fn class_hash(text: &str) -> Option<u32> {
    let text = text.trim();
    sim_classes().resolve(text).or_else(|| {
        (text.len() == 8)
            .then(|| u32::from_str_radix(text, 16).ok())
            .flatten()
    })
}

/// Parses a list in the form of `data/gf2/tunables.toml`.
pub fn parse(text: &str) -> Result<Vec<Tunable>> {
    let bad = |what: String| Error::InvalidArgument(format!("tunables: {what}"));
    let file: File = toml::from_str(text).map_err(|e| bad(e.to_string()))?;
    if file.version != 1 {
        return Err(bad(format!("version {} is not supported", file.version)));
    }
    let mut out: Vec<Tunable> = Vec::with_capacity(file.tunable.len());
    for row in file.tunable {
        let at = |what: &str| bad(format!("{}: {what}", row.id));
        if out.iter().any(|t| t.id == row.id) {
            return Err(at("the id is used twice"));
        }
        if row.source != "simgroup" {
            return Err(at(&format!("source `{}` is not supported", row.source)));
        }
        let resource: ResourceKey = row
            .resource
            .parse()
            .map_err(|e| at(&format!("resource: {e}")))?;
        if resource.type_id != TYPE_SIMGROUP {
            return Err(at("the resource is not a SimGroup"));
        }
        let class = row
            .class
            .as_deref()
            .and_then(class_hash)
            .ok_or_else(|| at("no known class"))?;
        let attribute = row.attribute.ok_or_else(|| at("no attribute"))?;
        if !row.object_names.is_empty() && row.object_names.len() != row.objects.len() {
            return Err(at("object_names and objects differ in length"));
        }
        if let (Some(min), Some(max)) = (row.min, row.max)
            && min > max
        {
            return Err(at("min is above max"));
        }
        out.push(Tunable {
            id: row.id,
            category: row.category,
            label: row.label,
            description: row.description,
            resource,
            class,
            attribute,
            objects: row.objects,
            object_names: row.object_names,
            kind: row.kind,
            min: row.min,
            max: row.max,
        });
    }
    Ok(out)
}

/// The curated list of The Godfather II (PC).
pub fn gf2() -> &'static [Tunable] {
    static LIST: std::sync::OnceLock<Vec<Tunable>> = std::sync::OnceLock::new();
    LIST.get_or_init(|| parse(GF2_TUNABLES).expect("data/gf2/tunables.toml is valid"))
}

/// The value of one object.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectValue {
    /// Index of the object in the SimGroup.
    pub object: usize,
    /// Its name, or its GUID.
    pub label: String,
    /// The value; an attribute that is not stored is 0.
    pub value: f64,
}

/// The current values of a tunable.
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    pub stream: usize,
    pub resource: usize,
    pub objects: Vec<ObjectValue>,
}

impl Reading {
    /// The distinct values, in order of first appearance.
    pub fn distinct(&self) -> Vec<f64> {
        let mut out: Vec<f64> = Vec::new();
        for o in &self.objects {
            if !out.contains(&o.value) {
                out.push(o.value);
            }
        }
        out
    }
}

/// `(stream, resource)` of a resource key in the game.
pub fn locate(game: &GameInstance, key: &ResourceKey) -> Result<(usize, usize)> {
    let stream = game.find_stream(&key.stream).ok_or_else(|| {
        Error::InvalidArgument(format!("stream {} is not in the game", key.stream))
    })?;
    let file = game.open_stream(stream)?;
    let resource = key
        .find_in(file.layout())
        .ok_or_else(|| Error::InvalidArgument(format!("{key} is not in the game")))?;
    Ok((stream, resource))
}

/// Data of a resource as the editor shows it: the workspace version, or the game's.
fn current(
    game: &GameInstance,
    ws: Option<&Workspace>,
    stream: usize,
    resource: usize,
) -> Result<Vec<u8>> {
    match ws {
        Some(ws) => ws.current_data(game, stream, resource),
        None => game.open_stream(stream)?.resource_data(resource),
    }
}

fn label_of(group: &SimGroup, object: &Object) -> String {
    group
        .object_name(object)
        .map_or_else(|| object.guid.to_string(), str::to_owned)
}

/// A stored value as a number; a value stored as a hash (the schema could
/// not tell its kind) is read as the bits of `kind`.
fn number(value: Option<&Value>, kind: ValueKind) -> Option<f64> {
    match value {
        None => Some(0.0),
        Some(Value::Int(v)) => Some(f64::from(*v)),
        Some(Value::Float(v)) => Some(f64::from(*v)),
        Some(Value::Hash(h)) => Some(match kind {
            ValueKind::Float => f64::from(f32::from_bits(h.0)),
            ValueKind::Int | ValueKind::Bool => f64::from(h.0 as i32),
        }),
        Some(_) => None,
    }
}

fn has_class(group: &SimGroup, object: &Object, class: u32) -> bool {
    group.object_packets(object).any(|p| p.class.0 == class)
}

impl Tunable {
    /// The class name, or its hash.
    pub fn class_name(&self) -> String {
        sim_classes().display(self.class)
    }

    /// The objects of `group` the tunable sets.
    pub fn objects_in(&self, group: &SimGroup) -> Result<Vec<usize>> {
        if self.objects.is_empty() {
            let all: Vec<usize> = (0..group.objects.len())
                .filter(|&i| has_class(group, &group.objects[i], self.class))
                .collect();
            if all.is_empty() {
                return Err(Error::InvalidArgument(format!(
                    "{}: no object has a {} packet",
                    self.id,
                    self.class_name()
                )));
            }
            return Ok(all);
        }
        self.objects
            .iter()
            .map(|wanted| {
                let wanted = wanted.trim();
                group
                    .objects
                    .iter()
                    .position(|o| {
                        o.guid.to_string().eq_ignore_ascii_case(wanted)
                            || group.object_name(o) == Some(wanted)
                    })
                    .filter(|&i| has_class(group, &group.objects[i], self.class))
                    .ok_or_else(|| {
                        Error::InvalidArgument(format!(
                            "{}: no object {wanted} with a {} packet",
                            self.id,
                            self.class_name()
                        ))
                    })
            })
            .collect()
    }

    /// Current values in a parsed SimGroup.
    pub fn read_group(&self, group: &SimGroup) -> Result<Vec<ObjectValue>> {
        self.objects_in(group)?
            .into_iter()
            .enumerate()
            .map(|(k, i)| {
                let object = &group.objects[i];
                let value = number(group.attr(object, self.class, self.attribute), self.kind)
                    .ok_or_else(|| {
                        Error::InvalidArgument(format!(
                            "{}: object {i} holds no number in attribute {}",
                            self.id, self.attribute
                        ))
                    })?;
                let label = self
                    .object_names
                    .get(k)
                    .cloned()
                    .unwrap_or_else(|| label_of(group, object));
                Ok(ObjectValue {
                    object: i,
                    label,
                    value,
                })
            })
            .collect()
    }

    /// Current values: the workspace version when there is one.
    pub fn read(&self, game: &GameInstance, ws: Option<&Workspace>) -> Result<Reading> {
        let (stream, resource) = locate(game, &self.resource)?;
        let group = SimGroup::parse(&current(game, ws, stream, resource)?)?;
        Ok(Reading {
            stream,
            resource,
            objects: self.read_group(&group)?,
        })
    }

    /// Checks a value against the kind and the bounds of the tunable.
    pub fn check(&self, value: f64) -> std::result::Result<(), String> {
        if !value.is_finite() {
            return Err("not a number".into());
        }
        match self.kind {
            ValueKind::Int if value.fract() != 0.0 => return Err("must be a whole number".into()),
            ValueKind::Int if value < f64::from(i32::MIN) || value > f64::from(i32::MAX) => {
                return Err("out of the range of a 32-bit number".into());
            }
            ValueKind::Bool if value != 0.0 && value != 1.0 => return Err("must be 0 or 1".into()),
            ValueKind::Float if value.abs() > f64::from(f32::MAX) => {
                return Err("too large".into());
            }
            _ => {}
        }
        if let Some(min) = self.min
            && value < min
        {
            return Err(format!("below the minimum {}", self.format(min)));
        }
        if let Some(max) = self.max
            && value > max
        {
            return Err(format!("above the maximum {}", self.format(max)));
        }
        Ok(())
    }

    /// Parses a value typed by the user and checks it.
    pub fn parse_value(&self, text: &str) -> std::result::Result<f64, String> {
        let text = text.trim();
        let value = match (self.kind, text.to_ascii_lowercase().as_str()) {
            (ValueKind::Bool, "true" | "yes" | "on") => 1.0,
            (ValueKind::Bool, "false" | "no" | "off") => 0.0,
            _ => text
                .parse::<f64>()
                .map_err(|_| format!("`{text}` is not a number"))?,
        };
        self.check(value)?;
        Ok(value)
    }

    /// A value as the user enters it.
    pub fn format(&self, value: f64) -> String {
        match self.kind {
            ValueKind::Int | ValueKind::Bool => format!("{}", value as i64),
            // As the game stores it: the shortest text of the f32.
            ValueKind::Float => format!("{}", value as f32),
        }
    }

    fn stored(&self, value: f64) -> Value {
        match self.kind {
            ValueKind::Int | ValueKind::Bool => Value::Int(value as i32),
            ValueKind::Float => Value::Float(value as f32),
        }
    }

    /// A tunable for an attribute found by [`find_attribute`], not on the list.
    pub fn custom(
        resource: ResourceKey,
        class: u32,
        attribute: u16,
        object: String,
        kind: ValueKind,
    ) -> Self {
        let class_name = sim_classes().display(class);
        Self {
            id: format!("custom.{class_name}.{attribute}.{object}").to_ascii_lowercase(),
            category: "custom".into(),
            label: format!("{class_name} attribute {attribute}"),
            description: String::new(),
            resource,
            class,
            attribute,
            objects: vec![object],
            object_names: Vec::new(),
            kind,
            min: None,
            max: None,
        }
    }
}

/// New values of one tunable, by object index.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub tunable: Tunable,
    pub values: Vec<(usize, f64)>,
}

/// One value that changes, for the summary of a set of changes.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct DiffLine {
    pub id: String,
    pub label: String,
    pub object: String,
    pub old: String,
    pub new: String,
}

/// New data of resources by stream index, as [`Workspace::change_resources`] takes it.
pub type StreamEdits = Vec<(usize, Vec<ResourceEdit>)>;

/// The new data of the SimGroups `changes` touch and what changes in them.
/// Values equal to the current ones are left out.
pub fn plan(
    game: &GameInstance,
    ws: Option<&Workspace>,
    changes: &[Change],
) -> Result<(StreamEdits, Vec<DiffLine>)> {
    let mut groups: BTreeMap<(usize, usize), (SimGroup, bool)> = BTreeMap::new();
    let mut diff = Vec::new();
    for change in changes {
        let t = &change.tunable;
        for &(_, value) in &change.values {
            t.check(value)
                .map_err(|e| Error::InvalidArgument(format!("{}: {e}", t.label)))?;
        }
        let at = locate(game, &t.resource)?;
        let (group, touched) = match groups.entry(at) {
            std::collections::btree_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::btree_map::Entry::Vacant(e) => {
                e.insert((SimGroup::parse(&current(game, ws, at.0, at.1)?)?, false))
            }
        };
        let allowed = t.objects_in(group)?;
        for &(object, value) in &change.values {
            if !allowed.contains(&object) {
                return Err(Error::InvalidArgument(format!(
                    "{}: object {object} is not one of its objects",
                    t.id
                )));
            }
            let o = &group.objects[object];
            let old = number(group.attr(o, t.class, t.attribute), t.kind).unwrap_or(f64::NAN);
            let label = allowed
                .iter()
                .position(|&a| a == object)
                .and_then(|k| t.object_names.get(k))
                .cloned()
                .unwrap_or_else(|| label_of(group, o));
            if group.set_class_attr(object, t.class, t.attribute, Some(t.stored(value)))? {
                *touched = true;
                diff.push(DiffLine {
                    id: t.id.clone(),
                    label: t.label.clone(),
                    object: label,
                    old: t.format(old),
                    new: t.format(value),
                });
            }
        }
    }
    let mut edits: BTreeMap<usize, Vec<ResourceEdit>> = BTreeMap::new();
    for ((stream, resource), (group, touched)) in groups {
        if touched {
            edits.entry(stream).or_default().push(ResourceEdit {
                index: resource,
                data: group.encode(),
                storage: StorageChoice::Keep,
            });
        }
    }
    Ok((edits.into_iter().collect(), diff))
}

/// Records `changes` as one undoable operation named `label`. Returns the
/// sequence number of the operation, or `None` when nothing changes.
pub fn apply(
    game: &GameInstance,
    ws: &mut Workspace,
    changes: &[Change],
    label: &str,
) -> Result<Option<u64>> {
    let (edits, _) = plan(game, Some(ws), changes)?;
    if edits.is_empty() {
        return Ok(None);
    }
    ws.change_resources(game, label, &edits, &[])
}

/// One object with a packet of the class searched by [`find_attribute`].
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub resource: ResourceKey,
    pub stream: usize,
    pub index: usize,
    pub object: usize,
    /// Name or GUID of the object.
    pub label: String,
    /// Index of the attribute and its value; every stored attribute of the
    /// class when no index was asked for.
    pub attrs: Vec<(u16, Value)>,
    /// Attributes of the class.
    pub attr_count: u16,
}

/// The objects of class `class` in the SimGroups of `streams`, with the
/// value of `attribute` (or of every stored attribute). At most `limit`.
pub fn find_attribute(
    game: &GameInstance,
    ws: Option<&Workspace>,
    streams: &[usize],
    class: u32,
    attribute: Option<u16>,
    limit: usize,
) -> Result<Vec<Found>> {
    let mut out = Vec::new();
    for &stream in streams {
        let file = game.open_stream(stream)?;
        let name = game.streams()[stream].stream_name();
        for (index, r) in file.layout().resources.iter().enumerate() {
            if r.header.type_id != TYPE_SIMGROUP {
                continue;
            }
            let group = SimGroup::parse(&current(game, ws, stream, index)?)?;
            for (i, object) in group.objects.iter().enumerate() {
                let Some(packet) = group.object_packets(object).find(|p| p.class.0 == class) else {
                    continue;
                };
                let attrs = packet
                    .attrs
                    .iter()
                    .filter(|a| attribute.is_none_or(|w| a.index == w))
                    .map(|a| (a.index, a.value.clone()))
                    .collect();
                out.push(Found {
                    resource: ResourceKey::new(&name, &r.header),
                    stream,
                    index,
                    object: i,
                    label: label_of(&group, object),
                    attrs,
                    attr_count: packet.attr_count,
                });
                if out.len() >= limit {
                    return Ok(out);
                }
            }
        }
    }
    Ok(out)
}

/// How a value of attribute `attribute` of `class` is entered, from the
/// schema of the game or the stored value; `None` for non-numbers.
pub fn kind_of(class: u32, attribute: u16, stored: Option<&Value>) -> Option<ValueKind> {
    let kind = match stored {
        Some(Value::Float(_)) => Kind::Float,
        Some(Value::Int(_)) => Kind::Int,
        Some(Value::Hash(_)) => Kind::Hash,
        Some(_) => return None,
        None => Schema::gf2().get(class, attribute)?.kind,
    };
    match kind {
        Kind::Float => Some(ValueKind::Float),
        Kind::Int | Kind::Hash => Some(ValueKind::Int),
        _ => None,
    }
}

// ------------------------------------------------------------ operation

/// How the new values of a tunable are given.
#[derive(Debug, Clone, PartialEq)]
pub enum NewValues {
    /// The same value for every object.
    All(f64),
    /// Every object's own value times this (whole numbers are rounded).
    Multiply(f64),
}

impl Reading {
    /// The new value of every object of `tunable`, checked against its kind
    /// and bounds.
    pub fn new_values(
        &self,
        tunable: &Tunable,
        new: &NewValues,
    ) -> std::result::Result<Vec<(usize, f64)>, String> {
        match *new {
            NewValues::All(value) => {
                tunable.check(value)?;
                Ok(self.objects.iter().map(|o| (o.object, value)).collect())
            }
            NewValues::Multiply(factor) => self
                .objects
                .iter()
                .map(|o| {
                    let mut v = o.value * factor;
                    if tunable.kind != ValueKind::Float {
                        v = v.round();
                    }
                    tunable
                        .check(v)
                        .map(|()| (o.object, v))
                        .map_err(|e| format!("{}: {} ({e})", o.label, tunable.format(v)))
                })
                .collect(),
        }
    }
}

/// One tunable to change, as the command line and the MCP server give it.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SetTunable {
    /// Id of the tunable, as the list gives it (`weapons.pistol.damage`).
    pub id: String,
    /// The new value for every object of the tunable (0 or 1 for a switch).
    #[serde(default)]
    pub value: Option<f64>,
    /// Or: multiply the value of every object by this (whole numbers are
    /// rounded), keeping their differences.
    #[serde(default)]
    pub multiply: Option<f64>,
}

/// A tunable of the curated list by id.
pub fn by_id(id: &str) -> Result<&'static Tunable> {
    let id = id.trim();
    gf2()
        .iter()
        .find(|t| t.id.eq_ignore_ascii_case(id))
        .ok_or_else(|| Error::InvalidArgument(format!("no tunable `{id}` on the list")))
}

/// The changes `sets` ask for, with the current values read through `ws`.
pub fn changes(
    game: &GameInstance,
    ws: Option<&Workspace>,
    sets: &[SetTunable],
) -> Result<Vec<Change>> {
    sets.iter()
        .map(|set| {
            let tunable = by_id(&set.id)?;
            let new = match (set.value, set.multiply) {
                (Some(v), None) => NewValues::All(v),
                (None, Some(f)) => NewValues::Multiply(f),
                _ => {
                    return Err(Error::InvalidArgument(format!(
                        "{}: give either a value or a factor",
                        tunable.id
                    )));
                }
            };
            let values = tunable
                .read(game, ws)?
                .new_values(tunable, &new)
                .map_err(|e| Error::InvalidArgument(format!("{}: {e}", tunable.id)))?;
            Ok(Change {
                tunable: tunable.clone(),
                values,
            })
        })
        .collect()
}

/// Changes tunables of the curated list as one undoable operation. Returns
/// what changed and the label of the operation (`None` when every value
/// was already there).
pub fn set(
    game: &GameInstance,
    ws: &mut Workspace,
    sets: &[SetTunable],
) -> Result<(Vec<DiffLine>, Option<String>)> {
    let changes = changes(game, Some(ws), sets)?;
    let (_, diff) = plan(game, Some(ws), &changes)?;
    let Some(first) = changes.first() else {
        return Ok((diff, None));
    };
    let label = match changes.len() {
        1 => format!("Game parameter: {}", first.tunable.label),
        n => format!(
            "Game parameters: {} and {} more",
            first.tunable.label,
            n - 1
        ),
    };
    let done = apply(game, ws, &changes, &label)?;
    Ok((diff, done.map(|_| label)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::tests::{LEVEL, SIMGROUP, fake_game, key, stream};

    #[test]
    fn the_curated_list_parses() {
        let list = gf2();
        assert!(list.len() >= 30, "{}", list.len());
        for t in list {
            assert!(CATEGORIES.contains(&t.category.as_str()), "{}", t.id);
            assert!(!t.label.is_empty() && !t.description.is_empty(), "{}", t.id);
            assert!(t.id.starts_with(&t.category), "{}", t.id);
        }
    }

    #[test]
    fn bad_lists_are_refused() {
        let row = |extra: &str| {
            format!(
                "version = 1\n[[tunable]]\nid = \"a.b\"\ncategory = \"weapons\"\nlabel = \"A\"\n\
                 source = \"simgroup\"\nresource = \"gf2/x:simgroup:00000001\"\nclass = \"Gun\"\n\
                 attribute = 1\nkind = \"int\"\n{extra}"
            )
        };
        assert_eq!(
            parse(&row("")).unwrap()[0].class,
            class_hash("gun").unwrap()
        );
        assert!(parse(&row("min = 5\nmax = 1\n")).is_err());
        assert!(parse(&row("colour = 1\n")).is_err());
        assert!(parse(&row("").replace("Gun", "NoSuchClass")).is_err());
        assert!(parse(&row("").replace("simgroup:", "mesh:")).is_err());
        assert!(parse(&row("").replace("version = 1", "version = 9")).is_err());
        let twice = row("") + &row("").replace("version = 1\n", "");
        assert!(parse(&twice).is_err());
    }

    #[test]
    fn values_are_checked() {
        let t = Tunable {
            kind: ValueKind::Int,
            min: Some(1.0),
            max: Some(10.0),
            ..Tunable::custom(
                "gf2/x:simgroup:00000001".parse().unwrap(),
                1,
                2,
                "x".into(),
                ValueKind::Int,
            )
        };
        assert_eq!(t.parse_value(" 7 "), Ok(7.0));
        assert!(t.parse_value("7.5").is_err());
        assert!(t.parse_value("0").is_err());
        assert!(t.parse_value("11").is_err());
        assert!(t.parse_value("abc").is_err());
        let b = Tunable {
            kind: ValueKind::Bool,
            min: None,
            max: None,
            ..t.clone()
        };
        assert_eq!(b.parse_value("yes"), Ok(1.0));
        assert!(b.parse_value("2").is_err());
        let f = Tunable {
            kind: ValueKind::Float,
            ..b
        };
        assert_eq!(f.format(0.1), "0.1");
        assert!(f.parse_value("NaN").is_err());
    }

    /// GUID of the object of the sample SimGroup.
    const GUID: &str = "01020304050607084348524953543432";

    #[test]
    fn reads_and_records_values_as_one_operation() {
        let (dir, game) = fake_game();
        let mut ws = Workspace::for_game(&game, Some(&dir.path().join("ws"))).unwrap();
        // The sample group: one object, class 0x11111111 with an int at 2.
        let t = Tunable {
            min: Some(-100.0),
            ..Tunable::custom(
                key(&game, LEVEL, SIMGROUP),
                0x1111_1111,
                2,
                GUID.into(),
                ValueKind::Int,
            )
        };
        let reading = t.read(&game, Some(&ws)).unwrap();
        assert_eq!(reading.stream, stream(&game, LEVEL));
        assert_eq!(reading.objects[0].value, -7.0);
        assert_eq!(reading.objects[0].label, GUID);
        assert_eq!(reading.distinct(), [-7.0]);
        let all = Tunable {
            objects: Vec::new(),
            ..t.clone()
        };
        assert_eq!(all.read(&game, None).unwrap().objects.len(), 1);

        let change = Change {
            tunable: t.clone(),
            values: vec![(0, 42.0)],
        };
        let (edits, diff) = plan(&game, Some(&ws), std::slice::from_ref(&change)).unwrap();
        assert_eq!(edits.len(), 1);
        assert_eq!((diff[0].old.as_str(), diff[0].new.as_str()), ("-7", "42"));
        let bad = Change {
            values: vec![(0, -500.0)],
            ..change.clone()
        };
        assert!(apply(&game, &mut ws, &[bad], "x").is_err());
        assert!(
            apply(
                &game,
                &mut ws,
                std::slice::from_ref(&change),
                "Set parameters"
            )
            .unwrap()
            .is_some()
        );
        assert_eq!(ws.operations().len(), 1);
        assert_eq!(t.read(&game, Some(&ws)).unwrap().objects[0].value, 42.0);
        assert_eq!(t.read(&game, None).unwrap().objects[0].value, -7.0);
        // The same values again record nothing.
        assert_eq!(apply(&game, &mut ws, &[change], "again").unwrap(), None);

        let found = find_attribute(
            &game,
            Some(&ws),
            &[stream(&game, LEVEL)],
            0x1111_1111,
            Some(2),
            10,
        )
        .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].attrs,
            [(2, Value::Hash(crate::resource::simgroup::Hash32(42)))]
        );
        assert_eq!(found[0].attr_count, 3);
        assert_eq!(
            kind_of(0x1111_1111, 2, Some(&Value::Int(1))),
            Some(ValueKind::Int)
        );
        assert_eq!(
            kind_of(0x1111_1111, 0, Some(&Value::String("x".into()))),
            None
        );
    }
}

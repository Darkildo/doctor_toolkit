//! Classes of native mods (stage 80): the schema of SimGroup classes that
//! are not in `godfather2.exe` but come from a DLL of a mod
//! (`docs/analysis/asi-platform.md` §6).
//!
//! A mod describes its classes in a TOML file:
//!
//! ```toml
//! [class.DoctorCounter]
//! category = "logic"
//! summary = "Counts a message and sends another one every N-th time"
//! attrs = [
//!   { i = 0, name = "count_on", pin = "in", kind = "string" },
//!   { i = 1, name = "send", pin = "out", kind = "string" },
//!   { i = 2, name = "limit", pin = "param", kind = "int" },
//! ]
//! ```
//!
//! The files named by the environment variable `DOCTOR_TOOLKIT_CLASSES`
//! (separated like `PATH`) are read once; their classes join the schema of
//! the logic graph ([`LogicSchema::gf2`]), the value kinds of SimGroup
//! attributes (`simgroup::Schema::gf2`) and the class names
//! (`names::sim_classes`), so every command, the editor and the MCP server
//! treat them as they treat the classes of the engine: names instead of
//! hashes, typed values, pins of messages, new objects of the class.

use std::collections::HashSet;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::error::{Error, Result};
use crate::hash::sdbm;
use crate::logic_graph::schema::{AttrPin, Category, Confidence, LogicSchema, PinKind};
use crate::resource::simgroup::Kind;

/// The environment variable that lists the class files of the mods.
pub const ENV: &str = "DOCTOR_TOOLKIT_CLASSES";

/// The class whose base packets a new object of a mod class gets: the
/// plainest logic class of the engine.
const LAYOUT_OF: &str = "TriggerAuto";

/// One attribute of a mod class.
#[derive(Debug, Clone, PartialEq)]
pub struct CustomAttr {
    pub index: u16,
    pub name: String,
    pub pin: PinKind,
    pub kind: Kind,
    /// Index of the attribute that holds the delay of an `out` pin.
    pub delay: Option<u16>,
}

/// A class of a native mod.
#[derive(Debug, Clone, PartialEq)]
pub struct CustomClass {
    pub name: String,
    /// `sdbm(lower(name))`: the class and its packet.
    pub hash: u32,
    pub category: Category,
    pub summary: String,
    /// Number of attributes of the packet: the highest index plus one.
    pub attr_count: u16,
    pub attrs: Vec<CustomAttr>,
}

impl CustomClass {
    /// Packets of a new object: the base packets of a logic object, then
    /// the packet of the class.
    pub fn layout(&self, base: &LogicSchema) -> Vec<(u32, u16)> {
        let template = sdbm(LAYOUT_OF.to_ascii_lowercase().as_bytes());
        let mut layout: Vec<(u32, u16)> = base
            .layout(template)
            .unwrap_or(&[])
            .iter()
            .copied()
            .filter(|&(class, _)| class != template)
            .collect();
        layout.push((self.hash, self.attr_count));
        layout
    }

    /// The attributes as pins of the logic graph.
    pub fn pins(&self) -> Vec<AttrPin> {
        self.attrs
            .iter()
            .map(|a| AttrPin {
                index: a.index,
                pin: a.pin,
                conf: Some(Confidence::Exe),
                name: Some(a.name.clone()),
                name_conf: Some(Confidence::Exe),
                delay: a.delay,
                kind: Some(a.kind),
            })
            .collect()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileAttr {
    i: u16,
    name: String,
    pin: PinKind,
    kind: String,
    delay: Option<u16>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileClass {
    category: Category,
    #[serde(default)]
    summary: String,
    attrs: Vec<FileAttr>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    class: std::collections::BTreeMap<String, FileClass>,
}

fn bad(text: String) -> Error {
    Error::InvalidArgument(format!("class schema: {text}"))
}

fn is_name(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 63
        && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !text.starts_with(|c: char| c.is_ascii_digit())
}

fn kind_of(name: &str) -> Option<Kind> {
    Some(match name {
        "int" => Kind::Int,
        "float" => Kind::Float,
        "hash" => Kind::Hash,
        "string" => Kind::String,
        "guid" => Kind::Guid,
        "vec3" => Kind::Vec3,
        "matrix" => Kind::Matrix,
        _ => return None,
    })
}

/// Parses and checks a class file against the classes of the engine
/// (`engine`: the schema without mod classes).
///
/// Refused: a name that is (or hashes like) a class or a packet of the
/// engine, two classes with one hash, an index twice, an unknown kind, a
/// message pin that is not a string, a link that is not a GUID, a delay
/// that does not name a float attribute of the class.
pub fn parse(text: &str, engine: &LogicSchema) -> Result<Vec<CustomClass>> {
    let file: File = toml::from_str(text).map_err(|e| bad(e.to_string()))?;
    let engine_classes = crate::resource::names::engine_sim_classes();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (name, class) in file.class {
        if !is_name(&name) {
            return Err(bad(format!(
                "`{name}` is not a class name (letters, digits and `_`, at most 63)"
            )));
        }
        let hash = sdbm(name.to_ascii_lowercase().as_bytes());
        if let Some(taken) = engine_classes.get(hash) {
            return Err(bad(format!(
                "`{name}` has the hash {hash:08x} of the engine's class `{taken}`"
            )));
        }
        if !engine.packet(hash).is_empty() || engine.class_name(hash).is_some() {
            return Err(bad(format!(
                "`{name}` has the hash {hash:08x} of a packet of the engine"
            )));
        }
        if !seen.insert(hash) {
            return Err(bad(format!(
                "two classes have the hash {hash:08x} (`{name}`)"
            )));
        }
        if matches!(class.category, Category::Other) {
            return Err(bad(format!(
                "`{name}`: `other` is not a category of objects"
            )));
        }
        let mut attrs = Vec::new();
        for a in class.attrs {
            let kind = kind_of(&a.kind).ok_or_else(|| {
                bad(format!(
                    "`{name}` attribute {}: unknown kind `{}` (int, float, hash, string, guid, vec3, matrix)",
                    a.i, a.kind
                ))
            })?;
            if a.i > 255 {
                return Err(bad(format!(
                    "`{name}`: attribute index {} is above 255",
                    a.i
                )));
            }
            if !is_name(&a.name) {
                return Err(bad(format!(
                    "`{name}` attribute {}: bad name `{}`",
                    a.i, a.name
                )));
            }
            match (a.pin, kind) {
                (PinKind::In | PinKind::Out, Kind::String)
                | (PinKind::Link, Kind::Guid)
                | (PinKind::Param | PinKind::Asset, _) => {}
                (PinKind::In | PinKind::Out, _) => {
                    return Err(bad(format!(
                        "`{name}` attribute {} `{}`: a message is a string",
                        a.i, a.name
                    )));
                }
                (PinKind::Link, _) => {
                    return Err(bad(format!(
                        "`{name}` attribute {} `{}`: a link is a guid",
                        a.i, a.name
                    )));
                }
            }
            attrs.push(CustomAttr {
                index: a.i,
                name: a.name,
                pin: a.pin,
                kind,
                delay: a.delay,
            });
        }
        attrs.sort_by_key(|a| a.index);
        if let Some(w) = attrs.windows(2).find(|w| w[0].index == w[1].index) {
            return Err(bad(format!(
                "`{name}`: attribute {} is listed twice",
                w[0].index
            )));
        }
        if let Some(w) = attrs
            .iter()
            .enumerate()
            .find(|(i, a)| attrs[..*i].iter().any(|b| b.name == a.name))
        {
            return Err(bad(format!(
                "`{name}`: two attributes are named `{}`",
                w.1.name
            )));
        }
        for a in &attrs {
            let Some(delay) = a.delay else { continue };
            let target = attrs.iter().find(|b| b.index == delay);
            if a.pin != PinKind::Out || !target.is_some_and(|b| b.kind == Kind::Float) {
                return Err(bad(format!(
                    "`{name}` attribute {} `{}`: `delay` names a float attribute and belongs to an `out` pin",
                    a.index, a.name
                )));
            }
        }
        let Some(last) = attrs.last() else {
            return Err(bad(format!("`{name}` has no attributes")));
        };
        out.push(CustomClass {
            hash,
            category: class.category,
            summary: class.summary,
            attr_count: last.index + 1,
            attrs,
            name,
        });
    }
    Ok(out)
}

/// Why a file of [`ENV`] was not used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadError {
    pub file: std::path::PathBuf,
    pub error: String,
}

struct Installed {
    classes: Vec<CustomClass>,
    errors: Vec<LoadError>,
}

fn load() -> &'static Installed {
    static INSTALLED: OnceLock<Installed> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        let mut installed = Installed {
            classes: Vec::new(),
            errors: Vec::new(),
        };
        let Some(list) = std::env::var_os(ENV) else {
            return installed;
        };
        let engine = LogicSchema::engine();
        for file in std::env::split_paths(&list).filter(|p| !p.as_os_str().is_empty()) {
            let parsed = std::fs::read_to_string(&file)
                .map_err(|e| e.to_string())
                .and_then(|text| parse(&text, &engine).map_err(|e| e.to_string()))
                .and_then(|classes| {
                    match classes
                        .iter()
                        .find(|c| installed.classes.iter().any(|d| d.hash == c.hash))
                    {
                        Some(c) => Err(format!("class `{}` is in another file too", c.name)),
                        None => Ok(classes),
                    }
                });
            match parsed {
                Ok(classes) => installed.classes.extend(classes),
                Err(error) => installed.errors.push(LoadError { file, error }),
            }
        }
        installed
    })
}

/// The classes of the files of [`ENV`]; a file with an error gives none
/// (see [`load_errors`]).
pub fn installed() -> &'static [CustomClass] {
    &load().classes
}

/// Files of [`ENV`] that could not be used, with the reason.
pub fn load_errors() -> &'static [LoadError] {
    &load().errors
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
[class.DoctorCounter]
category = "logic"
summary = "Counts a message"
attrs = [
  { i = 1, name = "send", pin = "out", kind = "string", delay = 4 },
  { i = 0, name = "count_on", pin = "in", kind = "string" },
  { i = 2, name = "limit", pin = "param", kind = "int" },
  { i = 4, name = "delay", pin = "param", kind = "float" },
]
"#;

    fn parse_text(text: &str) -> Result<Vec<CustomClass>> {
        parse(text, &LogicSchema::engine())
    }

    fn error(text: &str) -> String {
        parse_text(text).unwrap_err().to_string()
    }

    #[test]
    fn a_class_file_gives_the_packet_and_the_layout() {
        let classes = parse_text(GOOD).unwrap();
        assert_eq!(classes.len(), 1);
        let c = &classes[0];
        assert_eq!(c.name, "DoctorCounter");
        assert_eq!(c.hash, sdbm(b"doctorcounter"));
        // Index 3 is not described: the packet still has five attributes.
        assert_eq!(c.attr_count, 5);
        assert_eq!(
            c.attrs.iter().map(|a| a.index).collect::<Vec<_>>(),
            [0, 1, 2, 4]
        );
        // The base packets of a logic object of the engine, then its own.
        let engine = LogicSchema::engine();
        let auto = engine.layout(sdbm(b"triggerauto")).unwrap();
        let layout = c.layout(&engine);
        assert_eq!(layout.len(), auto.len());
        assert_eq!(layout[..layout.len() - 1], auto[..auto.len() - 1]);
        assert_eq!(layout[layout.len() - 1], (c.hash, 5));
        let pins = c.pins();
        assert_eq!(pins[1].pin, PinKind::Out);
        assert_eq!(pins[1].delay, Some(4));
        assert_eq!(pins[0].kind, Some(Kind::String));
    }

    #[test]
    fn a_class_of_the_engine_cannot_be_redefined() {
        let text = GOOD.replace("DoctorCounter", "TriggerAuto");
        assert!(error(&text).contains("engine's class `TriggerAuto`"));
        // The hash ignores the case of the letters.
        let text = GOOD.replace("DoctorCounter", "multimanager");
        assert!(error(&text).contains("MultiManager"));
    }

    #[test]
    fn a_bad_attribute_is_refused_with_its_place() {
        let twice = GOOD.replace("i = 2", "i = 0");
        assert!(error(&twice).contains("attribute 0 is listed twice"));
        let kind = GOOD.replace("kind = \"int\"", "kind = \"bool\"");
        assert!(error(&kind).contains("unknown kind `bool`"));
        let message = GOOD.replace(
            "name = \"count_on\", pin = \"in\", kind = \"string\"",
            "name = \"count_on\", pin = \"in\", kind = \"int\"",
        );
        assert!(error(&message).contains("a message is a string"));
        let delay = GOOD.replace("delay = 4", "delay = 2");
        assert!(error(&delay).contains("`delay` names a float attribute"));
        let name = GOOD.replace("DoctorCounter", "9Counter");
        assert!(error(&name).contains("not a class name"));
        let field = GOOD.replace("category", "group");
        assert!(error(&field).contains("class schema"));
        let empty = "[class.Nothing]\ncategory = \"logic\"\nattrs = []\n";
        assert!(error(empty).contains("no attributes"));
    }

    #[test]
    fn the_schema_takes_a_mod_class_like_a_class_of_the_engine() {
        let classes = parse_text(GOOD).unwrap();
        let mut schema = LogicSchema::engine();
        schema.add_custom(&classes);
        let c = &classes[0];
        assert_eq!(schema.class_name(c.hash), Some("DoctorCounter"));
        assert_eq!(schema.category(c.hash), Category::Logic);
        assert_eq!(schema.layout(c.hash).unwrap().last(), Some(&(c.hash, 5)));
        assert_eq!(
            schema.attr(c.hash, 2).unwrap().name.as_deref(),
            Some("limit")
        );
        assert_eq!(schema.attr(c.hash, 3), None);
        // A new object of the class can be made and filled.
        let mut group = crate::resource::simgroup::SimGroup::empty();
        let guid = crate::resource::simgroup_edit::new_object_guid("doctor/custom", |_| false);
        let o = group
            .push_object(c.hash, guid, schema.layout(c.hash).unwrap())
            .unwrap();
        let slot = group.slot_of(o, c.hash).unwrap();
        let value = crate::resource::simgroup::Value::String("doctor_ping".into());
        assert!(group.set_attr(o, slot, 0, Some(value.clone())).unwrap());
        let again = crate::resource::simgroup::SimGroup::parse(&group.encode()).unwrap();
        assert_eq!(again.objects[o].class, c.hash);
        assert_eq!(again.attr(&again.objects[o], c.hash, 0), Some(&value));
    }
}

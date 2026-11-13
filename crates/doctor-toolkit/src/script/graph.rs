//! The graph of a visual script: nodes, links, variables; read and written as JSON.
//!
//! ```json
//! {
//!   "format": "doctor-toolkit-script", "version": 1,
//!   "name": "hello", "description": "",
//!   "variables": [{ "name": "count", "type": "number", "default": 0 }],
//!   "nodes": [
//!     { "id": 1, "kind": "event.message", "pos": [0, 0], "params": { "message": "iMsgX" } },
//!     { "id": 2, "kind": "action.send_message", "pos": [240, 0],
//!       "inputs": { "message": "doctor_hello" } }
//!   ],
//!   "links": [{ "from": [1, "then"], "to": [2, "exec"] }]
//! }
//! ```
//!
//! `params` are the settings of a node (a message name, a timer name), `inputs`
//! the values of data inputs that have no link. Pins are named; what pins a
//! node has follows from its kind and its params ([`super::library`]).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::{Error, Result};

/// Value of the `format` field.
pub const FORMAT: &str = "doctor-toolkit-script";
/// Current version of the JSON format.
pub const VERSION: u32 = 1;

/// Type of a pin or a variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PinType {
    /// Order of execution, not a value.
    Exec,
    Number,
    Bool,
    String,
    /// A position object of the game (`GetPlayerPosition`).
    Position,
    /// A group of the game's `GroupManager`.
    Group,
    /// Any value: message data, results of game functions.
    Any,
}

impl PinType {
    pub const DATA: [PinType; 6] = [
        PinType::Number,
        PinType::Bool,
        PinType::String,
        PinType::Position,
        PinType::Group,
        PinType::Any,
    ];

    pub fn name(self) -> &'static str {
        match self {
            PinType::Exec => "exec",
            PinType::Number => "number",
            PinType::Bool => "bool",
            PinType::String => "string",
            PinType::Position => "position",
            PinType::Group => "group",
            PinType::Any => "any",
        }
    }

    pub fn parse(s: &str) -> Option<PinType> {
        [PinType::Exec]
            .into_iter()
            .chain(Self::DATA)
            .find(|t| t.name() == s)
    }

    /// Whether a link from an output of type `self` to an input of type `to` is allowed.
    /// Numbers and booleans convert to text; `any` goes both ways.
    pub fn connects_to(self, to: PinType) -> bool {
        match (self, to) {
            (a, b) if a == b => true,
            (PinType::Exec, _) | (_, PinType::Exec) => false,
            (PinType::Any, _) | (_, PinType::Any) => true,
            (PinType::Number | PinType::Bool, PinType::String) => true,
            _ => false,
        }
    }
}

/// A variable of the script, kept between events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Variable {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: PinType,
    /// Initial value (number, bool or string); `null` for nil.
    #[serde(default)]
    pub default: Json,
}

/// One node of the graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: u32,
    /// Node kind from the library, e.g. `event.message`.
    pub kind: String,
    /// Position in the editor.
    #[serde(default)]
    pub pos: [f32; 2],
    /// Settings of the node.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, Json>,
    /// Values of data inputs without a link.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, Json>,
}

impl Node {
    pub fn new(id: u32, kind: &str, pos: [f32; 2]) -> Self {
        Self {
            id,
            kind: kind.to_owned(),
            pos,
            params: BTreeMap::new(),
            inputs: BTreeMap::new(),
        }
    }

    /// Sets a param (builder style).
    pub fn param(mut self, name: &str, value: impl Into<Json>) -> Self {
        self.params.insert(name.to_owned(), value.into());
        self
    }

    /// Sets the value of a data input (builder style).
    pub fn input(mut self, name: &str, value: impl Into<Json>) -> Self {
        self.inputs.insert(name.to_owned(), value.into());
        self
    }

    pub fn param_str(&self, name: &str) -> Option<&str> {
        self.params.get(name).and_then(Json::as_str)
    }

    pub fn param_f64(&self, name: &str) -> Option<f64> {
        self.params.get(name).and_then(Json::as_f64)
    }

    pub fn param_bool(&self, name: &str) -> Option<bool> {
        self.params.get(name).and_then(Json::as_bool)
    }
}

/// End of a link: node id and pin name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PinRef(pub u32, pub String);

impl PinRef {
    pub fn new(node: u32, pin: &str) -> Self {
        Self(node, pin.to_owned())
    }
}

/// A link from an output pin to an input pin.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Link {
    pub from: PinRef,
    pub to: PinRef,
}

/// How the script starts when deployed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartSettings {
    /// Run when the level loads (`LuaAudioScript` flag bit 0).
    #[serde(default = "yes")]
    pub on_load: bool,
    /// Or run when this message is sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Stop (`shutdown`) when this message is sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_message: Option<String>,
}

fn yes() -> bool {
    true
}

impl Default for StartSettings {
    fn default() -> Self {
        Self {
            on_load: true,
            message: None,
            stop_message: None,
        }
    }
}

/// A whole script.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScriptGraph {
    #[serde(default = "format_name")]
    pub format: String,
    #[serde(default = "version")]
    pub version: u32,
    /// Script name: letters, digits and `_`; the chunk is `<name>.lua`.
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default)]
    pub start: StartSettings,
    /// Write a file probe (`doctor_error_<script>_<node>`) when a node fails
    /// in the game, so errors show up in a file trace.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub trace_errors: bool,
    /// Every node prints `dt:<script>:<node>;` when it runs (stage 35): the
    /// game's journal (live link) then shows which nodes fired, and a
    /// breakpoint on that text stops the game at the node.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub trace_nodes: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variables: Vec<Variable>,
    #[serde(default)]
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub links: Vec<Link>,
}

fn format_name() -> String {
    FORMAT.to_owned()
}

fn version() -> u32 {
    VERSION
}

impl ScriptGraph {
    pub fn new(name: &str) -> Self {
        Self {
            format: FORMAT.to_owned(),
            version: VERSION,
            name: name.to_owned(),
            description: String::new(),
            start: StartSettings::default(),
            trace_errors: false,
            trace_nodes: false,
            variables: Vec::new(),
            nodes: Vec::new(),
            links: Vec::new(),
        }
    }

    /// Reads a graph from JSON text.
    pub fn from_json(text: &str) -> Result<Self> {
        let g: ScriptGraph = serde_json::from_str(text)
            .map_err(|e| Error::InvalidArgument(format!("not a script graph: {e}")))?;
        if g.format != FORMAT {
            return Err(Error::InvalidArgument(format!(
                "not a script graph: format is {:?}",
                g.format
            )));
        }
        if g.version > VERSION {
            return Err(Error::InvalidArgument(format!(
                "script graph version {} is newer than this program ({VERSION})",
                g.version
            )));
        }
        Ok(g)
    }

    /// Pretty JSON with a trailing newline; nodes and links are sorted, so
    /// the text is the same for the same graph.
    pub fn to_json(&self) -> String {
        let mut g = self.clone();
        g.nodes.sort_by_key(|n| n.id);
        g.links.sort();
        g.links.dedup();
        let mut s = serde_json::to_string_pretty(&g).unwrap_or_default();
        s.push('\n');
        s
    }

    pub fn node(&self, id: u32) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn node_mut(&mut self, id: u32) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }

    /// An id no node has.
    pub fn next_id(&self) -> u32 {
        self.nodes.iter().map(|n| n.id).max().unwrap_or(0) + 1
    }

    /// Adds a node with a new id and returns the id.
    pub fn add(&mut self, mut node: Node) -> u32 {
        node.id = self.next_id();
        let id = node.id;
        self.nodes.push(node);
        id
    }

    /// Links output `from` to input `to` (no checks; the compiler reports bad links).
    pub fn connect(&mut self, from: (u32, &str), to: (u32, &str)) {
        self.links.push(Link {
            from: PinRef::new(from.0, from.1),
            to: PinRef::new(to.0, to.1),
        });
    }

    /// Removes a node and its links.
    pub fn remove(&mut self, id: u32) {
        self.nodes.retain(|n| n.id != id);
        self.links.retain(|l| l.from.0 != id && l.to.0 != id);
    }

    pub fn variable(&self, name: &str) -> Option<&Variable> {
        self.variables.iter().find(|v| v.name == name)
    }

    /// Name of the chunk: `<name>.lua`.
    pub fn chunk_name(&self) -> String {
        format!("{}.lua", self.name)
    }
}

/// Whether `s` is a Lua identifier that is not a reserved word.
pub fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
        && !crate::resource::lua::is_reserved(s.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_round_trip_is_stable() {
        let mut g = ScriptGraph::new("hello");
        let a = g.add(Node::new(0, "event.load", [0.0, 0.0]));
        let b = g.add(Node::new(0, "action.send_message", [200.0, 0.0]).input("message", "hi"));
        g.connect((a, "then"), (b, "exec"));
        let text = g.to_json();
        let back = ScriptGraph::from_json(&text).unwrap();
        assert_eq!(back.to_json(), text);
        assert_eq!(back.node(b).unwrap().inputs["message"], "hi");
        assert!(ScriptGraph::from_json("{\"format\":\"x\",\"name\":\"a\"}").is_err());
    }

    #[test]
    fn identifiers_and_pin_types() {
        assert!(is_identifier("count_2"));
        assert!(!is_identifier("2count"));
        assert!(!is_identifier("end"));
        assert!(!is_identifier("a-b"));
        assert!(PinType::Number.connects_to(PinType::String));
        assert!(!PinType::String.connects_to(PinType::Number));
        assert!(PinType::Any.connects_to(PinType::Position));
        assert!(!PinType::Exec.connects_to(PinType::Any));
        assert_eq!(PinType::parse("group"), Some(PinType::Group));
    }
}

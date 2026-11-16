//! The logic of a mission as a graph: SimGroup objects are nodes, message
//! names and GUID links are wires (like Kismet in Unreal Engine 3 or entity
//! I/O in Source).
//!
//! Objects of a SimGroup do not point at each other: a trigger sends a
//! message by name (`sdbm(lower(name))`, the engine's message table), and
//! every object that listens to that name reacts. The [`schema`] says which
//! attributes of which class are messages heard (`in`), messages sent
//! (`out`), links to other objects and parameters. [`GraphBuilder`] reads the
//! SimGroups of a [`scope`] (one group, a stream, a mission, a level with its
//! ancestors, the whole game), matches senders with listeners by hash and
//! classifies every message:
//!
//! - connected: sent and heard in the scope;
//! - from the engine: heard in the scope and named by `godfather2.exe` (the
//!   game code sends it: `iMsgNewCheckpointLoaded`, …);
//! - never sent / never heard in the scope — dangling; [`MessageIndex`] tells
//!   whether some other part of the game sends or hears it.
//!
//! Edits of the graph (connect, disconnect, add and remove nodes, change
//! parameters) are SimGroup edits recorded in a workspace: [`edit`]; [`check`]
//! tells when an edit gives a new message the hash of a game message or
//! leaves a message with one end only.
//! Messages as they go — from the running game or a run without it — are
//! matched to nodes and wires by [`trace`].
//! See `docs/logic-graph.md`.

pub mod check;
pub mod edit;
pub mod index;
pub mod run;
pub mod schema;
pub mod scope;
pub mod trace;

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use serde::Serialize;

use crate::game_types::{MessageId, SemanticType};
use crate::hash::string_id;
use crate::level::Location;
use crate::resource::simgroup::{Guid, SimGroup, Value};

pub use index::{MessageIndex, Site};
pub use schema::{Category, Confidence, LogicSchema, PinKind};
pub use scope::{Mission, ScopeSpec};

/// Index of a node in [`LogicGraph::nodes`].
pub type NodeId = usize;

/// A pin of a node: `(node, index in Node::pins)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct PinRef {
    pub node: NodeId,
    pub pin: usize,
}

/// What a node stands for.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NodeSource {
    /// Object `object` of the SimGroup at `group`.
    Object {
        #[serde(serialize_with = "ser_location")]
        group: Location,
        object: usize,
    },
    /// The game code: sends the engine messages heard in the scope and hears
    /// the ones sent to it.
    Engine,
    /// A message with many senders and listeners, drawn as one node.
    Hub { message: u32 },
}

fn ser_location<S: serde::Serializer>(l: &Location, s: S) -> Result<S::Ok, S::Error> {
    use serde::ser::SerializeStruct;
    let mut st = s.serialize_struct("Location", 2)?;
    st.serialize_field("stream", &l.stream)?;
    st.serialize_field("resource", &l.resource)?;
    st.end()
}

/// A pin: one attribute of the object (or a port of a virtual node).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Pin {
    /// Slot of the packet in the object; 0 for virtual nodes.
    pub slot: usize,
    /// Class of the packet that holds the attribute.
    pub class: u32,
    pub index: u16,
    /// Name from the schema, or `<class>.<index>`.
    pub name: String,
    pub kind: PinKind,
    pub conf: Option<Confidence>,
    /// The message name of an `in` or `out` pin; `None` when unset.
    pub message: Option<String>,
    /// Hash of [`Self::message`].
    pub hash: Option<u32>,
    /// Delay of an `out` pin, seconds.
    pub delay: Option<f32>,
    /// The GUID of a `link` or `asset` pin.
    pub target: Option<String>,
    /// The value shown for parameters.
    pub value: Option<String>,
    /// What the value means, when the schema or `attr_types.toml` tells:
    /// `MsgIn`, `Ref<CommonNPC>`, `Enum<family>`, `Seconds`…
    pub ty: Option<SemanticType>,
    /// The name of the value in its list, for `Enum` types (`corleone` for
    /// the hash `0637b907`).
    pub label: Option<String>,
}

/// A node of the graph.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Node {
    pub source: NodeSource,
    /// Name of the stream the object is in; empty for virtual nodes.
    pub stream: String,
    pub class: u32,
    pub class_name: String,
    pub category: Category,
    /// The object's name (`CMD_SetName`), when it has one.
    pub name: String,
    pub guid: Option<String>,
    /// World position, for objects placed in a level.
    pub position: Option<[f32; 3]>,
    pub pins: Vec<Pin>,
}

impl Node {
    /// A short title: the name, or the class with the first message heard.
    pub fn title(&self) -> String {
        if !self.name.is_empty() {
            return self.name.clone();
        }
        match self
            .pins
            .iter()
            .find(|p| p.kind == PinKind::In && p.message.is_some())
        {
            Some(p) => format!(
                "{} ({})",
                self.class_name,
                p.message.as_deref().unwrap_or("")
            ),
            None => self.class_name.clone(),
        }
    }

    /// The object, for nodes that are SimGroup objects.
    pub fn object(&self) -> Option<(Location, usize)> {
        match self.source {
            NodeSource::Object { group, object } => Some((group, object)),
            _ => None,
        }
    }
}

/// What a wire carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WireKind {
    /// A message from a sender to a listener.
    Message,
    /// A message from or to the engine.
    Engine,
    /// A GUID attribute pointing at another object.
    Link,
}

/// A wire from an `out` (or `link`) pin to an `in` pin (or a linked node).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Wire {
    pub from: PinRef,
    /// The pin the wire ends at; for links, pin 0 of the linked node is not
    /// meaningful: the wire ends at the node ([`Wire::to_node`]).
    pub to: PinRef,
    pub kind: WireKind,
    /// Hash of the message, for message wires.
    pub message: Option<u32>,
}

impl Wire {
    pub fn to_node(&self) -> NodeId {
        self.to.node
    }
}

/// How a message is used in the scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageState {
    /// Sent and heard in the scope.
    Connected,
    /// Heard in the scope; the engine sends it.
    FromEngine,
    /// Sent in the scope; the engine hears it.
    ToEngine,
    /// Heard in the scope, sent by nobody in it.
    NeverSent,
    /// Sent in the scope, heard by nobody in it.
    NeverHeard,
}

/// A message used in the scope.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Message {
    pub hash: u32,
    /// The name as first written in the scope.
    pub name: String,
    pub senders: Vec<PinRef>,
    pub listeners: Vec<PinRef>,
    pub state: MessageState,
    /// Whether the engine names this message.
    pub engine: bool,
    /// Where the name is known from: `engine` (a literal of the executable),
    /// `game` (the SimGroups of the game) or `custom` (new: a mod's message).
    pub origin: &'static str,
    /// Drawn through a hub node (too many senders × listeners).
    pub hub: Option<NodeId>,
    /// Senders and listeners in the rest of the game, outside the scope
    /// (set by [`LogicGraph::annotate`]).
    pub elsewhere: Option<(usize, usize)>,
}

impl Message {
    /// Dangling in the whole game: heard but sent nowhere (not even by the
    /// engine), or sent but heard nowhere. Needs [`LogicGraph::annotate`].
    pub fn dangling_in_game(&self) -> bool {
        matches!(
            (self.state, self.elsewhere),
            (MessageState::NeverSent, Some((0, _))) | (MessageState::NeverHeard, Some((_, 0)))
        )
    }
}

/// A graph of the logic of a scope.
#[derive(Debug, Clone, Default, Serialize)]
pub struct LogicGraph {
    pub nodes: Vec<Node>,
    pub wires: Vec<Wire>,
    /// Messages by hash.
    pub messages: BTreeMap<u32, Message>,
    /// SimGroups the graph was built from: location and stream name.
    #[serde(skip)]
    pub groups: Vec<(Location, String)>,
}

/// Options of [`GraphBuilder::build`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BuildOptions {
    /// Messages with more senders × listeners than this are drawn through a
    /// hub node; 0 never uses hubs.
    pub hub_threshold: usize,
    /// Add the engine node.
    pub engine_node: bool,
    /// Keep objects without any message or link pin set (props, buildings).
    pub keep_unwired: bool,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            hub_threshold: 48,
            engine_node: true,
            keep_unwired: false,
        }
    }
}

/// Collects SimGroups and makes a [`LogicGraph`].
pub struct GraphBuilder<'s> {
    schema: &'s LogicSchema,
    nodes: Vec<Node>,
    groups: Vec<(Location, String)>,
    /// GUID → node, for links.
    guids: HashMap<Guid, NodeId>,
    /// Pending links: (from pin, target GUID).
    links: Vec<(PinRef, Guid)>,
}

impl<'s> GraphBuilder<'s> {
    pub fn new(schema: &'s LogicSchema) -> Self {
        Self {
            schema,
            nodes: Vec::new(),
            groups: Vec::new(),
            guids: HashMap::new(),
            links: Vec::new(),
        }
    }

    /// Adds the objects of a SimGroup.
    pub fn add_group(&mut self, location: Location, stream: &str, group: &SimGroup) {
        self.groups.push((location, stream.to_owned()));
        for (i, o) in group.objects.iter().enumerate() {
            let node = self.nodes.len();
            let mut pins = Vec::new();
            for op in self.schema.object_pins(group, o) {
                let a = &op.attr;
                let name = a.name.clone().unwrap_or_else(|| {
                    let class = self
                        .schema
                        .class_name(op.class)
                        .map_or_else(|| format!("{:08x}", op.class), str::to_owned);
                    format!("{class}.{}", a.index)
                });
                let mut pin = Pin {
                    slot: op.slot,
                    class: op.class,
                    index: a.index,
                    name,
                    kind: a.pin,
                    conf: a.conf,
                    message: None,
                    hash: None,
                    delay: None,
                    target: None,
                    value: None,
                    ty: self.schema.semantic(op.class, a.index).map(|t| t.0),
                    label: None,
                };
                if let (Some(list), Some(v)) = (pin.ty.and_then(SemanticType::enum_def), &op.value)
                {
                    pin.label = list.find(v).map(|x| x.label());
                }
                match (a.pin, &op.value) {
                    (PinKind::In | PinKind::Out, Some(Value::String(s))) => {
                        pin.hash = Some(string_id(s));
                        pin.message = Some(s.clone());
                    }
                    (PinKind::Link, Some(Value::Guid(g))) => {
                        pin.target = Some(g.to_string());
                        self.links.push((
                            PinRef {
                                node,
                                pin: pins.len(),
                            },
                            *g,
                        ));
                    }
                    (PinKind::Asset, Some(Value::Guid(g))) => {
                        pin.target = Some(match g.resource_id() {
                            Some(id) => format!("{id:08x}"),
                            None => g.to_string(),
                        });
                    }
                    (_, Some(v)) => pin.value = Some(v.to_string()),
                    _ => {}
                }
                if a.pin == PinKind::Out
                    && let Some(d) = a.delay
                {
                    pin.delay = group
                        .object_packets(o)
                        .nth(op.slot)
                        .and_then(|p| p.attrs.iter().find(|x| x.index == d))
                        .and_then(|x| match x.value {
                            Value::Float(f) => Some(f),
                            Value::Int(i) => Some(i as f32),
                            _ => None,
                        });
                }
                pins.push(pin);
            }
            let class_name = o.class_name().map_or_else(
                || {
                    self.schema
                        .class_name(o.class)
                        .map_or_else(|| format!("{:08x}", o.class), str::to_owned)
                },
                str::to_owned,
            );
            self.guids.insert(o.guid, node);
            self.nodes.push(Node {
                source: NodeSource::Object {
                    group: location,
                    object: i,
                },
                stream: stream.to_owned(),
                class: o.class,
                class_name,
                category: self.schema.category(o.class),
                name: group.object_name(o).unwrap_or_default().to_owned(),
                guid: Some(o.guid.to_string()),
                position: group.transform(o).map(|m| [m[3][0], m[3][1], m[3][2]]),
                pins,
            });
        }
    }

    /// Matches senders with listeners and makes the graph.
    pub fn build(mut self, options: BuildOptions) -> LogicGraph {
        let schema = self.schema;
        // Messages.
        let mut messages: BTreeMap<u32, Message> = BTreeMap::new();
        for (n, node) in self.nodes.iter().enumerate() {
            for (p, pin) in node.pins.iter().enumerate() {
                let (Some(h), Some(name)) = (pin.hash, &pin.message) else {
                    continue;
                };
                let m = messages.entry(h).or_insert_with(|| Message {
                    hash: h,
                    name: name.clone(),
                    senders: Vec::new(),
                    listeners: Vec::new(),
                    state: MessageState::Connected,
                    engine: schema.engine_message(h).is_some(),
                    origin: MessageId::from_name(name).kind(),
                    hub: None,
                    elsewhere: None,
                });
                let at = PinRef { node: n, pin: p };
                match pin.kind {
                    PinKind::Out => m.senders.push(at),
                    PinKind::In => m.listeners.push(at),
                    _ => {}
                }
            }
        }
        // Objects that take part in nothing are dropped unless asked.
        let linked: HashSet<NodeId> = self
            .links
            .iter()
            .flat_map(|(from, g)| [Some(from.node), self.guids.get(g).copied()])
            .flatten()
            .collect();
        let keep: Vec<bool> = self
            .nodes
            .iter()
            .enumerate()
            .map(|(n, node)| {
                options.keep_unwired
                    || linked.contains(&n)
                    || node.pins.iter().any(|p| p.hash.is_some())
            })
            .collect();
        let mut remap = vec![usize::MAX; self.nodes.len()];
        let mut nodes = Vec::new();
        for (n, node) in std::mem::take(&mut self.nodes).into_iter().enumerate() {
            if keep[n] {
                remap[n] = nodes.len();
                nodes.push(node);
            }
        }
        for m in messages.values_mut() {
            for r in m.senders.iter_mut().chain(m.listeners.iter_mut()) {
                r.node = remap[r.node];
            }
        }
        let mut graph = LogicGraph {
            nodes,
            wires: Vec::new(),
            messages,
            groups: std::mem::take(&mut self.groups),
        };
        // Engine node.
        let mut engine: Option<NodeId> = None;
        let mut engine_pins: HashMap<(u32, PinKind), usize> = HashMap::new();
        let hashes: Vec<u32> = graph.messages.keys().copied().collect();
        for h in hashes {
            let (senders, listeners, is_engine) = {
                let m = &graph.messages[&h];
                (m.senders.clone(), m.listeners.clone(), m.engine)
            };
            let state = match (senders.is_empty(), listeners.is_empty(), is_engine) {
                (false, false, _) => MessageState::Connected,
                (true, false, true) => MessageState::FromEngine,
                (false, true, true) => MessageState::ToEngine,
                (true, false, false) => MessageState::NeverSent,
                _ => MessageState::NeverHeard,
            };
            graph.messages.get_mut(&h).expect("present").state = state;
            if !senders.is_empty() && !listeners.is_empty() {
                let fan = senders.len() * listeners.len();
                if options.hub_threshold > 0 && fan > options.hub_threshold {
                    let name = graph.messages[&h].name.clone();
                    let hub = graph.nodes.len();
                    graph.nodes.push(virtual_node(
                        NodeSource::Hub { message: h },
                        "Message",
                        vec![
                            virtual_pin("in", PinKind::In, &name),
                            virtual_pin("out", PinKind::Out, &name),
                        ],
                    ));
                    graph.messages.get_mut(&h).expect("present").hub = Some(hub);
                    for &s in &senders {
                        graph.wires.push(Wire {
                            from: s,
                            to: PinRef { node: hub, pin: 0 },
                            kind: WireKind::Message,
                            message: Some(h),
                        });
                    }
                    for &l in &listeners {
                        graph.wires.push(Wire {
                            from: PinRef { node: hub, pin: 1 },
                            to: l,
                            kind: WireKind::Message,
                            message: Some(h),
                        });
                    }
                } else {
                    for &s in &senders {
                        for &l in &listeners {
                            graph.wires.push(Wire {
                                from: s,
                                to: l,
                                kind: WireKind::Message,
                                message: Some(h),
                            });
                        }
                    }
                }
            } else if options.engine_node
                && matches!(state, MessageState::FromEngine | MessageState::ToEngine)
            {
                let e = *engine.get_or_insert_with(|| {
                    graph
                        .nodes
                        .push(virtual_node(NodeSource::Engine, "Engine", Vec::new()));
                    graph.nodes.len() - 1
                });
                let name = graph.messages[&h].name.clone();
                let kind = if state == MessageState::FromEngine {
                    PinKind::Out
                } else {
                    PinKind::In
                };
                let pin = *engine_pins.entry((h, kind)).or_insert_with(|| {
                    let pins = &mut graph.nodes[e].pins;
                    pins.push(virtual_pin(&name, kind, &name));
                    pins.len() - 1
                });
                let ep = PinRef { node: e, pin };
                if kind == PinKind::Out {
                    for &l in &listeners {
                        graph.wires.push(Wire {
                            from: ep,
                            to: l,
                            kind: WireKind::Engine,
                            message: Some(h),
                        });
                    }
                } else {
                    for &s in &senders {
                        graph.wires.push(Wire {
                            from: s,
                            to: ep,
                            kind: WireKind::Engine,
                            message: Some(h),
                        });
                    }
                }
            }
        }
        // Links.
        for (from, guid) in &self.links {
            let (Some(&to), from_node) = (self.guids.get(guid), remap[from.node]) else {
                continue;
            };
            let to = remap[to];
            if to == usize::MAX || from_node == usize::MAX || to == from_node {
                continue;
            }
            graph.wires.push(Wire {
                from: PinRef {
                    node: from_node,
                    pin: from.pin,
                },
                to: PinRef { node: to, pin: 0 },
                kind: WireKind::Link,
                message: None,
            });
        }
        graph
    }
}

fn virtual_node(source: NodeSource, class_name: &str, pins: Vec<Pin>) -> Node {
    Node {
        source,
        stream: String::new(),
        class: 0,
        class_name: class_name.to_owned(),
        category: Category::Other,
        name: String::new(),
        guid: None,
        position: None,
        pins,
    }
}

fn virtual_pin(name: &str, kind: PinKind, message: &str) -> Pin {
    Pin {
        slot: 0,
        class: 0,
        index: 0,
        name: name.to_owned(),
        kind,
        conf: None,
        message: Some(message.to_owned()),
        hash: Some(string_id(message)),
        delay: None,
        target: None,
        value: None,
        ty: Some(match kind {
            PinKind::In => SemanticType::MsgIn,
            _ => SemanticType::MsgOut,
        }),
        label: None,
    }
}

/// Counts of a graph.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Overview {
    pub groups: usize,
    pub nodes: usize,
    pub wires: usize,
    pub messages: usize,
    /// Messages by state.
    pub states: BTreeMap<String, usize>,
    /// Nodes by category.
    pub categories: BTreeMap<String, usize>,
    /// Nodes by stream.
    pub streams: BTreeMap<String, usize>,
    /// Dangling messages that no other part of the game sends or hears
    /// either (after [`LogicGraph::annotate`]).
    pub dangling_in_game: usize,
}

impl LogicGraph {
    /// A message by name.
    pub fn message(&self, name: &str) -> Option<&Message> {
        self.messages.get(&string_id(name))
    }

    /// Node of a SimGroup object.
    pub fn node_of(&self, group: Location, object: usize) -> Option<NodeId> {
        self.nodes
            .iter()
            .position(|n| n.source == NodeSource::Object { group, object })
    }

    /// Wires leaving a node.
    pub fn outgoing(&self, node: NodeId) -> impl Iterator<Item = &Wire> {
        self.wires.iter().filter(move |w| w.from.node == node)
    }

    /// Wires reaching a node.
    pub fn incoming(&self, node: NodeId) -> impl Iterator<Item = &Wire> {
        self.wires.iter().filter(move |w| w.to.node == node)
    }

    /// The shortest chain of wires from `from` to `to` (following their
    /// direction), as the nodes on the way including both ends.
    pub fn path(&self, from: NodeId, to: NodeId) -> Option<Vec<NodeId>> {
        let mut next: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for w in &self.wires {
            next.entry(w.from.node).or_default().push(w.to.node);
        }
        let mut prev: HashMap<NodeId, NodeId> = HashMap::new();
        let mut queue = VecDeque::from([from]);
        let mut seen = HashSet::from([from]);
        while let Some(n) = queue.pop_front() {
            if n == to {
                let mut path = vec![to];
                let mut at = to;
                while let Some(&p) = prev.get(&at) {
                    path.push(p);
                    at = p;
                }
                path.reverse();
                return Some(path);
            }
            for &m in next.get(&n).into_iter().flatten() {
                if seen.insert(m) {
                    prev.insert(m, n);
                    queue.push_back(m);
                }
            }
        }
        None
    }

    /// Counts, for every message, its senders and listeners outside the
    /// SimGroups of the graph.
    pub fn annotate(&mut self, index: &MessageIndex) {
        let inside: HashSet<Location> = self.groups.iter().map(|(l, _)| *l).collect();
        for (h, m) in &mut self.messages {
            let (s, l) = index.by_hash.get(h).map_or((0, 0), |sites| {
                (
                    sites
                        .senders
                        .iter()
                        .filter(|s| !inside.contains(&s.group))
                        .count(),
                    sites
                        .listeners
                        .iter()
                        .filter(|s| !inside.contains(&s.group))
                        .count(),
                )
            });
            m.elsewhere = Some((s, l));
        }
    }

    /// Messages that are dangling in the scope.
    pub fn dangling(&self) -> impl Iterator<Item = &Message> {
        self.messages
            .values()
            .filter(|m| matches!(m.state, MessageState::NeverSent | MessageState::NeverHeard))
    }

    /// Counts by state, category and stream.
    pub fn overview(&self) -> Overview {
        let mut o = Overview {
            groups: self.groups.len(),
            nodes: self.nodes.len(),
            wires: self.wires.len(),
            messages: self.messages.len(),
            ..Overview::default()
        };
        for m in self.messages.values() {
            *o.states.entry(state_name(m.state).to_owned()).or_default() += 1;
            o.dangling_in_game += usize::from(m.dangling_in_game());
        }
        for n in &self.nodes {
            *o.categories
                .entry(n.category.name().to_owned())
                .or_default() += 1;
            if !n.stream.is_empty() {
                *o.streams.entry(n.stream.clone()).or_default() += 1;
            }
        }
        o
    }

    /// The graph in the DOT language of Graphviz.
    pub fn to_dot(&self) -> String {
        let mut out =
            String::from("digraph logic {\n  rankdir=LR;\n  node [shape=box, fontsize=10];\n");
        for (i, n) in self.nodes.iter().enumerate() {
            let shape = match n.source {
                NodeSource::Engine => ", shape=doubleoctagon",
                NodeSource::Hub { .. } => ", shape=ellipse",
                NodeSource::Object { .. } => "",
            };
            out += &format!(
                "  n{i} [label=\"{}\\n{}\"{shape}];\n",
                dot_escape(&n.class_name),
                dot_escape(&n.title())
            );
        }
        for w in &self.wires {
            let label = w
                .message
                .and_then(|h| self.messages.get(&h))
                .map(|m| m.name.as_str())
                .unwrap_or("");
            let style = match w.kind {
                WireKind::Message => "",
                WireKind::Engine => ", style=dashed",
                WireKind::Link => ", style=dotted, arrowhead=odot",
            };
            out += &format!(
                "  n{} -> n{} [label=\"{}\"{style}];\n",
                w.from.node,
                w.to.node,
                dot_escape(label)
            );
        }
        out += "}\n";
        out
    }
}

/// Snake-case name of a message state.
pub fn state_name(state: MessageState) -> &'static str {
    match state {
        MessageState::Connected => "connected",
        MessageState::FromEngine => "from_engine",
        MessageState::ToEngine => "to_engine",
        MessageState::NeverSent => "never_sent",
        MessageState::NeverHeard => "never_heard",
    }
}

fn dot_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

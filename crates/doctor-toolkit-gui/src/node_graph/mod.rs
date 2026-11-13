//! A node-graph editor widget for egui: nodes with typed pins, wires,
//! comments, pan and zoom, selection, box selection, dragging, a minimap,
//! layered auto-layout and a sidecar file for node positions.
//!
//! The widget knows nothing about the game. The caller keeps a [`Graph`]
//! (nodes, wires, comments) made from its own model, draws it with
//! [`GraphView::show`] and turns the returned [`GraphEvent`]s into edits of
//! its model; the widget only moves nodes and comments itself.
//!
//! ```ignore
//! let mut graph = Graph::default();
//! graph.nodes.push(Node::new(NodeId(1), "Add").input(Pin::new("a", 0)).output(Pin::new("sum", 0)));
//! graph.nodes.push(Node::new(NodeId(2), "Print").input(Pin::new("text", 0)));
//! graph.wires.push(Wire::new((NodeId(1), 0), (NodeId(2), 0)));
//! let response = view.show(ui, &mut graph, &mut |ui, target| {
//!     // context menu items for `target`
//! });
//! for event in response.events {
//!     match event {
//!         GraphEvent::Connect { from, to } => { /* add a wire to the model */ }
//!         GraphEvent::Disconnect { wire } => { /* remove graph.wires[wire] */ }
//!         _ => {}
//!     }
//! }
//! ```
//!
//! Pins are typed by [`Pin::ty`]: a wire may join an output and an input of
//! the same type, or of any type when either is [`ANY`]. The caller can
//! refuse more with [`GraphView::can_connect`]. See `docs/logic-graph.md` §5
//! for the full API.

// The widget is the shared base of the logic editor and the script editor
// (stage 28); not every item of its API has a user yet.
#![allow(dead_code)]

mod layout;
mod store;
mod view;

pub use layout::{LayoutOptions, layered};
#[allow(unused_imports)]
pub use store::{LayoutStore, StoredComment};
#[allow(unused_imports)]
pub use view::{ContextTarget, GraphEvent, GraphResponse, GraphView, PinAddr};

use egui::{Color32, Pos2, Rect, Vec2};

/// A pin type that connects to any other.
pub const ANY: u32 = u32::MAX;

/// Stable id of a node, chosen by the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u64);

/// A pin on the left (input) or right (output) of a node.
#[derive(Debug, Clone, PartialEq)]
pub struct Pin {
    pub label: String,
    /// Type of the values the pin carries; see [`ANY`].
    pub ty: u32,
    pub color: Color32,
    /// The pin holds a value (drawn filled) even without a wire.
    pub set: bool,
    /// Shown when the pointer is over the pin.
    pub tooltip: Option<String>,
}

impl Pin {
    pub fn new(label: impl Into<String>, ty: u32) -> Self {
        Self {
            label: label.into(),
            ty,
            color: Color32::from_gray(200),
            set: false,
            tooltip: None,
        }
    }

    pub fn color(mut self, color: Color32) -> Self {
        self.color = color;
        self
    }

    pub fn set(mut self, set: bool) -> Self {
        self.set = set;
        self
    }

    pub fn tooltip(mut self, text: impl Into<String>) -> Self {
        self.tooltip = Some(text.into());
        self
    }
}

/// A small coloured mark in the header of a node (a warning, a state).
#[derive(Debug, Clone, PartialEq)]
pub struct Badge {
    pub text: String,
    pub color: Color32,
    pub tooltip: String,
}

/// A node.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub id: NodeId,
    /// Top-left corner in graph space.
    pub pos: Pos2,
    pub title: String,
    pub subtitle: String,
    pub header_color: Color32,
    /// Lines of text under the pins (key parameters).
    pub lines: Vec<String>,
    pub inputs: Vec<Pin>,
    pub outputs: Vec<Pin>,
    pub badges: Vec<Badge>,
    /// Drawn dimmed (a node from outside the edited part).
    pub ghost: bool,
}

impl Node {
    pub fn new(id: NodeId, title: impl Into<String>) -> Self {
        Self {
            id,
            pos: Pos2::ZERO,
            title: title.into(),
            subtitle: String::new(),
            header_color: Color32::from_rgb(70, 80, 100),
            lines: Vec::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            badges: Vec::new(),
            ghost: false,
        }
    }

    pub fn input(mut self, pin: Pin) -> Self {
        self.inputs.push(pin);
        self
    }

    pub fn output(mut self, pin: Pin) -> Self {
        self.outputs.push(pin);
        self
    }

    /// Size in graph space (zoom 1).
    pub fn size(&self) -> Vec2 {
        let chars = |s: &str| s.chars().count();
        let pin_chars = (0..self.inputs.len().max(self.outputs.len()))
            .map(|i| {
                self.inputs.get(i).map_or(0, |p| chars(&p.label))
                    + self.outputs.get(i).map_or(0, |p| chars(&p.label))
            })
            .max()
            .unwrap_or(0)
            + 4;
        let text = [
            chars(&self.title) + 3 * self.badges.len(),
            chars(&self.subtitle),
            pin_chars,
        ]
        .into_iter()
        .chain(self.lines.iter().map(|l| chars(l)))
        .max()
        .unwrap_or(0);
        let width = (text as f32 * metrics::CHAR).clamp(metrics::MIN_WIDTH, metrics::MAX_WIDTH);
        let rows = self.inputs.len().max(self.outputs.len()) + self.lines.len();
        Vec2::new(
            width,
            metrics::HEADER + rows as f32 * metrics::ROW + metrics::PAD,
        )
    }

    pub fn rect(&self) -> Rect {
        Rect::from_min_size(self.pos, self.size())
    }

    /// Position of input pin `i` (`output` false) or output pin `i`, in graph space.
    pub fn pin_pos(&self, output: bool, i: usize) -> Pos2 {
        let r = self.rect();
        let y = r.min.y + metrics::HEADER + (i as f32 + 0.5) * metrics::ROW;
        Pos2::new(if output { r.max.x } else { r.min.x }, y)
    }
}

/// How a wire is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireStyle {
    Solid,
    Dashed,
    Dotted,
}

/// A wire from output pin `from.1` of node `from.0` to input pin `to.1` of
/// node `to.0`.
#[derive(Debug, Clone, PartialEq)]
pub struct Wire {
    pub from: (NodeId, usize),
    pub to: (NodeId, usize),
    pub color: Color32,
    pub style: WireStyle,
    /// Shown when the pointer is over the wire.
    pub label: Option<String>,
}

impl Wire {
    pub fn new(from: (NodeId, usize), to: (NodeId, usize)) -> Self {
        Self {
            from,
            to,
            color: Color32::from_gray(180),
            style: WireStyle::Solid,
            label: None,
        }
    }
}

/// A coloured box with a title behind nodes; moving it moves the nodes inside.
#[derive(Debug, Clone, PartialEq)]
pub struct Comment {
    pub id: u64,
    /// In graph space.
    pub rect: Rect,
    pub text: String,
    pub color: Color32,
}

/// Everything the widget draws.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub wires: Vec<Wire>,
    pub comments: Vec<Comment>,
}

impl Graph {
    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn node_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }

    /// Bounding box of all nodes and comments in graph space.
    pub fn bounds(&self) -> Option<Rect> {
        self.nodes
            .iter()
            .map(Node::rect)
            .chain(self.comments.iter().map(|c| c.rect))
            .reduce(|a, b| a.union(b))
    }

    /// A comment around the given nodes, with a margin.
    pub fn comment_around(&self, ids: &[NodeId], text: &str) -> Option<Comment> {
        let rect = ids
            .iter()
            .filter_map(|&id| self.node(id))
            .map(Node::rect)
            .reduce(|a, b| a.union(b))?;
        Some(Comment {
            id: self.comments.iter().map(|c| c.id + 1).max().unwrap_or(1),
            rect: Rect::from_min_max(
                rect.min - Vec2::new(20.0, 20.0 + metrics::COMMENT_BAR),
                rect.max + Vec2::new(20.0, 20.0),
            ),
            text: text.to_owned(),
            color: Color32::from_rgba_unmultiplied(90, 110, 150, 60),
        })
    }
}

/// Sizes of the drawing at zoom 1.
pub(crate) mod metrics {
    pub const CHAR: f32 = 7.0;
    pub const MIN_WIDTH: f32 = 140.0;
    pub const MAX_WIDTH: f32 = 380.0;
    pub const HEADER: f32 = 34.0;
    pub const ROW: f32 = 18.0;
    pub const PAD: f32 = 6.0;
    pub const PIN_RADIUS: f32 = 5.0;
    pub const COMMENT_BAR: f32 = 22.0;
}

#[cfg(test)]
mod tests;

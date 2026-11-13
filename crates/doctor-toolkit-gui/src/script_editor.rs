//! The visual script editor: a graph of nodes ([`doctor_toolkit::script`])
//! drawn with the node-graph widget, compiled to Lua on every edit.
//!
//! Left: the node palette. Centre: the graph (right click adds nodes, wires
//! join typed pins, errors show as badges on the nodes). Right: the selected
//! node's settings and input values, and the script's start settings.
//! Bottom: diagnostics, the generated Lua and a dry run against the mock
//! game. The editor never touches the game or the workspace itself: Deploy
//! and Remove are returned as [`ScriptAction`]s for the app to record.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use doctor_toolkit::script::graph::{Link, Node, PinRef, PinType, ScriptGraph, Variable};
use doctor_toolkit::script::library::{self, Category, ParamKind};
use doctor_toolkit::script::sim::{Sim, SimEvent};
use doctor_toolkit::script::{Compiled, Severity, compile};
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::{Color32, Pos2, RichText};
use serde_json::Value as Json;

use crate::node_graph::{
    self as ng, Badge, ContextTarget, GraphEvent, GraphView, NodeId, PinAddr, Wire, WireStyle,
};

/// What the app should do for the editor.
pub enum ScriptAction {
    /// Put the compiled script into the level stream `stream`.
    Deploy {
        stream: String,
        graph: Box<ScriptGraph>,
        chunk: Vec<u8>,
    },
    /// Take the script out of `stream`.
    Remove { stream: String, name: String },
}

/// Tabs of the bottom panel.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Bottom {
    Problems,
    Lua,
    Run,
    /// The script's journal in the running game (stage 35).
    Game,
}

pub struct ScriptEditor {
    pub open: bool,
    graph: ScriptGraph,
    /// File the graph was opened from or saved to.
    pub path: Option<PathBuf>,
    view: GraphView,
    shown: ng::Graph,
    /// Index of `graph.links` for every wire of `shown`.
    wire_links: Vec<usize>,
    compiled: Compiled,
    selected: Option<u32>,
    filter: String,
    bottom: Bottom,
    /// Level stream to deploy to.
    pub stream: String,
    steps: String,
    run_log: Vec<String>,
    clipboard: Vec<Node>,
    clipboard_links: Vec<Link>,
    undo: Vec<ScriptGraph>,
    redo: Vec<ScriptGraph>,
    pub status: String,
    dirty: bool,
    /// The script in the running game: nodes that fired, errors, breakpoints.
    pub live: crate::script_live::ScriptLive,
}

impl Default for ScriptEditor {
    fn default() -> Self {
        Self::new(starter())
    }
}

/// A new script: On start → File probe.
fn starter() -> ScriptGraph {
    let mut g = ScriptGraph::new("my_script");
    let a = g.add(Node::new(0, "event.load", [0.0, 0.0]));
    let b = g.add(Node::new(0, "debug.probe", [260.0, 0.0]).input("tag", "my_script_started"));
    g.connect((a, "then"), (b, "exec"));
    g
}

/// Number of a pin type for the widget (same number ⇔ wires may join).
fn pin_ty(t: PinType) -> u32 {
    match t {
        PinType::Any => ng::ANY,
        other => PinType::DATA
            .iter()
            .position(|d| *d == other)
            .map_or(100, |i| i as u32 + 1),
    }
}

fn pin_color(t: PinType) -> Color32 {
    match t {
        PinType::Exec => Color32::from_gray(235),
        PinType::Number => Color32::from_rgb(120, 200, 120),
        PinType::Bool => Color32::from_rgb(220, 90, 90),
        PinType::String => Color32::from_rgb(230, 120, 220),
        PinType::Position => Color32::from_rgb(240, 200, 80),
        PinType::Group => Color32::from_rgb(90, 200, 230),
        PinType::Any => Color32::from_gray(150),
    }
}

fn header_color(c: Category) -> Color32 {
    match c {
        Category::Event => Color32::from_rgb(140, 40, 40),
        Category::Flow => Color32::from_rgb(70, 70, 80),
        Category::Value | Category::Math | Category::Logic | Category::Text => {
            Color32::from_rgb(50, 90, 60)
        }
        Category::Variable => Color32::from_rgb(60, 80, 120),
        Category::Game | Category::Group => Color32::from_rgb(100, 80, 40),
        Category::Message | Category::Timer => Color32::from_rgb(40, 90, 110),
        Category::Audio => Color32::from_rgb(90, 60, 110),
        Category::Lua | Category::Debug | Category::Comment => Color32::from_rgb(80, 80, 80),
    }
}

fn json_text(v: &Json) -> String {
    match v {
        Json::String(s) => s.clone(),
        Json::Null => String::new(),
        other => other.to_string(),
    }
}

impl ScriptEditor {
    pub fn new(graph: ScriptGraph) -> Self {
        let compiled = compile(&graph);
        let mut e = Self {
            open: false,
            graph,
            path: None,
            view: GraphView::default(),
            shown: ng::Graph::default(),
            wire_links: Vec::new(),
            compiled,
            selected: None,
            filter: String::new(),
            bottom: Bottom::Problems,
            stream: "gf2/globals_global".into(),
            steps: "wait:1".into(),
            run_log: Vec::new(),
            clipboard: Vec::new(),
            clipboard_links: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            status: String::new(),
            dirty: false,
            live: crate::script_live::ScriptLive::default(),
        };
        e.rebuild();
        e.view.frame_all();
        e
    }

    /// Opens a graph (replacing the current one).
    pub fn load(&mut self, graph: ScriptGraph, path: Option<PathBuf>) {
        let stream = std::mem::take(&mut self.stream);
        *self = Self::new(graph);
        self.stream = stream;
        self.path = path;
        self.open = true;
    }

    pub fn graph(&self) -> &ScriptGraph {
        &self.graph
    }

    /// Shows the script's journal in the game (the bottom panel).
    pub fn show_game_journal(&mut self) {
        self.bottom = Bottom::Game;
    }

    /// Prints and Lua errors of the running game (stage 35).
    pub fn live_feed(&mut self, events: &[doctor_toolkit::live::proto::Event]) {
        self.live.feed(&self.graph, &self.compiled, events);
    }

    pub fn has_unsaved_edits(&self) -> bool {
        self.dirty
    }

    pub fn mark_saved(&mut self, path: PathBuf) {
        self.path = Some(path);
        self.dirty = false;
    }

    /// Records the graph before an edit (undo) and recompiles after it.
    fn edit(&mut self, f: impl FnOnce(&mut ScriptGraph)) {
        let before = self.graph.clone();
        f(&mut self.graph);
        if self.graph != before {
            self.undo.push(before);
            if self.undo.len() > 200 {
                self.undo.remove(0);
            }
            self.redo.clear();
            self.dirty = true;
            self.changed();
        }
    }

    fn changed(&mut self) {
        self.compiled = compile(&self.graph);
        self.rebuild();
    }

    pub fn undo(&mut self) {
        if let Some(g) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.graph, g));
            self.dirty = true;
            self.changed();
        }
    }

    pub fn redo(&mut self) {
        if let Some(g) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.graph, g));
            self.dirty = true;
            self.changed();
        }
    }

    /// The widget's graph from the script graph.
    fn rebuild(&mut self) {
        let mut shown = ng::Graph::default();
        let mut pins: HashMap<u32, (Vec<library::Pin>, Vec<library::Pin>)> = HashMap::new();
        for n in &self.graph.nodes {
            let Some(def) = library::get(&n.kind) else {
                let mut node = ng::Node::new(NodeId(u64::from(n.id)), format!("? {}", n.kind));
                node.pos = Pos2::new(n.pos[0], n.pos[1]);
                node.header_color = Color32::DARK_RED;
                shown.nodes.push(node);
                continue;
            };
            if def.category == Category::Comment {
                let w = n.param_f64("width").unwrap_or(300.0) as f32;
                let h = n.param_f64("height").unwrap_or(120.0) as f32;
                shown.comments.push(ng::Comment {
                    id: u64::from(n.id),
                    rect: egui::Rect::from_min_size(
                        Pos2::new(n.pos[0], n.pos[1]),
                        egui::vec2(w, h),
                    ),
                    text: n.param_str("text").unwrap_or_default().to_owned(),
                    color: Color32::from_rgba_unmultiplied(90, 110, 150, 60),
                });
                continue;
            }
            let (ins, outs) = library::pins(&self.graph, n).unwrap_or_default();
            let mut node = ng::Node::new(NodeId(u64::from(n.id)), def.title);
            node.pos = Pos2::new(n.pos[0], n.pos[1]);
            node.header_color = header_color(def.category);
            node.subtitle = format!("#{} {}", n.id, def.kind);
            for p in def.params {
                if matches!(p.kind, ParamKind::Pins | ParamKind::Code) {
                    continue;
                }
                let v = library::param(n, def, p.name);
                let text = json_text(&v);
                if !text.is_empty() {
                    node.lines.push(format!("{}: {text}", p.label));
                }
            }
            if !def.verified {
                node.badges.push(Badge {
                    text: "?".into(),
                    color: Color32::from_rgb(150, 150, 150),
                    tooltip: tr("script.unverified").to_owned(),
                });
            }
            for p in &ins {
                let linked = self
                    .graph
                    .links
                    .iter()
                    .any(|l| l.to.0 == n.id && l.to.1 == p.name);
                let mut pin = ng::Pin::new(p.label.clone(), pin_ty(p.ty)).color(pin_color(p.ty));
                if p.ty != PinType::Exec {
                    let value = n.inputs.get(&p.name).map(json_text);
                    pin.set = value.is_some();
                    let label = match &value {
                        Some(v) if !linked => format!("{} = {v}", p.label),
                        _ => p.label.clone(),
                    };
                    pin.label = label;
                    pin = pin.tooltip(format!("{} ({})", p.name, p.ty.name()));
                }
                node.inputs.push(pin);
            }
            for p in &outs {
                node.outputs.push(
                    ng::Pin::new(p.label.clone(), pin_ty(p.ty))
                        .color(pin_color(p.ty))
                        .tooltip(format!("{} ({})", p.name, p.ty.name())),
                );
            }
            for d in self.compiled.for_node(n.id) {
                let (text, color) = match d.severity {
                    Severity::Error => ("!", Color32::from_rgb(230, 60, 60)),
                    Severity::Warning => ("⚠", Color32::from_rgb(230, 180, 60)),
                };
                node.badges.push(Badge {
                    text: text.into(),
                    color,
                    tooltip: d.message.clone(),
                });
            }
            pins.insert(n.id, (ins, outs));
            shown.nodes.push(node);
        }
        let mut wire_links = Vec::new();
        for (i, l) in self.graph.links.iter().enumerate() {
            let (Some((_, outs)), Some((ins, _))) = (pins.get(&l.from.0), pins.get(&l.to.0)) else {
                continue;
            };
            let (Some(a), Some(b)) = (
                outs.iter().position(|p| p.name == l.from.1),
                ins.iter().position(|p| p.name == l.to.1),
            ) else {
                continue;
            };
            let ty = outs[a].ty;
            let mut w = Wire::new(
                (NodeId(u64::from(l.from.0)), a),
                (NodeId(u64::from(l.to.0)), b),
            );
            w.color = pin_color(ty);
            if ty == PinType::Exec {
                w.style = WireStyle::Solid;
            }
            shown.wires.push(w);
            wire_links.push(i);
        }
        self.shown = shown;
        self.wire_links = wire_links;
    }

    /// Places the nodes in layers along the wires (the logic graph's layout).
    fn auto_layout(&mut self) {
        let pos = ng::layered(&self.shown, None, &ng::LayoutOptions::default());
        self.edit(|g| {
            for n in &mut g.nodes {
                if let Some(p) = pos.get(&NodeId(u64::from(n.id))) {
                    n.pos = [p.x, p.y];
                }
            }
        });
        self.view.frame_all();
    }

    fn pin_name(&self, addr: PinAddr) -> Option<(u32, String, PinType)> {
        let id = u32::try_from(addr.node.0).ok()?;
        let n = self.graph.node(id)?;
        let (ins, outs) = library::pins(&self.graph, n)?;
        let p = if addr.output { outs } else { ins }
            .into_iter()
            .nth(addr.pin)?;
        Some((id, p.name, p.ty))
    }

    fn add_node(&mut self, kind: &str, at: Pos2) {
        let mut node = Node::new(0, kind, [at.x, at.y]);
        if let Some(def) = library::get(kind) {
            for p in def.params {
                if let Ok(v) = serde_json::from_str::<Json>(p.default) {
                    node.params.insert(p.name.to_owned(), v);
                }
            }
        }
        let mut id = 0;
        self.edit(|g| id = g.add(node));
        self.selected = Some(id);
        self.view.set_selection([NodeId(u64::from(id))]);
    }

    fn on_event(&mut self, event: GraphEvent) {
        match event {
            GraphEvent::Selection(ids) => {
                self.selected = ids.first().and_then(|id| u32::try_from(id.0).ok());
            }
            GraphEvent::Activate(id) => self.selected = u32::try_from(id.0).ok(),
            GraphEvent::Moved(ids) => {
                let pos: BTreeMap<u32, Pos2> = ids
                    .iter()
                    .filter_map(|id| Some((u32::try_from(id.0).ok()?, self.shown.node(*id)?.pos)))
                    .collect();
                self.edit(|g| {
                    for n in &mut g.nodes {
                        if let Some(p) = pos.get(&n.id) {
                            n.pos = [p.x, p.y];
                        }
                    }
                });
            }
            GraphEvent::CommentsChanged => {
                let comments: BTreeMap<u32, (egui::Rect, String)> = self
                    .shown
                    .comments
                    .iter()
                    .filter_map(|c| Some((u32::try_from(c.id).ok()?, (c.rect, c.text.clone()))))
                    .collect();
                // Comments moved with their nodes: take the node positions too.
                let pos: BTreeMap<u32, Pos2> = self
                    .shown
                    .nodes
                    .iter()
                    .filter_map(|n| Some((u32::try_from(n.id.0).ok()?, n.pos)))
                    .collect();
                self.edit(|g| {
                    for n in &mut g.nodes {
                        if let Some((r, text)) = comments.get(&n.id) {
                            n.pos = [r.min.x, r.min.y];
                            n.params
                                .insert("width".into(), Json::from(f64::from(r.width())));
                            n.params
                                .insert("height".into(), Json::from(f64::from(r.height())));
                            n.params.insert("text".into(), Json::from(text.clone()));
                        } else if let Some(p) = pos.get(&n.id) {
                            n.pos = [p.x, p.y];
                        }
                    }
                });
            }
            GraphEvent::Connect { from, to } => {
                let (Some(a), Some(b)) = (self.pin_name(from), self.pin_name(to)) else {
                    return;
                };
                self.edit(|g| {
                    // An exec output goes to one place, a data input takes one wire.
                    if a.2 == PinType::Exec {
                        g.links.retain(|l| !(l.from.0 == a.0 && l.from.1 == a.1));
                    } else {
                        g.links.retain(|l| !(l.to.0 == b.0 && l.to.1 == b.1));
                    }
                    g.links.push(Link {
                        from: PinRef(a.0, a.1),
                        to: PinRef(b.0, b.1),
                    });
                });
            }
            GraphEvent::Disconnect { wire } => {
                if let Some(&i) = self.wire_links.get(wire) {
                    self.edit(|g| {
                        g.links.remove(i);
                    });
                }
            }
            GraphEvent::Delete(ids) => {
                let ids: Vec<u32> = ids.iter().filter_map(|i| u32::try_from(i.0).ok()).collect();
                self.edit(|g| {
                    for id in &ids {
                        g.remove(*id);
                    }
                });
                self.selected = None;
            }
            GraphEvent::Copy(ids) => self.copy(&ids),
            GraphEvent::Paste(at) => self.paste(at),
        }
    }

    fn copy(&mut self, ids: &[NodeId]) {
        let ids: Vec<u32> = ids.iter().filter_map(|i| u32::try_from(i.0).ok()).collect();
        self.clipboard = self
            .graph
            .nodes
            .iter()
            .filter(|n| ids.contains(&n.id))
            .cloned()
            .collect();
        self.clipboard_links = self
            .graph
            .links
            .iter()
            .filter(|l| ids.contains(&l.from.0) && ids.contains(&l.to.0))
            .cloned()
            .collect();
        self.status = trf(
            "script.copied",
            &[(
                "nodes",
                &trn("count.nodes", self.clipboard.len() as u64, &[]),
            )],
        );
    }

    fn paste(&mut self, at: Pos2) {
        if self.clipboard.is_empty() {
            return;
        }
        let min_x = self
            .clipboard
            .iter()
            .map(|n| n.pos[0])
            .fold(f32::MAX, f32::min);
        let min_y = self
            .clipboard
            .iter()
            .map(|n| n.pos[1])
            .fold(f32::MAX, f32::min);
        let (nodes, links) = (self.clipboard.clone(), self.clipboard_links.clone());
        let mut new_ids = Vec::new();
        self.edit(|g| {
            let mut map = HashMap::new();
            for n in nodes {
                let mut m = n.clone();
                m.pos = [n.pos[0] - min_x + at.x, n.pos[1] - min_y + at.y];
                let id = g.add(m);
                map.insert(n.id, id);
                new_ids.push(id);
            }
            for l in links {
                g.links.push(Link {
                    from: PinRef(map[&l.from.0], l.from.1),
                    to: PinRef(map[&l.to.0], l.to.1),
                });
            }
        });
        self.view
            .set_selection(new_ids.iter().map(|&i| NodeId(u64::from(i))));
    }

    /// Draws the editor. Returns what the app should do.
    pub fn show(&mut self, ui: &mut egui::Ui) -> Option<ScriptAction> {
        let mut action = None;
        self.toolbar(ui, &mut action);
        egui::Panel::left("script-palette")
            .resizable(true)
            .default_size(200.0)
            .show(ui, |ui| self.palette(ui));
        egui::Panel::right("script-inspector")
            .resizable(true)
            .default_size(300.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| self.inspector(ui));
            });
        egui::Panel::bottom("script-bottom")
            .resizable(true)
            .default_size(180.0)
            .show(ui, |ui| self.bottom_panel(ui));
        let mut menu: Option<MenuCmd> = None;
        let clipboard = !self.clipboard.is_empty();
        let response = self.view.show(ui, &mut self.shown, &mut |ui, target| {
            if let Some(cmd) = context_menu(ui, target, clipboard) {
                menu = Some(cmd);
            }
        });
        self.live.paint(
            ui,
            response.response.rect,
            &self.view,
            &self.shown,
            &self.graph,
        );
        if self.live.animating() {
            ui.ctx().request_repaint();
        }
        for event in response.events {
            self.on_event(event);
        }
        match menu {
            Some(MenuCmd::Add(kind, at)) => self.add_node(kind, at),
            Some(MenuCmd::Paste(at)) => self.paste(at),
            Some(MenuCmd::Delete(id)) => {
                self.edit(|g| g.remove(id));
                self.selected = None;
            }
            Some(MenuCmd::Duplicate(id)) => {
                self.copy(&[NodeId(u64::from(id))]);
                let at = self
                    .graph
                    .node(id)
                    .map_or(Pos2::ZERO, |n| Pos2::new(n.pos[0] + 40.0, n.pos[1] + 40.0));
                self.paste(at);
            }
            Some(MenuCmd::Disconnect(w)) => self.on_event(GraphEvent::Disconnect { wire: w }),
            Some(MenuCmd::Comment) => {
                let ids: Vec<NodeId> = self.view.selection().iter().copied().collect();
                if let Some(c) = self.shown.comment_around(&ids, tr("script.comment")) {
                    let node = Node::new(0, "comment", [c.rect.min.x, c.rect.min.y])
                        .param("text", c.text)
                        .param("width", f64::from(c.rect.width()))
                        .param("height", f64::from(c.rect.height()));
                    self.edit(|g| {
                        g.add(node);
                    });
                }
            }
            Some(MenuCmd::Frame) => self.view.frame_all(),
            None => {}
        }
        if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Z)) {
            if ui.input(|i| i.modifiers.shift) {
                self.redo();
            } else {
                self.undo();
            }
        }
        action
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, action: &mut Option<ScriptAction>) {
        ui.horizontal_wrapped(|ui| {
            ui.label(tr("script.name"));
            let mut name = self.graph.name.clone();
            if ui
                .add(egui::TextEdit::singleline(&mut name).desired_width(140.0))
                .changed()
            {
                self.edit(|g| g.name = name);
            }
            if ui.button(tr("script.new")).clicked() {
                self.load(starter(), None);
            }
            if ui.button(tr("script.open")).clicked()
                && let Some(p) = rfd::FileDialog::new()
                    .add_filter(tr("script.graph_filter"), &["json"])
                    .pick_file()
            {
                match std::fs::read_to_string(&p)
                    .map_err(|e| e.to_string())
                    .and_then(|t| ScriptGraph::from_json(&t).map_err(|e| e.to_string()))
                {
                    Ok(g) => self.load(g, Some(p)),
                    Err(e) => self.status = e,
                }
            }
            let save_as = ui.button(tr("script.save_as")).clicked();
            if (ui.button(tr("script.save")).clicked() && self.path.is_some()) || save_as {
                let path = if save_as || self.path.is_none() {
                    rfd::FileDialog::new()
                        .add_filter(tr("script.graph_filter"), &["json"])
                        .set_file_name(format!("{}.json", self.graph.name))
                        .save_file()
                } else {
                    self.path.clone()
                };
                if let Some(p) = path {
                    match std::fs::write(&p, self.graph.to_json()) {
                        Ok(()) => {
                            self.status = trf("script.saved", &[("path", &p.display())]);
                            self.mark_saved(p);
                        }
                        Err(e) => self.status = e.to_string(),
                    }
                }
            }
            ui.separator();
            if ui
                .add_enabled(!self.undo.is_empty(), egui::Button::new(tr("script.undo")))
                .clicked()
            {
                self.undo();
            }
            if ui
                .add_enabled(!self.redo.is_empty(), egui::Button::new(tr("script.redo")))
                .clicked()
            {
                self.redo();
            }
            if ui.button(tr("script.frame")).clicked() {
                self.view.frame_all();
            }
            if ui.button(tr("script.auto_layout")).clicked() {
                self.auto_layout();
            }
            ui.separator();
            ui.label(tr("script.stream"));
            ui.add(
                egui::TextEdit::singleline(&mut self.stream)
                    .desired_width(220.0)
                    .hint_text("gf2/globals_global"),
            );
            let ok = !self.compiled.has_errors();
            if ui
                .add_enabled(ok, egui::Button::new(tr("script.deploy")))
                .on_hover_text(tr("script.deploy_hint"))
                .clicked()
                && let Some(chunk) = self.compiled.chunk.clone()
            {
                *action = Some(ScriptAction::Deploy {
                    stream: self.stream.trim().to_owned(),
                    graph: Box::new(self.graph.clone()),
                    chunk,
                });
            }
            if ui.button(tr("script.remove")).clicked() {
                *action = Some(ScriptAction::Remove {
                    stream: self.stream.trim().to_owned(),
                    name: self.graph.chunk_name(),
                });
            }
            let errors = self.compiled.errors().count();
            let text = if errors == 0 {
                RichText::new(tr("script.compiles")).color(Color32::from_rgb(110, 200, 110))
            } else {
                RichText::new(trn("count.errors", errors as u64, &[]))
                    .color(Color32::from_rgb(230, 80, 80))
            };
            ui.label(text);
            if !self.status.is_empty() {
                ui.label(RichText::new(&self.status).weak());
            }
        });
    }

    fn palette(&mut self, ui: &mut egui::Ui) {
        ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text(tr("script.search_nodes")));
        let filter = self.filter.to_lowercase();
        let at = self
            .shown
            .bounds()
            .map_or(Pos2::ZERO, |b| Pos2::new(b.min.x, b.max.y + 40.0));
        let mut add = None;
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                let mut by_cat: BTreeMap<Category, Vec<&library::NodeDef>> = BTreeMap::new();
                for d in library::all() {
                    if filter.is_empty()
                        || d.title.to_lowercase().contains(&filter)
                        || d.kind.contains(&filter)
                    {
                        by_cat.entry(d.category).or_default().push(d);
                    }
                }
                for (cat, defs) in by_cat {
                    egui::CollapsingHeader::new(cat.name())
                        .default_open(!filter.is_empty() || cat == Category::Event)
                        .show(ui, |ui| {
                            for d in defs {
                                let r = ui
                                    .button(d.title)
                                    .on_hover_text(format!("{}\n{}", d.kind, d.help));
                                if r.clicked() {
                                    add = Some(d.kind);
                                }
                            }
                        });
                }
            });
        if let Some(kind) = add {
            self.add_node(kind, at);
        }
    }

    fn inspector(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.selected.filter(|id| self.graph.node(*id).is_some()) else {
            self.script_settings(ui);
            return;
        };
        let node = self
            .graph
            .node(id)
            .cloned()
            .unwrap_or_else(|| Node::new(id, "", [0.0; 2]));
        let Some(def) = library::get(&node.kind) else {
            ui.label(trf("script.unknown_kind", &[("kind", &node.kind)]));
            return;
        };
        ui.heading(def.title);
        ui.label(RichText::new(format!("#{id} {}", def.kind)).weak());
        ui.label(def.help);
        self.live.node_ui(ui, &self.graph, id);
        if !def.verified {
            ui.label(RichText::new(tr("script.unverified")).italics());
        }
        for d in self.compiled.for_node(id) {
            let color = match d.severity {
                Severity::Error => Color32::from_rgb(230, 80, 80),
                Severity::Warning => Color32::from_rgb(230, 180, 60),
            };
            let pin = d
                .pin
                .as_deref()
                .map(|p| format!("{p}: "))
                .unwrap_or_default();
            ui.colored_label(color, format!("{pin}{}", d.message));
        }
        ui.separator();
        let mut new_params = node.params.clone();
        for p in def.params {
            let current = library::param(&node, def, p.name);
            ui.label(p.label);
            match p.kind {
                ParamKind::Bool => {
                    let mut b = current.as_bool().unwrap_or(false);
                    if ui.checkbox(&mut b, "").changed() {
                        new_params.insert(p.name.into(), Json::from(b));
                    }
                }
                ParamKind::Number => {
                    let mut x = current.as_f64().unwrap_or(0.0);
                    if ui.add(egui::DragValue::new(&mut x).speed(0.1)).changed() {
                        new_params.insert(p.name.into(), Json::from(x));
                    }
                }
                ParamKind::Code => {
                    let mut text = json_text(&current);
                    if ui
                        .add(
                            egui::TextEdit::multiline(&mut text)
                                .code_editor()
                                .desired_rows(6)
                                .desired_width(f32::INFINITY),
                        )
                        .changed()
                    {
                        new_params.insert(p.name.into(), Json::from(text));
                    }
                }
                ParamKind::Pins => {
                    // name:type, one per line.
                    let mut text = library::pin_list(&current)
                        .iter()
                        .map(|p| format!("{}:{}", p.name, p.ty.name()))
                        .collect::<Vec<_>>()
                        .join("\n");
                    if ui
                        .add(
                            egui::TextEdit::multiline(&mut text)
                                .desired_rows(3)
                                .hint_text("name:number"),
                        )
                        .on_hover_text(tr("script.pins_hint"))
                        .changed()
                    {
                        let list: Vec<Json> = text
                            .lines()
                            .filter_map(|l| {
                                let (n, t) = l.split_once(':').unwrap_or((l, "any"));
                                let n = n.trim();
                                (!n.is_empty())
                                    .then(|| serde_json::json!({"name": n, "type": t.trim()}))
                            })
                            .collect();
                        new_params.insert(p.name.into(), Json::from(list));
                    }
                }
                ParamKind::Text | ParamKind::Name => {
                    let mut text = json_text(&current);
                    if ui.text_edit_singleline(&mut text).changed() {
                        new_params.insert(p.name.into(), Json::from(text));
                    }
                }
            }
        }
        if new_params != node.params {
            self.edit(|g| {
                if let Some(n) = g.node_mut(id) {
                    n.params = new_params;
                }
            });
        }
        // Values of unlinked data inputs.
        let (ins, _) = library::pins(&self.graph, &node).unwrap_or_default();
        let mut new_inputs = node.inputs.clone();
        let mut any = false;
        for p in ins.iter().filter(|p| p.ty != PinType::Exec) {
            let linked = self
                .graph
                .links
                .iter()
                .any(|l| l.to.0 == id && l.to.1 == p.name);
            if linked || matches!(p.ty, PinType::Position | PinType::Group) {
                continue;
            }
            if !any {
                ui.separator();
                ui.label(RichText::new(tr("script.inputs")).strong());
                any = true;
            }
            ui.horizontal(|ui| {
                ui.label(&p.label);
                let current = node
                    .inputs
                    .get(&p.name)
                    .cloned()
                    .or_else(|| library::input_default(def.kind, &p.name));
                match p.ty {
                    PinType::Bool => {
                        let mut b = current.and_then(|v| v.as_bool()).unwrap_or(false);
                        if ui.checkbox(&mut b, "").changed() {
                            new_inputs.insert(p.name.clone(), Json::from(b));
                        }
                    }
                    PinType::Number => {
                        let mut x = current.and_then(|v| v.as_f64()).unwrap_or(0.0);
                        if ui.add(egui::DragValue::new(&mut x).speed(0.1)).changed() {
                            new_inputs.insert(p.name.clone(), Json::from(x));
                        }
                    }
                    _ => {
                        let mut text = current.as_ref().map(json_text).unwrap_or_default();
                        if ui.text_edit_singleline(&mut text).changed() {
                            let v = if p.ty == PinType::Any {
                                text.parse::<f64>()
                                    .map_or(Json::from(text.clone()), Json::from)
                            } else {
                                Json::from(text)
                            };
                            new_inputs.insert(p.name.clone(), v);
                        }
                    }
                }
            });
        }
        if new_inputs != node.inputs {
            self.edit(|g| {
                if let Some(n) = g.node_mut(id) {
                    n.inputs = new_inputs;
                }
            });
        }
    }

    /// Settings of the whole script (nothing selected).
    fn script_settings(&mut self, ui: &mut egui::Ui) {
        ui.heading(tr("script.settings"));
        let mut g = self.graph.clone();
        ui.checkbox(&mut g.start.on_load, tr("script.on_load"));
        let mut start = g.start.message.clone().unwrap_or_default();
        ui.label(tr("script.start_message"));
        ui.text_edit_singleline(&mut start);
        g.start.message = Some(start.trim().to_owned()).filter(|s| !s.is_empty());
        let mut stop = g.start.stop_message.clone().unwrap_or_default();
        ui.label(tr("script.stop_message"));
        ui.text_edit_singleline(&mut stop);
        g.start.stop_message = Some(stop.trim().to_owned()).filter(|s| !s.is_empty());
        ui.checkbox(&mut g.trace_errors, tr("script.trace_errors"))
            .on_hover_text(tr("script.trace_errors_hint"));
        ui.checkbox(&mut g.trace_nodes, tr("debug.trace_nodes"))
            .on_hover_text(tr("debug.trace_nodes_hint"));
        ui.label(tr("script.description"));
        ui.text_edit_multiline(&mut g.description);
        ui.separator();
        ui.label(RichText::new(tr("script.variables")).strong());
        let mut remove = None;
        for (i, v) in g.variables.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut v.name).desired_width(100.0));
                egui::ComboBox::from_id_salt(("var-type", i))
                    .selected_text(v.ty.name())
                    .show_ui(ui, |ui| {
                        for t in PinType::DATA {
                            ui.selectable_value(&mut v.ty, t, t.name());
                        }
                    });
                let mut text = json_text(&v.default);
                if ui
                    .add(egui::TextEdit::singleline(&mut text).desired_width(60.0))
                    .changed()
                {
                    v.default = match v.ty {
                        PinType::Number => text.parse::<f64>().map_or(Json::Null, Json::from),
                        PinType::Bool => Json::from(text.trim() == "true"),
                        PinType::String => Json::from(text),
                        _ => Json::Null,
                    };
                }
                if ui.small_button("✕").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            g.variables.remove(i);
        }
        if ui.button(tr("script.add_variable")).clicked() {
            let n = g.variables.len() + 1;
            g.variables.push(Variable {
                name: format!("var{n}"),
                ty: PinType::Number,
                default: Json::from(0),
            });
        }
        if g != self.graph {
            self.edit(|graph| *graph = g);
        }
    }

    fn bottom_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let n = self.compiled.diagnostics.len();
            ui.selectable_value(
                &mut self.bottom,
                Bottom::Problems,
                trf("script.problems", &[("n", &n)]),
            );
            ui.selectable_value(&mut self.bottom, Bottom::Lua, tr("script.lua"));
            ui.selectable_value(&mut self.bottom, Bottom::Run, tr("script.run"));
            ui.selectable_value(&mut self.bottom, Bottom::Game, tr("debug.script_game"));
        });
        ui.separator();
        match self.bottom {
            Bottom::Problems => {
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        if self.compiled.diagnostics.is_empty() {
                            ui.label(tr("script.no_problems"));
                        }
                        let mut select = None;
                        for d in &self.compiled.diagnostics {
                            let color = match d.severity {
                                Severity::Error => Color32::from_rgb(230, 80, 80),
                                Severity::Warning => Color32::from_rgb(230, 180, 60),
                            };
                            if ui
                                .add(
                                    egui::Label::new(RichText::new(d.to_string()).color(color))
                                        .sense(egui::Sense::click()),
                                )
                                .clicked()
                            {
                                select = d.node;
                            }
                        }
                        if let Some(id) = select {
                            self.selected = Some(id);
                            self.view.set_selection([NodeId(u64::from(id))]);
                            self.view.frame_selection();
                        }
                    });
            }
            Bottom::Lua => {
                egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
                    let mut text = self.compiled.source.as_str();
                    ui.add(
                        egui::TextEdit::multiline(&mut text)
                            .code_editor()
                            .desired_width(f32::INFINITY),
                    );
                });
            }
            Bottom::Game => {
                if let Some(id) = self.live.journal_ui(ui, &self.graph) {
                    self.selected = Some(id);
                    self.view.set_selection([NodeId(u64::from(id))]);
                    self.view.frame_selection();
                }
            }
            Bottom::Run => {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.steps)
                            .desired_width(420.0)
                            .hint_text("send:iMsgX wait:2 var:name=1 player:1,2,3 relay:a=b stop"),
                    )
                    .on_hover_text(tr("script.steps_hint"));
                    if ui
                        .add_enabled(
                            self.compiled.chunk.is_some(),
                            egui::Button::new(tr("script.run_button")),
                        )
                        .clicked()
                    {
                        self.run_log = self.dry_run();
                    }
                });
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        for l in &self.run_log {
                            ui.monospace(l);
                        }
                    });
            }
        }
    }

    /// Runs the compiled script against the mock game with the steps.
    fn dry_run(&self) -> Vec<String> {
        let Some(chunk) = &self.compiled.chunk else {
            return vec![tr("script.has_errors").to_owned()];
        };
        let mut sim = Sim::new();
        let mut out = Vec::new();
        let name = self.graph.chunk_name();
        let log = |sim: &mut Sim, out: &mut Vec<String>| {
            for e in sim.take_log() {
                out.push(describe(&e));
            }
        };
        if let Err(e) = sim.load(chunk).and_then(|()| sim.start(&name)) {
            return vec![e.to_string()];
        }
        sim.advance(0.001).ok();
        log(&mut sim, &mut out);
        for step in self.steps.split_whitespace() {
            let (what, arg) = step.split_once(':').unwrap_or((step, ""));
            let r = match what {
                "send" => sim.send(arg),
                "wait" => sim.advance(arg.parse().unwrap_or(0.0)),
                "var" => {
                    if let Some((n, v)) = arg.split_once('=') {
                        sim.set_variable(
                            n,
                            doctor_toolkit::resource::lua::vm::Value::Number(
                                v.parse().unwrap_or(0.0),
                            ),
                        );
                    }
                    Ok(())
                }
                "player" => {
                    let p: Vec<f64> = arg
                        .split(',')
                        .filter_map(|s| s.trim().parse().ok())
                        .collect();
                    if p.len() == 3 {
                        sim.state.borrow_mut().player = [p[0], p[1], p[2]];
                    }
                    Ok(())
                }
                "relay" => {
                    if let Some((a, b)) = arg.split_once('=') {
                        sim.state.borrow_mut().relays.push((a.into(), b.into()));
                    }
                    Ok(())
                }
                "stop" => sim.stop(&name),
                _ => {
                    out.push(trf("script.bad_step", &[("step", &step)]));
                    Ok(())
                }
            };
            if let Err(e) = r {
                out.push(e.to_string());
            }
            log(&mut sim, &mut out);
        }
        out
    }
}

fn describe(e: &SimEvent) -> String {
    match e {
        SimEvent::Message {
            time,
            message,
            data,
        } => match data {
            Some(d) => format!("{time:8.3}  send {message} ({d})"),
            None => format!("{time:8.3}  send {message}"),
        },
        SimEvent::Probe { time, file } => format!("{time:8.3}  probe {file}"),
        SimEvent::Variable { time, name, value } => format!("{time:8.3}  set {name} = {value}"),
        SimEvent::Call {
            time,
            function,
            args,
        } => format!("{time:8.3}  {function}({})", args.join(", ")),
        SimEvent::Timer { time, callback } => format!("{time:8.3}  timer {callback}"),
        SimEvent::Error { time, message } => format!("{time:8.3}  error {message}"),
    }
}

enum MenuCmd {
    Add(&'static str, Pos2),
    Paste(Pos2),
    Delete(u32),
    Duplicate(u32),
    Disconnect(usize),
    Comment,
    Frame,
}

fn context_menu(ui: &mut egui::Ui, target: ContextTarget, clipboard: bool) -> Option<MenuCmd> {
    let mut cmd = None;
    match target {
        ContextTarget::Wire(w) => {
            if ui.button(tr("script.disconnect")).clicked() {
                cmd = Some(MenuCmd::Disconnect(w));
                ui.close();
            }
        }
        ContextTarget::Node(id) => {
            let Ok(id) = u32::try_from(id.0) else {
                return None;
            };
            if ui.button(tr("script.duplicate")).clicked() {
                cmd = Some(MenuCmd::Duplicate(id));
                ui.close();
            }
            if ui.button(tr("script.comment_selection")).clicked() {
                cmd = Some(MenuCmd::Comment);
                ui.close();
            }
            if ui.button(tr("script.delete")).clicked() {
                cmd = Some(MenuCmd::Delete(id));
                ui.close();
            }
        }
        ContextTarget::Pin(_) => {}
        ContextTarget::Canvas(_) | ContextTarget::Comment(_) => {
            let at = match target {
                ContextTarget::Canvas(p) => p,
                _ => Pos2::ZERO,
            };
            ui.menu_button(tr("script.add_node"), |ui| {
                let mut by_cat: BTreeMap<Category, Vec<&library::NodeDef>> = BTreeMap::new();
                for d in library::all() {
                    by_cat.entry(d.category).or_default().push(d);
                }
                for (cat, defs) in by_cat {
                    ui.menu_button(cat.name(), |ui| {
                        for d in defs {
                            if ui.button(d.title).on_hover_text(d.help).clicked() {
                                cmd = Some(MenuCmd::Add(d.kind, at));
                                ui.close();
                            }
                        }
                    });
                }
            });
            if clipboard && ui.button(tr("script.paste")).clicked() {
                cmd = Some(MenuCmd::Paste(at));
                ui.close();
            }
            if ui.button(tr("script.comment_selection")).clicked() {
                cmd = Some(MenuCmd::Comment);
                ui.close();
            }
            if ui.button(tr("script.frame")).clicked() {
                cmd = Some(MenuCmd::Frame);
                ui.close();
            }
        }
    }
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_maps_to_the_widget_and_back() {
        let mut e = ScriptEditor::default();
        assert_eq!(e.shown.nodes.len(), 2);
        assert_eq!(e.shown.wires.len(), 1);
        // Connect a new node's exec input to the probe's exec output.
        e.add_node("action.send_message", Pos2::new(500.0, 0.0));
        let send = e.selected.unwrap();
        let probe = e
            .graph
            .nodes
            .iter()
            .find(|n| n.kind == "debug.probe")
            .unwrap()
            .id;
        let from = PinAddr {
            node: NodeId(u64::from(probe)),
            output: true,
            pin: 0,
        };
        let to = PinAddr {
            node: NodeId(u64::from(send)),
            output: false,
            pin: 0,
        };
        e.on_event(GraphEvent::Connect { from, to });
        assert_eq!(e.graph.links.len(), 2);
        assert_eq!(e.shown.wires.len(), 2);
        // Disconnect the new wire, undo, redo.
        let w = e
            .wire_links
            .iter()
            .position(|&i| e.graph.links[i].to.0 == send)
            .unwrap();
        e.on_event(GraphEvent::Disconnect { wire: w });
        assert_eq!(e.graph.links.len(), 1);
        e.undo();
        assert_eq!(e.graph.links.len(), 2);
        e.redo();
        assert_eq!(e.graph.links.len(), 1);
        // Copy and paste the two nodes keeps their link.
        e.copy(&[NodeId(1), NodeId(2)]);
        e.paste(Pos2::new(0.0, 300.0));
        assert_eq!(e.graph.nodes.len(), 5);
        assert_eq!(e.graph.links.len(), 2);
        // Delete removes nodes and their links.
        e.on_event(GraphEvent::Delete(vec![NodeId(1)]));
        assert_eq!(e.graph.links.len(), 1);
        assert!(e.dirty);
    }

    #[test]
    fn dry_run_logs_what_the_script_does() {
        let e = ScriptEditor {
            steps: "wait:1".into(),
            ..Default::default()
        };
        let log = e.dry_run();
        assert!(
            log.iter()
                .any(|l| l.contains("doctor_probe_my_script_started.lua")),
            "{log:?}"
        );
    }
}
